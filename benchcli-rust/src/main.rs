//! benchcli-rust: the benchmark client of benchcli-go, on tokio - the
//! runtime github.com/tokio-rs/axum runs on - rather than Go's.
//!
//! It takes the same flags, runs the same three benchmarks the same way, and
//! writes the same JSON report files, with "benchcli-rust" as their
//! BenchClient, so the Summary table's Client reads "rust". Turning those
//! files into the markdown tables is left to the Go client: script/report.sh
//! runs it on whatever either client wrote, so there is one report format
//! and one implementation of it.
//!
//! Neither client uses an HTTP client library. axum is a server framework
//! with no client of its own, and hyper's client, which it is built on,
//! answers one request at a time on a connection and would not pipeline.

mod bench;
mod config;
mod flags;
mod http;
mod log;
mod ps;
mod report;
mod stats;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::bench::{Dialer, EchoConfig, PipelineConfig};
use crate::http::control_request;
use crate::log::{LONG_LINE, SHORT_LINE, print};
use crate::report::{
    BenchEchoReport, BenchRateReport, ConnectionsReport, client_name, cpu, dur, mem,
};
use crate::stats::{eer, rate_tps};

type Pprof = Arc<Mutex<Option<(Vec<u8>, Vec<u8>)>>>;

fn main() {
    let wd = std::env::current_dir()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    logf!("pwd: {wd}");
    let f = flags::parse(std::env::args().skip(1));

    if f.report_sort != "result" && f.report_sort != "framework" {
        fatalf!(
            "report: unknown sort order {:?}, want one of [result framework]",
            f.report_sort
        );
    }
    if !["auto", "local", "remote"].contains(&f.ps_mode.as_str()) {
        fatalf!(
            "unsupported -ps value {:?} (want auto, local or remote)",
            f.ps_mode
        );
    }
    if f.gen_report {
        fatalf!(
            "-r: benchcli-rust writes the JSON reports and leaves the tables to the Go client; run script/report.sh, or output/bin/bench.report -r=true"
        );
    }
    if f.rate_enabled {
        if let Err(err) = http::validate_pipeline(f.rate_pipeline, f.rate_send_rate.max(1)) {
            fatalf!("-rpl={}: {err}", f.rate_pipeline);
        }
    }
    if !config::known(&f.framework) {
        fatalf!(
            "-f={}: unknown framework {:?}, want one of {:?}",
            f.framework,
            f.framework,
            config::names()
        );
    }

    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|err| fatalf!("tokio runtime: {err}"))
        .block_on(run(f));
}

