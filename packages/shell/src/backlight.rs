//! A backlight that follows the ambient light. The state is integers moved
//! in fixed ticks of elapsed time, so the device, the simulator and a replay
//! of a trace reach the same percent from the same readings.
//!
//! Each tick compares the curve's percent for the reading with the held
//! percent. A target outside the dead band starts a count in its direction,
//! and the held percent follows only once that count outlasts the hold for
//! the direction. A reading back inside the band, or on the other side, drops
//! the count, so a tunnel, a shadow or a street lamp never moves the screen.
//! The shown percent then ramps to the held one, quickly up and slowly down.
use device_api::observation::Observation;
use device_api::sensors::Light;

const TICK_MS: u64 = 100;
/// A brighter target must last longer than this before the screen follows.
/// A passing light or a lamp is shorter, and a rider waits no longer than
/// this to see a lasting brightening start.
const BRIGHTEN_HOLD_TICKS: u16 = 20;
/// A darker target must last longer than this. A tunnel or a cloud is
/// shorter, and the eye adapts to a lasting dimming slowly anyway.
const DIM_HOLD_TICKS: u16 = 60;
/// The most the shown percent rises in one tick, 80% a second: a lasting
/// brightening, once started, finishes in about a second.
const RISE_PERCENT: u8 = 8;
/// The most it falls in one tick, 10% a second: 100% to 10% takes 9 s.
const FALL_PERCENT: u8 = 1;
/// The target must leave the held percent by more than this to count. Noise
/// of a few light counts moves the target by about this much at the steepest
/// part of the curve.
const BAND_PERCENT: u8 = 3;
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

    /// Whether the knots rise in light and never fall in percent, which
    /// `percent` needs to stay in range.
    pub fn rises(&self) -> bool {
        self.0
            .windows(2)
            .all(|pair| pair[0].0 < pair[1].0 && pair[0].1 <= pair[1].1)
    }

    fn is_end(&self, percent: u8) -> bool {
        percent == self.0[0].1 || percent == self.0[self.0.len() - 1].1
    }
}

