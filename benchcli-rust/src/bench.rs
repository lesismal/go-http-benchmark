//! The three benchmarks, as benchcli-go runs them: Connections dials the
//! keep-alive connections, BenchEcho makes request/response round trips on
//! them, and BenchPipeline pipelines requests on them at a set rate.

use std::collections::HashMap;
use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicI64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use tokio::io::AsyncWriteExt;
use tokio::net::TcpStream;
use tokio::sync::Mutex;

use crate::config;
use crate::http::{NoLength, RBuf, batch_buffers, encode_request, pipeline_buffers};
use crate::logf;
use crate::stats::Calc;

/// One keep-alive connection to the server and the buffer its responses are
/// parsed from. A request that failed leaves the stream at an unknown point,
/// so a broken connection carries nothing more until it is redialed.
pub struct Conn {
    stream: TcpStream,
    rbuf: RBuf,
    addr: String,
    broken: bool,
}

/// What every dial needs: the request that proves the server is serving the
/// connection, and how long to wait for it.
pub struct Dialer {
    request: Vec<u8>,
    timeout: Duration,
    rbuf_size: usize,
}

impl Dialer {
    pub fn new(ip: &str, timeout: Duration, rbuf_size: usize) -> Self {
        Dialer {
            request: encode_request("GET", &config::url_host(ip), config::ECHO_PATH, None),
            timeout,
            rbuf_size,
        }
    }

    /// connections.dial: a TCP connection and one GET answered on it.
    async fn dial(&self, addr: &str) -> io::Result<(TcpStream, RBuf)> {
        let fut = async {
            let mut stream = TcpStream::connect(addr).await?;
            stream.set_nodelay(true)?;
            stream.write_all(&self.request).await?;
            let mut rbuf = RBuf::new(self.rbuf_size);
            let (status, _) = rbuf
                .read_response(&mut stream, None, NoLength::Error)
                .await?;
            if status != 200 {
                return Err(io::Error::other(format!(
                    "GET {}: status {status}",
                    config::ECHO_PATH
                )));
            }
            Ok((stream, rbuf))
        };
        tokio::time::timeout(self.timeout, fut)
            .await
            .map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "i/o timeout"))?
    }

    async fn redial(&self, c: &mut Conn) -> io::Result<()> {
        let (stream, rbuf) = self.dial(&c.addr).await?;
        c.stream = stream;
        c.rbuf = rbuf;
        c.broken = false;
        Ok(())
    }
}

fn default_concurrency() -> usize {
    std::thread::available_parallelism().map_or(1, |n| n.get()) * 1000
}

pub struct ConnectionsResult {
    pub calc: Calc,
    pub concurrency: usize,
    pub conns: Vec<Conn>,
}

