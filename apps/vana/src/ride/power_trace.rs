//! The whole ride's power for Ride done: mean watts over equal stretches of
//! active time, kept in a fixed number of buckets however long the ride.
//!
//! A bucket covers `FIRST_SPAN_MS` at the start. When every bucket is full,
//! neighbours merge and each covers twice the time, so the buckets always
//! span the whole ride. Time without a power reading counts as no watts, as
//! the ride's energy does.

/// The most buckets kept: twice `BARS`, so a full trace shows `BARS` bars.
pub const BUCKETS: usize = 2 * BARS;
pub const BARS: usize = 18;
const FIRST_SPAN_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PowerTrace {
    /// Watt-milliseconds in each closed bucket.
    energy: [u64; BUCKETS],
    len: usize,
    span_ms: u64,
    /// The open bucket: its time so far and its watt-milliseconds.
    open_ms: u64,
    open_energy: u64,
    powered: bool,
}

impl Default for PowerTrace {
    fn default() -> Self {
        Self {
            energy: [0; BUCKETS],
            len: 0,
            span_ms: FIRST_SPAN_MS,
            open_ms: 0,
            open_energy: 0,
            powered: false,
        }
    }
}

impl PowerTrace {
    pub fn add(&mut self, mut elapsed_ms: u64, watts: Option<u16>) {
        self.powered |= watts.is_some();
        let watts = u64::from(watts.unwrap_or(0));
        while elapsed_ms > 0 {
            let room = self.span_ms - self.open_ms;
            let taken = elapsed_ms.min(room);
            self.open_ms += taken;
            self.open_energy += watts * taken;
            elapsed_ms -= taken;
            if self.open_ms == self.span_ms {
                self.close();
            }
        }
    }

    fn close(&mut self) {
        if self.len == BUCKETS {
            self.halve();
            return;
        }
        self.energy[self.len] = self.open_energy;
        self.len += 1;
        self.open_ms = 0;
        self.open_energy = 0;
    }

    /// Merge neighbours so each bucket spans twice the time. The open
    /// bucket stays open, half full at the new span.
    fn halve(&mut self) {
        for index in 0..BUCKETS / 2 {
            self.energy[index] = self.energy[2 * index] + self.energy[2 * index + 1];
        }
        self.len = BUCKETS / 2;
        self.span_ms *= 2;
    }

    pub fn powered(&self) -> bool {
        self.powered
    }

    /// Mean watts of up to `BARS` equal stretches of the ride, oldest first:
    /// fewer for a ride shorter than `BARS` buckets. A stretch still open
    /// counts for the time it covers.
    pub fn bars(&self) -> heapless::Vec<u16, BARS> {
        let mut spans: heapless::Vec<(u64, u64), { BUCKETS + 1 }> = self.energy[..self.len]
            .iter()
            .map(|&energy| (energy, self.span_ms))
            .collect();
        if self.open_ms > 0 {
            let _ = spans.push((self.open_energy, self.open_ms));
        }
        let count = spans.len();
        let bars = count.min(BARS);
        (0..bars)
            .map(|bar| {
                let group = &spans[bar * count / bars..(bar + 1) * count / bars];
                let (energy, ms) = group
                    .iter()
                    .fold((0, 0), |(e, t), &(energy, ms)| (e + energy, t + ms));
                (energy / ms.max(1)) as u16
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_ride_shows_a_bar_a_bucket_and_its_open_stretch() {
        let mut trace = PowerTrace::default();
        trace.add(5_000, Some(100));
        trace.add(5_000, Some(200));
        trace.add(2_000, Some(300));
        assert_eq!(trace.bars().as_slice(), [100, 200, 300]);
        assert!(trace.powered());
    }

    #[test]
    fn a_long_ride_keeps_its_mean_power_in_bars_bars() {
        let mut trace = PowerTrace::default();
        for second in 0..3 * 3600 {
            let watts = if second < 3 * 1800 { 150 } else { 250 };
            trace.add(1_000, Some(watts));
        }
        let bars = trace.bars();
        assert_eq!(bars.len(), BARS);
        assert_eq!(bars.first(), Some(&150));
        assert_eq!(bars.last(), Some(&250));
    }

    #[test]
    fn time_without_power_counts_as_none() {
        let mut trace = PowerTrace::default();
        trace.add(5_000, None);
        assert_eq!(trace.bars().as_slice(), [0]);
        assert!(!trace.powered());
    }
}
