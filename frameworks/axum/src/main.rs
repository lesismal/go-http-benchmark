//! The axum benchmark server: the Rust counterpart of the Go servers under
//! frameworks/, answering the same routes on the same kind of ports.
//!
//! - `POST /echo` on 50 benchmark ports, [`FIRST_PORT`] to [`LAST_PORT`]
//!   (config.Ports in config/config.go), answered with the request body and
//!   its Content-Length.
//! - `/init` and `/ps` on the port after the last one, on a server of their
//!   own, as frameworks.StartControlServer serves them for the Go servers.
//!   There is no Go runtime here to run github.com/lesismal/perf in, so the
//!   sampling is done below, in the same shape: `/init` starts sampling this
//!   process' CPU and resident memory every `PsInterval` and answers with the
//!   pid, and `/ps` answers with the samples as perf.PSCounter's JSON - only
//!   its `cpu` and `mem[].rss` fields, which are all the clients read. There
//!   is no `/debug/pprof`, so the client's pprof fetch fails for this server
//!   and says so.
//!
//! It takes the flags script/benchmark.sh forwards to every server, in Go's
//! flag syntax, and exits on one it does not know, as a Go server would.

use std::{
    net::SocketAddr,
    process,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    Router,
    body::Bytes,
    http::header,
    response::IntoResponse,
    routing::{any, get, post},
    serve::ListenerExt,
};
use socket2::{Domain, Protocol, Socket, Type};
use tokio::net::TcpListener;

/// The framework's benchmark ports, which config.Ports in config/config.go
/// lists as "14001:14050"; config's tests hold the two to each other.
const FIRST_PORT: u16 = 14001;
const LAST_PORT: u16 = 14050;

const NAME: &str = "axum";

struct Flags {
    nodelay: bool,
    reuseport: bool,
    payload: usize,
    mem_limit: i64,
}

fn main() {
    let flags = parse_flags(std::env::args().skip(1));
    // One worker thread per CPU this process may run on: tokio sizes its
    // pool by available_parallelism, which on Linux reads the affinity mask
    // script/env.sh pins the server with, and the cgroup quota Docker sets.
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap_or_else(|err| fatal(&format!("tokio runtime: {err}")));
    eprintln!(
        "{NAME} server: nodelay={}, reuseport={}, payload={}, memory limit={} (ignored: no GC to limit), workers={}",
        flags.nodelay,
        flags.reuseport,
        flags.payload,
        flags.mem_limit,
        std::thread::available_parallelism().map_or(1, |n| n.get()),
    );
    runtime.block_on(run(flags));
}

async fn run(flags: Flags) {
    let control = listen(LAST_PORT + 1, flags.reuseport);
    let sampler = Arc::new(Sampler::default());
    let control_router = Router::new()
        .route("/init", post(on_init))
        .route("/ps", get(on_ps))
        .with_state(sampler);
    tokio::spawn(async move {
        if let Err(err) = axum::serve(control, control_router).await {
            fatal(&format!("control server exit: {err}"));
        }
    });

    let router = Router::new().route("/echo", any(on_echo));
    for port in FIRST_PORT..=LAST_PORT {
        let nodelay = flags.nodelay;
        let listener = listen(port, flags.reuseport).tap_io(move |tcp| {
            // Set whichever way it was asked, as frameworks.Listen does for
            // the Go servers: tokio leaves the option as the kernel has it.
            let _ = tcp.set_nodelay(nodelay);
        });
        let router = router.clone();
        tokio::spawn(async move {
            if let Err(err) = axum::serve(listener, router).await {
                eprintln!("{NAME} server: port {port} exit: {err}");
            }
        });
    }
    eprintln!(
        "{NAME} server: listening on {} ports, :{FIRST_PORT} to :{LAST_PORT}",
        LAST_PORT - FIRST_PORT + 1
    );

    wait_signal().await;
    eprintln!("{NAME} server: exit");
}

