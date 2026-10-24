//! Laps: the ride split into manual and automatic segments, each with its own
//! time, distance and averages.
//!
//! The caller feeds the same recorded seconds as the ride totals, so a pause
//! adds nothing to the lap. Lap time is recorded (active) time, the clock the
//! main page shows; average speed is distance over moving time, as in the
//! totals. A rider's lap closes the current lap unless another lap closed less
//! than `DEBOUNCE_MS` before; an automatic lap closes it every `AUTO_LAP_MM`
//! of lap distance, or the distance the rider chose (`add_every`), and a
//! workout closes one as each step ends (`split`). The rider may choose laps
//! at the ride's start point instead (`at_start`), which closes one when the
//! breadcrumb reports a pass, if the lap is long enough. All count in one
//! numbered list, and the latest `HISTORY` summaries are kept.
//!
//! Each closed lap waits in `take_closed` for the recorder, which marks the
//! next recorded sample as the first of a new lap (`log::LapStart`). So the
//! journal keeps where laps began, to the second, and exports and saved rides
//! read them back; the summaries here stay in RAM.
use crate::ride::log::{LapStart, Sample};
use crate::ride::totals::MOVING_MIN_MM_S;

/// An automatic lap closes after this much lap distance (5 km).
pub const AUTO_LAP_MM: u64 = 5_000_000;
/// A pass of the start closes no lap shorter than this.
pub const AT_START_MIN_MM: u64 = 500_000;
/// Nor one shorter than this, in recorded time.
pub const AT_START_MIN_MS: u64 = 60_000;
/// A second lap within this long of the last one is ignored.
pub const DEBOUNCE_MS: u64 = 3000;
/// Finished laps whose summaries are kept, newest last.
pub const HISTORY: usize = 16;

/// What closes laps by itself, as the rider chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AutoLap {
    Off,
    /// Every this many kilometres of lap distance.
    Every(u8),
    /// Each pass of the ride's start point.
    AtStart,
}

impl AutoLap {
    /// The lap distance that closes a lap, if distance does.
    pub fn every_mm(self) -> Option<u64> {
        match self {
            Self::Every(km) if km != 0 => Some(u64::from(km) * 1_000_000),
            _ => None,
        }
    }
}

/// How a lap was closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Trigger {
    #[default]
    Manual,
    /// Every lap distance.
    Auto,
    /// A workout step that lasts a time, or until it is marked, ended.
    Step,
    /// A workout step that lasts a distance ended.
    DistanceStep,
    /// The rider passed the ride's start point.
    Position,
}

impl From<Trigger> for LapStart {
    fn from(trigger: Trigger) -> Self {
        match trigger {
            Trigger::Manual => Self::Manual,
            Trigger::Auto => Self::Distance,
            Trigger::Step => Self::Step,
            Trigger::DistanceStep => Self::DistanceStep,
            Trigger::Position => Self::Position,
        }
    }
}

/// What one lap added up; the open lap is one too.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Lap {
    /// One for the first lap of a ride.
    pub number: u16,
    pub time_ms: u64,
    pub distance_mm: u64,
    moving_ms: u64,
    /// Watt-milliseconds and the time they cover; zeros count.
    energy_mj: u64,
    power_ms: u64,
    heart_sum: u32,
    heart_count: u32,
    max_speed_mm_s: Option<u32>,
    max_power: Option<u16>,
    max_heart: Option<u16>,
    cadence_tenths_sum: u64,
    cadence_count: u32,
    /// How it closed, once finished.
    pub trigger: Trigger,
}

impl Lap {
    fn numbered(number: u16) -> Self {
        Self {
            number,
            ..Self::default()
        }
    }

