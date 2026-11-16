//! The words and numbers entries are written in.
use device_api::ant;
use std::fmt;

/// `mm:ss.mmm`, minutes unbounded.
pub fn session_clock(ms: u64) -> impl fmt::Display {
    fmt::from_fn(move |f| {
        write!(
            f,
            "{:02}:{:02}.{:03}",
            ms / 60_000,
            ms / 1000 % 60,
            ms % 1000
        )
    })
}

/// `mm:ss`, minutes unbounded.
pub fn ride_clock(ms: u64) -> impl fmt::Display {
    fmt::from_fn(move |f| write!(f, "{:02}:{:02}", ms / 60_000, ms / 1000 % 60))
}

/// `1:05:03`, `5:03` or `3.2s`: a length of time.
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else if s >= 60 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}.{}s", s, ms % 1000 / 100)
    }
}

/// An ANT+ sensor as `power#12345`.
pub fn sensor(identity: ant::Identity) -> impl fmt::Display {
    fmt::from_fn(move |f| {
        match identity.device_type {
            11 => f.write_str("power"),
            40 => f.write_str("radar"),
            120 => f.write_str("hr"),
            121 => f.write_str("speed_cadence"),
            122 => f.write_str("cadence"),
            123 => f.write_str("speed"),
            other => write!(f, "type{other}"),
        }?;
        write!(f, "#{}", identity.device_number)
    })
}

pub fn rate(per_second: f64) -> impl fmt::Display {
    fmt::from_fn(move |f| match per_second {
        per_second if per_second >= 10.0 => write!(f, "{per_second:.0}/s"),
        per_second => write!(f, "{per_second:.1}/s"),
    })
}

pub fn percent(share: f64) -> impl fmt::Display {
    fmt::from_fn(move |f| write!(f, "{:.1}%", share * 100.0))
}

pub fn bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.2} MiB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", n as f64 / 1024.0)
    }
}

pub fn millis(us: u32) -> String {
    format!("{:.1} ms", f64::from(us) / 1000.0)
}

/// `850 µs`, or `3.15 ms` from a millisecond up.
pub fn micros(us: u64) -> impl fmt::Display {
    fmt::from_fn(move |f| match us {
        us if us >= 1000 => write!(f, "{:.2} ms", us as f64 / 1000.0),
        us => write!(f, "{us} µs"),
    })
}

pub(super) fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}
