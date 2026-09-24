//! The Go client's flags, by the same names, with the same defaults, in Go's
//! flag syntax: `-name=value` or `-name value`, one dash or two, and a bool
//! flag on its own for true. The scripts pass one set of flags whichever
//! client they built, so every flag benchcli-go defines is defined here, and
//! one that is not exits the way Go's flag package does.

use std::time::Duration;

pub struct Flags {
    pub mem_limit: i64,
    pub framework: String,
    pub ip: String,
    pub num_connections: usize,
    pub dial_concurrency: usize,
    pub dial_timeout: Duration,
    pub dial_retries: usize,
    pub dial_retry_interval: Duration,
    pub payload: usize,
    pub check_valid: bool,
    pub ps_interval_ms: u64,
    pub ps_mode: String,
    pub enable_tpn: bool,
    pub echo_concurrency: usize,
    pub echo_times: usize,
    pub echo_tps_limit: usize,
    pub echo_pprof: bool,
    pub echo_pprof_duration: u64,
    pub rate_enabled: bool,
    pub rate_concurrency: usize,
    pub rate_duration: u64,
    pub rate_send_rate: usize,
    pub rate_batch_size: usize,
    pub rate_pipeline: i64,
    pub rate_send_limit: usize,
    pub rate_pprof: bool,
    pub rate_pprof_duration: u64,
    pub gen_report: bool,
    pub preffix: String,
    pub suffix: String,
    pub report_sort: String,
}

impl Default for Flags {
    fn default() -> Self {
        Flags {
            mem_limit: 4 << 30,
            framework: "nethttp".into(),
            ip: "127.0.0.1".into(),
            num_connections: 10000,
            dial_concurrency: 2000,
            dial_timeout: Duration::from_secs(5),
            dial_retries: 5,
            dial_retry_interval: Duration::from_millis(100),
            payload: 1024,
            check_valid: false,
            ps_interval_ms: 1000,
            ps_mode: "auto".into(),
            enable_tpn: true,
            echo_concurrency: 10000,
            echo_times: 2000000,
            echo_tps_limit: 0,
            echo_pprof: false,
            echo_pprof_duration: 5,
            rate_enabled: false,
            rate_concurrency: 10000,
            rate_duration: 10,
            rate_send_rate: 200,
            rate_batch_size: 16 * 1024,
            rate_pipeline: 0,
            rate_send_limit: 0,
            rate_pprof: false,
            rate_pprof_duration: 5,
            gen_report: false,
            preffix: String::new(),
            suffix: String::new(),
            report_sort: "result".into(),
        }
    }
}

enum Kind {
    Bool,
    Value,
}