/// The body back, byte for byte. axum answers a whole `Bytes` with its
/// Content-Length, never chunked.
async fn on_echo(body: Bytes) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/octet-stream")], body)
}

async fn on_init(
    axum::extract::State(sampler): axum::extract::State<Arc<Sampler>>,
    body: Bytes,
) -> String {
    // config.InitArgs: {"PsInterval": <nanoseconds>}.
    let interval = serde_json::from_slice::<serde_json::Value>(&body)
        .ok()
        .and_then(|args| args.get("PsInterval").and_then(|v| v.as_u64()))
        .filter(|&ns| ns > 0)
        .map_or(Duration::from_secs(1), Duration::from_nanos);
    sampler.start(interval);
    process::id().to_string()
}

async fn on_ps(
    axum::extract::State(sampler): axum::extract::State<Arc<Sampler>>,
) -> impl IntoResponse {
    ([(header::CONTENT_TYPE, "application/json")], sampler.json())
}

/// Binds port on every interface, with SO_REUSEPORT unless -reuseport=false,
/// and the backlog Go's net package asks for: the kernel's somaxconn.
fn listen(port: u16, reuseport: bool) -> TcpListener {
    let addr: SocketAddr = ([0, 0, 0, 0], port).into();
    let bind = || -> std::io::Result<TcpListener> {
        let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
        socket.set_reuse_address(true)?;
        if reuseport {
            socket.set_reuse_port(true)?;
        }
        socket.set_nonblocking(true)?;
        socket.bind(&addr.into())?;
        socket.listen(somaxconn())?;
        TcpListener::from_std(socket.into())
    };
    bind().unwrap_or_else(|err| fatal(&format!("listen :{port} failed: {err}")))
}

fn somaxconn() -> i32 {
    std::fs::read_to_string("/proc/sys/net/core/somaxconn")
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(4096)
}

/// Blocks until the server is told to stop: SIGINT, which script/killone.sh
/// sends, or SIGTERM.
async fn wait_signal() {
    use tokio::signal::unix::{SignalKind, signal};
    let mut term =
        signal(SignalKind::terminate()).unwrap_or_else(|err| fatal(&format!("signal: {err}")));
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}

/// This process' CPU and resident memory, sampled once per interval once
/// `/init` has started it - the numbers gopsutil gives the Go servers'
/// perf.PSCounter: CPU as the percent of one core used over the interval, so
/// 100 is one core busy, and memory in bytes.
#[derive(Default)]
struct Sampler {
    started: AtomicBool,
    samples: Mutex<(Vec<f64>, Vec<u64>)>,
}

impl Sampler {
    /// Starts sampling, once, however many times `/init` arrives: a client
    /// that retried the request can deliver it twice.
    fn start(self: &Arc<Self>, interval: Duration) {
        if self.started.swap(true, Ordering::SeqCst) {
            eprintln!("{NAME} server: /init called again; the ps counter is already running");
            return;
        }
        let sampler = Arc::clone(self);
        std::thread::spawn(move || {
            let mut last_cpu = cpu_time();
            let mut last_tick = Instant::now();
            loop {
                std::thread::sleep(interval);
                let cpu = cpu_time();
                let now = Instant::now();
                let wall = now.duration_since(last_tick).as_secs_f64();
                let percent = if wall > 0.0 {
                    (cpu - last_cpu).as_secs_f64() / wall * 100.0
                } else {
                    0.0
                };
                last_cpu = cpu;
                last_tick = now;
                let rss = rss_bytes();
                let mut samples = sampler.samples.lock().unwrap();
                samples.0.push(percent);
                samples.1.push(rss);
            }
        });
    }

    fn json(&self) -> String {
        let samples = self.samples.lock().unwrap();
        let mem: Vec<_> = samples
            .1
            .iter()
            .map(|rss| serde_json::json!({ "rss": rss }))
            .collect();
        serde_json::json!({ "cpu": samples.0, "mem": mem }).to_string()
    }
}

