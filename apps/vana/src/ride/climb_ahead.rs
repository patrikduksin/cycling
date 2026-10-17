//! Climbs on the road ahead, from the terrain look-ahead (`terrain`).
//!
//! The look-ahead predicts the road and its map elevation every 20 m for up
//! to 10 km. This runs the same detector the ride uses (`climb::Climbs`:
//! 3% over 200 m starts a climb, 20 m of gain confirms it, it ends below 1%
//! or 10 m under its top) over that profile, so a climb ahead is exactly
//! what the ride would detect when riding it, found before the rider gets
//! there. Categories follow `climb::Category`.
//!
//! A forecast is shown only when the look-ahead is confident (`MIN_CONFIDENCE`)
//! and reaches far enough to say something; otherwise the climb page keeps
//! its map-free view.
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
            tile: terrain::TileKey { row: 0, col: 0 },
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
}
