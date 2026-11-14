//! A backlight that follows the ambient light. The state is integers moved
//! in fixed ticks of elapsed time, so the device, the simulator and a replay
//! of a trace reach the same percent from the same readings.
use device_api::observation::Observation;
use device_api::sensors::Light;

const TICK_MS: u64 = 100;
/// The most the shown percent moves in one tick, 40% a second.
const RAMP_PERCENT: u8 = 4;
/// The target must leave the held percent by more than this to move it.
const BAND_PERCENT: u8 = 3;
/// Each tick the smoothed light closes 1/n of its gap to a brighter reading,
/// or to a dimmer one: bright is quick, dark is slow, so a passing shadow
/// does not pump the screen.
const BRIGHTEN_DIVISOR: u16 = 2;
const DIM_DIVISOR: u16 = 30;
/// Ticks worked off after a long pause between calls.
const CATCH_UP_TICKS: u64 = 50;

/// Light to duty percent, as knots with rising light. The duty is linear in
/// power, so the knots carry the perceptual shape. Light below the first knot
/// shows its percent and light above the last shows the last.
#[derive(Debug)]
pub struct Curve(pub [(u16, u8); 5]);

impl Curve {
    pub fn percent(&self, light: u16) -> u8 {
        let knots = &self.0;
        match knots.iter().position(|&(knot, _)| light < knot) {
            None => knots[knots.len() - 1].1,
            Some(0) => knots[0].1,
            Some(above) => {
                let (from, low) = knots[above - 1];
                let (to, high) = knots[above];
                let (low, high) = (i32::from(low), i32::from(high));
                let span = i32::from(to - from);
                let along = i32::from(light - from);
                (low + (high - low) * along / span) as u8
            }
        }
    }