// name, kind, usage - in benchcli-go's order.
const FLAGS: &[(&str, Kind, &str)] = &[
    (
        "nodelay",
        Kind::Bool,
        "tcp nodelay, always set (default true)",
    ),
    (
        "m",
        Kind::Value,
        "memory limit, ignored: no GC to limit (default 4294967296)",
    ),
    (
        "f",
        Kind::Value,
        "framework, e.g. \"nethttp\" (default \"nethttp\")",
    ),
    (
        "ip",
        Kind::Value,
        "ip, e.g. \"127.0.0.1\" (default \"127.0.0.1\")",
    ),
    (
        "c",
        Kind::Value,
        "client: num of connections (default 10000)",
    ),
    ("dc", Kind::Value, "client: dial concurrency (default 2000)"),
    (
        "dt",
        Kind::Value,
        "client: dial timeout, which also bounds the first request on a connection (default 5s)",
    ),
    ("dr", Kind::Value, "client: dial retry times (default 5)"),
    (
        "dri",
        Kind::Value,
        "client: dial retry interval (default 100ms)",
    ),
    (
        "b",
        Kind::Value,
        "benchmark: request body size, which the server echoes back (default 1024)",
    ),
    (
        "check",
        Kind::Bool,
        "benchmark: whether to check the validity of the response data",
    ),
    (
        "pi",
        Kind::Value,
        "benchmark: ps interval in ms (default 1000)",
    ),
    (
        "ps",
        Kind::Value,
        "benchmark: where the server's CPU and MEM samples come from: auto, local or remote (default \"auto\")",
    ),
    (
        "tpn",
        Kind::Bool,
        "benchmark: whether enable TPN caculation (default true)",
    ),
    ("ec", Kind::Value, "benchecho: concurrency (default 10000)"),
    (
        "en",
        Kind::Value,
        "benchecho: benchmark times (default 2000000)",
    ),
    ("el", Kind::Value, "benchecho: TPS limitation per second"),
    ("ep", Kind::Bool, "benchecho: generate pprof report"),
    ("epd", Kind::Value, "benchecho: pprof duration (default 5)"),
    ("rate", Kind::Bool, "benchrate: whether run benchrate"),
    (
        "rc",
        Kind::Value,
        "benchrate: concurrency: how many tasks write the pipelined requests (default 10000)",
    ),
    (
        "rd",
        Kind::Value,
        "benchrate: how long to spend to do the test (default 10)",
    ),
    (
        "rr",
        Kind::Value,
        "benchrate: how many requests can be sent to 1 conn every second (default 200)",
    ),
    (
        "rbs",
        Kind::Value,
        "benchrate: how many bytes of pipelined requests can be written to 1 conn every time, when -rpl is 0 (default 16384)",
    ),
    (
        "rpl",
        Kind::Value,
        "benchrate: pipeline: how many requests are merged into one write to 1 conn, which must divide -rr; 0 takes as many as fit in -rbs bytes",
    ),
    (
        "rl",
        Kind::Value,
        "benchrate: request sending limitation per second",
    ),
    ("rp", Kind::Bool, "benchrate: generate pprof report"),
    ("rpd", Kind::Value, "benchrate: pprof duration (default 5)"),
    (
        "r",
        Kind::Bool,
        "make report: done by the Go client, see script/report.sh",
    ),
    (
        "preffix",
        Kind::Value,
        "report file preffix, e.g. \"1m_connections_\"",
    ),
    (
        "suffix",
        Kind::Value,
        "report file suffix, e.g. \"_20060102150405\"",
    ),
    (
        "sort",
        Kind::Value,
        "report row order: \"result\" or \"framework\" (default \"result\")",
    ),
];

pub fn parse(args: impl Iterator<Item = String>) -> Flags {
    let mut flags = Flags::default();
    let mut args = args.peekable();
    while let Some(arg) = args.next() {
        let Some(flag) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
            // Go's flag package stops at the first non-flag argument.
            break;
        };
        let (name, inline) = match flag.split_once('=') {
            Some((name, value)) => (name.to_string(), Some(value.to_string())),
            None => (flag.to_string(), None),
        };
        if name == "h" || name == "help" {
            usage("");
        }
        let Some((_, kind, _)) = FLAGS.iter().find(|(n, _, _)| *n == name) else {
            usage(&format!("flag provided but not defined: -{name}"));
        };
        let value = match kind {
            Kind::Bool => inline.unwrap_or_else(|| "true".into()),
            Kind::Value => inline
                .or_else(|| args.next())
                .unwrap_or_else(|| usage(&format!("flag needs an argument: -{name}"))),
        };
        set(&mut flags, &name, &value)
            .unwrap_or_else(|_| usage(&format!("invalid value {value:?} for flag -{name}")));
    }
    flags
}