/// connections.Run.
pub async fn connections(
    framework: &str,
    ip: &str,
    num: usize,
    concurrency: usize,
    retries: usize,
    retry_interval: Duration,
    dialer: Arc<Dialer>,
) -> ConnectionsResult {
    let num = if num == 0 { 1000 } else { num };
    let concurrency = if concurrency == 0 {
        default_concurrency()
    } else {
        concurrency
    }
    .min(num);
    let retries = if retries == 0 { 3 } else { retries };
    let retry_interval = if retry_interval.is_zero() {
        Duration::from_millis(100)
    } else {
        retry_interval
    };
    logf!("Dial Connections: [{num}]");
    logf!("Dial Concurrency: [{concurrency}]");

    let addrs = Arc::new(config::benchmark_addrs(framework, ip));
    let next = Arc::new(AtomicUsize::new(0));
    let server_idx = Arc::new(AtomicUsize::new(0));
    let success = Arc::new(AtomicI64::new(0));
    let conns = Arc::new(std::sync::Mutex::new(Vec::with_capacity(num)));

    let progress = {
        let success = Arc::clone(&success);
        tokio::spawn(async move {
            let mut i = 0;
            loop {
                i += 1;
                tokio::time::sleep(Duration::from_secs(1)).await;
                logf!(
                    "{i:03} seconds passed, {} Connected ...",
                    success.load(Ordering::Relaxed)
                );
            }
        })
    };

    logf!("Connections start ...");
    let begin = Instant::now();
    let mut tasks = Vec::with_capacity(concurrency);
    for _ in 0..concurrency {
        let (addrs, next, server_idx, success, conns, dialer) = (
            Arc::clone(&addrs),
            Arc::clone(&next),
            Arc::clone(&server_idx),
            Arc::clone(&success),
            Arc::clone(&conns),
            Arc::clone(&dialer),
        );
        tasks.push(tokio::spawn(async move {
            let (mut costs, mut failed) = (Vec::new(), 0i64);
            while next.fetch_add(1, Ordering::Relaxed) < num {
                let t = Instant::now();
                let mut dialed = None;
                for attempt in 0..retries {
                    if attempt > 0 {
                        tokio::time::sleep(retry_interval).await;
                    }
                    let addr =
                        &addrs[(server_idx.fetch_add(1, Ordering::Relaxed) + 1) % addrs.len()];
                    if let Ok((stream, rbuf)) = dialer.dial(addr).await {
                        dialed = Some(Conn {
                            stream,
                            rbuf,
                            addr: addr.clone(),
                            broken: false,
                        });
                        break;
                    }
                }
                match dialed {
                    Some(c) => {
                        costs.push(t.elapsed().as_nanos() as i64);
                        success.fetch_add(1, Ordering::Relaxed);
                        conns.lock().unwrap().push(c);
                    }
                    None => failed += 1,
                }
            }
            (costs, failed)
        }));
    }
    let (mut costs, mut failed) = (Vec::with_capacity(num), 0);
    for t in tasks {
        let (c, f) = t.await.expect("dial task");
        costs.extend(c);
        failed += f;
    }
    let used = begin.elapsed();
    progress.abort();
    let calc = Calc::new(used, costs, failed);
    logf!(
        "Connections done: {} Success, {} Failed",
        calc.success,
        calc.failed
    );
    let conns = std::mem::take(&mut *conns.lock().unwrap());
    ConnectionsResult {
        calc,
        concurrency,
        conns,
    }
}

/// A rate as golang.org/x/time/rate would give -el and -rl if it were set
/// to what their usage says: that many a second, spaced evenly.
struct Limiter {
    per_second: usize,
    next: Mutex<Instant>,
}

impl Limiter {
    fn new(per_second: usize) -> Option<Arc<Self>> {
        (per_second > 0).then(|| {
            Arc::new(Limiter {
                per_second,
                next: Mutex::new(Instant::now()),
            })
        })
    }

    async fn wait(&self, n: usize) {
        let at = {
            let mut next = self.next.lock().await;
            let at = (*next).max(Instant::now());
            *next = at + Duration::from_secs_f64(n as f64 / self.per_second as f64);
            at
        };
        tokio::time::sleep_until(at.into()).await;
    }
}

/// A small, fast, per-task generator for the random payloads and for which
/// of them the next request carries.
struct XorShift(u64);

impl XorShift {
    fn seeded(salt: u64) -> Self {
        let t = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64;
        XorShift((t ^ salt.wrapping_mul(0x9E37_79B9_7F4A_7C15)) | 1)
    }

    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(8) {
            let v = self.next().to_le_bytes();
            chunk.copy_from_slice(&v[..chunk.len()]);
        }
    }
}

fn random_payload(payload: usize, salt: u64) -> Vec<u8> {
    let mut buf = vec![0; payload];
    XorShift::seeded(salt).fill(&mut buf);
    buf
}

pub struct EchoConfig {
    pub concurrency: usize,
    pub total: usize,
    pub payload: usize,
    pub limit: usize,
    pub check: bool,
    pub ip: String,
}

pub struct EchoResult {
    pub calc: Calc,
    pub concurrency: usize,
    pub conns: Vec<Conn>,
}

/// One echo request/response: a POST of one of the payloads, and the same
/// bytes back.
struct EchoShared {
    requests: Vec<(Vec<u8>, Vec<u8>)>,
    next: AtomicUsize,
    total: usize,
    check: bool,
    payload: usize,
    limiter: Option<Arc<Limiter>>,
    dialer: Arc<Dialer>,
}

