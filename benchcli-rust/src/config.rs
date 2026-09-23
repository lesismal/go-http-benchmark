//! What the Go client reads from package config: which frameworks there are,
//! the ports each one's server listens on, and where its control routes are.

use std::net::{IpAddr, ToSocketAddrs};

/// Every framework, as config.FrameworkList orders them, with config.Langs'
/// language and config.Ports' range. config's tests hold this table to the Go
/// one, line for line.
pub const FRAMEWORKS: &[(&str, &str, u16, u16)] = &[
    ("axum", "rust", 14001, 14050),
    ("fasthttp", "go", 10001, 10050),
    ("fib", "go", 11001, 11050),
    ("gin", "go", 12001, 12050),
    ("nethttp", "go", 13001, 13050),
];

pub const ECHO_PATH: &str = "/echo";

fn find(framework: &str) -> Option<&'static (&'static str, &'static str, u16, u16)> {
    FRAMEWORKS.iter().find(|f| f.0 == framework)
}

pub fn known(framework: &str) -> bool {
    find(framework).is_some()
}

pub fn names() -> Vec<&'static str> {
    FRAMEWORKS.iter().map(|f| f.0).collect()
}

/// config.FrameworkLang.
pub fn lang(framework: &str) -> &'static str {
    find(framework).map_or("-", |f| f.1)
}

/// config.GetFrameworkBenchmarkAddrs: host:port for each benchmark port.
pub fn benchmark_addrs(framework: &str, ip: &str) -> Vec<String> {
    let (_, _, first, last) = find(framework).expect("known framework");
    let host = url_host(ip);
    (*first..=*last)
        .map(|port| format!("{host}:{port}"))
        .collect()
}

/// The port after the last benchmark port, where /init, /ps and pprof are.
pub fn control_port(framework: &str) -> u16 {
    find(framework).expect("known framework").3 + 1
}

/// The host as a URL or a host:port carries it: an IPv6 literal in brackets.
pub fn url_host(ip: &str) -> String {
    if ip.contains(':') && !ip.starts_with('[') {
        format!("[{ip}]")
    } else {
        ip.to_string()
    }
}

/// config.ServerProcessName.
pub fn server_process_name(framework: &str) -> String {
    format!("{framework}.server")
}

/// config.IsLocalHost: the loopback address, or one of this machine's own
/// interface addresses.
pub fn is_local_host(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.is_empty() {
        return false;
    }
    let ips: Vec<IpAddr> = match host.parse::<IpAddr>() {
        Ok(ip) => vec![ip],
        Err(_) => match (host, 0).to_socket_addrs() {
            Ok(addrs) => addrs.map(|a| a.ip()).collect(),
            Err(_) => return false,
        },
    };
    let local = local_addrs();
    ips.iter()
        .any(|ip| ip.is_loopback() || ip.is_unspecified() || local.contains(ip))
}

fn local_addrs() -> Vec<IpAddr> {
    let mut out = Vec::new();
    let mut ifap: *mut libc::ifaddrs = std::ptr::null_mut();
    if unsafe { libc::getifaddrs(&mut ifap) } != 0 {
        return out;
    }
    let mut cur = ifap;
    while !cur.is_null() {
        let ifa = unsafe { &*cur };
        if !ifa.ifa_addr.is_null() {
            match unsafe { (*ifa.ifa_addr).sa_family } as i32 {
                libc::AF_INET => {
                    let sin = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in) };
                    out.push(IpAddr::from(
                        u32::from_be(sin.sin_addr.s_addr).to_be_bytes(),
                    ));
                }
                libc::AF_INET6 => {
                    let sin6 = unsafe { &*(ifa.ifa_addr as *const libc::sockaddr_in6) };
                    out.push(IpAddr::from(sin6.sin6_addr.s6_addr));
                }
                _ => {}
            }
        }
        cur = ifa.ifa_next;
    }
    unsafe { libc::freeifaddrs(ifap) };
    out
}

/// config.FindServerProcess: the one process on this machine whose argv[0]
/// has the framework's server binary name.
pub fn find_server_process(framework: &str) -> Result<i32, String> {
    let name = server_process_name(framework);
    let pids: Vec<i32> = list_processes()?
        .into_iter()
        .filter(|(_, n)| *n == name)
        .map(|(pid, _)| pid)
        .collect();
    match pids.len() {
        0 => Err(format!("no {name} process on this machine")),
        1 => Ok(pids[0]),
        n => Err(format!(
            "{n} {name} processes on this machine ({pids:?}): stop the leftovers of earlier runs, e.g. with script/killall.sh"
        )),
    }
}

/// config.VerifyServerProcess.
pub fn verify_server_process(pid: i32, framework: &str) -> Result<(), String> {
    let want = server_process_name(framework);
    match list_processes()?.into_iter().find(|(p, _)| *p == pid) {
        Some((_, name)) if name == want => Ok(()),
        Some((_, name)) => Err(format!(
            "pid {pid} on this machine is {name:?}, not {want:?}"
        )),
        None => Err(format!("pid {pid}: no such process")),
    }
}

fn base_name(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// pid and the base name of argv[0] for every process this user can see:
/// /proc where there is one, ps where there is not.
fn list_processes() -> Result<Vec<(i32, String)>, String> {
    if let Ok(entries) = std::fs::read_dir("/proc") {
        let mut out = Vec::new();
        for entry in entries.flatten() {
            let Ok(pid) = entry.file_name().to_string_lossy().parse::<i32>() else {
                continue;
            };
            if let Ok(cmdline) = std::fs::read(format!("/proc/{pid}/cmdline")) {
                let argv0 = cmdline.split(|&b| b == 0).next().unwrap_or(&[]);
                if !argv0.is_empty() {
                    out.push((pid, base_name(&String::from_utf8_lossy(argv0))));
                }
            }
        }
        if !out.is_empty() {
            return Ok(out);
        }
    }
    let output = std::process::Command::new("ps")
        .args(["-o", "pid=,comm=", "-A"])
        .output()
        .map_err(|err| format!("ps: {err}"))?;
    let mut out = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut fields = line.split_whitespace();
        let Some(Ok(pid)) = fields.next().map(str::parse::<i32>) else {
            continue;
        };
        let rest: Vec<&str> = fields.collect();
        if !rest.is_empty() {
            out.push((pid, base_name(&rest.join(" "))));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addrs() {
        let addrs = benchmark_addrs("nethttp", "::1");
        assert_eq!(addrs.len(), 50);
        assert_eq!(addrs[0], "[::1]:13001");
        assert_eq!(control_port("axum"), 14051);
        assert_eq!(lang("axum"), "rust");
        assert_eq!(lang("nope"), "-");
    }

    #[test]
    fn local_hosts() {
        for h in ["127.0.0.1", "::1", "[::1]", "localhost", "0.0.0.0"] {
            assert!(is_local_host(h), "{h}");
        }
        for h in ["", "192.0.2.10", "2001:db8::1"] {
            assert!(!is_local_host(h), "{h}");
        }
    }
}
