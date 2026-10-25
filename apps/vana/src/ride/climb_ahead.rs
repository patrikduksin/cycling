//! Climbs on the road ahead, from the terrain look-ahead (`terrain`).
//!
//! The look-ahead predicts the road and its map elevation every 20 m for up
//! to 10 km. This runs the same detector the ride uses (`climb::Climbs`:
//! 3% over 200 m starts a climb, 20 m of gain confirms it, it ends below 1%
//! or 10 m under its top) over that profile, so a climb ahead is exactly
//! what the ride would detect when riding it, found before the rider gets
//! there. Categories follow `climb::Category`.
//!
//! A forecast is used only when the look-ahead is confident (`MIN_CONFIDENCE`)
//! and reaches far enough to say something; otherwise the climb page draws
//! from the barometer alone.
//!
//! The forecast moves with the rider, so it forgets a climb's foot once the
//! rider is past it. `Pinned` keeps the climb on the ride's odometer from
//! `ANNOUNCE_MM` before its foot to `PAST_TOP_MM` after its top, so the page
//! draws the whole climb and the rider moving along it.
use crate::ride::climb::{Climbs, DROP_DM, END_TENTHS, Event, STEP_MM, Steepness, Summary};
use terrain::{Ahead, End};

/// Profile samples: 10 km at one per 20 m.
pub const SAMPLES: usize = terrain::MAX_SAMPLES;
const _: () = assert!(STEP_MM == terrain::SAMPLE_M as u64 * 1000);
/// Climbs kept from one forecast.
pub const CLIMBS: usize = 3;
/// Below this look-ahead confidence the forecast is not shown.
pub const MIN_CONFIDENCE: u8 = 50;
/// A forecast that reaches less than this is not worth showing (400 m).
const MIN_SAMPLES: usize = 20;
/// The detector's flattening window: 200 m of steps.
const FLAT_STEPS: usize = 10;
/// A climb that starts within this distance is the one the rider is on:
/// the detector finds the start of a climb under way up to a window behind.
pub const ON_CLIMB_MM: u64 = 60_000;
/// A climb is pinned, and the climb page comes up, this far before its foot.
pub const ANNOUNCE_MM: u64 = 500_000;
/// A pinned climb stays this far past its top, so the page does not leave
/// on the summit.
pub const PAST_TOP_MM: u64 = 100_000;
/// A foot the map puts this close to the pinned one is the same climb.
const SAME_FOOT_MM: u64 = 200_000;
/// Within this of the top the map may no longer see the climb under way
/// (`Forecast::remainder` needs 200 m still rising).
const LAST_MM: u64 = FLAT_STEPS as u64 * STEP_MM;
/// Samples a pinned climb keeps (12 km): the forecast's reach from where it
/// was first seen, and some road ridden since.
pub const PINNED: usize = SAMPLES + 100;

/// A climb on the road ahead.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClimbAhead {
    /// `start_mm` is the distance from the rider; altitudes are map
    /// elevations relative to the rider.
    pub summary: Summary,
    /// The road beyond the forecast is unknown, and the climb still rises
    /// there: its length and gain are at least these.
    pub open: bool,
}

impl ClimbAhead {
    /// Distance from the rider to its top.
    pub fn end_mm(&self) -> u64 {
        self.summary.start_mm + self.summary.length_mm
    }
}

/// What the climb page knows about the road ahead.
#[derive(Clone, Debug, PartialEq)]
pub struct Forecast {
    /// The look-ahead result this came from.
    pub generation: u32,
    /// Map elevation every 20 m from the rider, relative to the rider, dm.
    pub samples: [i16; SAMPLES],
    pub len: usize,
    pub end: End,
    pub confidence: u8,
    pub climbs: [Option<ClimbAhead>; CLIMBS],
    /// The last usable forecast had the rider on a climb.
    riding: bool,
    /// A terrain pack and its tile cache are there; the climb page says when
    /// they are not.
    pub map: bool,
}