/// benchecho.Run: a warmup of five round trips per connection - two million
/// at most - and then cfg.total of them, measured, each connection carrying
/// one request at a time. Every task has connections of its own, so no
/// connection is ever wanted by two tasks at once.
pub async fn echo(
    conns: Vec<Conn>,
    cfg: EchoConfig,
    dialer: Arc<Dialer>,
    on_warmup: impl FnOnce(),
) -> EchoResult {
    let concurrency = if cfg.concurrency == 0 {
        default_concurrency()
    } else {
        cfg.concurrency
    }
    .min(conns.len());
    if concurrency == 0 {
        crate::fatalf!("BenchEcho: no connections to run on");
    }
    let payload = if cfg.payload == 0 { 1024 } else { cfg.payload };
    let host = config::url_host(&cfg.ip);
    let requests = (0..1024)
        .map(|i| {
            let body = random_payload(payload, i);
            let req = encode_request("POST", &host, config::ECHO_PATH, Some(&body));
            (body, req)
        })
        .collect();
    let warmup = (conns.len() * 5).min(2_000_000);
    let mut shared = EchoShared {
        requests,
        next: AtomicUsize::new(0),
        total: warmup,
        check: cfg.check,
        payload,
        limiter: Limiter::new(cfg.limit),
        dialer,
    };

    let mut groups: Vec<Vec<Conn>> = (0..concurrency).map(|_| Vec::new()).collect();
    for (i, c) in conns.into_iter().enumerate() {
        groups[i % concurrency].push(c);
    }

    logf!("BenchEcho Warmup for {warmup} times ...");
    on_warmup();
    let (g, _, _, _) = echo_phase(groups, shared).await;
    groups = g.0;
    shared = g.1;
    logf!("BenchEcho Warmup for {warmup} times done");

    shared.next.store(0, Ordering::SeqCst);
    shared.total = cfg.total;
    logf!("BenchEcho for {} times ...", cfg.total);
    let begin = Instant::now();
    let ((groups, _), costs, failed, errors) = echo_phase(groups, shared).await;
    let used = begin.elapsed();
    logf!("BenchEcho for {} times done", cfg.total);
    if !errors.is_empty() {
        logf!("BenchEcho errors: {errors:?}");
    }
    EchoResult {
        calc: Calc::new(used, costs, failed),
        concurrency,
        conns: groups.into_iter().flatten().collect(),
    }
}

type Groups = (Vec<Vec<Conn>>, EchoShared);

async fn echo_phase(
    groups: Vec<Vec<Conn>>,
    shared: EchoShared,
) -> (Groups, Vec<i64>, i64, HashMap<String, usize>) {
    let shared = Arc::new(shared);
    let mut tasks = Vec::with_capacity(groups.len());
    for (i, mut group) in groups.into_iter().enumerate() {
        let shared = Arc::clone(&shared);
        tasks.push(tokio::spawn(async move {
            let mut rng = XorShift::seeded(i as u64 + 1);
            let (mut costs, mut failed, mut errors) =
                (Vec::new(), 0i64, HashMap::<String, usize>::new());
            let mut body = Vec::new();
            let mut at = 0;
            while shared.next.fetch_add(1, Ordering::Relaxed) < shared.total {
                let this = at;
                at = (at + 1) % group.len();
                let c = &mut group[this];
                let t = Instant::now();
                match echo_once(c, &shared, &mut rng, &mut body).await {
                    Ok(()) => costs.push(t.elapsed().as_nanos() as i64),
                    Err(err) => {
                        failed += 1;
                        *errors.entry(err).or_default() += 1;
                    }
                }
            }
            (group, costs, failed, errors)
        }));
    }
    let (mut groups, mut costs, mut failed, mut errors) =
        (Vec::new(), Vec::new(), 0, HashMap::new());
    for t in tasks {
        let (g, c, f, e) = t.await.expect("echo task");
        groups.push(g);
        costs.extend(c);
        failed += f;
        for (k, v) in e {
            *errors.entry(k).or_default() += v;
        }
    }
    let shared = Arc::try_unwrap(shared).ok().expect("echo tasks done");
    ((groups, shared), costs, failed, errors)
}