fn set(f: &mut Flags, name: &str, v: &str) -> Result<(), ()> {
    let num = |v: &str| v.parse::<usize>().map_err(|_| ());
    let int = |v: &str| v.parse::<i64>().map_err(|_| ());
    let secs = |v: &str| v.parse::<u64>().map_err(|_| ());
    match name {
        "nodelay" => {
            parse_bool(v)?;
        }
        "m" => f.mem_limit = int(v)?,
        "f" => f.framework = v.into(),
        "ip" => f.ip = v.into(),
        "c" => f.num_connections = num(v)?,
        "dc" => f.dial_concurrency = num(v)?,
        "dt" => f.dial_timeout = parse_duration(v)?,
        "dr" => f.dial_retries = num(v)?,
        "dri" => f.dial_retry_interval = parse_duration(v)?,
        "b" => f.payload = num(v)?,
        "check" => f.check_valid = parse_bool(v)?,
        "pi" => f.ps_interval_ms = secs(v)?,
        "ps" => f.ps_mode = v.into(),
        "tpn" => f.enable_tpn = parse_bool(v)?,
        "ec" => f.echo_concurrency = num(v)?,
        "en" => f.echo_times = num(v)?,
        "el" => f.echo_tps_limit = num(v)?,
        "ep" => f.echo_pprof = parse_bool(v)?,
        "epd" => f.echo_pprof_duration = secs(v)?,
        "rate" => f.rate_enabled = parse_bool(v)?,
        "rc" => f.rate_concurrency = num(v)?,
        "rd" => f.rate_duration = secs(v)?,
        "rr" => f.rate_send_rate = num(v)?,
        "rbs" => f.rate_batch_size = num(v)?,
        "rpl" => f.rate_pipeline = int(v)?,
        "rl" => f.rate_send_limit = num(v)?,
        "rp" => f.rate_pprof = parse_bool(v)?,
        "rpd" => f.rate_pprof_duration = secs(v)?,
        "r" => f.gen_report = parse_bool(v)?,
        "preffix" => f.preffix = v.into(),
        "suffix" => f.suffix = v.into(),
        "sort" => f.report_sort = v.into(),
        _ => return Err(()),
    }
    Ok(())
}

/// strconv.ParseBool's spellings.
fn parse_bool(v: &str) -> Result<bool, ()> {
    match v {
        "1" | "t" | "T" | "true" | "TRUE" | "True" => Ok(true),
        "0" | "f" | "F" | "false" | "FALSE" | "False" => Ok(false),
        _ => Err(()),
    }
}

/// time.ParseDuration: a sequence of decimal numbers, each with a unit - ns,
/// us (or µs), ms, s, m, h - such as "300ms" or "1m30s"; "0" alone is zero.
pub fn parse_duration(s: &str) -> Result<Duration, ()> {
    if s == "0" {
        return Ok(Duration::ZERO);
    }
    let mut rest = s;
    let mut total = 0f64;
    if rest.is_empty() {
        return Err(());
    }
    while !rest.is_empty() {
        let n = rest
            .find(|c: char| !(c.is_ascii_digit() || c == '.'))
            .ok_or(())?;
        let value: f64 = rest[..n].parse().map_err(|_| ())?;
        rest = &rest[n..];
        let u = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let unit = match &rest[..u] {
            "ns" => 1.0,
            "us" | "µs" => 1e3,
            "ms" => 1e6,
            "s" => 1e9,
            "m" => 60e9,
            "h" => 3600e9,
            _ => return Err(()),
        };
        total += value * unit;
        rest = &rest[u..];
    }
    Ok(Duration::from_nanos(total as u64))
}

fn usage(err: &str) -> ! {
    if !err.is_empty() {
        eprintln!("{err}");
    }
    eprintln!("Usage of bench.client (benchcli-rust):");
    for (name, kind, text) in FLAGS {
        let arg = match kind {
            Kind::Bool => "",
            Kind::Value => " value",
        };
        eprintln!("  -{name}{arg}\n    \t{text}");
    }
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn go_flag_syntax() {
        let args = [
            "-f=axum",
            "-c",
            "100",
            "--check",
            "-ep=true",
            "-dt=1m30s",
            "-rpl=25",
        ];
        let f = parse(args.iter().map(|s| s.to_string()));
        assert_eq!(f.framework, "axum");
        assert_eq!(f.num_connections, 100);
        assert!(f.check_valid);
        assert!(f.echo_pprof);
        assert!(!Flags::default().echo_pprof);
        assert_eq!(f.dial_timeout, Duration::from_secs(90));
        assert_eq!(f.rate_pipeline, 25);
    }

    #[test]
    fn durations() {
        assert_eq!(parse_duration("100ms"), Ok(Duration::from_millis(100)));
        assert_eq!(parse_duration("1.5s"), Ok(Duration::from_millis(1500)));
        assert_eq!(parse_duration("0"), Ok(Duration::ZERO));
        assert!(parse_duration("5").is_err());
        assert!(parse_duration("5x").is_err());
    }
}