    /// Account one recorded step, as `Totals::add` does.
    pub fn add(&mut self, elapsed_ms: u64, distance_mm: u64, sample: &Sample) {
        self.time_ms = self.time_ms.saturating_add(elapsed_ms);
        self.distance_mm = self.distance_mm.saturating_add(distance_mm);
        if let Some(speed) = sample.speed_mm_s {
            if speed >= MOVING_MIN_MM_S {
                self.moving_ms = self.moving_ms.saturating_add(elapsed_ms);
            }
            self.max_speed_mm_s = self.max_speed_mm_s.max(Some(speed));
        }
        if let Some(watts) = sample.power_watts {
            self.energy_mj = self.energy_mj.saturating_add(u64::from(watts) * elapsed_ms);
            self.power_ms = self.power_ms.saturating_add(elapsed_ms);
            self.max_power = self.max_power.max(Some(watts));
        }
        if let Some(bpm) = sample.heart_bpm {
            self.heart_sum = self.heart_sum.saturating_add(u32::from(bpm));
            self.heart_count = self.heart_count.saturating_add(1);
            self.max_heart = self.max_heart.max(Some(bpm));
        }
        if let Some(cadence) = sample.cadence_tenths.filter(|&c| c != 0) {
            self.cadence_tenths_sum = self.cadence_tenths_sum.saturating_add(u64::from(cadence));
            self.cadence_count = self.cadence_count.saturating_add(1);
        }
    }

    /// Kilocalories from the lap's work, as `Totals::kcal` has it.
    pub fn kcal(&self) -> Option<u32> {
        (self.power_ms != 0).then(|| ((self.energy_mj + 500_000) / 1_000_000) as u32)
    }

    pub fn max_speed_mm_s(&self) -> Option<u32> {
        self.max_speed_mm_s
    }

    pub fn max_power(&self) -> Option<u16> {
        self.max_power
    }

    pub fn max_heart_bpm(&self) -> Option<u16> {
        self.max_heart
    }

    /// Mean cadence while pedalling, in rpm.
    pub fn avg_cadence_rpm(&self) -> Option<u16> {
        (self.cadence_count != 0).then(|| {
            let count = u64::from(self.cadence_count);
            let tenths = (self.cadence_tenths_sum + count / 2) / count;
            ((tenths + 5) / 10).min(u64::from(u16::MAX)) as u16
        })
    }

    /// Time-weighted mean over every second with power, zeros included.
    pub fn avg_power(&self) -> Option<u16> {
        (self.power_ms != 0).then(|| {
            ((self.energy_mj + self.power_ms / 2) / self.power_ms).min(u64::from(u16::MAX)) as u16
        })
    }

    /// Lap distance over lap moving time.
    pub fn avg_speed_mm_s(&self) -> Option<u32> {
        (self.moving_ms != 0).then(|| (self.distance_mm * 1000 / self.moving_ms) as u32)
    }

    pub fn avg_heart_bpm(&self) -> Option<u16> {
        (self.heart_count != 0).then(|| {
            (self.heart_sum.saturating_add(self.heart_count / 2) / self.heart_count)
                .min(u32::from(u16::MAX)) as u16
        })
    }
}

#[derive(Clone, Copy)]
pub struct Laps {
    current: Lap,
    /// Finished laps in a ring; `finished` counts every one this ride.
    history: [Lap; HISTORY],
    finished: u16,
    /// When the last lap closed, for the rider's debounce.
    closed_at_ms: Option<u64>,
    /// Laps close every `AUTO_LAP_MM`; a workout turns this off, as its
    /// steps close the laps instead.
    pub by_distance: bool,
    unmarked: Option<Trigger>,
}

impl Default for Laps {
    fn default() -> Self {
        Self {
            current: Lap::numbered(1),
            history: [Lap::default(); HISTORY],
            finished: 0,
            closed_at_ms: None,
            by_distance: true,
            unmarked: None,
        }
    }
}

