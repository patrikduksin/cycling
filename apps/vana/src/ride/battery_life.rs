//! Battery time left: how long the charge should last at the rate it has
//! actually been falling.
//!
//! The device reports whole percent, coarse and with an unvalidated
//! calibration (issue #88), so the estimator trusts little and prefers to be
//! early. It tracks the lowest percent seen and notes when each new low was
//! reached, so a gauge that flickers by a point leaves it alone; charging, a
//! rise of two steps that holds, or a fall of over `MAX_JUMP` points start it
//! over. A smaller sudden fall counts as one moment and only restarts the
//! recent slope. The rate is the worse of two readings: the median of the
//! rates between pairs of moments over `WINDOW_MS` (Theil-Sen), and the slope
//! of the last `RECENT_MS`, since load steps make the old rate too kind. It
//! says nothing until the discharge has run `MIN_SPAN_MS` over `MIN_MOMENTS`
//! points, nor near full, when a fresh charge sags. Below `KNEE_PERCENT` the
//! answer is cut by `KNEE_SCALE`, because lithium gauges fall faster near
//! empty. A gauge that steps by 5 points needs six steps inside the window,
//! so it shows an estimate only above about 8 %/h.
//!
//! Pure policy: the caller feeds each fresh report with the clock and asks
//! for minutes left. The rate is worked out once per new moment and kept.

/// Moments older than this stop counting.
pub const WINDOW_MS: u64 = 3 * 60 * 60 * 1000;
/// The recent slope looks back this far.
pub const RECENT_MS: u64 = 45 * 60 * 1000;
/// Points and time the discharge must show before the first estimate.
pub const MIN_MOMENTS: usize = 6;
pub const MIN_SPAN_MS: u64 = 40 * 60 * 1000;
/// The recent slope needs this much of its own.
const RECENT_MOMENTS: usize = 3;
const RECENT_SPAN_MS: u64 = 15 * 60 * 1000;
/// A fall larger than this between two reports may be a gauge jump: it
/// counts as one moment, but the recent slope starts over from it.
pub const MAX_STEP: u8 = 3;
/// A fall this large is no discharge at all: start over.
pub const MAX_JUMP: u8 = 20;
/// A rise of at least this many points over the lowest seen (or two steps of
/// the gauge, if more), held for `RISE_HOLD_MS`, ends the discharge.
const RISE: u8 = 2;
const RISE_HOLD_MS: u64 = 2 * 60 * 1000;
/// Nothing above this percent: a fresh charge sags before it settles.
pub const FULL_PERCENT: u8 = 95;
/// Below this percent the estimate is multiplied by `KNEE_SCALE` / 10.
pub const KNEE_PERCENT: u8 = 40;
const KNEE_SCALE: u64 = 7;
/// A report older than this makes the estimate unknown.
pub const STALE_MS: u64 = 60_000;
/// Moments closer than this say little about a rate and are not paired.
const MIN_PAIR_MS: u64 = 5 * 60 * 1000;
/// Estimates of this long or longer are not shown: beyond any ride.
pub const MAX_MINUTES: u32 = 99 * 60;
const KEPT: usize = 12;
const PAIRS: usize = KEPT * (KEPT - 1) / 2;
const HOUR_MS: u64 = 3_600_000;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Moment {
    at_ms: u64,
    percent: u8,
}

/// The discharge seen so far.
#[derive(Clone, Copy, Debug, Default)]
pub struct BatteryLife {
    moments: [Moment; KEPT],
    len: usize,
    /// The lowest percent seen this discharge.
    low: Option<u8>,
    /// When the last fall of more than `MAX_STEP` points landed; the recent
    /// slope does not look back past it.
    jump_ms: u64,
    /// Points of the last fall: the gauge's step, which sizes the overdue cap.
    step: u8,
    rise_since: Option<u64>,
    seen_ms: Option<u64>,
    /// Worse of the window and recent rates, in thousandths of a percent per
    /// hour; zero while unknown.
    rate: u32,
}

