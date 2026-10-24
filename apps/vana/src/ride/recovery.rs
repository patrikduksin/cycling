//! Heart-rate recovery: how far the heart rate falls in the minute after the
//! ride ends, a common marker of fitness.
//!
//! A ride is always paused before it ends, so the measurement starts from
//! the heart rate at the moment the ride stopped recording, not at the
//! rider's confirmation: sitting out an auto-pause first would read zero. It
//! runs for a minute from that stop. A finish more than a few seconds past
//! the minute, or without a live sensor at the stop or a minute later,
//! shows nothing rather than a number from another moment.
//!
//! The result lives in RAM for the summary only. The journal has no field
//! for it, and adding one would be a format change, so a ride read back
//! from the list shows none.

/// Time from the finish to the second reading.
pub const WINDOW_MS: u64 = 60_000;
/// How long past the minute a first reading may still arrive.
const GRACE_MS: u64 = 5_000;

/// What the summary shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reading {
    /// Whole seconds still to wait.
    Measuring(u32),
    /// Beats per minute below the finish; negative when the heart rate rose.
    Recovered(i16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// The latest tick while recording, and its heart rate.
    riding: Option<(u64, Option<u16>)>,
    /// When recording last stopped, and the heart rate then.
    stopped: Option<(u64, u16)>,
    state: State,
}

fn live(bpm: Option<u16>) -> Option<u16> {
    bpm.filter(|bpm| *bpm > 0)
}

impl Recovery {
    /// Offer each tick whether the ride is recording and the live heart rate.
    pub fn track(&mut self, now: u64, recording: bool, bpm: Option<u16>) {
        if recording {
            self.riding = Some((now, bpm));
            self.stopped = None;
        } else if let Some((at, bpm)) = self.riding.take() {
            self.stopped = live(bpm).map(|bpm| (at, bpm));
        }
        self.observe(now, bpm);
    }

    /// The ride ended at `now`. Replaces any earlier measurement.
    pub fn finish(&mut self, now: u64) {
        let stop = self.stopped.or_else(|| {
            let (at, bpm) = self.riding?;
            live(bpm).map(|bpm| (at, bpm))
        });
        self.state = match stop {
            Some((since, at_finish)) if now.saturating_sub(since) <= WINDOW_MS + GRACE_MS => {
                State::Measuring { since, at_finish }
            }
            _ => State::Idle,
        };
    }

    fn observe(&mut self, now: u64, bpm: Option<u16>) {
        let State::Measuring { since, at_finish } = self.state else {
            return;
        };
        let elapsed = now.saturating_sub(since);
        if elapsed < WINDOW_MS {
            return;
        }
        self.state = match live(bpm) {
            Some(bpm) => State::Done {
                drop: (i32::from(at_finish) - i32::from(bpm)) as i16,
            },
            None if elapsed > WINDOW_MS + GRACE_MS => State::Idle,
            None => return,
        };
    }

    pub fn clear(&mut self) {
        self.state = State::Idle;
    }

    pub fn reading(&self, now: u64) -> Option<Reading> {
        match self.state {
            State::Idle => None,
            State::Measuring { since, .. } => {
                let left = WINDOW_MS.saturating_sub(now.saturating_sub(since));
                // Past the minute and waiting on a reading: one second left.
                Some(Reading::Measuring(left.div_ceil(1000).max(1) as u32))
            }
            State::Done { drop } => Some(Reading::Recovered(drop)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Recording until `stop_ms` at `bpm`, then stopped.
    fn stopped_at(stop_ms: u64, bpm: Option<u16>) -> Recovery {
        let mut recovery = Recovery::default();
        recovery.track(stop_ms - 1_000, true, bpm);
        recovery.track(stop_ms, false, None);
        recovery
    }

    #[test]
    fn a_minute_after_the_stop_the_fall_is_known() {
        let mut recovery = stopped_at(1_000, Some(170));
        recovery.finish(4_000);
        assert_eq!(recovery.reading(4_000), Some(Reading::Measuring(56)));
        recovery.track(40_000, false, Some(140));
        assert_eq!(recovery.reading(40_000), Some(Reading::Measuring(20)));
        recovery.track(61_000, false, Some(142));
        assert_eq!(recovery.reading(62_000), Some(Reading::Recovered(28)));
        recovery.track(90_000, false, Some(100));
        assert_eq!(
            recovery.reading(90_000),
            Some(Reading::Recovered(28)),
            "kept"
        );
    }

    #[test]
    fn no_sensor_at_the_stop_shows_nothing() {
        let mut recovery = stopped_at(5_000, None);
        recovery.finish(8_000);
        recovery.track(70_000, false, Some(120));
        assert_eq!(recovery.reading(70_000), None);
        let mut zero = stopped_at(5_000, Some(0));
        zero.finish(8_000);
        assert_eq!(zero.reading(8_000), None, "zero is no reading");
    }

    #[test]
    fn a_long_wait_before_the_finish_shows_nothing() {
        let mut recovery = stopped_at(5_000, Some(150));
        recovery.finish(5_000 + 3 * 60_000);
        assert_eq!(recovery.reading(5_000 + 3 * 60_000), None);
    }

    #[test]
    fn a_finish_just_past_the_minute_measures_at_once() {
        let mut recovery = stopped_at(5_000, Some(150));
        recovery.finish(5_000 + WINDOW_MS + 1_000);
        recovery.track(5_000 + WINDOW_MS + 1_000, false, Some(120));
        assert_eq!(
            recovery.reading(0),
            Some(Reading::Recovered(30)),
            "the minute after the stop"
        );
    }

    #[test]
    fn a_silent_sensor_waits_briefly_then_gives_up() {
        let mut recovery = stopped_at(5_000, Some(150));
        recovery.finish(8_000);
        let mark = 5_000 + WINDOW_MS;
        recovery.track(mark, false, None);
        assert_eq!(recovery.reading(mark), Some(Reading::Measuring(1)));
        recovery.track(mark + GRACE_MS, false, Some(130));
        assert_eq!(recovery.reading(mark), Some(Reading::Recovered(20)));
        let mut silent = stopped_at(5_000, Some(150));
        silent.finish(8_000);
        silent.track(mark + GRACE_MS + 1, false, None);
        assert_eq!(silent.reading(mark + GRACE_MS + 1), None);
    }

    #[test]
    fn resuming_forgets_the_stop_and_a_rise_is_negative() {
        let mut recovery = stopped_at(5_000, Some(100));
        recovery.track(6_000, true, Some(110));
        recovery.track(9_000, false, None);
        recovery.finish(10_000);
        recovery.track(70_000, false, Some(114));
        assert_eq!(recovery.reading(70_000), Some(Reading::Recovered(-4)));
        recovery.clear();
        assert_eq!(recovery.reading(70_000), None);
    }
}
