//! config/pssource.go and config/localps.go: where a report's CPU and MEM
//! columns come from. A server on this machine is sampled from here, straight
//! from the operating system; one elsewhere is asked for the samples it took
//! of itself, over /init and /ps.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::config;
use crate::http::control_request;
use crate::logf;
use crate::stats::Samples;

/// config's control request retries: patient, since the server may be
/// draining the backlog of a phase that has just finished.
const CONTROL_ATTEMPTS: u32 = 4;
const CONTROL_TIMEOUT: Duration = Duration::from_secs(30);
const CONTROL_BACKOFF: Duration = Duration::from_secs(2);

/// config.controlRequest: a transport failure is retried, a reply the server
/// produced - a 404 included - is not.
pub async fn control(
    framework: &str,
    ip: &str,
    method: &str,
    path: &str,
    body: Option<&[u8]>,
    attempts: u32,
) -> Result<Vec<u8>, String> {
    let port = config::control_port(framework);
    let url = format!("http://{}:{port}{path}", config::url_host(ip));
    let mut last = String::new();
    for attempt in 1..=attempts {
        if attempt > 1 {
            tokio::time::sleep(CONTROL_BACKOFF * (attempt - 1)).await;
        }
        match control_request(ip, port, method, path, body, CONTROL_TIMEOUT).await {
            Ok((200, data)) => return Ok(data),
            Ok((status, data)) => {
                return Err(format!(
                    "{url}: {status}: {}",
                    String::from_utf8_lossy(&data).trim()
                ));
            }
            Err(err) => {
                last = format!("{url}: {err}");
                if attempt < attempts {
                    logf!("control request failed, retrying ({attempt}/{attempts}): {last}");
                }
            }
        }
    }
    Err(last)
}

pub enum Source {
    Remote { framework: String, ip: String },
    Local(Arc<LocalSampler>),
}

impl Source {
    /// PSSource.PsInfo: every sample so far, or an error saying why there
    /// are none - alongside what did arrive, as the Go client returns it.
    pub async fn ps_info(&self) -> (Samples, Option<String>) {
        match self {
            Source::Remote { framework, ip } => remote_ps(framework, ip).await,
            Source::Local(s) => {
                let samples = s.samples.lock().unwrap().clone();
                if !samples.cpu.is_empty() {
                    return (samples, None);
                }
                if let Some((framework, ip)) = &s.fallback {
                    return remote_ps(framework, ip).await;
                }
                (
                    samples,
                    Some(format!(
                        "pid {}: no CPU samples yet, so the phase was shorter than the -pi sampling interval",
                        s.pid
                    )),
                )
            }
        }
    }

