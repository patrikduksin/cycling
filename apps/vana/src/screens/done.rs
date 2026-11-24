//! Ride done: the saved ride as a treat, full screen until a key or a tap.
//! Under "Ride saved" and the day, its picture: the route drawn
//! from the GPS track with a dot at the start; without a track the whole
//! ride's power as bars in zone colours; without power either the distance
//! as one big number. Three numbers under it follow the picture, and the
//! time in each power zone runs along the foot.
//!
//! The picture's route and bars are placed once, when the screen's tree
//! puts the picture's box, and painted a row at a time under the tiles.
use crate::clock::Day;
use crate::compose::data::Metric;
use crate::ride::breadcrumb::Crumb;
use crate::ride::power_trace::{BARS, PowerTrace};
use crate::ride::zones::TrainingProfile;
use crate::screens::canvas::paint_bar;
use crate::screens::chrome::{Rect, Row, paint_disc_over};
use crate::screens::ride::map::raster::{Cover, Stroke};
use crate::screens::scaled::Scaled;
use crate::screens::theme::Tone;
use crate::screens::theme::{LAYOUT, Theme};
use crate::screens::workout::Text;
use core::fmt::Write;

const ROUTE_POINTS: usize = 40;
/// A track that spans less than this, metres, draws no route.
const ROUTE_MIN_M: f32 = 100.0;
/// The route's width, and the start dot's diameter and the ring of
/// background around it, in units.
const STROKE_UNITS: usize = 5;
const DOT_UNITS: usize = 14;
const RING_UNITS: usize = 3;
const SEAM_UNITS: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Picture {
    Route,
    Power,
    #[default]
    Distance,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Kept {
    /// The finish is queued, or its last samples are being written.
    #[default]
    Saving,
    Saved,
    /// On a journal that loses it at power-off until the phone has a copy.
    Unsynced,
    /// Storage failed at the end: what was written before is kept.
    Failed,
}

impl Kept {
    pub const fn title(self) -> &'static str {
        match self {
            Self::Saving => "Saving ride",
            Self::Saved => "Ride saved",
            Self::Unsynced => "Sync to save",
            Self::Failed => "Not all saved",
        }
    }

    pub const TITLES: [&str; 4] = ["Saving ride", "Ride saved", "Sync to save", "Not all saved"];
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Stat {
    Distance,
    Time,
    Climbed,
    Np,
}

impl Stat {
    pub const fn metric(self) -> Metric {
        match self {
            Self::Distance => Metric::Distance,
            Self::Time => Metric::Elapsed,
            Self::Climbed => Metric::Ascent,
            Self::Np => Metric::NormalizedPower,
        }
    }

    pub const fn caption(self) -> &'static str {
        match self {
            Self::Distance => "distance",
            Self::Time => "time",
            Self::Climbed => "climbed",
            Self::Np => "NP",
        }
    }

    pub const fn tone(self) -> Tone {
        match self {
            Self::Np => Tone::Power,
            Self::Distance | Self::Time | Self::Climbed => Tone::Text,
        }
    }

    /// The number for `reading`, in the metric's base unit, and its unit:
    /// `42.7` km, `1:23` h, `412` m, `214` W; `--` without one.
    pub fn write(self, reading: Option<i32>, out: &mut Text<8>) -> &'static str {
        let unit = match self {
            Self::Distance => "km",
            Self::Time => "h",
            Self::Climbed => "m",
            Self::Np => "W",
        };
        let _ = match reading.filter(|&reading| reading >= 0) {
            None => write!(out, "--"),
            Some(metres) if self == Self::Distance && metres < 100_000 => {
                let tenths = (metres + 50) / 100;
                write!(out, "{}.{}", tenths / 10, tenths % 10)
            }
            Some(metres) if self == Self::Distance => write!(out, "{}", (metres + 500) / 1000),
            Some(seconds) if self == Self::Time => {
                write!(out, "{}:{:02}", seconds / 3600, seconds / 60 % 60)
            }
            Some(reading) => write!(out, "{reading}"),
        };
        unit
    }

    /// Texts as wide as the numbers a ride most often writes, and every
    /// unit: a wider number takes a smaller face.
    pub const WIDEST: [&str; 3] = ["9:59", "99.9", "999"];
    pub const UNITS: [&str; 4] = ["km", "h", "m", "W"];
}