impl Laps {
    /// Account one recorded step; returns the lap it closed automatically.
    pub fn add(
        &mut self,
        elapsed_ms: u64,
        distance_mm: u64,
        sample: &Sample,
        now_ms: u64,
    ) -> Option<Lap> {
        self.add_every(elapsed_ms, distance_mm, sample, now_ms, Some(AUTO_LAP_MM))
    }

    /// `add` with automatic laps every `every_mm` of lap distance, or none.
    pub fn add_every(
        &mut self,
        elapsed_ms: u64,
        distance_mm: u64,
        sample: &Sample,
        now_ms: u64,
        every_mm: Option<u64>,
    ) -> Option<Lap> {
        self.current.add(elapsed_ms, distance_mm, sample);
        (self.by_distance && every_mm.is_some_and(|every| self.current.distance_mm >= every))
            .then(|| self.close(Trigger::Auto, now_ms))
    }

    /// The rider passed the start: close the current lap if it has run
    /// `AT_START_MIN_MM` and `AT_START_MIN_MS`, so a pass soon after the last
    /// lap, or soon after the start, is no lap.
    pub fn at_start(&mut self, now_ms: u64) -> Option<Lap> {
        let long_enough =
            self.current.distance_mm >= AT_START_MIN_MM && self.current.time_ms >= AT_START_MIN_MS;
        (self.by_distance && long_enough).then(|| self.close(Trigger::Position, now_ms))
    }

    /// Close the current lap as a workout step ends: `by_distance` for a
    /// step that lasts a distance.
    pub fn split(&mut self, now_ms: u64, by_distance: bool) -> Lap {
        let trigger = if by_distance {
            Trigger::DistanceStep
        } else {
            Trigger::Step
        };
        self.close(trigger, now_ms)
    }

    /// The rider's lap: close the current lap unless one closed within
    /// `DEBOUNCE_MS`. Returns the lap it closed.
    pub fn lap(&mut self, now_ms: u64) -> Option<Lap> {
        if self
            .closed_at_ms
            .is_some_and(|at| now_ms.saturating_sub(at) < DEBOUNCE_MS)
        {
            return None;
        }
        Some(self.close(Trigger::Manual, now_ms))
    }

    fn close(&mut self, trigger: Trigger, now_ms: u64) -> Lap {
        let mut lap = self.current;
        lap.trigger = trigger;
        self.history[usize::from(self.finished) % HISTORY] = lap;
        self.finished = self.finished.saturating_add(1);
        self.current = Lap::numbered(lap.number.saturating_add(1));
        self.closed_at_ms = Some(now_ms);
        self.unmarked = Some(trigger);
        lap
    }

    /// How the last lap closed, once, for the recorder to mark the next
    /// sample. Laps closing within one sample share its mark: the latest wins.
    pub fn take_closed(&mut self) -> Option<Trigger> {
        self.unmarked.take()
    }

    /// The open lap.
    pub fn current(&self) -> Lap {
        self.current
    }

    /// The last finished lap.
    pub fn previous(&self) -> Option<Lap> {
        self.finished(0)
    }

    /// A finished lap, `back` laps before the previous one, while kept.
    pub fn finished(&self, back: usize) -> Option<Lap> {
        let finished = usize::from(self.finished);
        (back < finished.min(HISTORY)).then(|| self.history[(finished - 1 - back) % HISTORY])
    }