impl Default for Forecast {
    fn default() -> Self {
        Self::NONE
    }
}

impl Forecast {
    /// No look-ahead result yet, and no map.
    pub const NONE: Self = Self {
        generation: 0,
        samples: [0; SAMPLES],
        len: 0,
        end: End::Uncertain,
        confidence: 0,
        climbs: [None; CLIMBS],
        riding: false,
        map: false,
    };
}

impl Forecast {
    /// Drop what the map said: no pack any more, or one that failed.
    pub fn forget(&mut self) {
        // The generation stays: the engine's next result is still new.
        self.len = 0;
        self.end = End::Uncertain;
        self.confidence = 0;
        self.climbs = [None; CLIMBS];
        self.riding = false;
    }

    /// Take a new look-ahead result; false when it is the one already held.
    pub fn update(&mut self, ahead: &Ahead) -> bool {
        if ahead.generation == self.generation {
            return false;
        }
        // Riding a climb the map had already seen: its last metres may be too
        // little gain for the detector to start a climb from here.
        let was_on = self.riding || self.current().is_some();
        self.generation = ahead.generation;
        self.len = if ahead.on_road() { ahead.len } else { 0 };
        self.samples = ahead.samples;
        self.end = ahead.end;
        self.confidence = ahead.confidence;
        self.climbs = [None; CLIMBS];
        let mut climbs = Climbs::default();
        let mut found = 0;
        for (index, sample) in self.samples[..self.len].iter().enumerate() {
            let step = if index == 0 { 0 } else { STEP_MM };
            if let Some(Event::Finished(summary)) = climbs.add(0, step, Some(i32::from(*sample)))
                && found < CLIMBS
            {
                self.climbs[found] = Some(ClimbAhead {
                    summary,
                    open: false,
                });
                found += 1;
            }
        }
        if let Some(summary) = climbs.climbing()
            && found < CLIMBS
        {
            self.climbs[found] = Some(ClimbAhead {
                summary,
                open: true,
            });
        }
        let starts_here = self.climbs().any(|c| c.summary.start_mm <= ON_CLIMB_MM);
        if was_on
            && !starts_here
            && let Some(rest) = self.remainder()
        {
            // The climb under way comes first; a later one may drop off.
            self.climbs.copy_within(..CLIMBS - 1, 1);
            self.climbs[0] = Some(rest);
        }
        // An unusable forecast (a fork close ahead) keeps what the last
        // usable one said.
        if self.usable() {
            self.riding = self.current().is_some();
        }
        true
    }

    /// The rest of the climb the rider is on: from here up to the highest
    /// point before the road flattens below the end grade (`END_TENTHS`)
    /// over 200 m or drops `DROP_DM` below it, as the ride's detector ends a
    /// climb; none unless the road still rises at the end grade over the
    /// next 200 m.
    fn remainder(&self) -> Option<ClimbAhead> {
        let samples = &self.samples[..self.len];
        // A 20 m step is 200 dm: tenths of a percent = rise * 5 / steps.
        let flat = |from: usize, to: usize| {
            let rise = i32::from(samples[to]) - i32::from(samples[from]);
            rise * 5 < END_TENTHS * (to - from) as i32
        };
        let window = samples.len().min(FLAT_STEPS + 1).checked_sub(1)?;
        if window == 0 || flat(0, window) {
            return None;
        }
        let mut top = (0usize, i32::from(*samples.first()?));
        for (index, sample) in samples.iter().enumerate() {
            let z = i32::from(*sample);
            if z > top.1 {
                top = (index, z);
            } else if z < top.1 - DROP_DM {
                break;
            }
            if index > window && flat(index - FLAT_STEPS, index) {
                break;
            }
        }
        let gain = top.1 - i32::from(samples[0]);
        (top.0 > 0 && gain > 0).then(|| ClimbAhead {
            summary: Summary {
                start_mm: 0,
                start_altitude_dm: i32::from(samples[0]),
                gain_dm: gain as u32,
                length_mm: top.0 as u64 * STEP_MM,
                time_ms: 0,
            },
            open: top.0 + 1 == samples.len(),
        })
    }