impl BatteryLife {
    /// Take a fresh report. `percent` or `charging` unknown, or charging,
    /// forgets what was learned: a discharge is not known to be under way.
    pub fn observe(&mut self, now: u64, percent: Option<u8>, charging: Option<bool>) {
        let (Some(percent), Some(false)) = (percent, charging) else {
            *self = Self::default();
            return;
        };
        self.seen_ms = Some(now);
        let expired = self.expire(now);
        let Some(low) = self.low else {
            self.begin(now, percent);
            return;
        };
        if percent >= low.saturating_add(RISE.max(self.step.saturating_mul(2))) {
            let since = *self.rise_since.get_or_insert(now);
            if now.saturating_sub(since) >= RISE_HOLD_MS {
                self.begin(now, percent);
            }
            return;
        }
        self.rise_since = None;
        if percent >= low {
            if expired {
                self.refresh();
            }
            return;
        }
        if low - percent > MAX_JUMP {
            self.begin(now, percent);
            return;
        }
        if low - percent > MAX_STEP {
            self.jump_ms = now;
        }
        self.step = low - percent;
        if self.len == KEPT {
            // Thin the old end, not the oldest point, so a fast drain keeps its span.
            self.moments.copy_within(2.., 1);
            self.len -= 1;
        }
        self.moments[self.len] = Moment {
            at_ms: now,
            percent,
        };
        self.len += 1;
        self.low = Some(percent);
        self.refresh();
    }

    fn begin(&mut self, now: u64, percent: u8) {
        *self = Self {
            low: Some(percent),
            seen_ms: Some(now),
            ..Self::default()
        };
    }

    fn expire(&mut self, now: u64) -> bool {
        let old = self.moments[..self.len]
            .iter()
            .take_while(|m| now.saturating_sub(m.at_ms) > WINDOW_MS)
            .count();
        if old > 0 {
            self.moments.copy_within(old..self.len, 0);
            self.len -= old;
        }
        old > 0
    }

    /// Work the rate out again; called when the moments change.
    fn refresh(&mut self) {
        self.rate = 0;
        let moments = &self.moments[..self.len];
        let (Some(first), Some(last)) = (moments.first(), moments.last()) else {
            return;
        };
        if moments.len() < MIN_MOMENTS || last.at_ms.saturating_sub(first.at_ms) < MIN_SPAN_MS {
            return;
        }
        let mut rates = [0u32; PAIRS];
        let mut count = 0;
        for (index, a) in moments.iter().enumerate() {
            for b in &moments[index + 1..] {
                let ms = b.at_ms.saturating_sub(a.at_ms);
                if ms >= MIN_PAIR_MS && a.percent > b.percent {
                    rates[count] = slope(a, b);
                    count += 1;
                }
            }
        }
        let rates = &mut rates[..count];
        rates.sort_unstable();
        let Some(&long) = rates.get(count / 2) else {
            return;
        };
        let recent: &[Moment] = &moments[moments
            .iter()
            .position(|m| {
                last.at_ms.saturating_sub(m.at_ms) <= RECENT_MS && m.at_ms >= self.jump_ms
            })
            .unwrap_or(0)..];
        let recent = match (recent.first(), recent.len() >= RECENT_MOMENTS) {
            (Some(a), true) if last.at_ms.saturating_sub(a.at_ms) >= RECENT_SPAN_MS => {
                slope(a, last)
            }
            _ => 0,
        };
        self.rate = long.max(recent);
    }

    /// Minutes the charge should last at the observed rate, or `None` when
    /// the rate is not yet trustworthy, the report is stale or the answer is
    /// beyond any ride.
    pub fn minutes_left(&self, now: u64) -> Option<u16> {
        let percent = self.low?;
        let last = self.moments[..self.len].last()?;
        if self.rate == 0
            || percent == 0
            || percent > FULL_PERCENT
            || now.saturating_sub(self.seen_ms?) > STALE_MS
        {
            return None;
        }
        let mut rate = u64::from(self.rate);
        // A point more than two gaps overdue: the rate is at most two steps
        // of the gauge over the time since the last. Two, not one: tracking
        // the lowest reading of a flickering gauge puts the last point early.
        let since = now.saturating_sub(last.at_ms);
        if let Some(cap) = (2000 * HOUR_MS * u64::from(self.step.max(1))).checked_div(since) {
            rate = rate.min(cap);
        }
        let mut minutes = u64::from(percent) * 60_000 / rate.max(1);
        if percent < KNEE_PERCENT {
            minutes = minutes * KNEE_SCALE / 10;
        }
        (minutes < u64::from(MAX_MINUTES)).then_some(minutes as u16)
    }
}

/// Thousandths of a percent per hour between two moments.
fn slope(a: &Moment, b: &Moment) -> u32 {
    let drop = u64::from(a.percent.saturating_sub(b.percent));
    (drop * 1000 * HOUR_MS / b.at_ms.saturating_sub(a.at_ms).max(1)) as u32
}