    /// Laps finished this ride, including those no longer kept.
    pub fn count(&self) -> u16 {
        self.finished
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn riding(speed_mm_s: u32, watts: u16, bpm: u16) -> Sample {
        Sample {
            speed_mm_s: Some(speed_mm_s),
            power_watts: Some(watts),
            heart_bpm: Some(bpm),
            ..Sample::default()
        }
    }

    #[test]
    fn a_saturated_heart_sum_still_averages() {
        let lap = Lap {
            heart_sum: u32::MAX,
            heart_count: 65_538,
            ..Lap::default()
        };
        assert_eq!(lap.avg_heart_bpm(), Some(65_534));
    }

    /// `seconds` one-second steps at a steady sample, from `now`; returns the
    /// laps that closed automatically and the time after.
    fn ride(laps: &mut Laps, now: u64, seconds: u64, sample: Sample) -> (Vec<Lap>, u64) {
        let speed = u64::from(sample.speed_mm_s.unwrap_or(0));
        let mut closed = Vec::new();
        let mut now = now;
        for _ in 0..seconds {
            now += 1000;
            closed.extend(laps.add(1000, speed, &sample, now));
        }
        (closed, now)
    }

    #[test]
    fn workouts_split_laps_by_step_instead_of_distance() {
        let mut laps = Laps {
            by_distance: false,
            ..Laps::default()
        };
        let (closed, now) = ride(&mut laps, 0, 700, riding(8000, 200, 140));
        assert!(closed.is_empty(), "5.6 km without a distance lap");
        let lap = laps.split(now, false);
        assert_eq!((lap.number, lap.trigger), (1, Trigger::Step));
        assert_eq!(laps.take_closed(), Some(Trigger::Step));
        assert_eq!(laps.take_closed(), None, "handed over once");
        assert_eq!(laps.current().number, 2);
        assert_eq!(laps.split(now, true).trigger, Trigger::DistanceStep);
    }

    #[test]
    fn a_manual_lap_summarises_and_the_next_starts_from_zero() {
        let mut laps = Laps::default();
        assert_eq!(laps.current().number, 1);
        assert_eq!(laps.previous(), None);
        let (_, now) = ride(&mut laps, 0, 300, riding(8333, 200, 140));
        let first = laps.lap(now).unwrap();
        assert_eq!(first.number, 1);
        assert_eq!(first.trigger, Trigger::Manual);
        assert_eq!(first.time_ms, 300_000);
        assert_eq!(first.distance_mm, 2_499_900);
        assert_eq!(first.avg_power(), Some(200));
        assert_eq!(first.avg_speed_mm_s(), Some(8333));
        assert_eq!(first.avg_heart_bpm(), Some(140));

        let current = laps.current();
        assert_eq!((current.number, current.time_ms), (2, 0));
        assert_eq!(current.avg_power(), None);
        let (_, now) = ride(&mut laps, now, 120, riding(10_000, 300, 160));
        let second = laps.lap(now).unwrap();
        assert_eq!((second.number, second.time_ms), (2, 120_000));
        assert_eq!(second.avg_power(), Some(300));
        assert_eq!(laps.previous(), Some(second));
        assert_eq!(laps.finished(1), Some(first));
        assert_eq!(laps.count(), 2);
    }

    #[test]
    fn averages_count_zeros_for_power_and_skip_missing_readings() {
        let mut laps = Laps::default();
        ride(&mut laps, 0, 10, riding(8000, 300, 150));
        // Coasting: zero watts count, a stop adds no moving time, no strap no HR.
        let coasting = Sample {
            heart_bpm: None,
            ..riding(8000, 0, 0)
        };
        ride(&mut laps, 10_000, 10, coasting);
        ride(&mut laps, 20_000, 10, Sample::default());
        let lap = laps.current();
        assert_eq!(lap.time_ms, 30_000);
        assert_eq!(lap.avg_power(), Some(150));
        assert_eq!(lap.avg_heart_bpm(), Some(150));
        assert_eq!(lap.avg_speed_mm_s(), Some(8000));
    }

    #[test]
    fn a_second_lap_within_three_seconds_is_ignored() {
        let mut laps = Laps::default();
        let (_, now) = ride(&mut laps, 0, 60, riding(8000, 200, 140));
        assert!(laps.lap(now).is_some());
        assert_eq!(laps.lap(now + 400), None);
        assert_eq!(laps.lap(now + DEBOUNCE_MS - 1), None);
        assert_eq!(laps.count(), 1);
        assert_eq!(laps.lap(now + DEBOUNCE_MS).map(|lap| lap.number), Some(2));
    }

    #[test]
    fn auto_lap_closes_every_five_kilometres_of_lap_distance() {
        let mut laps = Laps::default();
        // 10 m/s: 500 s per lap.
        let (closed, now) = ride(&mut laps, 0, 1250, riding(10_000, 250, 150));
        assert_eq!(closed.len(), 2);
        for (index, lap) in closed.iter().enumerate() {
            assert_eq!(lap.number, index as u16 + 1);
            assert_eq!(lap.trigger, Trigger::Auto);
            assert_eq!((lap.time_ms, lap.distance_mm), (500_000, AUTO_LAP_MM));
        }
        assert_eq!(laps.current().number, 3);
        assert_eq!(laps.current().distance_mm, 2_500_000);
        // A rider's lap counts in the same list and restarts the 5 km.
        assert_eq!(laps.lap(now).map(|lap| lap.number), Some(3));
        let (closed, _) = ride(&mut laps, now, 499, riding(10_000, 250, 150));
        assert!(closed.is_empty());
    }

    #[test]
    fn keeps_the_latest_sixteen_laps() {
        let mut laps = Laps::default();
        let mut now = 0;
        for _ in 0..20 {
            (_, now) = ride(&mut laps, now, 10, riding(8000, 200, 140));
            laps.lap(now).unwrap();
        }
        assert_eq!(laps.count(), 20);
        assert_eq!(laps.previous().map(|lap| lap.number), Some(20));
        assert_eq!(laps.finished(15).map(|lap| lap.number), Some(5));
        assert_eq!(laps.finished(16), None);
        assert_eq!(laps.current().number, 21);
    }

    #[test]
    fn a_pass_of_the_start_closes_a_lap_only_once_it_is_long_enough() {
        let mut laps = Laps::default();
        // 10 m/s for 59 s: far enough, not long enough.
        let (_, now) = ride(&mut laps, 0, 59, riding(10_000, 200, 140));
        assert_eq!(laps.at_start(now), None, "59 s");
        let (_, now) = ride(&mut laps, now, 1, riding(10_000, 200, 140));
        let lap = laps.at_start(now).expect("60 s and 600 m");
        assert_eq!((lap.number, lap.trigger), (1, Trigger::Position));
        assert_eq!(laps.take_closed(), Some(Trigger::Position));
        assert_eq!(laps.current().number, 2);
        // 1 m/s for two minutes is time enough, 120 m is not far enough.
        let (_, now) = ride(&mut laps, now, 120, riding(1_000, 100, 100));
        assert_eq!(laps.at_start(now), None, "120 m");
        // 499 m is short; the lap carries on from there.
        let (_, now) = ride(&mut laps, now, 379, riding(1_000, 100, 100));
        assert_eq!(laps.at_start(now), None, "499 m");
        let (_, now) = ride(&mut laps, now, 1, riding(1_000, 100, 100));
        assert_eq!(laps.at_start(now).map(|lap| lap.distance_mm), Some(500_000));
    }

    #[test]
    fn laps_at_the_start_leave_workouts_to_their_steps() {
        let mut laps = Laps {
            by_distance: false,
            ..Laps::default()
        };
        let (_, now) = ride(&mut laps, 0, 300, riding(10_000, 200, 140));
        assert_eq!(laps.at_start(now), None);
        assert_eq!(laps.count(), 0);
    }

    #[test]
    fn auto_lap_choices_say_what_closes_laps() {
        assert_eq!(AutoLap::Off.every_mm(), None);
        assert_eq!(AutoLap::Every(0).every_mm(), None);
        assert_eq!(AutoLap::Every(5).every_mm(), Some(AUTO_LAP_MM));
        assert_eq!(AutoLap::Every(100).every_mm(), Some(100_000_000));
        assert_eq!(AutoLap::AtStart.every_mm(), None);
    }
}
