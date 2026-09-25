//! The three reports benchcli-go writes, field for field and under the same
//! JSON names, so that the Go client's report step reads them into the same
//! tables. config's tests hold the names here to the Go structs' json tags.

use serde::Serialize;

use crate::stats::{mem_string, time_string};

pub const BENCH_CLIENT: &str = "benchcli-rust";

#[derive(Serialize, Default)]
pub struct ConnectionsReport {
    #[serde(rename = "Framework")]
    pub framework: String,
    #[serde(rename = "Lang")]
    pub lang: String,
    #[serde(rename = "BenchClient")]
    pub bench_client: String,
    #[serde(rename = "TPS")]
    pub tps: i64,
    #[serde(rename = "Min")]
    pub min: i64,
    #[serde(rename = "Avg")]
    pub avg: i64,
    #[serde(rename = "Max")]
    pub max: i64,
    #[serde(rename = "TP50")]
    pub tp50: i64,
    #[serde(rename = "TP75")]
    pub tp75: i64,
    #[serde(rename = "TP90")]
    pub tp90: i64,
    #[serde(rename = "TP95")]
    pub tp95: i64,
    #[serde(rename = "TP99")]
    pub tp99: i64,
    #[serde(rename = "Used")]
    pub used: i64,
    #[serde(rename = "Total")]
    pub total: usize,
    #[serde(rename = "Success")]
    pub success: i64,
    #[serde(rename = "Failed")]
    pub failed: i64,
    #[serde(rename = "Concurrency")]
    pub concurrency: usize,
}

#[derive(Serialize, Default)]
pub struct BenchEchoReport {
    #[serde(rename = "Framework")]
    pub framework: String,
    #[serde(rename = "Lang")]
    pub lang: String,
    #[serde(rename = "BenchClient")]
    pub bench_client: String,
    #[serde(rename = "TPS")]
    pub tps: i64,
    #[serde(rename = "EER")]
    pub eer: f64,
    #[serde(rename = "Min")]
    pub min: i64,
    #[serde(rename = "Avg")]
    pub avg: i64,
    #[serde(rename = "Max")]
    pub max: i64,
    #[serde(rename = "TP50")]
    pub tp50: i64,
    #[serde(rename = "TP75")]
    pub tp75: i64,
    #[serde(rename = "TP90")]
    pub tp90: i64,
    #[serde(rename = "TP95")]
    pub tp95: i64,
    #[serde(rename = "TP99")]
    pub tp99: i64,
    #[serde(rename = "Used")]
    pub used: i64,
    #[serde(rename = "Total")]
    pub total: usize,
    #[serde(rename = "Success")]
    pub success: i64,
    #[serde(rename = "Failed")]
    pub failed: i64,
    #[serde(rename = "Conns")]
    pub connections: usize,
    #[serde(rename = "Concurrency")]
    pub concurrency: usize,
    #[serde(rename = "Payload")]
    pub payload: usize,
    #[serde(rename = "Pprof")]
    pub pprof: bool,
    #[serde(rename = "CPUMin")]
    pub cpu_min: f64,
    #[serde(rename = "CPUAvg")]
    pub cpu_avg: f64,
    #[serde(rename = "CPUMax")]
    pub cpu_max: f64,
    #[serde(rename = "MEMMin")]
    pub mem_min: u64,
    #[serde(rename = "MEMAvg")]
    pub mem_avg: u64,
    #[serde(rename = "MEMMax")]
    pub mem_max: u64,
}

