//! The spike's two workloads, shared by its guest (`apps/vana/guests/bench.rs`,
//! compiled to wasm) and the native baseline in [`super::bench`]. `Host` is
//! what the tile may call.
pub trait Host {
    fn sample(&mut self, i: i32) -> i32;
    fn ring(&mut self, cx: i32, cy: i32, r: i32, frac_q16: i32, color: i32);
    fn text(&mut self, digits: u32, len: i32, x: i32, y: i32, color: i32);
    fn line_to(&mut self, x: i32, y: i32);
}

/// A Countdown tile: a ring that empties and the minutes left.
pub fn countdown<H: Host>(h: &mut H, left_ms: i32, total_ms: i32) {
    let total = if total_ms <= 0 { 1 } else { total_ms };
    let frac = ((left_ms as i64) << 16) / total as i64;
    let color = if left_ms < 60_000 { 0xfd20 } else { 0x07e0 };
    h.ring(52, 52, 38, frac as i32, color);
    let mins = (left_ms + 59_999) / 60_000;
    let (mut digits, mut len, mut m) = (0u32, 0, mins);
    loop {
        digits = (digits << 4) | (m % 10) as u32;
        len += 1;
        m /= 10;
        if m == 0 || len == 8 {
            break;
        }
    }
    h.text(digits, len, 52, 60, 0xffff);
}

/// A sparkline tile: `n` samples averaged into a `width`-pixel line.
pub fn sparkline<H: Host>(h: &mut H, n: i32, width: i32, height: i32) {
    let (mut lo, mut hi) = (i32::MAX, i32::MIN);
    for i in 0..n {
        let v = h.sample(i);
        lo = lo.min(v);
        hi = hi.max(v);
    }
    let span = (hi - lo).max(1);
    for x in 0..width {
        let a = x * n / width;
        let b = ((x + 1) * n / width).max(a + 1);
        let mut sum = 0;
        for i in a..b {
            sum += h.sample(i);
        }
        let avg = sum / (b - a);
        h.line_to(x, height - 1 - (avg - lo) * (height - 1) / span);
    }
}