    /// Confident and long enough to show.
    pub fn usable(&self) -> bool {
        self.len >= MIN_SAMPLES && self.confidence >= MIN_CONFIDENCE
    }

    fn climbs(&self) -> impl Iterator<Item = ClimbAhead> + '_ {
        self.climbs.iter().flatten().copied()
    }

    /// The climb the rider is on, by the map.
    pub fn current(&self) -> Option<ClimbAhead> {
        self.usable()
            .then(|| self.climbs().find(|c| c.summary.start_mm <= ON_CLIMB_MM))
            .flatten()
    }

    /// The next climb that has not started yet.
    pub fn upcoming(&self) -> Option<ClimbAhead> {
        self.usable()
            .then(|| self.climbs().find(|c| c.summary.start_mm > ON_CLIMB_MM))
            .flatten()
    }

    /// Average grade between two distances ahead, tenths of a percent.
    pub fn grade(&self, from_mm: u64, to_mm: u64) -> Option<i32> {
        let step = STEP_MM;
        let from = (from_mm / step) as usize;
        let to = ((to_mm / step) as usize).min(self.len.checked_sub(1)?);
        (to > from).then(|| {
            let rise = i32::from(self.samples[to]) - i32::from(self.samples[from]);
            // A step is 20 m = 200 dm: tenths of a percent = rise * 5 / steps.
            rise * 5 / (to - from) as i32
        })
    }

    /// Steepness of each 20 m step, smoothed like the ride profile: grade
    /// over 100 m each side.
    pub fn steepness(&self, index: usize) -> Steepness {
        const LOCAL: usize = 5;
        let from = index.saturating_sub(LOCAL);
        let to = (index + LOCAL).min(self.len.saturating_sub(1));
        if to <= from {
            return Steepness::Easy;
        }
        let rise = i32::from(self.samples[to]) - i32::from(self.samples[from]);
        Steepness::of(rise * 5 / (to - from) as i32)
    }
}

/// A climb from the map, pinned to the ride's odometer: its foot and top,
/// and the map's elevation of the road from where it was first seen.
#[derive(Clone, Debug, PartialEq)]
pub struct Pinned {
    /// Odometer distance of the first sample: where the climb was pinned.
    origin_mm: u64,
    /// Odometer distance of the foot and of the top.
    pub foot_mm: u64,
    pub top_mm: u64,
    /// The map ended before the top: the climb goes on at least this far.
    pub open: bool,
    /// Map elevation every 20 m from `origin_mm`, dm, relative to the first.
    samples: [i16; PINNED],
    len: usize,
}

impl Pinned {
    /// Pin `climb` from `forecast`, with the rider at `odometer_mm`.
    fn new(forecast: &Forecast, climb: ClimbAhead, odometer_mm: u64) -> Self {
        let mut pinned = Self {
            origin_mm: odometer_mm,
            foot_mm: odometer_mm + climb.summary.start_mm,
            top_mm: odometer_mm + climb.end_mm(),
            open: climb.open,
            samples: [0; PINNED],
            len: 0,
        };
        pinned.take(forecast, 0, 0);
        pinned
    }

    /// The forecast's samples from sample `at` on, `anchor` dm above it.
    fn take(&mut self, forecast: &Forecast, at: usize, anchor: i32) {
        let len = forecast.len.min(PINNED - at);
        let base = i32::from(forecast.samples[0]);
        for (slot, sample) in self.samples[at..at + len].iter_mut().zip(&forecast.samples) {
            let z = anchor + i32::from(*sample) - base;
            *slot = z.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16;
        }
        self.len = at + len;
    }

