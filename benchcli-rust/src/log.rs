//! The Go client's logging package: a timestamp in its format in front of
//! every line, on stderr.

pub const SHORT_LINE: &str = "--------------------------------------------------------------\n";
pub const LONG_LINE: &str = "----------------------------------------------------------------------------------------------------\n";

/// Now, as the Go client's logging.NowString writes it: "20060102 15:04.05.000".
pub fn now_string() -> String {
    let mut tv = libc::timeval {
        tv_sec: 0,
        tv_usec: 0,
    };
    unsafe { libc::gettimeofday(&mut tv, std::ptr::null_mut()) };
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let secs = tv.tv_sec;
    unsafe { libc::localtime_r(&secs, &mut tm) };
    format!(
        "{:04}{:02}{:02} {:02}:{:02}.{:02}.{:03}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec,
        tv.tv_usec / 1000
    )
}

#[macro_export]
macro_rules! logf {
    ($($arg:tt)*) => {
        eprintln!("{} {}", $crate::log::now_string(), format!($($arg)*))
    };
}

#[macro_export]
macro_rules! fatalf {
    ($($arg:tt)*) => {{
        $crate::logf!($($arg)*);
        std::process::exit(1)
    }};
}

pub fn print(s: &str) {
    eprint!("{s}");
}