/// Whether the target has been somewhere other than the held percent, and
/// for how many ticks in a row.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Pull {
    #[default]
    Settled,
    Brighter(u16),
    Darker(u16),
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Backlight {
    at: Option<u64>,
    held: Option<u8>,
    pull: Pull,
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
        let held = match (reading, self.held) {
            (None, _) => {
                self.pull = Pull::Settled;
                fallback
            }
            (Some(reading), None) => curve.percent(reading),
            (Some(reading), Some(held)) => self.follow(curve.percent(reading), held, curve),
        };
        self.held = reading.map(|_| held);
        let shown = self.shown.unwrap_or(fallback);
        self.shown = Some(if shown < held {
            shown.saturating_add(RISE_PERCENT).min(held)
        } else {
            shown.saturating_sub(FALL_PERCENT).max(held)
        });
    }

    /// The held percent after one more tick toward `target`.
    fn follow(&mut self, target: u8, held: u8, curve: &Curve) -> u8 {
        // The ends are reached exactly, and left only by the band.
        let outside =
            target.abs_diff(held) > BAND_PERCENT || (target != held && curve.is_end(target));
        self.pull = match (outside, target > held, self.pull) {
            (false, _, _) => Pull::Settled,
            (true, true, Pull::Brighter(ticks)) => Pull::Brighter(ticks.saturating_add(1)),
            (true, true, _) => Pull::Brighter(1),
            (true, false, Pull::Darker(ticks)) => Pull::Darker(ticks.saturating_add(1)),
            (true, false, _) => Pull::Darker(1),
        };
        let lasted = match self.pull {
            Pull::Brighter(ticks) => ticks > BRIGHTEN_HOLD_TICKS,
            Pull::Darker(ticks) => ticks > DIM_HOLD_TICKS,
            Pull::Settled => false,
        };
        if lasted {
            self.pull = Pull::Settled;
            target
        } else {
            held
        }
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

    /// Settle on `schedule(0)`, then call every `pass_ms` for `seconds`,
    /// reading the light as the companion reports it, three times a second.
    /// Returns the percent after each pass, with its time.
    fn feed(schedule: impl Fn(u64) -> u16, pass_ms: u64, seconds: u64) -> Vec<(u64, u8)> {
        let mut backlight = Backlight::default();
        let settle = 30_000;
        let reading = |ms: u64| schedule(ms.saturating_sub(settle) / 333 * 333);
        run(&mut backlight, fresh(schedule(0)), 0, settle / 1_000);
        (1..=seconds * 1_000 / pass_ms)
            .map(|pass| {
                let now = settle + pass * pass_ms;
                (now, backlight.step(fresh(reading(now)), &CURVE, 50, now))
            })
            .collect()
    }

    fn swing(shown: &[(u64, u8)]) -> u8 {
        let low = shown.iter().map(|&(_, percent)| percent).min().unwrap();
        let high = shown.iter().map(|&(_, percent)| percent).max().unwrap();
        high - low
    }

    /// `dip` for the last `for_ms` of every `every_ms`, else `base`.
    fn pulses(base: u16, dip: u16, every_ms: u64, for_ms: u64) -> impl Fn(u64) -> u16 {
        move |ms| {
            if ms % every_ms >= every_ms - for_ms {
                dip
            } else {
                base
            }
        }
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
    fn a_lasting_step_down_reaches_the_curve_slowly_and_monotonically() {
        let mut backlight = Backlight::default();
        let bright = run(&mut backlight, fresh(1_500), 0, 20);
        assert_eq!(bright.last(), Some(&85));
        let dim = run(&mut backlight, fresh(68), 20_000, 30);
        assert!(dim.windows(2).all(|pair| pair[1] <= pair[0]), "{dim:?}");
        assert_eq!(dim[..55], [85; 55], "no move inside the hold");
        assert_eq!(dim.last(), Some(&CURVE.percent(68)));
        assert!(dim[79] > 60, "dims slowly: {}", dim[79]);
    }

    #[test]
    fn a_lasting_step_up_reaches_the_curve_within_a_few_seconds() {
        let mut backlight = Backlight::default();
        run(&mut backlight, fresh(68), 0, 20);
        let up = run(&mut backlight, fresh(1_500), 20_000, 6);
        assert!(up.windows(2).all(|pair| pair[1] >= pair[0]), "{up:?}");
        assert!(up[..20].iter().all(|&percent| percent == 24), "{up:?}");
        assert_eq!(up[29], 85, "{up:?}");
    }

    #[test]
    fn a_two_second_tunnel_never_moves_the_screen() {
        for (base, tunnel) in [(1_500, 40), (300, 40), (4_000, 40), (1_500, 300)] {
            for pass_ms in [10, 40, 70, 100] {
                let shown = feed(pulses(base, tunnel, 60_000, 2_000), pass_ms, 20);
                assert!(
                    swing(&shown) <= 2,
                    "{base} {tunnel} {pass_ms}: {}",
                    swing(&shown)
                );
            }
        }
    }

    #[test]
    fn a_two_second_bright_spell_never_moves_the_screen() {
        for (base, glare) in [(40, 300), (70, 1_500), (300, 4_000)] {
            let shown = feed(pulses(base, glare, 60_000, 2_000), 40, 20);
            assert!(swing(&shown) <= 2, "{base} {glare}: {}", swing(&shown));
        }
    }

    #[test]
    fn street_lamps_every_four_seconds_at_night_never_move_the_screen() {
        let shown = feed(pulses(40, 300, 4_000, 1_200), 40, 120);
        assert!(swing(&shown) <= 2, "{}", swing(&shown));
    }

    #[test]
    fn shadows_every_two_seconds_in_daylight_never_move_the_screen() {
        let darker = feed(pulses(1_500, 300, 2_000, 600), 40, 120);
        assert!(swing(&darker) <= 2, "{}", swing(&darker));
        let brighter = feed(pulses(300, 1_500, 2_000, 600), 40, 120);
        assert!(swing(&brighter) <= 2, "{}", swing(&brighter));
        let lamps = feed(pulses(40, 70, 2_000, 600), 40, 120);
        assert!(swing(&lamps) <= 2, "{}", swing(&lamps));
    }

    #[test]
    fn noise_around_a_knot_never_moves_the_screen() {
        let mut state = 12_345_u32;
        let mut noise = move |center: u16| {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            center - 7 + (state >> 16) as u16 % 15
        };
        for center in [40, 70, 300, 1_500] {
            let mut backlight = Backlight::default();
            run(&mut backlight, fresh(center), 0, 10);
            let shown: Vec<u8> = (1..=6_000)
                .map(|tick| {
                    backlight.step(fresh(noise(center)), &CURVE, 50, 10_000 + tick * TICK_MS)
                })
                .collect();
            let low = shown.iter().min().unwrap();
            let high = shown.iter().max().unwrap();
            assert!(high - low <= 2, "{center}: {low} to {high}");
        }
    }

    #[test]
    fn the_output_never_moves_faster_than_the_ramp() {
        let mut backlight = Backlight::default();
        let mut previous = backlight.step(fresh(68), &CURVE, 50, 0);
        for (index, raw) in [1_500, 68, 4_000, 40, 1_500].into_iter().enumerate() {
            for tick in 1..=100 {
                let now = (index as u64 * 100 + tick) * TICK_MS;
                let shown = backlight.step(fresh(raw), &CURVE, 50, now);
                assert!(shown <= previous + RISE_PERCENT, "{previous} to {shown}");
                assert!(shown + FALL_PERCENT >= previous, "{previous} to {shown}");
                previous = shown;
            }
        }
    }

    #[test]
    fn passes_of_any_length_converge_to_the_same_output() {
        let light = |ms: u64| match ms {
            0..=9_999 => 68,
            10_000..=29_999 => 1_500,
            30_000..=50_999 => 300,
            _ => 68,
        };
        let reference = feed(light, 10, 120);
        for pass_ms in [20, 30, 40, 70] {
            let other = feed(light, pass_ms, 120);
            for &(time, percent) in &other {
                if let Some(&(_, expected)) = reference.iter().find(|&&(t, _)| t == time) {
                    assert!(
                        percent.abs_diff(expected) <= RISE_PERCENT,
                        "{pass_ms} at {time}"
                    );
                }
            }
            assert_eq!(other.last().unwrap().1, reference.last().unwrap().1);
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