async fn run(f: flags::Flags) {
    let fw = f.framework.clone();
    let lang = config::lang(&fw).to_string();
    print(LONG_LINE);
    logf!(
        "Benchmark [{fw}]: {} connections, {} payload, {} times",
        f.num_connections,
        f.payload,
        f.echo_times
    );
    print(SHORT_LINE);

    // Room for a whole echo response, header and body, in one read.
    let dialer = Arc::new(Dialer::new(
        &f.ip,
        f.dial_timeout,
        (f.payload + 1024).max(4096),
    ));
    let cs = bench::connections(
        &fw,
        &f.ip,
        f.num_connections,
        f.dial_concurrency,
        f.dial_retries,
        f.dial_retry_interval,
        Arc::clone(&dialer),
    )
    .await;
    let c = &cs.calc;
    let tpn = |p| if f.enable_tpn { c.tpn(p) } else { 0 };
    let r = ConnectionsReport {
        framework: fw.clone(),
        lang: lang.clone(),
        bench_client: report::BENCH_CLIENT.into(),
        tps: c.tps(),
        min: c.min(),
        avg: c.avg(),
        max: c.max(),
        tp50: tpn(50),
        tp75: tpn(75),
        tp90: tpn(90),
        tp95: tpn(95),
        tp99: tpn(99),
        used: c.used.as_nanos() as i64,
        total: f.num_connections,
        success: c.success,
        failed: c.failed,
        concurrency: cs.concurrency,
    };
    report::to_file(
        &r,
        &format!("{fw}-Connections"),
        "Connections",
        &f.preffix,
        &f.suffix,
        None,
    );
    print(SHORT_LINE);
    print(&report::console(
        "Connections",
        &[
            ("Framework", r.framework.clone(), false),
            ("Lang", r.lang.clone(), false),
            ("Client", client_name(), false),
            ("TPS", r.tps.to_string(), false),
            ("Min", dur(r.min), true),
            ("Avg", dur(r.avg), true),
            ("Max", dur(r.max), true),
            ("TP95", dur(r.tp95), true),
            ("TP99", dur(r.tp99), true),
            ("Used", dur(r.used), false),
            ("Total", r.total.to_string(), false),
            ("Success", r.success.to_string(), false),
            ("Failed", r.failed.to_string(), false),
            ("Concurrency", r.concurrency.to_string(), false),
        ],
        f.enable_tpn,
    ));
    print("\n");
    print(SHORT_LINE);
    if cs.conns.is_empty() {
        fatalf!(
            "Connections: no connection to {fw} at {} succeeded, so there is nothing to benchmark on",
            f.ip
        );
    }

    // How the server's CPU and MEM - and so EER - are sampled; see
    // config.SetupPS.
    let (ps_source, _server_pid, ps_err) = ps::setup(
        &fw,
        &f.ip,
        &f.ps_mode,
        Duration::from_millis(f.ps_interval_ms),
    )
    .await;
    if let Some(err) = ps_err {
        logf!("SetupPS({fw}) failed: {err}");
    }
    let control_host = f.ip.clone();
    let control_port = config::control_port(&fw);
    let control_url = format!("http://{}:{control_port}", config::url_host(&f.ip));
    // Only a Go server serves /debug/pprof/; any other is not asked for a
    // profile at all, rather than asked and logged as failing every run.
    let has_pprof = config::has_pprof(&fw);
    if has_pprof {
        println!("pprof cpu :\n  curl --output ./cpu_profile {control_url}/debug/pprof/profile");
        println!("  go tool pprof -http=:6060 ./cpu_profile");
        println!("pprof heap:\n  curl --output ./mem_profile {control_url}/debug/pprof/heap");
        println!("  go tool pprof -http=:6061 ./mem_profile");
    } else {
        logf!("{fw}: {lang} server, no /debug/pprof/, pprof profiles are not fetched");
    }
    print(SHORT_LINE);

    // The Go client's pprof hooks: two seconds into the phase, a CPU profile
    // of -epd/-rpd seconds and a heap profile, written next to the report
    // when they arrived in time.
    let fetch_pprof = |kind: &'static str, seconds: u64, slot: Pprof| {
        let (host, name) = (control_host.clone(), kind);
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_secs(2)).await;
            let get = |path: String, timeout| {
                let host = host.clone();
                async move {
                    match control_request(&host, control_port, "GET", &path, None, timeout).await {
                        Ok((200, body)) => Ok(body),
                        Ok((status, _)) => Err(format!("status {status}")),
                        Err(err) => Err(err.to_string()),
                    }
                }
            };
            let cpu = match get(
                format!("/debug/pprof/profile?seconds={seconds}"),
                Duration::from_secs(seconds + 30),
            )
            .await
            {
                Ok(b) => b,
                Err(err) => return println!("{name}: [pprof cpu] httpGet failed: {err}"),
            };
            let mem = match get("/debug/pprof/heap".into(), Duration::from_secs(30)).await {
                Ok(b) => b,
                Err(err) => return println!("{name}: [pprof mem] httpGet failed: {err}"),
            };
            *slot.lock().unwrap() = Some((cpu, mem));
        });
    };

    let echo_pprof: Pprof = Arc::default();
    let echo_cfg = EchoConfig {
        concurrency: f.echo_concurrency,
        total: f.echo_times,
        payload: f.payload,
        limit: f.echo_tps_limit,
        check: f.check_valid,
        ip: f.ip.clone(),
    };
    let payload = if f.payload == 0 { 1024 } else { f.payload };
    let conns_len = cs.conns.len();
    let be = bench::echo(cs.conns, echo_cfg, Arc::clone(&dialer), || {
        if f.echo_pprof && has_pprof {
            fetch_pprof("BenchEcho", f.echo_pprof_duration, Arc::clone(&echo_pprof));
        }
    })
    .await;
    let (samples, ps_err) = ps_source.ps_info().await;
    if let Some(err) = ps_err {
        logf!("BenchEcho: resource statistics for {fw} incomplete, EER will read 0: {err}");
    }
    let res = samples.resources();
    let c = &be.calc;
    let tpn = |p| if f.enable_tpn { c.tpn(p) } else { 0 };
    let r = BenchEchoReport {
        framework: fw.clone(),
        lang: lang.clone(),
        bench_client: report::BENCH_CLIENT.into(),
        tps: c.tps(),
        eer: eer(c.tps() as f64, res.cpu_avg),
        min: c.min(),
        avg: c.avg(),
        max: c.max(),
        tp50: tpn(50),
        tp75: tpn(75),
        tp90: tpn(90),
        tp95: tpn(95),
        tp99: tpn(99),
        used: c.used.as_nanos() as i64,
        total: f.echo_times,
        success: c.success,
        failed: c.failed,
        connections: conns_len,
        concurrency: be.concurrency,
        payload,
        pprof: f.echo_pprof,
        cpu_min: res.cpu_min,
        cpu_avg: res.cpu_avg,
        cpu_max: res.cpu_max,
        mem_min: res.mem_min,
        mem_avg: res.mem_avg,
        mem_max: res.mem_max,
    };
    let pprof = echo_pprof.lock().unwrap().take();
    report::to_file(
        &r,
        &format!("{fw}-BenchEcho"),
        "BenchEcho",
        &f.preffix,
        &f.suffix,
        pprof.as_ref(),
    );
    print(SHORT_LINE);
    print(&report::console(
        "BenchEcho",
        &[
            ("Framework", r.framework.clone(), false),
            ("Lang", r.lang.clone(), false),
            ("Client", client_name(), false),
            ("TPS", r.tps.to_string(), false),
            ("EER", format!("{:.2}", r.eer), false),
            ("Min", dur(r.min), true),
            ("Avg", dur(r.avg), true),
            ("Max", dur(r.max), true),
            ("TP95", dur(r.tp95), true),
            ("TP99", dur(r.tp99), true),
            ("Used", dur(r.used), false),
            ("Total", r.total.to_string(), false),
            ("Success", r.success.to_string(), false),
            ("Failed", r.failed.to_string(), false),
            ("Conns", r.connections.to_string(), false),
            ("Concurrency", r.concurrency.to_string(), false),
            ("Payload", r.payload.to_string(), false),
            ("CPU Avg", cpu(r.cpu_avg), false),
            ("CPU Max", cpu(r.cpu_max), false),
            ("MEM Avg", mem(r.mem_avg), false),
            ("MEM Max", mem(r.mem_max), false),
        ],
        f.enable_tpn,
    ));
    print("\n");
    print(SHORT_LINE);

    if f.rate_enabled {
        let rate_pprof: Pprof = Arc::default();
        let duration = Duration::from_secs(if f.rate_duration == 0 {
            10
        } else {
            f.rate_duration
        });
        let cfg = PipelineConfig {
            concurrency: f.rate_concurrency,
            duration,
            send_rate: f.rate_send_rate,
            batch_size: f.rate_batch_size,
            pipeline: f.rate_pipeline as usize,
            payload,
            send_limit: f.rate_send_limit,
            check: f.check_valid,
            ip: f.ip.clone(),
        };
        let br = bench::pipeline(be.conns, cfg, Arc::clone(&dialer), || {
            if f.rate_pprof && has_pprof {
                fetch_pprof(
                    "BenchPipeline",
                    f.rate_pprof_duration,
                    Arc::clone(&rate_pprof),
                );
            }
        })
        .await;
        let (samples, ps_err) = ps_source.ps_info().await;
        if let Some(err) = ps_err {
            logf!(
                "BenchPipeline: resource statistics for {fw} incomplete, EchoEER will read 0: {err}"
            );
        }
        let res = samples.resources();
        let duration_ns = duration.as_nanos() as i64;
        let tps = rate_tps(br.recv_times, duration_ns);
        let r = BenchRateReport {
            framework: fw.clone(),
            lang: lang.clone(),
            bench_client: report::BENCH_CLIENT.into(),
            duration: duration_ns,
            tps: tps.floor() as i64,
            echo_eer: eer(tps, res.cpu_avg),
            send_times: br.send_times,
            send_bytes: br.send_bytes,
            recv_times: br.recv_times,
            recv_bytes: br.recv_bytes,
            connections: conns_len,
            concurrency: br.concurrency,
            send_rate: f.rate_send_rate.max(1),
            pipeline: br.batch,
            payload,
            pprof: f.rate_pprof,
            cpu_min: res.cpu_min,
            cpu_avg: res.cpu_avg,
            cpu_max: res.cpu_max,
            mem_min: res.mem_min,
            mem_avg: res.mem_avg,
            mem_max: res.mem_max,
        };
        let pprof = rate_pprof.lock().unwrap().take();
        report::to_file(
            &r,
            &format!("{fw}-BenchPipeline"),
            "BenchPipeline",
            &f.preffix,
            &f.suffix,
            pprof.as_ref(),
        );
        print(SHORT_LINE);
        print(&report::console(
            "BenchPipeline",
            &[
                ("Framework", r.framework.clone(), false),
                ("Lang", r.lang.clone(), false),
                ("Client", client_name(), false),
                ("Duration", dur(r.duration), false),
                ("TPS", r.tps.to_string(), false),
                ("EER", format!("{:.2}", r.echo_eer), false),
                ("Req Sent", r.send_times.to_string(), false),
                ("Bytes Sent", mem(r.send_bytes as u64), false),
                ("Resp Recv", r.recv_times.to_string(), false),
                ("Bytes Recv", mem(r.recv_bytes as u64), false),
                ("Conns", r.connections.to_string(), false),
                ("Concurrency", r.concurrency.to_string(), false),
                ("SendRate", r.send_rate.to_string(), false),
                ("Pipeline", r.pipeline.to_string(), false),
                ("Payload", r.payload.to_string(), false),
                ("CPU Avg", cpu(r.cpu_avg), false),
                ("CPU Max", cpu(r.cpu_max), false),
                ("MEM Avg", mem(r.mem_avg), false),
                ("MEM Max", mem(r.mem_max), false),
            ],
            f.enable_tpn,
        ));
        print("\n");
        print(SHORT_LINE);
    }
    ps_source.stop();
    print(LONG_LINE);
}