impl Picture {
    pub const fn stats(self) -> [Stat; 3] {
        match self {
            Self::Route => [Stat::Distance, Stat::Time, Stat::Climbed],
            Self::Power => [Stat::Distance, Stat::Time, Stat::Np],
            Self::Distance => [Stat::Time, Stat::Climbed, Stat::Np],
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Done {
    /// The day the ride was saved, while the clock knows it.
    day: Option<Day>,
    kept: Kept,
    picture: Picture,
    /// Mean watts of each stretch of the ride, with its zone.
    watts: heapless::Vec<(u16, u8), BARS>,
    placed: Option<Rect>,
    strokes: heapless::Vec<Stroke, { ROUTE_POINTS - 1 }>,
    half: f32,
    /// The start dot and its ring, centre and radii in quarter pixels.
    start: (i32, i32, i32, i32),
    bars: heapless::Vec<(Rect, u8), BARS>,
    radius: usize,
}

impl core::hash::Hash for Done {
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        (self.day, self.kept, self.picture, &self.watts, self.placed).hash(state);
    }
}

impl Done {
    pub fn new(
        day: Option<Day>,
        track: &[Crumb],
        power: &PowerTrace,
        profile: &TrainingProfile,
    ) -> Self {
        let picture = if Span::of(track).is_some_and(|span| span.larger_side() >= ROUTE_MIN_M) {
            Picture::Route
        } else if power.powered() {
            Picture::Power
        } else {
            Picture::Distance
        };
        let watts = match picture {
            Picture::Power => power
                .bars()
                .iter()
                .map(|&watts| (watts, profile.power_zone(watts)))
                .collect(),
            Picture::Route | Picture::Distance => heapless::Vec::new(),
        };
        Self {
            day,
            picture,
            watts,
            ..Self::default()
        }
    }

    pub fn day(&self) -> Option<Day> {
        self.day
    }

    pub fn kept(&self) -> Kept {
        self.kept
    }

    pub fn keep(&mut self, kept: Kept) {
        self.kept = kept;
    }

    pub fn picture(&self) -> Picture {
        self.picture
    }

    pub fn place(&mut self, rect: Rect, track: &[Crumb], at: &Scaled) {
        if self.placed == Some(rect) {
            return;
        }
        self.placed = Some(rect);
        self.strokes.clear();
        self.bars.clear();
        match self.picture() {
            Picture::Route => self.place_route(rect, track, at),
            Picture::Power => self.place_bars(rect, at),
            Picture::Distance => {}
        }
    }

    fn place_route(&mut self, rect: Rect, track: &[Crumb], at: &Scaled) {
        let Some(Span {
            west,
            south,
            east,
            north,
        }) = Span::of(track)
        else {
            return;
        };
        let (dot, ring) = (at.unit(DOT_UNITS), at.unit(RING_UNITS));
        let inset = (dot / 2 + ring) as f32;
        let (width, height) = (
            rect.width() as f32 - 2.0 * inset,
            rect.height() as f32 - 2.0 * inset,
        );
        let (across, up) = ((east - west).max(1.0), (north - south).max(1.0));
        let scale = (width / across).min(height / up);
        let left = rect.left as f32 + inset + (width - across * scale) / 2.0;
        let bottom = rect.bottom as f32 - inset - (height - up * scale) / 2.0;
        let point = |crumb: &Crumb| {
            (
                left + (crumb.east - west) * scale,
                bottom - (crumb.north - south) * scale,
            )
        };
        let step = track.len().div_ceil(ROUTE_POINTS - 1).max(1);
        let mut points = track.iter().step_by(step).chain(track.last()).map(point);
        let Some(first) = points.next() else {
            return;
        };
        let mut from = first;
        for to in points {
            let _ = self.strokes.push(Stroke::new(from, to));
            from = to;
        }
        self.half = at.unit(STROKE_UNITS) as f32 / 2.0;
        let quarter = |pixels: f32| (pixels * 4.0) as i32;
        self.start = (
            quarter(first.0),
            quarter(first.1),
            2 * dot as i32,
            2 * (dot + 2 * ring) as i32,
        );
    }

    fn place_bars(&mut self, rect: Rect, at: &Scaled) {
        let count = self.watts.len();
        let most = self
            .watts
            .iter()
            .map(|&(watts, _)| watts)
            .max()
            .unwrap_or(0);
        if count == 0 || most == 0 {
            return;
        }
        let seam = at.unit(SEAM_UNITS);
        self.radius = at.unit(usize::from(LAYOUT.radius.bar));
        let pitch = (rect.width() + seam) / BARS;
        let height = rect.height();
        for (index, &(watts, zone)) in self.watts.iter().enumerate() {
            let tall = (height * usize::from(watts) / usize::from(most)).max(1);
            let left = rect.left + index * pitch;
            let _ = self.bars.push((
                Rect {
                    left,
                    top: rect.bottom - tall,
                    right: left + pitch - seam,
                    bottom: rect.bottom,
                },
                zone,
            ));
        }
    }

    pub fn paint(&self, row: &mut Row, theme: &Theme) {
        if !self.strokes.is_empty() {
            let mut cover = Cover::new(row.y());
            for stroke in &self.strokes {
                cover.stroke(stroke, self.half);
            }
            cover.paint(row, theme.info);
            let (x, y, dot, ring) = self.start;
            paint_disc_over(row, (x, y, ring), theme.bg);
            paint_disc_over(row, (x, y, dot), theme.good);
        }
        for &(rect, zone) in &self.bars {
            paint_bar(row, rect, self.radius, theme.zone(zone));
        }
    }
}

/// The box around a track, metres east and north of its start.
struct Span {
    west: f32,
    south: f32,
    east: f32,
    north: f32,
}

impl Span {
    fn of(track: &[Crumb]) -> Option<Self> {
        let first = track.first()?;
        let start = Self {
            west: first.east,
            south: first.north,
            east: first.east,
            north: first.north,
        };
        Some(track.iter().fold(start, |span, crumb| Self {
            west: span.west.min(crumb.east),
            south: span.south.min(crumb.north),
            east: span.east.max(crumb.east),
            north: span.north.max(crumb.north),
        }))
    }

    fn larger_side(&self) -> f32 {
        (self.east - self.west).max(self.north - self.south)
    }
}