async fn echo_once(
    c: &mut Conn,
    s: &EchoShared,
    rng: &mut XorShift,
    body: &mut Vec<u8>,
) -> Result<(), String> {
    if c.broken {
        s.dialer
            .redial(c)
            .await
            .map_err(|err| format!("redial: {err}"))?;
    }
    if let Some(l) = &s.limiter {
        l.wait(1).await;
    }
    let (payload, request) = &s.requests[(rng.next() % s.requests.len() as u64) as usize];
    if let Err(err) = c.stream.write_all(request).await {
        c.broken = true;
        return Err(err.to_string());
    }
    let dst = if s.check { Some(&mut *body) } else { None };
    let (status, n) = match c
        .rbuf
        .read_response(&mut c.stream, dst, NoLength::Error)
        .await
    {
        Ok(v) => v,
        Err(err) => {
            c.broken = true;
            return Err(err.to_string());
        }
    };
    if status != 200 {
        return Err(format!("status {status}"));
    }
    if s.check && body.as_slice() != payload.as_slice() {
        return Err("response body is not equal to the request's".into());
    }
    if n != s.payload {
        return Err(format!("response body is {n} bytes, want {}", s.payload));
    }
    Ok(())
}

pub struct PipelineConfig {
    pub concurrency: usize,
    pub duration: Duration,
    pub send_rate: usize,
    pub batch_size: usize,
    pub pipeline: usize,
    pub payload: usize,
    pub send_limit: usize,
    pub check: bool,
    pub ip: String,
}

#[derive(Default)]
pub struct PipelineResult {
    pub concurrency: usize,
    pub batch: usize,
    pub send_times: i64,
    pub send_bytes: i64,
    pub recv_times: i64,
    pub recv_bytes: i64,
}

#[derive(Default)]
struct Counters {
    send_times: AtomicI64,
    send_bytes: AtomicI64,
    recv_times: AtomicI64,
    recv_bytes: AtomicI64,
    // Every response, whatever it said, where recv_times counts only the
    // 200s with the body that was sent.
    answered: AtomicI64,
}

/// benchrate.maxBatchesInFlight.
const MAX_BATCHES_IN_FLIGHT: i64 = 4;