    /// The same climb seen again from `odometer_mm`: the road ahead and the
    /// top from the new forecast, the road behind as it was.
    fn refresh(&mut self, forecast: &Forecast, climb: ClimbAhead, odometer_mm: u64, on: bool) {
        let at = self.index(odometer_mm);
        if at >= PINNED {
            // Longer than the pin holds: what it has stays.
            self.open = true;
            return;
        }
        if !on {
            self.foot_mm = odometer_mm + climb.summary.start_mm;
        }
        self.top_mm = (odometer_mm + climb.end_mm()).max(self.foot_mm);
        let anchor = i32::from(self.samples[at.min(self.len.saturating_sub(1))]);
        // A stretch the forecast skipped (no fix) is bridged level.
        for index in self.len..at {
            self.samples[index] = anchor as i16;
        }
        self.take(forecast, at, anchor);
        self.open = climb.open || self.index(self.top_mm) >= PINNED;
    }

    fn index(&self, odometer_mm: u64) -> usize {
        (odometer_mm.saturating_sub(self.origin_mm) / STEP_MM) as usize
    }

    /// Odometer distance of the first and the last sample.
    pub fn span_mm(&self) -> (u64, u64) {
        let last = self.len.saturating_sub(1) as u64 * STEP_MM;
        (self.origin_mm, self.origin_mm + last)
    }

    /// Map elevation at `odometer_mm`, dm relative to the first sample,
    /// between samples in a straight line.
    pub fn altitude_dm(&self, odometer_mm: u64) -> i32 {
        let last = self.len.saturating_sub(1);
        let index = self.index(odometer_mm);
        if index >= last {
            return i32::from(self.samples[last]);
        }
        let (a, b) = (
            i32::from(self.samples[index]),
            i32::from(self.samples[index + 1]),
        );
        let into = (odometer_mm.saturating_sub(self.origin_mm) % STEP_MM) as i32;
        a + (b - a) * into / STEP_MM as i32
    }

    /// Steepness at `odometer_mm`: the grade over 100 m each side, as the
    /// ride's profile has it.
    pub fn steepness(&self, odometer_mm: u64) -> Steepness {
        const LOCAL: usize = 5;
        let index = self.index(odometer_mm).min(self.len.saturating_sub(1));
        let from = index.saturating_sub(LOCAL);
        let to = (index + LOCAL).min(self.len.saturating_sub(1));
        if to <= from {
            return Steepness::Easy;
        }
        let rise = i32::from(self.samples[to]) - i32::from(self.samples[from]);
        Steepness::of(rise * 5 / (to - from) as i32)
    }

    /// The whole climb, foot to top; `start_mm` is the foot's odometer.
    pub fn summary(&self) -> Summary {
        let gain = self.altitude_dm(self.top_mm) - self.altitude_dm(self.foot_mm);
        Summary {
            start_mm: self.foot_mm,
            start_altitude_dm: self.altitude_dm(self.foot_mm),
            gain_dm: gain.max(0) as u32,
            length_mm: self.top_mm - self.foot_mm,
            time_ms: 0,
        }
    }

    /// The share of the climb behind the rider at `odometer_mm`, 0..=1000.
    pub fn done_permille(&self, odometer_mm: u64) -> u32 {
        let length = self.top_mm.saturating_sub(self.foot_mm).max(1);
        let done = odometer_mm.saturating_sub(self.foot_mm).min(length);
        (done * 1000 / length) as u32
    }
}

