//! Normalized power and training stress from a stream of power samples, shared
//! by a live ride's totals and the ride index so the two cannot disagree.
//!
//! Normalized power is the fourth root of the mean fourth power of 30 s
//! rolling power. The index keeps a ride's stress as an `Effort`, its
//! normalized power and the time it had power, so the score is worked out
//! against the FTP that is current when it is shown: the FTP in force when a
//! ride was recorded is not stored. A ride without power has no effort.

/// Normalized power smooths power over this many samples (seconds).
const WINDOW: usize = 30;

/// Seconds one unit of an `Effort`'s power time counts; a `u16` spans 72 h.
const UNIT_S: u64 = 4;

/// Training stress score for `power_ms` of riding at `np_watts` against
/// `ftp_watts`: an hour at FTP is 100. Seconds with power × NP² / (FTP² × 36).
pub fn stress_score(np_watts: u16, power_ms: u64, ftp_watts: u16) -> Option<u32> {
    let ftp = u128::from(ftp_watts);
    let np = u128::from(np_watts);
    (ftp != 0).then(|| {
        let scale = ftp * ftp * 36_000;
        ((u128::from(power_ms) * np * np + scale / 2) / scale).min(u128::from(u32::MAX)) as u32
    })
}

/// The rolling 30 s power of a ride so far.
#[derive(Clone, Copy)]
pub struct PowerLoad {
    window: [u16; WINDOW],
    window_len: usize,
    window_next: usize,
    window_sum: u32,
    /// Sum of (window sum)^4 over every full window, and how many there were.
    np_sum: u128,
    np_count: u32,
    power_ms: u64,
}

impl Default for PowerLoad {
    fn default() -> Self {
        Self {
            window: [0; WINDOW],
            window_len: 0,
            window_next: 0,
            window_sum: 0,
            np_sum: 0,
            np_count: 0,
            power_ms: 0,
        }
    }
}

impl PowerLoad {
    /// Account one power reading that covers `elapsed_ms`.
    pub fn add(&mut self, elapsed_ms: u64, watts: u16) {
        self.power_ms = self.power_ms.saturating_add(elapsed_ms);
        if self.window_len == WINDOW {
            self.window_sum -= u32::from(self.window[self.window_next]);
        } else {
            self.window_len += 1;
        }
        self.window[self.window_next] = watts;
        self.window_sum += u32::from(watts);
        self.window_next = (self.window_next + 1) % WINDOW;
        if self.window_len == WINDOW {
            self.np_sum = self
                .np_sum
                .saturating_add(u128::from(self.window_sum).pow(4));
            self.np_count = self.np_count.saturating_add(1);
        }
    }

    /// Time covered by power readings, zeros included.
    pub fn power_ms(&self) -> u64 {
        self.power_ms
    }

    /// None until the first 30 s of power.
    pub fn normalized_power(&self) -> Option<u16> {
        (self.np_count != 0).then(|| {
            let root = (self.np_sum / u128::from(self.np_count)).isqrt().isqrt();
            ((root + WINDOW as u128 / 2) / WINDOW as u128).min(u128::from(u16::MAX)) as u16
        })
    }

    pub fn stress_score(&self, ftp_watts: u16) -> Option<u32> {
        stress_score(self.normalized_power()?, self.power_ms, ftp_watts)
    }

    /// What the ride index keeps of this ride; none without 30 s of power.
    pub fn effort(&self) -> Option<Effort> {
        let units = self.power_ms.saturating_add(UNIT_S * 500) / (UNIT_S * 1000);
        Some(Effort {
            np_watts: self.normalized_power()?,
            power_units: units.min(u64::from(u16::MAX)) as u16,
        })
    }
}

/// A ride's normalized power and power time, four bytes in the index.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Effort {
    pub np_watts: u16,
    /// Time with power in units of `UNIT_S` seconds, saturating.
    pub power_units: u16,
}

impl Effort {
    /// Training stress at `ftp_watts`; none for an FTP of zero.
    pub fn stress_score(self, ftp_watts: u16) -> Option<u32> {
        stress_score(
            self.np_watts,
            u64::from(self.power_units) * UNIT_S * 1000,
            ftp_watts,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn effort_keeps_the_stress_within_a_unit_of_power_time() {
        let mut load = PowerLoad::default();
        for _ in 0..3_601 {
            load.add(1000, 250);
        }
        let exact = load.stress_score(250).unwrap();
        let kept = load.effort().unwrap().stress_score(250).unwrap();
        assert_eq!(exact, 100);
        assert_eq!(kept, 100);
    }

    #[test]
    fn hostile_power_and_time_saturate_instead_of_overflowing() {
        let mut load = PowerLoad::default();
        for _ in 0..40 {
            load.add(u64::MAX, u16::MAX);
        }
        assert_eq!(load.power_ms(), u64::MAX);
        assert_eq!(load.normalized_power(), Some(u16::MAX));
        assert_eq!(load.stress_score(1), Some(u32::MAX));
        assert_eq!(load.stress_score(0), None);
        let effort = load.effort().unwrap();
        assert_eq!(effort.power_units, u16::MAX);
        assert!(effort.stress_score(u16::MAX).is_some());
    }

    #[test]
    fn no_effort_without_thirty_seconds_of_power() {
        let mut load = PowerLoad::default();
        for _ in 0..29 {
            load.add(1000, 300);
        }
        assert_eq!(load.effort(), None);
    }
}
