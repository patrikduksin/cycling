//! Portable runtime metrics shown on-device and reported by the test harness.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub uptime_ms: u64,
    pub frame_ms: u32,
    pub max_frame_ms: u32,
    pub heap_free: usize,
    pub heap_min_sampled: usize,
    pub psram_capacity: usize,
    pub psram_free: usize,
    pub companion_valid: u32,
    pub companion_bad_crc: u32,
    pub uart_errors: u32,
    pub touch_errors: u32,
    pub harness: bool,
    pub recording: bool,
    pub selected_brightness: u8,
    pub effective_brightness: u8,
    pub dimmed: bool,
    pub idle_ms: u64,
    pub dim_timeout_secs: u16,
    pub dim_brightness: u8,
}

/// Format a growing counter in at most four characters using K/M suffixes.
pub fn compact(value: u64) -> ([u8; 4], usize) {
    if value < 10_000 {
        decimal(value as u32)
    } else if value < 1_000_000 {
        suffixed(value / 1_000, b'K')
    } else {
        suffixed((value / 1_000_000).min(999), b'M')
    }
}

fn decimal(value: u32) -> ([u8; 4], usize) {
    let mut output = [0; 4];
    let mut divisor = if value >= 1_000 {
        1_000
    } else if value >= 100 {
        100
    } else if value >= 10 {
        10
    } else {
        1
    };
    let mut length = 0;
    loop {
        output[length] = b'0' + ((value / divisor) % 10) as u8;
        length += 1;
        if divisor == 1 {
            return (output, length);
        }
        divisor /= 10;
    }
}

fn suffixed(value: u64, suffix: u8) -> ([u8; 4], usize) {
    let (mut output, length) = decimal(value.min(999) as u32);
    output[length] = suffix;
    (output, length + 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn formatted(value: u64) -> ([u8; 4], usize) {
        compact(value)
    }

    #[test]
    fn growing_counters_remain_bounded_and_readable() {
        assert_eq!(formatted(0), (*b"0\0\0\0", 1));
        assert_eq!(formatted(9_999), (*b"9999", 4));
        assert_eq!(formatted(10_000), (*b"10K\0", 3));
        assert_eq!(formatted(999_999), (*b"999K", 4));
        assert_eq!(formatted(1_000_000), (*b"1M\0\0", 2));
        assert_eq!(formatted(u64::MAX), (*b"999M", 4));
    }
}