/// benchrate.Run: every connection is sent cfg.send_rate requests a second,
/// written a batch at a time without waiting for the ones before, and a task
/// per connection reads the responses back. A connection with more than a
/// few batches unanswered is skipped for a tick.
pub async fn pipeline(
    conns: Vec<Conn>,
    cfg: PipelineConfig,
    dialer: Arc<Dialer>,
    on_benchmark: impl FnOnce(),
) -> PipelineResult {
    let concurrency = if cfg.concurrency == 0 {
        50000
    } else {
        cfg.concurrency
    }
    .min(conns.len());
    if concurrency == 0 {
        crate::fatalf!("BenchPipeline: no connections to run on");
    }
    let send_rate = cfg.send_rate.max(1);
    let payload = if cfg.payload == 0 { 1024 } else { cfg.payload };
    let body = Arc::new(random_payload(payload, 0xB0D1));
    let request = encode_request(
        "POST",
        &config::url_host(&cfg.ip),
        config::ECHO_PATH,
        Some(&body),
    );
    let (batch_buffer, batch, tick_rate) = if cfg.pipeline > 0 {
        pipeline_buffers(&request, send_rate, cfg.pipeline)
    } else {
        batch_buffers(&request, send_rate, cfg.batch_size)
    };
    if tick_rate == 0 || batch_buffer.is_empty() {
        crate::fatalf!(
            "BenchPipeline got a wrong tickRate: {tick_rate}, or batchBuffer: {}",
            batch_buffer.len()
        );
    }
    let batch_buffer = Arc::new(batch_buffer);
    let limiter = Limiter::new(cfg.send_limit);
    let counters = Arc::new(Counters::default());

    let mut writers = Vec::new();
    let mut readers = Vec::new();
    for mut c in conns {
        if c.broken {
            if let Err(err) = dialer.redial(&mut c).await {
                logf!(
                    "BenchPipeline: redial {} failed, leaving the connection out: {err}",
                    c.addr
                );
                continue;
            }
        }
        let (mut rh, wh) = c.stream.into_split();
        let mut rbuf = c.rbuf;
        let inflight = Arc::new((AtomicI64::new(0), AtomicI64::new(0)));
        let (counters, body, check, flight) = (
            Arc::clone(&counters),
            Arc::clone(&body),
            cfg.check,
            Arc::clone(&inflight),
        );
        readers.push(tokio::spawn(async move {
            let mut got = Vec::new();
            loop {
                let dst = if check { Some(&mut got) } else { None };
                let Ok((status, n)) = rbuf.read_response(&mut rh, dst, NoLength::Error).await
                else {
                    return;
                };
                flight.1.fetch_add(1, Ordering::Relaxed);
                counters.answered.fetch_add(1, Ordering::Relaxed);
                if status == 200 && (!check || got.as_slice() == body.as_slice()) {
                    counters.recv_times.fetch_add(1, Ordering::Relaxed);
                    counters.recv_bytes.fetch_add(n as i64, Ordering::Relaxed);
                }
            }
        }));
        writers.push((wh, inflight));
    }

    let mut teams: Vec<Vec<_>> = (0..concurrency).map(|_| Vec::new()).collect();
    for (i, w) in writers.into_iter().enumerate() {
        teams[i % concurrency].push(w);
    }

    logf!(
        "BenchPipeline for {:.2} seconds, {batch} requests pipelined per write ...",
        cfg.duration.as_secs_f64()
    );
    on_benchmark();
    let period = Duration::from_secs(1) / tick_rate as u32;
    let deadline = tokio::time::Instant::now() + cfg.duration;
    let mut tasks = Vec::with_capacity(teams.len());
    for mut team in teams {
        let (batch_buffer, counters, limiter) = (
            Arc::clone(&batch_buffer),
            Arc::clone(&counters),
            limiter.clone(),
        );
        tasks.push(tokio::spawn(async move {
            let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + period, period);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = tokio::time::sleep_until(deadline) => return team,
                    _ = ticker.tick() => {}
                }
                for (wh, flight) in team.iter_mut() {
                    if flight.0.load(Ordering::Relaxed) - flight.1.load(Ordering::Relaxed)
                        >= batch as i64 * MAX_BATCHES_IN_FLIGHT
                    {
                        continue;
                    }
                    if let Some(l) = &limiter {
                        l.wait(batch).await;
                    }
                    if wh.write_all(&batch_buffer).await.is_ok() {
                        counters
                            .send_times
                            .fetch_add(batch as i64, Ordering::Relaxed);
                        counters
                            .send_bytes
                            .fetch_add((batch * payload) as i64, Ordering::Relaxed);
                        flight.0.fetch_add(batch as i64, Ordering::Relaxed);
                    }
                }
            }
        }));
    }
    // The write halves are kept, not dropped, until the last batch has had
    // its tick to come back: dropping a tokio OwnedWriteHalf shuts the
    // connection down for writing, and a server that reads the batch and the
    // FIN together may close the connection without answering the requests it
    // has read but not yet served - nbio does - which would count against it
    // responses it was never given the time to send.
    let mut write_halves = Vec::with_capacity(tasks.len());
    for t in tasks {
        if let Ok(team) = t.await {
            write_halves.push(team);
        }
    }

    // One tick more for the last batch to come back, and no longer.
    let grace = Instant::now() + period;
    while counters.answered.load(Ordering::Relaxed) < counters.send_times.load(Ordering::Relaxed)
        && Instant::now() < grace
    {
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    let result = PipelineResult {
        concurrency,
        batch,
        send_times: counters.send_times.load(Ordering::Relaxed),
        send_bytes: counters.send_bytes.load(Ordering::Relaxed),
        recv_times: counters.recv_times.load(Ordering::Relaxed),
        recv_bytes: counters.recv_bytes.load(Ordering::Relaxed),
    };
    for r in readers {
        r.abort();
    }
    drop(write_halves);
    logf!(
        "BenchPipeline for {:.2} seconds done",
        cfg.duration.as_secs_f64()
    );
    result
}
