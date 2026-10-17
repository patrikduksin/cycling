//! Readings that glide. Sensors report a few times per second (ANT+ about
//! 4 Hz), so a number drawn straight from them jumps. A `Glide` eases the
//! drawn value from where it is toward each new reading over a fixed time,
//! fast at first and slowing as it arrives (ease-out). It never passes the
//! reading and lands on it exactly, so a settled number is the true one.
//!
//! Feed it every reading with `set`, ask `at` for the value to draw, and keep
//! drawing frames while `moving` says the value is still on its way.

/// Fraction bits of the drawn value between whole units.
const FRACTION: u32 = 8;
const ONE: i64 = 1 << FRACTION;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Glide {
    duration_ms: u32,
    /// Where the current step started, in units of 1 / `ONE`.
    from: i64,
    to: Option<i32>,
    start_ms: u64,
}

impl Glide {
    /// A glide that takes `duration_ms` to reach each new reading.
    pub const fn new(duration_ms: u32) -> Self {
        Self {
            duration_ms,
            from: 0,
            to: None,
            start_ms: 0,
        }
    }

    /// The latest reading at `now_ms`. A changed reading starts a new step
    /// from wherever the drawn value is now. The first reading, and a missing
    /// one, show at once: there is nothing to glide from or to.
    pub fn set(&mut self, reading: Option<i32>, now_ms: u64) {
        if reading == self.to {
            return;
        }
        self.from = match (self.to, reading) {
            (Some(_), Some(_)) => self.fixed(now_ms),
            (_, Some(value)) => i64::from(value) * ONE,
            _ => 0,
        };
        self.to = reading;
        self.start_ms = now_ms;
    }

    /// Start a step toward `reading` from `from` rather than from the drawn
    /// value: for callers that draw the value moved further, such as a radar
    /// target extrapolated by its closing speed.
    pub fn restart(&mut self, from: i32, reading: i32, now_ms: u64) {
        self.from = i64::from(from) * ONE;
        self.to = Some(reading);
        self.start_ms = now_ms;
    }

    /// The value to draw at `now_ms`, rounded to whole units.
    pub fn at(&self, now_ms: u64) -> Option<i32> {
        self.to?;
        let fixed = self.fixed(now_ms);
        Some(((fixed + ONE / 2).div_euclid(ONE)) as i32)
    }

    /// The latest reading, where the glide lands.
    pub fn reading(&self) -> Option<i32> {
        self.to
    }

    /// The drawn value is still on its way to the reading at `now_ms`.
    pub fn moving(&self, now_ms: u64) -> bool {
        self.to.is_some_and(|to| {
            now_ms < self.start_ms + u64::from(self.duration_ms) && self.from != i64::from(to) * ONE
        })
    }

    fn fixed(&self, now_ms: u64) -> i64 {
        let Some(to) = self.to else {
            return 0;
        };
        let to = i64::from(to) * ONE;
        let duration = i64::from(self.duration_ms);
        let elapsed = now_ms
            .saturating_sub(self.start_ms)
            .min(self.duration_ms as u64) as i64;
        if elapsed >= duration {
            return to;
        }
        // Ease-out: covered = 1 - (1 - t)^2, which only grows from 0 to 1.
        let left = duration - elapsed;
        let whole = duration * duration;
        self.from + (to - self.from) * (whole - left * left) / whole
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_first_reading_and_a_lost_one_show_at_once() {
        let mut glide = Glide::new(200);
        assert_eq!(glide.at(0), None);
        glide.set(Some(250), 10);
        assert_eq!(glide.at(10), Some(250));
        assert!(!glide.moving(10));
        glide.set(None, 20);
        assert_eq!(glide.at(20), None);
        assert!(!glide.moving(20));
        glide.set(Some(90), 30);
        assert_eq!(glide.at(30), Some(90));
    }

    #[test]
    fn a_new_reading_eases_in_without_passing_it_and_lands_exactly() {
        let mut glide = Glide::new(200);
        glide.set(Some(100), 0);
        glide.set(Some(200), 1000);
        let mut last = 100;
        let mut steps = std::vec::Vec::new();
        for now in (1000..=1200).step_by(40) {
            let value = glide.at(now).unwrap();
            assert!((last..=200).contains(&value), "{value} at {now}");
            steps.push(value - last);
            last = value;
        }
        assert_eq!(glide.at(1200), Some(200));
        assert!(!glide.moving(1200));
        assert!(glide.moving(1100));
        // Fast at first, then slowing.
        assert!(steps[1] > steps[steps.len() - 1], "{steps:?}");
        // Downward too, and never below the reading.
        glide.set(Some(150), 2000);
        for now in 2000..=2300 {
            let value = glide.at(now).unwrap();
            assert!((150..=200).contains(&value));
        }
        assert_eq!(glide.at(2200), Some(150));
    }

    #[test]
    fn a_reading_mid_step_continues_from_the_drawn_value() {
        let mut glide = Glide::new(200);
        glide.set(Some(0), 0);
        glide.set(Some(1000), 0);
        let middle = glide.at(100).unwrap();
        assert!((500..1000).contains(&middle));
        glide.set(Some(0), 100);
        assert_eq!(glide.at(100), Some(middle));
        assert!(glide.at(150).unwrap() < middle);
        assert_eq!(glide.at(300), Some(0));
    }

    #[test]
    fn negative_values_round_to_the_nearest_unit() {
        let mut glide = Glide::new(100);
        glide.set(Some(-10), 0);
        glide.set(Some(-20), 0);
        for now in 0..=100 {
            assert!((-20..=-10).contains(&glide.at(now).unwrap()));
        }
        assert_eq!(glide.at(100), Some(-20));
    }

    #[test]
    fn restart_starts_from_the_given_value() {
        let mut glide = Glide::new(100);
        glide.set(Some(50), 0);
        glide.restart(40, 30, 500);
        assert_eq!(glide.at(500), Some(40));
        assert!(glide.moving(550));
        assert_eq!(glide.at(600), Some(30));
        assert_eq!(glide.reading(), Some(30));
    }

    #[test]
    fn a_zero_duration_follows_readings_exactly() {
        let mut glide = Glide::new(0);
        glide.set(Some(1), 0);
        glide.set(Some(9), 5);
        assert_eq!(glide.at(5), Some(9));
        assert!(!glide.moving(5));
    }
}
