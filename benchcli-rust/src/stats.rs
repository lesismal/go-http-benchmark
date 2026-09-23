//! github.com/lesismal/perf's numbers, computed the same way, so that a row
//! the Rust client wrote reads like one the Go client wrote: Calculator's
//! TPS, Min, Avg, Max and TPN, and PSCounter's CPU and MEM columns.

use std::time::Duration;

/// A phase's outcome: how long it took and what each successful operation
/// cost, in nanoseconds.
#[derive(Default)]
pub struct Calc {
    pub success: i64,
    pub failed: i64,
    pub used: Duration,
    costs: Vec<i64>,
}

impl Calc {
    pub fn new(used: Duration, mut costs: Vec<i64>, failed: i64) -> Self {
        costs.sort_unstable();
        Calc {
            success: costs.len() as i64,
            failed,
            used,
            costs,
        }
    }

    /// Calculator.TPS: successes per second of the whole phase.
    pub fn tps(&self) -> i64 {
        let secs = self.used.as_secs_f64();
        if secs <= 0.0 {
            0
        } else {
            (self.success as f64 / secs) as i64
        }
    }

    pub fn min(&self) -> i64 {
        self.costs.first().copied().unwrap_or(0)
    }

    pub fn max(&self) -> i64 {
        self.costs.last().copied().unwrap_or(0)
    }

    /// perf.Avg: the mean of the successes, integer-divided.
    pub fn avg(&self) -> i64 {
        if self.costs.is_empty() {
            return 0;
        }
        self.costs.iter().sum::<i64>() / self.costs.len() as i64
    }

    /// perf.TPNFrom: the successes sorted, at index percent/100 of their
    /// count, clamped to the last one.
    pub fn tpn(&self, percent: u32) -> i64 {
        if self.costs.is_empty() {
            return 0;
        }
        let idx = (f64::from(percent) / 100.0 * self.costs.len() as f64) as usize;
        self.costs[idx.min(self.costs.len() - 1)]
    }
}

/// The server's CPU and resident memory samples, as perf.PSCounter's RetCPU
/// and RetMEM[].RSS hold them.
#[derive(Default, Clone)]
pub struct Samples {
    pub cpu: Vec<f64>,
    pub rss: Vec<u64>,
}

/// The six resource columns a report carries.
#[derive(Default, Clone, Copy)]
pub struct Resources {
    pub cpu_min: f64,
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub mem_min: u64,
    pub mem_avg: u64,
    pub mem_max: u64,
}

impl Samples {
    /// What the Go client's Report methods get from a PSCounter, including
    /// its quirks: the first CPU sample is left out of the minimum and the
    /// average, and MEMRSSMin sorts the memory samples in place before
    /// MEMRSSAvg reads them, so the average is of every sample but the
    /// smallest.
    pub fn resources(&self) -> Resources {
        let mut r = Resources::default();
        let cpu = &self.cpu;
        match cpu.len() {
            0 => {}
            1 => {
                r.cpu_min = cpu[0];
                r.cpu_avg = cpu[0];
            }
            n => {
                r.cpu_min = cpu[1..].iter().copied().fold(f64::MAX, f64::min);
                r.cpu_avg = cpu[1..].iter().sum::<f64>() / (n - 1) as f64;
            }
        }
        r.cpu_max = cpu.iter().copied().fold(0.0, f64::max);

        let mut mem = self.rss.clone();
        mem.sort_unstable();
        match mem.len() {
            0 => {}
            1 => {
                r.mem_min = mem[0];
                r.mem_avg = mem[0];
            }
            n => {
                r.mem_min = mem[1];
                r.mem_avg = mem[1..].iter().sum::<u64>() / (n - 1) as u64;
            }
        }
        r.mem_max = mem.last().copied().unwrap_or(0);
        r
    }
}

/// report.EER: throughput per percent of a core, or 0 with nothing to divide
/// by.
pub fn eer(throughput: f64, cpu_avg: f64) -> f64 {
    if cpu_avg <= 0.0 || cpu_avg.is_nan() || !throughput.is_finite() {
        return 0.0;
    }
    let v = throughput / cpu_avg;
    if v.is_finite() { v } else { 0.0 }
}

/// report.RateTPS.
pub fn rate_tps(recv_times: i64, duration_ns: i64) -> f64 {
    if duration_ns <= 0 {
        return 0.0;
    }
    recv_times as f64 / (duration_ns as f64 / 1e9)
}

/// perf.I2TimeString.
pub fn time_string(ns: i64) -> String {
    if ns / 1_000_000_000 >= 1 {
        format!("{:.2}s", ns as f64 / 1e9)
    } else if ns / 1_000_000 >= 1 {
        format!("{:.2}ms", ns as f64 / 1e6)
    } else if ns / 1_000 >= 1 {
        format!("{:.2}us", ns as f64 / 1e3)
    } else {
        format!("{ns}ns")
    }
}

/// perf.I2MemString.
pub fn mem_string(bytes: u64) -> String {
    let gb = bytes as f64 / (1u64 << 30) as f64;
    if gb >= 1.0 {
        return format!("{gb:.2}G");
    }
    let mb = bytes as f64 / (1u64 << 20) as f64;
    if mb >= 1.0 {
        return format!("{mb:.2}M");
    }
    format!("{:.2}K", bytes as f64 / 1024.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calc_matches_perf() {
        let c = Calc::new(Duration::from_secs(2), vec![5, 1, 3, 2, 4], 1);
        assert_eq!((c.success, c.failed, c.tps()), (5, 1, 2));
        assert_eq!((c.min(), c.avg(), c.max()), (1, 3, 5));
        assert_eq!((c.tpn(50), c.tpn(99), c.tpn(100)), (3, 5, 5));
        let empty = Calc::new(Duration::from_secs(1), vec![], 3);
        assert_eq!((empty.tps(), empty.min(), empty.tpn(99)), (0, 0, 0));
    }

    #[test]
    fn resources_match_pscounter() {
        let s = Samples {
            cpu: vec![900.0, 100.0, 300.0],
            rss: vec![30, 10, 20],
        };
        let r = s.resources();
        assert_eq!((r.cpu_min, r.cpu_avg, r.cpu_max), (100.0, 200.0, 900.0));
        assert_eq!((r.mem_min, r.mem_avg, r.mem_max), (20, 25, 30));
    }

    #[test]
    fn strings() {
        assert_eq!(time_string(1_500_000_000), "1.50s");
        assert_eq!(time_string(2_500_000), "2.50ms");
        assert_eq!(time_string(35_000), "35.00us");
        assert_eq!(time_string(20), "20ns");
        assert_eq!(mem_string(11_501_568), "10.97M");
        assert_eq!(mem_string(0), "0.00K");
    }
}
