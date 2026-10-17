//! Laps: the ride split into manual and automatic segments, each with its own
//! time, distance and averages.
//!
//! The caller feeds the same recorded seconds as the ride totals, so a pause
//! adds nothing to the lap. Lap time is recorded (active) time, the clock the
//! main page shows; average speed is distance over moving time, as in the
//! totals. A rider's lap closes the current lap unless another lap closed less
//! than `DEBOUNCE_MS` before; an automatic lap closes it every `AUTO_LAP_MM`
//! of lap distance, or the distance the rider chose (`add_every`). Both count in one numbered list, and the latest `HISTORY`
//! summaries are kept. Laps live in RAM only: the ride journal has no lap
//! record that older readers would skip, so power-off loses them.
use crate::ride::log::Sample;
use crate::ride::totals::MOVING_MIN_MM_S;

/// An automatic lap closes after this much lap distance (5 km).
pub const AUTO_LAP_MM: u64 = 5_000_000;
/// A second lap within this long of the last one is ignored.
pub const DEBOUNCE_MS: u64 = 3000;
/// Finished laps whose summaries are kept, newest last.
pub const HISTORY: usize = 16;

/// How a lap was closed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Trigger {
    #[default]
    Manual,
    Auto,
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

    fn add(&mut self, elapsed_ms: u64, distance_mm: u64, sample: &Sample) {
        self.time_ms = self.time_ms.saturating_add(elapsed_ms);
        self.distance_mm = self.distance_mm.saturating_add(distance_mm);
        if sample
            .speed_mm_s
            .is_some_and(|speed| speed >= MOVING_MIN_MM_S)
        {
            self.moving_ms = self.moving_ms.saturating_add(elapsed_ms);
        }
        if let Some(watts) = sample.power_watts {
            self.energy_mj = self.energy_mj.saturating_add(u64::from(watts) * elapsed_ms);
            self.power_ms = self.power_ms.saturating_add(elapsed_ms);
        }
        if let Some(bpm) = sample.heart_bpm {
            self.heart_sum = self.heart_sum.saturating_add(u32::from(bpm));
            self.heart_count = self.heart_count.saturating_add(1);
        }
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
            ((self.heart_sum + self.heart_count / 2) / self.heart_count).min(u32::from(u16::MAX))
                as u16
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
}

impl Default for Laps {
    fn default() -> Self {
        Self {
            current: Lap::numbered(1),
            history: [Lap::default(); HISTORY],
            finished: 0,
            closed_at_ms: None,
            by_distance: true,
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

    /// Close the current lap automatically, as a workout step ends.
    pub fn split(&mut self, now_ms: u64) -> Lap {
        self.close(Trigger::Auto, now_ms)
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
        lap
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
        let lap = laps.split(now);
        assert_eq!((lap.number, lap.trigger), (1, Trigger::Auto));
        assert_eq!(laps.current().number, 2);
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
}