    pub fn stop(&self) {
        if let Source::Local(s) = self {
            s.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// config.GetFrameworkPsInfo: the server's own samples, off /ps.
async fn remote_ps(framework: &str, ip: &str) -> (Samples, Option<String>) {
    let body = match control(framework, ip, "GET", "/ps", None, CONTROL_ATTEMPTS).await {
        Ok(body) => body,
        Err(err) => return (Samples::default(), Some(err)),
    };
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) else {
        return (Samples::default(), Some("/ps: not JSON".into()));
    };
    let cpu: Vec<f64> = v["cpu"]
        .as_array()
        .map_or(vec![], |a| a.iter().filter_map(|x| x.as_f64()).collect());
    let rss: Vec<u64> = v["mem"].as_array().map_or(vec![], |a| {
        a.iter().filter_map(|m| m["rss"].as_u64()).collect()
    });
    let samples = Samples { cpu, rss };
    let avg = samples.resources().cpu_avg;
    if avg <= 0.0 {
        return (samples, Some("/ps answered with no CPU samples, so either /init did not reach it or the phase was shorter than the -pi sampling interval".into()));
    }
    (samples, None)
}

/// config.LocalPSSampler: a server process on this machine, sampled every
/// interval on a thread of its own, so nothing is asked of the server.
pub struct LocalSampler {
    pub pid: i32,
    samples: Mutex<Samples>,
    stop: AtomicBool,
    fallback: Option<(String, String)>,
}

/// config.consecutiveSampleErrors.
const CONSECUTIVE_SAMPLE_ERRORS: u32 = 5;

impl LocalSampler {
    pub fn start(
        pid: i32,
        interval: Duration,
        fallback: Option<(String, String)>,
    ) -> Result<Arc<Self>, String> {
        // One read before returning, so that a process that cannot be
        // sampled at all fails here rather than as an empty column.
        let mut last_cpu =
            cpu_time(pid).ok_or_else(|| format!("pid {pid}: cannot read its CPU time"))?;
        rss_bytes(pid).ok_or_else(|| format!("pid {pid}: cannot read its memory"))?;
        let sampler = Arc::new(LocalSampler {
            pid,
            samples: Mutex::default(),
            stop: AtomicBool::new(false),
            fallback,
        });
        let s = Arc::clone(&sampler);
        std::thread::spawn(move || {
            let mut last_tick = Instant::now();
            let mut errors = 0;
            while !s.stop.load(Ordering::SeqCst) {
                std::thread::sleep(interval);
                let (Some(cpu), Some(rss)) = (cpu_time(pid), rss_bytes(pid)) else {
                    errors += 1;
                    if errors >= CONSECUTIVE_SAMPLE_ERRORS {
                        logf!(
                            "sampling pid {pid} stopped after {errors} failures, keeping {} samples",
                            s.samples.lock().unwrap().cpu.len()
                        );
                        return;
                    }
                    continue;
                };
                errors = 0;
                let now = Instant::now();
                let wall = now.duration_since(last_tick).as_secs_f64();
                let percent = if wall > 0.0 {
                    cpu.saturating_sub(last_cpu).as_secs_f64() / wall * 100.0
                } else {
                    0.0
                };
                last_cpu = cpu;
                last_tick = now;
                let mut samples = s.samples.lock().unwrap();
                samples.cpu.push(percent);
                samples.rss.push(rss);
            }
        });
        Ok(sampler)
    }
}

/// config.SetupPS: the source, the server's pid, and whether it came from
/// here or from the server.
pub async fn setup(
    framework: &str,
    ip: &str,
    mode: &str,
    interval: Duration,
) -> (Source, i32, Option<String>) {
    let remote = || Source::Remote {
        framework: framework.into(),
        ip: ip.into(),
    };
    let want_local = mode == "local" || (mode == "auto" && config::is_local_host(ip));
    if want_local {
        let started = config::find_server_process(framework)
            .and_then(|pid| LocalSampler::start(pid, interval, None));
        match started {
            Ok(s) => {
                logf!(
                    "{framework}: sampled here, from pid {}, so it is not asked to sample itself",
                    s.pid
                );
                let pid = s.pid;
                return (Source::Local(s), pid, None);
            }
            Err(err) => logf!(
                "{framework}: cannot sample the server from this machine, asking it over HTTP instead: {err}"
            ),
        }
    }

    // The server samples itself: /init starts that and answers with its pid.
    let args = format!("{{\"PsInterval\":{}}}", interval.as_nanos());
    let pid = match control(
        framework,
        ip,
        "POST",
        "/init",
        Some(args.as_bytes()),
        CONTROL_ATTEMPTS,
    )
    .await
    {
        Ok(body) => match String::from_utf8_lossy(&body).trim().parse::<i32>() {
            Ok(pid) => pid,
            Err(_) => {
                return (
                    remote(),
                    -1,
                    Some(format!(
                        "/init answered {:?}, not a pid",
                        String::from_utf8_lossy(&body)
                    )),
                );
            }
        },
        Err(err) => return (remote(), -1, Some(err)),
    };
    // A pid from the server's own namespace names it here only when the two
    // share one; where they do, sample it from here too, /ps the fallback.
    if want_local && config::verify_server_process(pid, framework).is_ok() {
        if let Ok(s) = LocalSampler::start(pid, interval, Some((framework.into(), ip.into()))) {
            logf!(
                "{framework}: sampled here, from pid {pid}, with the server's own /ps as the fallback"
            );
            return (Source::Local(s), pid, None);
        }
    }
    (remote(), pid, None)
}

/// User and system CPU time a process has used, over all its threads.
#[cfg(target_os = "linux")]
fn cpu_time(pid: i32) -> Option<Duration> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // Past the parenthesized comm, which may itself hold spaces: state is
    // field 3 overall, utime and stime 14 and 15.
    let fields: Vec<&str> = stat[stat.rfind(')')? + 2..].split_whitespace().collect();
    let ticks: u64 = fields.get(11)?.parse::<u64>().ok()? + fields.get(12)?.parse::<u64>().ok()?;
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
    if hz <= 0 {
        return None;
    }
    Some(Duration::from_nanos(ticks * 1_000_000_000 / hz as u64))
}

#[cfg(target_os = "linux")]
fn rss_bytes(pid: i32) -> Option<u64> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
    let kb: u64 = line
        .trim_start_matches("VmRSS:")
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(kb * 1024)
}

#[cfg(target_os = "macos")]
fn task_info(pid: i32) -> Option<libc::proc_taskinfo> {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            pid,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    (n == size).then_some(info)
}

#[cfg(target_os = "macos")]
fn cpu_time(pid: i32) -> Option<Duration> {
    let info = task_info(pid)?;
    // Mach absolute time units, which are nanoseconds on Intel and not on
    // Apple silicon.
    #[repr(C)]
    struct Timebase {
        numer: u32,
        denom: u32,
    }
    unsafe extern "C" {
        fn mach_timebase_info(info: *mut Timebase) -> libc::c_int;
    }
    let mut tb = Timebase { numer: 0, denom: 0 };
    unsafe { mach_timebase_info(&mut tb) };
    if tb.denom == 0 {
        return None;
    }
    let ticks = info.pti_total_user + info.pti_total_system;
    Some(Duration::from_nanos(
        (ticks as u128 * tb.numer as u128 / tb.denom as u128) as u64,
    ))
}

#[cfg(target_os = "macos")]
fn rss_bytes(pid: i32) -> Option<u64> {
    Some(task_info(pid)?.pti_resident_size)
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn cpu_time(_: i32) -> Option<Duration> {
    None
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rss_bytes(_: i32) -> Option<u64> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The test binary is the one process this test is sure about.
    #[test]
    fn samples_itself() {
        let pid = std::process::id() as i32;
        let s = LocalSampler::start(pid, Duration::from_millis(50), None).unwrap();
        let busy = Instant::now();
        while busy.elapsed() < Duration::from_millis(300) {
            std::hint::black_box(0u64.wrapping_add(1));
        }
        let samples = s.samples.lock().unwrap().clone();
        s.stop.store(true, Ordering::SeqCst);
        assert!(samples.cpu.len() >= 2, "{} samples", samples.cpu.len());
        assert!(samples.cpu.iter().any(|&c| c > 10.0), "{:?}", samples.cpu);
        assert!(samples.rss.iter().all(|&r| r > 0));
    }

    #[test]
    fn invalid_pid() {
        assert!(LocalSampler::start(0x7fff_fff0, Duration::from_secs(1), None).is_err());
    }
}