/// User and system CPU time this process has used, over all its threads.
fn cpu_time() -> Duration {
    let mut usage: libc::rusage = unsafe { std::mem::zeroed() };
    if unsafe { libc::getrusage(libc::RUSAGE_SELF, &mut usage) } != 0 {
        return Duration::ZERO;
    }
    let tv = |t: libc::timeval| Duration::new(t.tv_sec as u64, t.tv_usec as u32 * 1000);
    tv(usage.ru_utime) + tv(usage.ru_stime)
}

/// Resident memory now, rather than getrusage's high-water mark.
#[cfg(target_os = "linux")]
fn rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            let line = status.lines().find(|l| l.starts_with("VmRSS:"))?;
            let kb: u64 = line
                .trim_start_matches("VmRSS:")
                .trim()
                .trim_end_matches("kB")
                .trim()
                .parse()
                .ok()?;
            Some(kb * 1024)
        })
        .unwrap_or(0)
}

#[cfg(target_os = "macos")]
fn rss_bytes() -> u64 {
    let mut info: libc::proc_taskinfo = unsafe { std::mem::zeroed() };
    let size = std::mem::size_of::<libc::proc_taskinfo>() as libc::c_int;
    let n = unsafe {
        libc::proc_pidinfo(
            process::id() as libc::c_int,
            libc::PROC_PIDTASKINFO,
            0,
            &mut info as *mut _ as *mut libc::c_void,
            size,
        )
    };
    if n == size { info.pti_resident_size } else { 0 }
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn rss_bytes() -> u64 {
    0
}

/// Go's flag syntax for the flags every server defines: `-name=value` or
/// `-name value`, one dash or two, and a bool flag on its own for true.
fn parse_flags(args: impl Iterator<Item = String>) -> Flags {
    let mut flags = Flags {
        nodelay: true,
        reuseport: true,
        payload: 1024,
        mem_limit: 2 << 30,
    };
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let Some(flag) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
            usage(&format!("unexpected argument {arg:?}"));
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name, Some(value.to_string())),
            None => (flag, None),
        };
        let parse_bool = |value: Option<String>| match value.as_deref() {
            None | Some("true") | Some("1") | Some("t") | Some("T") | Some("TRUE")
            | Some("True") => true,
            Some("false") | Some("0") | Some("f") | Some("F") | Some("FALSE") | Some("False") => {
                false
            }
            Some(v) => usage(&format!("invalid boolean value {v:?} for -{name}")),
        };
        let mut value = |inline: Option<String>| {
            inline
                .or_else(|| args.next())
                .unwrap_or_else(|| usage(&format!("flag needs an argument: -{name}")))
        };
        match name {
            "nodelay" => flags.nodelay = parse_bool(inline),
            "reuseport" => flags.reuseport = parse_bool(inline),
            "b" => {
                let v = value(inline);
                flags.payload = v
                    .parse()
                    .unwrap_or_else(|_| usage(&format!("invalid value {v:?} for -b")));
            }
            "m" => {
                let v = value(inline);
                flags.mem_limit = v
                    .parse()
                    .unwrap_or_else(|_| usage(&format!("invalid value {v:?} for -m")));
            }
            "h" | "help" => usage(""),
            _ => usage(&format!("flag provided but not defined: -{name}")),
        }
    }
    flags
}

fn usage(err: &str) -> ! {
    if !err.is_empty() {
        eprintln!("{err}");
    }
    eprintln!(
        "Usage of {NAME}.server:\n  -b int\n    \texpected request body size (default 1024)\n  -m int\n    \tmemory limit, ignored (default 2147483648)\n  -nodelay\n    \ttcp nodelay (default true)\n  -reuseport\n    \treuse port (default true)"
    );
    process::exit(2);
}

fn fatal(msg: &str) -> ! {
    eprintln!("{NAME} server: {msg}");
    process::exit(1);
}