/// The pipelined rate benchmark's report, BenchRateReport in benchcli-go and
/// BenchPipeline in its file names.
#[derive(Serialize, Default)]
pub struct BenchRateReport {
    #[serde(rename = "Framework")]
    pub framework: String,
    #[serde(rename = "Lang")]
    pub lang: String,
    #[serde(rename = "BenchClient")]
    pub bench_client: String,
    #[serde(rename = "Duration")]
    pub duration: i64,
    #[serde(rename = "TPS")]
    pub tps: i64,
    #[serde(rename = "EchoEER")]
    pub echo_eer: f64,
    #[serde(rename = "SendTimes")]
    pub send_times: i64,
    #[serde(rename = "SendBytes")]
    pub send_bytes: i64,
    #[serde(rename = "RecvTimes")]
    pub recv_times: i64,
    #[serde(rename = "RecvBytes")]
    pub recv_bytes: i64,
    #[serde(rename = "Conns")]
    pub connections: usize,
    #[serde(rename = "Concurrency")]
    pub concurrency: usize,
    #[serde(rename = "SendRate")]
    pub send_rate: usize,
    #[serde(rename = "Pipeline")]
    pub pipeline: usize,
    #[serde(rename = "Payload")]
    pub payload: usize,
    #[serde(rename = "Pprof")]
    pub pprof: bool,
    #[serde(rename = "CPUMin")]
    pub cpu_min: f64,
    #[serde(rename = "CPUAvg")]
    pub cpu_avg: f64,
    #[serde(rename = "CPUMax")]
    pub cpu_max: f64,
    #[serde(rename = "MEMMin")]
    pub mem_min: u64,
    #[serde(rename = "MEMAvg")]
    pub mem_avg: u64,
    #[serde(rename = "MEMMax")]
    pub mem_max: u64,
    /// report.BenchRateReport's Skipped: BenchPipeline was not run, because
    /// the framework's server does not support pipelining.
    #[serde(rename = "Skipped")]
    pub skipped: bool,
}

/// report.Filename.
pub fn filename(base: &str, preffix: &str, suffix: &str) -> String {
    format!("./output/report/{preffix}{base}{suffix}")
}

/// report.ToFile: the report's JSON, and the pprof profiles when there are
/// any, next to the Go client's.
pub fn to_file<T: Serialize>(
    report: &T,
    name: &str,
    kind: &str,
    preffix: &str,
    suffix: &str,
    pprof: Option<&(Vec<u8>, Vec<u8>)>,
) {
    let write = |ext: &str, data: &[u8]| {
        let path = filename(name, preffix, &format!("{suffix}{ext}"));
        if let Err(err) = std::fs::write(&path, data) {
            crate::logf!("{name}: writing the {kind} report {path} failed: {err}");
        }
    };
    if let Some((cpu, mem)) = pprof {
        write(".pprof.cpu", cpu);
        write(".pprof.mem", mem);
    }
    match serde_json::to_vec(report) {
        Ok(json) => write(".json", &json),
        Err(err) => crate::logf!("{name}: encoding the {kind} report failed: {err}"),
    }
}

/// report.ObjString: the block each benchmark prints as it finishes, one
/// "Header: value" line per column the tables show, with the TPN ones only
/// under -tpn.
pub fn console(kind: &str, rows: &[(&str, String, bool)], enable_tpn: bool) -> String {
    let rows: Vec<&(&str, String, bool)> = rows.iter().filter(|r| enable_tpn || !r.2).collect();
    let width = rows
        .iter()
        .map(|r| r.0.len())
        .chain(["BenchType".len()])
        .max()
        .unwrap_or(0);
    let mut out = format!("{:width$}: {kind}\n", "BenchType");
    let lines: Vec<String> = rows
        .iter()
        .map(|(h, v, _)| format!("{h:width$}: {v}"))
        .collect();
    out.push_str(&lines.join("\n"));
    out
}

/// How the Summary shows this client, "<lang>-<framework>": the same name
/// benchcli-go's report package gives "benchcli-rust".
pub fn client_name() -> String {
    "rust-tokio".to_string()
}

pub fn cpu(v: f64) -> String {
    format!("{v:.2}%")
}

pub fn dur(v: i64) -> String {
    time_string(v)
}

pub fn mem(v: u64) -> String {
    mem_string(v)
}