/// Minutes as the rider reads them, always rounded down so the answer errs
/// short: `45 MIN` under an hour (to 5 min), otherwise whole hours, `9 H`.
pub fn about(minutes: u16) -> heapless::String<8> {
    use core::fmt::Write;
    let mut text = heapless::String::new();
    if minutes < 60 {
        let _ = write!(text, "{} MIN", (minutes / 5 * 5).max(5));
    } else {
        let _ = write!(text, "{} H", minutes / 60);
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::println;
    use std::vec::Vec;

    const MIN: u64 = 60_000;

    /// A deterministic uniform number in -1.0..1.0.
    struct Noise(u64);

    impl Noise {
        fn next(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) as f64 / (1u64 << 31) as f64) * 2.0 - 1.0
        }
    }

    /// The true charge in percent after `minute` of unplugged time.
    type Charge = fn(f64) -> f64;

    fn clean(minute: f64) -> f64 {
        90.0 - 8.0 * minute / 60.0
    }

    fn knee(minute: f64) -> f64 {
        // 8 %/h down to 40%, then three times as steep.
        let to_knee = 50.0 / 8.0 * 60.0;
        if minute <= to_knee {
            90.0 - 8.0 * minute / 60.0
        } else {
            40.0 - 24.0 * (minute - to_knee) / 60.0
        }
    }

    fn step(minute: f64) -> f64 {
        // 6 %/h for 90 min, then 16 %/h.
        if minute <= 90.0 {
            90.0 - 6.0 * minute / 60.0
        } else {
            81.0 - 16.0 * (minute - 90.0) / 60.0
        }
    }

    /// Minutes until `charge` reaches zero, from `minute`.
    fn truth(charge: Charge, minute: f64) -> f64 {
        let mut at = minute;
        while charge(at) > 0.0 {
            at += 0.5;
        }
        at - minute
    }

    /// A gauge: `noise` points of jitter on the charge, shown in steps of
    /// `quantum` points.
    #[derive(Clone, Copy)]
    struct Gauge {
        noise: f64,
        quantum: f64,
        seed: u64,
    }

    impl Gauge {
        const fn new(noise: f64) -> Self {
            Self {
                noise,
                quantum: 1.0,
                seed: 7,
            }
        }
    }

    /// Feed one report a minute of the gauge and return, at each of `asks`
    /// minutes, (estimate, truth).
    fn run_gauge(charge: Charge, gauge: Gauge, asks: &[u64]) -> Vec<(Option<u16>, f64)> {
        let mut life = BatteryLife::default();
        let mut jitter = Noise(gauge.seed);
        let mut out = Vec::new();
        for minute in 0..=*asks.last().unwrap() {
            let now = minute * MIN;
            let reading = charge(minute as f64) + gauge.noise * jitter.next();
            let percent = (reading / gauge.quantum).floor() * gauge.quantum;
            life.observe(now, Some(percent.clamp(1.0, 100.0) as u8), Some(false));
            if asks.contains(&minute) {
                out.push((life.minutes_left(now), truth(charge, minute as f64)));
            }
        }
        out
    }

    fn run(charge: Charge, noise: f64, asks: &[u64]) -> Vec<(Option<u16>, f64)> {
        run_gauge(charge, Gauge::new(noise), asks)
    }

    const SEEDS: u64 = 40;

    /// Over `SEEDS` runs and every fifth minute from `from` while at least
    /// an hour of truth remains: the worst estimate over truth, the worst
    /// under it, and how often there was no estimate.
    fn sweep(charge: Charge, gauge: Gauge, from: u64) -> (f64, f64, f64) {
        let last = (from..)
            .step_by(5)
            .take_while(|&m| truth(charge, m as f64) >= 60.0)
            .last()
            .unwrap();
        let asks: Vec<u64> = (from..=last).step_by(5).collect();
        let (mut high, mut low, mut none, mut total) = (0.0f64, f64::MAX, 0, 0);
        for seed in 0..SEEDS {
            for (estimate, truth) in run_gauge(charge, Gauge { seed, ..gauge }, &asks) {
                total += 1;
                match estimate {
                    Some(m) => {
                        high = high.max(f64::from(m) / truth);
                        low = low.min(f64::from(m) / truth);
                    }
                    None => none += 1,
                }
            }
        }
        (high, low, f64::from(none) / f64::from(total))
    }

    fn steps(noise: f64, quantum: f64) -> Gauge {
        Gauge {
            noise,
            quantum,
            seed: 0,
        }
    }

    /// Worst case over every sampled minute and seed, as a Markdown table.
    #[test]
    #[ignore = "prints the accuracy table"]
    fn estimate_against_truth() {
        println!("| drain | gauge | worst over | worst under | no estimate |");
        println!("| --- | --- | --- | --- | --- |");
        // The load step is swept from 45 min after it: no history foretells it.
        for (name, charge, from) in [
            ("8 %/h", clean as Charge, 60),
            ("load step", step as Charge, 135),
        ] {
            for (gauge, label) in [
                (steps(0.0, 1.0), "clean"),
                (steps(0.6, 1.0), "+-0.6"),
                (steps(1.0, 1.0), "+-1.0"),
                (steps(0.0, 2.0), "2 point steps"),
                (steps(0.0, 5.0), "5 point steps"),
            ] {
                let (high, low, none) = sweep(charge, gauge, from);
                let none_pct = none * 100.0;
                if none >= 1.0 {
                    println!("| {name} | {label} | - | - | {none_pct:.0}% |");
                } else {
                    println!("| {name} | {label} | {high:.2}x | {low:.2}x | {none_pct:.0}% |");
                }
            }
        }
    }

    #[test]
    fn a_straight_drain_is_never_much_too_kind_on_any_gauge() {
        for gauge in [
            steps(0.0, 1.0),
            steps(0.6, 1.0),
            steps(1.0, 1.0),
            steps(0.0, 2.0),
            steps(0.6, 2.0),
        ] {
            let (high, low, none) = sweep(clean, gauge, 60);
            assert!(high <= 1.15, "over by {high} at noise {}", gauge.noise);
            assert!(low >= 0.4, "under by {low} at noise {}", gauge.noise);
            assert!(none <= 0.05, "no estimate {none} at noise {}", gauge.noise);
        }
    }

    #[test]
    fn a_five_point_gauge_gives_an_estimate_only_at_a_brisk_drain() {
        // 8 %/h is a step every 37 min: six steps do not fit the 3 h window.
        let (_, _, none) = sweep(clean, steps(0.0, 5.0), 60);
        assert!(none > 0.9, "{none}");
        // At 25 %/h a step comes every 12 min, and a fall of 5 is no restart.
        fn brisk(minute: f64) -> f64 {
            95.0 - 25.0 * minute / 60.0
        }
        let (high, _, none) = sweep(brisk, steps(0.0, 5.0), 60);
        assert!(none < 0.5, "{none}");
        assert!(high <= 1.5, "{high}");
    }

    #[test]
    fn says_nothing_before_enough_has_been_seen() {
        let life = BatteryLife::default();
        assert_eq!(life.minutes_left(0), None);
        for (estimate, _) in run(clean, 0.0, &[10, 30, 39]) {
            assert_eq!(estimate, None);
        }
        let mut life = BatteryLife::default();
        for m in 0..=90u64 {
            life.observe(m * MIN, Some(80 - (m / 30) as u8), Some(false));
        }
        assert_eq!(life.minutes_left(90 * MIN), None, "too few points");
    }

    #[test]
    fn steady_discharge_gives_about_the_true_time_left() {
        let [(estimate, truth)] = run(clean, 0.0, &[120])[..] else {
            unreachable!()
        };
        let estimate = f64::from(estimate.unwrap());
        assert!(
            (0.8 * truth..=1.1 * truth).contains(&estimate),
            "{estimate} v {truth}"
        );
    }

    #[test]
    fn a_gauge_that_flickers_still_gives_an_estimate_that_is_not_too_kind() {
        for (estimate, truth) in run(clean, 0.6, &[90, 150, 220]) {
            let estimate = f64::from(estimate.expect("jitter must not hide the estimate"));
            assert!(
                (0.7 * truth..=1.15 * truth).contains(&estimate),
                "{estimate} v {truth}"
            );
        }
    }

    #[test]
    fn a_steeper_knee_is_not_promised_away() {
        // The knee comes at minute 375 and no gauge history foretells it;
        // an hour on, the recent slope and the near-empty cut have caught up.
        let asked = run(knee, 0.0, &[440]);
        let (estimate, truth) = asked[0];
        let estimate = f64::from(estimate.unwrap());
        assert!(estimate <= truth * 1.3, "{estimate} v {truth}");
    }

    #[test]
    fn a_load_step_pulls_the_estimate_down() {
        let asked = run(step, 0.0, &[85, 130]);
        let before = f64::from(asked[0].0.unwrap());
        let after = f64::from(asked[1].0.unwrap());
        assert!(after < before * 0.8, "{before} then {after}");
        assert!(after <= asked[1].1 * 1.6);
    }

    #[test]
    fn a_plateau_stretches_the_estimate_but_never_hides_it() {
        let mut life = BatteryLife::default();
        for m in 0..=120u64 {
            life.observe(
                m * MIN,
                Some((90.0 - 10.0 * m as f64 / 60.0) as u8),
                Some(false),
            );
        }
        let steady = life.minutes_left(120 * MIN).unwrap();
        life.observe(150 * MIN, Some(73), Some(false));
        let stalled = life.minutes_left(150 * MIN).unwrap();
        assert!(stalled > steady, "{steady} then {stalled}");
    }

    #[test]
    fn nothing_near_full_or_just_after_unplugging() {
        let mut life = BatteryLife::default();
        for m in 0..=120u64 {
            life.observe(m * MIN, Some(100 - (m / 6) as u8), Some(false));
        }
        assert!(life.minutes_left(120 * MIN).is_some());
        let mut full = BatteryLife::default();
        for m in 0..=60u64 {
            full.observe(m * MIN, Some(99 - (m / 12) as u8), Some(false));
        }
        assert_eq!(full.minutes_left(60 * MIN), None, "above 95%");
        // The same discharge, asked inside the settling time of its start.
        let mut early = BatteryLife::default();
        early.observe(0, Some(80), Some(false));
        assert_eq!(early.minutes_left(10 * MIN), None);
    }

    #[test]
    fn charging_forgets_the_discharge() {
        let mut life = BatteryLife::default();
        for (m, minute) in (0..=120u64).enumerate() {
            life.observe(minute * MIN, Some(90 - (m / 6) as u8), Some(false));
        }
        assert!(life.minutes_left(120 * MIN).is_some());
        life.observe(121 * MIN, Some(70), Some(true));
        assert_eq!(life.minutes_left(121 * MIN), None);
        life.observe(122 * MIN, Some(75), Some(false));
        assert_eq!(life.minutes_left(122 * MIN), None);
    }

    #[test]
    fn unknown_state_forgets_and_says_nothing() {
        let mut life = BatteryLife::default();
        for minute in 0..=120u64 {
            life.observe(minute * MIN, Some(90 - (minute / 6) as u8), Some(false));
        }
        life.observe(121 * MIN, Some(70), None);
        assert_eq!(life.minutes_left(121 * MIN), None);
        life.observe(122 * MIN, None, Some(false));
        assert_eq!(life.minutes_left(122 * MIN), None);
    }

    #[test]
    fn a_stale_report_makes_the_estimate_unknown() {
        let mut life = BatteryLife::default();
        for minute in 0..=120u64 {
            life.observe(minute * MIN, Some(90 - (minute / 6) as u8), Some(false));
        }
        assert!(life.minutes_left(120 * MIN + STALE_MS).is_some());
        assert_eq!(life.minutes_left(120 * MIN + STALE_MS + 1), None);
    }

    #[test]
    fn a_huge_fall_or_a_lasting_rise_starts_over_but_a_blip_does_not() {
        let build = || {
            let mut life = BatteryLife::default();
            for minute in 0..=120u64 {
                life.observe(minute * MIN, Some(90 - (minute / 6) as u8), Some(false));
            }
            life
        };
        let mut life = build();
        let before = life.minutes_left(120 * MIN).unwrap();
        life.observe(121 * MIN, Some(58), Some(false));
        let after = life.minutes_left(121 * MIN).unwrap();
        assert!(
            after < before,
            "a big fall counts as a moment: {before} then {after}"
        );
        let mut life = build();
        life.observe(121 * MIN, Some(40), Some(false));
        assert_eq!(life.minutes_left(121 * MIN), None, "no discharge falls so");
        let mut life = build();
        life.observe(121 * MIN, Some(76), Some(false));
        life.observe(122 * MIN, Some(70), Some(false));
        assert!(
            life.minutes_left(122 * MIN).is_some(),
            "a blip that falls back"
        );
        let mut life = build();
        for minute in 121..=124u64 {
            life.observe(minute * MIN, Some(76), Some(false));
        }
        assert_eq!(life.minutes_left(124 * MIN), None, "a rise that holds");
    }

    #[test]
    fn old_moments_age_out_of_the_window() {
        let mut life = BatteryLife::default();
        for minute in 0..=90u64 {
            life.observe(minute * MIN, Some(90 - (minute / 3) as u8), Some(false));
        }
        let later = 90 * MIN + WINDOW_MS + MIN;
        life.observe(later, Some(50), Some(false));
        assert_eq!(life.minutes_left(later), None);
    }

    #[test]
    fn reads_in_hours_and_minutes_rounded_down() {
        assert_eq!(about(540).as_str(), "9 H");
        assert_eq!(about(60).as_str(), "1 H");
        assert_eq!(about(89).as_str(), "1 H");
        assert_eq!(about(119).as_str(), "1 H");
        assert_eq!(about(59).as_str(), "55 MIN");
        assert_eq!(about(44).as_str(), "40 MIN");
        assert_eq!(about(3).as_str(), "5 MIN");
    }
}