/// Keep `pinned` on the map's climb as the rider at `odometer_mm` rides, each
/// pass: let it go `PAST_TOP_MM` after the top. With a `fresh` forecast, also
/// pin the climb under way or within `ANNOUNCE_MM`, follow its top as the
/// map sees further, and let it go when the map shows the rider left it. A
/// forecast already followed is not looked at again: it describes the road
/// from where the rider was.
pub fn follow(pinned: &mut Option<Pinned>, fresh: Option<&Forecast>, odometer_mm: u64) {
    if pinned
        .as_ref()
        .is_some_and(|p| odometer_mm > p.top_mm + PAST_TOP_MM)
    {
        *pinned = None;
    }
    // An unsure forecast (a fork close ahead) changes nothing.
    let Some(forecast) = fresh.filter(|forecast| forecast.usable()) else {
        return;
    };
    let current = forecast.current();
    // A pinned climb ahead keeps its pin a little past the announce distance,
    // so a foot estimate wobbling at 500 m does not pin and unpin it.
    let reach = ANNOUNCE_MM + if pinned.is_some() { SAME_FOOT_MM } else { 0 };
    let near = current.or(forecast.upcoming().filter(|c| c.summary.start_mm <= reach));
    match (pinned.as_mut(), near) {
        (None, Some(climb)) => *pinned = Some(Pinned::new(forecast, climb, odometer_mm)),
        (Some(pin), Some(climb)) => {
            let on = current.is_some();
            let same = if on {
                odometer_mm + SAME_FOOT_MM >= pin.foot_mm && odometer_mm <= pin.top_mm + PAST_TOP_MM
            } else {
                (odometer_mm + climb.summary.start_mm).abs_diff(pin.foot_mm) <= SAME_FOOT_MM
            };
            if same {
                pin.refresh(forecast, climb, odometer_mm, on);
            } else if odometer_mm > pin.top_mm || odometer_mm < pin.foot_mm {
                // Over the top the next climb is a climb of its own; before
                // the foot, the road now leads to another one.
                *pinned = Some(Pinned::new(forecast, climb, odometer_mm));
            }
        }
        (Some(pin), None) => {
            // No climb here by the map: the rider turned off before the foot
            // or partway up. Near the top the map no longer sees the rest.
            if odometer_mm < pin.foot_mm || odometer_mm + LAST_MM < pin.top_mm {
                *pinned = None;
            }
        }
        (None, None) => {}
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ride::climb::Category;

    /// A look-ahead result with elevations from `road(metres)` in metres.
    pub(crate) fn ahead(generation: u32, km: usize, road: impl Fn(f64) -> f64) -> Ahead {
        let mut ahead = Ahead {
            generation,
            confidence: 90,
            end: End::Horizon,
            ..Ahead::default()
        };
        ahead.len = km * 50 + 1;
        for index in 0..ahead.len {
            ahead.samples[index] = (road(index as f64 * 20.0) * 10.0).round() as i16;
        }
        ahead.matched = Some(terrain::Matched {
            tile: terrain::TileId {
                key: terrain::TileKey { row: 0, col: 0 },
                crc: 0,
                edges: 1,
            },
            edge: 0,
            segment: 0,
            fraction: 0.0,
            forward: Some(true),
            offset_m: 0.0,
            z_dm: 0,
            class: 3,
            flags: 0,
            confidence: 90,
        });
        ahead
    }

    #[test]
    fn a_climb_ahead_is_measured_before_the_rider_gets_there() {
        // 2 km flat, then 3 km at 7% (210 m), then flat.
        let road = |m: f64| (m - 2000.0).clamp(0.0, 3000.0) * 0.07;
        let mut forecast = Forecast::default();
        assert!(forecast.update(&ahead(1, 10, road)));
        assert!(!forecast.update(&ahead(1, 10, road)), "same generation");
        assert_eq!(forecast.current(), None);
        let climb = forecast.upcoming().unwrap();
        assert!(!climb.open);
        let s = climb.summary;
        assert!((1_960_000..=2_040_000).contains(&s.start_mm), "{s:?}");
        assert!((2_960_000..=3_040_000).contains(&s.length_mm), "{s:?}");
        assert!((2_080..=2_100).contains(&s.gain_dm), "{s:?}");
        assert!((69..=71).contains(&s.grade_tenths().unwrap()), "{s:?}");
        assert_eq!(s.category(), Category::Three);
        assert_eq!(forecast.grade(2_000_000, 2_500_000), Some(70));
        assert_eq!(forecast.steepness(150), Steepness::Steep);
    }

    #[test]
    fn on_a_climb_the_rest_of_it_is_the_current_climb() {
        // Already climbing at 9% for 1.5 km more, then a descent.
        let road = |m: f64| {
            if m < 1500.0 {
                m * 0.09
            } else {
                135.0 - (m - 1500.0) * 0.05
            }
        };
        let mut forecast = Forecast::default();
        forecast.update(&ahead(2, 5, road));
        let climb = forecast.current().unwrap();
        assert_eq!(climb.summary.start_mm, 0);
        assert!(
            (1_340..=1_360).contains(&climb.summary.gain_dm),
            "{climb:?}"
        );
        assert_eq!(forecast.upcoming(), None);
    }

    #[test]
    fn a_climb_past_the_horizon_is_open() {
        let road = |m: f64| (m - 8000.0).max(0.0) * 0.1;
        let mut forecast = Forecast::default();
        forecast.update(&ahead(3, 10, road));
        let climb = forecast.upcoming().unwrap();
        assert!(climb.open);
        assert!(climb.summary.gain_dm >= 1_900, "{climb:?}");
    }

    #[test]
    fn an_unsure_or_short_forecast_is_not_shown() {
        let road = |m: f64| (m - 1000.0).max(0.0) * 0.08;
        let mut forecast = Forecast::default();
        let mut unsure = ahead(4, 10, road);
        unsure.confidence = 30;
        forecast.update(&unsure);
        assert!(!forecast.usable());
        assert_eq!(forecast.upcoming(), None);
        let mut short = ahead(5, 10, road);
        short.len = 10;
        forecast.update(&short);
        assert!(!forecast.usable());
    }

    #[test]
    fn the_last_metres_of_a_climb_stay_the_climb_under_way() {
        let mut forecast = Forecast::default();
        // On a 4% climb with 600 m to go: the detector sees it from here.
        let road = |m: f64| {
            if m < 600.0 {
                m * 0.04
            } else {
                24.0 - (m - 600.0) * 0.02
            }
        };
        forecast.update(&ahead(1, 5, road));
        assert!(forecast.current().is_some());
        // 400 m on: 8 m left, too little for the detector to start a climb,
        // yet it is the same climb.
        let road = |m: f64| {
            if m < 200.0 {
                m * 0.04
            } else {
                8.0 - (m - 200.0) * 0.02
            }
        };
        forecast.update(&ahead(2, 5, road));
        let rest = forecast.current().expect("the climb under way");
        assert_eq!(rest.summary.length_mm, 200_000);
        assert_eq!(rest.summary.gain_dm, 80);
        // Over the top the road drops: no climb under way any more.
        let road = |m: f64| -m * 0.02;
        forecast.update(&ahead(3, 5, road));
        assert!(forecast.current().is_none());
        forecast.update(&ahead(4, 5, |m: f64| m.min(100.0) * 0.04));
        assert!(
            forecast.current().is_none(),
            "a new ramp is not the old climb"
        );
    }

    #[test]
    fn the_climb_under_way_ends_where_the_road_flattens() {
        let mut forecast = Forecast::default();
        forecast.update(&ahead(1, 5, |m: f64| m.min(600.0) * 0.04));
        assert!(forecast.current().is_some());
        // 200 m of 4% left, 1 km level, then 1 km of 8%: two climbs.
        let road = |m: f64| {
            if m < 200.0 {
                m * 0.04
            } else if m < 1200.0 {
                8.0
            } else {
                8.0 + (m.min(2200.0) - 1200.0) * 0.08
            }
        };
        forecast.update(&ahead(2, 5, road));
        let rest = forecast.current().expect("the climb under way");
        assert_eq!(rest.summary.length_mm, 200_000);
        assert_eq!(rest.summary.gain_dm, 80);
        let next = forecast.upcoming().expect("the 8% climb");
        assert!(next.summary.start_mm >= 1_100_000);
    }

    /// Follow a new forecast of `road(metres)` with the rider at `at_m`.
    fn seen(pinned: &mut Option<Pinned>, generation: u32, at_m: u64, road: impl Fn(f64) -> f64) {
        let mut forecast = Forecast::default();
        forecast.update(&ahead(generation, 5, road));
        follow(pinned, Some(&forecast), at_m * 1000);
    }

    #[test]
    fn a_climb_is_pinned_once_it_is_within_500_m() {
        let mut pinned = None;
        seen(&mut pinned, 1, 0, |m| (m - 900.0).clamp(0.0, 1000.0) * 0.08);
        assert!(pinned.is_none(), "900 m away");
        seen(&mut pinned, 2, 500, |m| {
            (m - 400.0).clamp(0.0, 1000.0) * 0.08
        });
        let pin = pinned.as_ref().expect("400 m away");
        assert!((880_000..=920_000).contains(&pin.foot_mm), "{pin:?}");
        assert!((1_880_000..=1_920_000).contains(&pin.top_mm), "{pin:?}");
        let climb = pin.summary();
        assert!((780..=820).contains(&climb.gain_dm), "{climb:?}");
        assert_eq!(climb.category(), Category::Four);
        assert_eq!(pin.done_permille(500_000), 0);
    }

    #[test]
    fn the_foot_stays_while_the_map_moves_the_top() {
        let mut pinned = None;
        seen(&mut pinned, 1, 0, |m| (m - 200.0).clamp(0.0, 1000.0) * 0.08);
        let foot = pinned.as_ref().unwrap().foot_mm;
        // 500 m up, the map now sees the climb go on 1 km more than it did.
        seen(&mut pinned, 2, 700, |m| m.min(1500.0) * 0.08);
        let pin = pinned.as_ref().unwrap();
        assert_eq!(pin.foot_mm, foot, "the foot is behind the rider");
        assert!((2_180_000..=2_220_000).contains(&pin.top_mm), "{pin:?}");
        // What was ridden keeps the first forecast's road: 40 m up so far.
        let up = pin.altitude_dm(700_000) - pin.altitude_dm(pin.foot_mm);
        assert!((390..=410).contains(&up), "{up}");
        assert!((240..=260).contains(&pin.done_permille(700_000)));
    }

    #[test]
    fn a_pin_goes_past_the_top_or_when_the_rider_turns_off() {
        let climb = |m: f64| (m - 200.0).clamp(0.0, 1000.0) * 0.08;
        let mut pinned = None;
        seen(&mut pinned, 1, 0, climb);
        // Without a new forecast it stays to 100 m past the top.
        follow(&mut pinned, None, 1_250_000);
        assert!(pinned.is_some());
        follow(&mut pinned, None, 1_310_000);
        assert!(pinned.is_none());
        // Partway up, the map shows a level road: the rider turned off.
        seen(&mut pinned, 2, 0, climb);
        seen(&mut pinned, 3, 500, |_| 0.0);
        assert!(pinned.is_none());
        // In the last 200 m the map no longer sees the climb: it stays.
        seen(&mut pinned, 4, 0, climb);
        seen(&mut pinned, 5, 1100, |m| -m * 0.02);
        assert!(pinned.is_some());
        // An unsure forecast changes nothing.
        let mut unsure = ahead(6, 5, |_| 0.0);
        unsure.confidence = 10;
        let mut forecast = Forecast::default();
        forecast.update(&unsure);
        follow(&mut pinned, Some(&forecast), 1_150_000);
        assert!(pinned.is_some());
    }

    #[test]
    fn over_the_top_the_next_climb_is_pinned_afresh() {
        let mut pinned = None;
        seen(&mut pinned, 1, 0, |m| m.min(1000.0) * 0.08);
        let first = pinned.as_ref().unwrap().foot_mm;
        // 50 m past the top, the next climb starts 300 m on.
        seen(&mut pinned, 2, 1050, |m| {
            (m - 300.0).clamp(0.0, 800.0) * 0.06
        });
        let pin = pinned.as_ref().unwrap();
        assert_ne!(pin.foot_mm, first);
        assert!((1_330_000..=1_370_000).contains(&pin.foot_mm), "{pin:?}");
    }
}
