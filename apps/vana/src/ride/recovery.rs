//! Heart-rate recovery: how far the heart rate falls in the minute after the
//! ride ends, a common marker of fitness.
//!
//! The summary starts a measurement when it opens, noting the rider's heart
//! rate at that moment. A minute later the first live reading is compared
//! with it. A rider without a live heart-rate sensor at the finish never
//! gets one, and a sensor that stays silent past the minute and a short
//! grace gives up: nothing is shown rather than a stale number.
//!
//! The result lives in RAM for the summary only. The journal has no field
//! for it, and adding one would be a format change, so a ride read back
//! from the list shows none.

/// Time from the finish to the second reading.
pub const WINDOW_MS: u64 = 60_000;
/// How long past the minute a first reading may still arrive.
const GRACE_MS: u64 = 15_000;

/// What the summary shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Whole seconds still to wait.
    Measuring(u32),
    /// Beats per minute below the finish; negative when the heart rate rose.
    Recovered(i16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Recovery {
    #[default]
    Idle,
    Measuring {
        since: u64,
        at_finish: u16,
    },
    Done {
        drop: i16,
    },
}

impl Recovery {
    /// The ride ended at `now` with this live heart rate. Replaces any
    /// earlier measurement, so a ride without a sensor shows none.
    pub fn finish(&mut self, now: u64, bpm: Option<u16>) {
        *self = match bpm.filter(|bpm| *bpm > 0) {
            Some(at_finish) => Self::Measuring {
                since: now,
                at_finish,
            },
            None => Self::Idle,
        };
    }

    /// Offer the live heart rate, if any, at `now`.
    pub fn observe(&mut self, now: u64, bpm: Option<u16>) {
        let Self::Measuring { since, at_finish } = *self else {
            return;
        };
        let elapsed = now.saturating_sub(since);
        if elapsed < WINDOW_MS {
            return;
        }
        *self = match bpm.filter(|bpm| *bpm > 0) {
            Some(bpm) => Self::Done {
                drop: (i32::from(at_finish) - i32::from(bpm)) as i16,
            },
            None if elapsed > WINDOW_MS + GRACE_MS => Self::Idle,
            None => return,
        };
    }

    pub fn clear(&mut self) {
        *self = Self::Idle;
    }

    pub fn reading(&self, now: u64) -> Option<Reading> {
        match *self {
            Self::Idle => None,
            Self::Measuring { since, .. } => {
                let left = WINDOW_MS.saturating_sub(now.saturating_sub(since));
                // Past the minute and waiting on a reading: one second left.
                Some(Reading::Measuring(left.div_ceil(1000).max(1) as u32))
            }
            Self::Done { drop } => Some(Reading::Recovered(drop)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_minute_later_the_fall_is_known() {
        let mut recovery = Recovery::default();
        recovery.finish(1_000, Some(170));
        assert_eq!(recovery.reading(1_000), Some(Reading::Measuring(60)));
        assert_eq!(recovery.reading(31_000), Some(Reading::Measuring(30)));
        recovery.observe(40_000, Some(140));
        assert_eq!(recovery.reading(40_000), Some(Reading::Measuring(21)));
        recovery.observe(61_000, Some(142));
        assert_eq!(recovery.reading(62_000), Some(Reading::Recovered(28)));
        recovery.observe(90_000, Some(100));
        assert_eq!(
            recovery.reading(90_000),
            Some(Reading::Recovered(28)),
            "kept"
        );
    }

    #[test]
    fn no_sensor_at_the_finish_shows_nothing() {
        let mut recovery = Recovery::default();
        recovery.finish(0, None);
        recovery.observe(70_000, Some(120));
        assert_eq!(recovery.reading(70_000), None);
        recovery.finish(0, Some(0));
        assert_eq!(recovery.reading(0), None, "zero is no reading");
    }

    #[test]
    fn a_silent_sensor_waits_a_little_then_gives_up() {
        let mut recovery = Recovery::default();
        recovery.finish(0, Some(150));
        recovery.observe(60_000, None);
        assert_eq!(recovery.reading(60_000), Some(Reading::Measuring(1)));
        recovery.observe(70_000, None);
        assert!(recovery.reading(70_000).is_some(), "still within the grace");
        recovery.observe(70_500, Some(130));
        assert_eq!(recovery.reading(70_500), Some(Reading::Recovered(20)));
        recovery.finish(0, Some(150));
        recovery.observe(WINDOW_MS + GRACE_MS + 1, None);
        assert_eq!(recovery.reading(WINDOW_MS + GRACE_MS + 1), None);
    }

    #[test]
    fn a_rising_heart_rate_is_negative_and_a_new_finish_starts_over() {
        let mut recovery = Recovery::default();
        recovery.finish(0, Some(100));
        recovery.observe(60_000, Some(104));
        assert_eq!(recovery.reading(60_000), Some(Reading::Recovered(-4)));
        recovery.finish(100_000, Some(160));
        assert_eq!(recovery.reading(100_000), Some(Reading::Measuring(60)));
        recovery.clear();
        assert_eq!(recovery.reading(100_000), None);
    }
}