    fn is_end(&self, percent: u8) -> bool {
        percent == self.0[0].1 || percent == self.0[self.0.len() - 1].1
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backlight {
    at: Option<u64>,
    smoothed: Option<u16>,
    held: u8,
    shown: Option<u8>,
}

impl Backlight {
    /// The percent to show at `now`. Without a fresh reading it ramps to
    /// `fallback`, the rider's stored brightness. Calling again at the same
    /// `now` changes nothing.
    pub fn step(&mut self, light: Observation<Light>, curve: &Curve, fallback: u8, now: u64) -> u8 {
        let ticks = match self.at {
            None => {
                self.at = Some(now);
                1
            }
            Some(at) => {
                let ticks = now.saturating_sub(at) / TICK_MS;
                self.at = Some(at + ticks * TICK_MS);
                ticks
            }
        };
        let reading = match light {
            Observation::Fresh { value, .. } => Some(value.0),
            Observation::Stale { .. } | Observation::Unavailable => None,
        };
        for _ in 0..ticks.min(CATCH_UP_TICKS) {
            self.tick(reading, curve, fallback);
        }
        self.shown.unwrap_or(fallback)
    }

    fn tick(&mut self, reading: Option<u16>, curve: &Curve, fallback: u8) {
        match reading {
            Some(reading) => {
                let seeded = self.smoothed.is_none();
                let smoothed = self
                    .smoothed
                    .map_or(reading, |level| smooth(level, reading));
                self.smoothed = Some(smoothed);
                let target = curve.percent(smoothed);
                let leaves = target.abs_diff(self.held) > BAND_PERCENT;
                // The ends are reached exactly, and left only by the band.
                if seeded || leaves || (target != self.held && curve.is_end(target)) {
                    self.held = target;
                }
            }
            None => {
                self.smoothed = None;
                self.held = fallback;
            }
        }
        let shown = self.shown.unwrap_or(fallback);
        self.shown = Some(if shown < self.held {
            shown.saturating_add(RAMP_PERCENT).min(self.held)
        } else {
            shown.saturating_sub(RAMP_PERCENT).max(self.held)
        });
    }
}

fn smooth(level: u16, reading: u16) -> u16 {
    if reading > level {
        level + (reading - level).div_ceil(BRIGHTEN_DIVISOR)
    } else {
        level - (level - reading).div_ceil(DIM_DIVISOR)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    const CURVE: Curve = Curve([(40, 10), (70, 25), (300, 55), (1_500, 85), (4_000, 100)]);

    fn fresh(raw: u16) -> Observation<Light> {
        Observation::Fresh {
            value: Light(raw),
            received_ms: 0,
        }
    }

    /// Run `seconds` of ticks on `raw`, returning the percent after each.
    fn run(
        backlight: &mut Backlight,
        light: Observation<Light>,
        from_ms: u64,
        seconds: u64,
    ) -> Vec<u8> {
        (1..=seconds * 1_000 / TICK_MS)
            .map(|tick| backlight.step(light, &CURVE, 50, from_ms + tick * TICK_MS))
            .collect()
    }

    #[test]
    fn the_curve_is_flat_outside_its_knots_and_straight_between_them() {
        assert_eq!(CURVE.percent(0), 10);
        assert_eq!(CURVE.percent(40), 10);
        assert_eq!(CURVE.percent(55), 17);
        assert_eq!(CURVE.percent(300), 55);
        assert_eq!(CURVE.percent(4_000), 100);
        assert_eq!(CURVE.percent(u16::MAX), 100);
    }

    #[test]
    fn a_step_down_in_light_dims_monotonically_to_the_new_level() {
        let mut backlight = Backlight::default();
        let bright = run(&mut backlight, fresh(1_500), 0, 20);
        assert_eq!(bright.last(), Some(&85));
        let dim = run(&mut backlight, fresh(68), 20_000, 120);
        assert!(dim.windows(2).all(|pair| pair[1] <= pair[0]), "{dim:?}");
        assert!(dim.windows(2).any(|pair| pair[1] < pair[0]));
        assert!(dim.last().unwrap().abs_diff(CURVE.percent(68)) <= BAND_PERCENT);
    }

    #[test]
    fn dimming_is_slower_than_brightening() {
        let mut backlight = Backlight::default();
        run(&mut backlight, fresh(68), 0, 20);
        let up = run(&mut backlight, fresh(1_500), 20_000, 5);
        assert!(up.last().unwrap().abs_diff(85) <= BAND_PERCENT, "{up:?}");
        let down = run(&mut backlight, fresh(68), 25_000, 5);
        assert!(*down.last().unwrap() > 50, "{down:?}");
    }

    #[test]
    fn noise_around_a_knot_never_toggles_the_output() {
        let mut backlight = Backlight::default();
        run(&mut backlight, fresh(300), 0, 10);
        let mut shown = Vec::new();
        for (index, raw) in [296, 304, 299, 302, 297, 303]
            .into_iter()
            .cycle()
            .take(600)
            .enumerate()
        {
            let now = 10_000 + (index as u64 + 1) * TICK_MS;
            shown.push(backlight.step(fresh(raw), &CURVE, 50, now));
        }
        assert!(shown.windows(2).all(|pair| pair[0] == pair[1]), "{shown:?}");
    }

    #[test]
    fn the_output_never_moves_faster_than_the_ramp() {
        let mut backlight = Backlight::default();
        let mut previous = backlight.step(fresh(68), &CURVE, 50, 0);
        for (index, raw) in [1_500, 68, 4_000, 40, 1_500].into_iter().enumerate() {
            for tick in 1..=30 {
                let now = (index as u64 * 30 + tick) * TICK_MS;
                let shown = backlight.step(fresh(raw), &CURVE, 50, now);
                assert!(
                    shown.abs_diff(previous) <= RAMP_PERCENT,
                    "{previous} to {shown}"
                );
                previous = shown;
            }
        }
    }

    #[test]
    fn calling_again_at_the_same_time_changes_nothing() {
        let mut backlight = Backlight::default();
        run(&mut backlight, fresh(68), 0, 5);
        let first = backlight.step(fresh(1_500), &CURVE, 50, 5_300);
        let state = backlight;
        for _ in 0..3 {
            assert_eq!(backlight.step(fresh(40), &CURVE, 50, 5_300), first);
            assert_eq!(backlight, state);
        }
    }

    #[test]
    fn the_ends_are_reached_exactly() {
        let mut backlight = Backlight::default();
        assert_eq!(run(&mut backlight, fresh(9_000), 0, 10).last(), Some(&100));
        assert_eq!(run(&mut backlight, fresh(0), 10_000, 200).last(), Some(&10));
    }

    #[test]
    fn without_a_fresh_reading_it_returns_to_the_stored_brightness() {
        let mut backlight = Backlight::default();
        assert_eq!(backlight.step(Observation::Unavailable, &CURVE, 50, 0), 50);
        run(&mut backlight, fresh(40), 100, 10);
        let back = run(&mut backlight, Observation::Unavailable, 10_100, 10);
        assert_eq!(back.last(), Some(&50));
        let stale = Observation::Stale {
            value: Light(40),
            received_ms: 0,
        };
        let mut backlight = Backlight::default();
        assert_eq!(run(&mut backlight, stale, 0, 5).last(), Some(&50));
    }

    #[test]
    fn a_long_pause_works_off_a_bounded_number_of_ticks() {
        let mut backlight = Backlight::default();
        backlight.step(fresh(1_500), &CURVE, 50, 0);
        assert_eq!(backlight.step(fresh(1_500), &CURVE, 50, 3_600_000), 85);
    }
}
