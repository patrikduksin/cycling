//! The roads under the map, copied out of the terrain cache a tile at a
//! time and kept free of the heading, so a fill only projects them.
//!
//! Each pass the app asks [`Roads::lacking`] which tiles of the view are
//! not held at the view's [`Detail`], has the terrain cache load them, and
//! hands one cached tile to [`Roads::extract`]. A tile keeps the roads its
//! detail draws, simplified so no point strays more than half a pixel at
//! that detail's closest step, as tile-local micro-degrees. Its corner is
//! placed in the ride's frame, so a new ride's frame places them again, and
//! a new reading of the pack copies them again.
use super::pose::Step;
use crate::ride::homing::Frame as RideFrame;
use terrain::{PackId, Road, RoadClass, TileKey};

/// Tiles held at once: as many as a map may ask the terrain cache for.
pub const TILES: usize = terrain::WANTED;
/// Points and roads held across those tiles: the densest 3 km view, about
/// 16 tiles of central Santiago's arteries, or 4 tiles of its streets.
pub const POINTS: usize = 8_192;
pub const RUNS: usize = 4_096;
const _: () = assert!(
    RUNS <= POINTS && POINTS <= u16::MAX as usize,
    "roads end at u16 indices"
);
/// The longest stretch of one road simplified at once; a longer road is
/// simplified in pieces that share their ends.
const STRETCH: usize = 32;

/// What a step draws of the road network.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Detail {
    /// Every road, at the 300 m and 400 m steps.
    #[default]
    Streets,
    /// No service roads, tracks or paths, at 1 km.
    Town,
    /// Trunk to tertiary roads, at 3 km.
    Arteries,
}

impl Detail {
    pub fn of(step: Step) -> Self {
        match step {
            Step::M300 | Step::M400 => Self::Streets,
            Step::Km1 => Self::Town,
            Step::Km3 => Self::Arteries,
        }
    }

    pub fn keeps(self, class: RoadClass) -> bool {
        match self {
            Self::Streets => true,
            Self::Town => !matches!(
                class,
                RoadClass::Service | RoadClass::Track | RoadClass::Path
            ),
            Self::Arteries => class <= RoadClass::Tertiary,
        }
    }

    /// Half a pixel at the closest step this detail is drawn at, metres.
    fn tolerance_m(self) -> f32 {
        let closest = match self {
            Self::Streets => Step::M300,
            Self::Town => Step::Km1,
            Self::Arteries => Step::Km3,
        };
        0.5 / closest.px_per_m()
    }
}

/// One held tile: its roads are `runs[previous tile's end..end]`.
#[derive(Clone, Copy, Debug, Default)]
struct Held {
    key: Option<TileKey>,
    detail: Detail,
    /// The tile's south-west corner in the ride's frame, and metres per
    /// micro-degree east and north there.
    corner: (f32, f32),
    scale: (f32, f32),
    end: u16,
}

/// One road: its points are `points[previous run's end..end]`.
#[derive(Clone, Copy, Debug)]
struct Run {
    end: u16,
    class: RoadClass,
}

impl Default for Run {
    fn default() -> Self {
        Self {
            end: 0,
            class: RoadClass::Trunk,
        }
    }
}

/// What held roads were copied from and placed in.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub pack: PackId,
    pub frame: RideFrame,
}

/// Zero bytes are a valid `Roads`: nothing held, from no source.
#[derive(Clone)]
pub struct Roads {
    source: Option<Source>,
    held: [Held; TILES],
    tiles: usize,
    runs: [Run; RUNS],
    /// East and north of the tile's corner, micro-degrees.
    points: [(u16, u16); POINTS],
    /// Changes whenever what is held changes, so the map fills again.
    generation: u32,
}

impl Default for Roads {
    fn default() -> Self {
        Self {
            source: None,
            held: [Held::default(); TILES],
            tiles: 0,
            runs: [Run::default(); RUNS],
            points: [(0, 0); POINTS],
            generation: 0,
        }
    }
}

/// A road of a held tile, in the ride's frame.
pub struct Placed<'a> {
    pub class: RoadClass,
    held: &'a Held,
    points: &'a [(u16, u16)],
}

impl Placed<'_> {
    /// Metres east and north of the ride's start.
    pub fn points(&self) -> impl Iterator<Item = (f32, f32)> + '_ {
        let Held { corner, scale, .. } = *self.held;
        self.points.iter().map(move |&(x, y)| {
            (
                corner.0 + f32::from(x) * scale.0,
                corner.1 + f32::from(y) * scale.1,
            )
        })
    }
}

impl Roads {
    pub fn generation(&self) -> u32 {
        self.generation
    }

    /// Hold roads of `source` from now on, letting go of any of another;
    /// none without one.
    pub fn hold(&mut self, source: Option<Source>) {
        if self.source != source {
            self.source = source;
            self.keep(|_| false);
        }
    }

    /// The tiles of `keys` not held at `detail`, to load.
    pub fn lacking<'k>(
        &'k self,
        keys: &'k [TileKey],
        detail: Detail,
    ) -> impl Iterator<Item = TileKey> + 'k {
        keys.iter()
            .copied()
            .filter(move |&key| !self.holds(key, detail))
    }

    fn holds(&self, key: TileKey, detail: Detail) -> bool {
        self.held[..self.tiles]
            .iter()
            .any(|held| held.key == Some(key) && held.detail == detail)
    }

    /// Hold the roads of tile `key` of the source at `detail`, its corner
    /// `corner` as `(lat_e7, lon_e7)`, in place of any other copy of it.
    /// Tiles not in `view` make room first; when even so the roads do not
    /// fit, the minor ones are left out. Whether every road fit.
    pub fn extract<'t>(
        &mut self,
        view: &[TileKey],
        key: TileKey,
        detail: Detail,
        corner: (i32, i32),
        roads: impl Iterator<Item = Road<'t>> + Clone,
    ) -> bool {
        let Some(Source { frame, .. }) = self.source else {
            return false;
        };
        self.keep(|held| held.key != Some(key) && held.key.is_some_and(|k| view.contains(&k)));
        if self.tiles == TILES {
            return false;
        }
        let (runs, points) = self.ends();
        let scale = frame.metres_per_e7();
        let scale = (scale.0 * 10.0, scale.1 * 10.0);
        let mut room = Room {
            roads: self,
            runs,
            points,
            full: false,
            corner,
            scale,
            tolerance: detail.tolerance_m(),
            open: None,
            stretch: heapless::Vec::new(),
        };
        for major in [true, false] {
            for road in roads.clone() {
                let class = road.class();
                if class.major() == major && detail.keeps(class) {
                    room.road(class, road.points());
                }
            }
        }
        room.close();
        let (full, end) = (room.full, room.runs);
        let place = frame.place(corner.0, corner.1);
        self.held[self.tiles] = Held {
            key: Some(key),
            detail,
            corner: place,
            scale,
            end: end as u16,
        };
        self.tiles += 1;
        self.generation = self.generation.wrapping_add(1);
        !full
    }

    /// Where the next tile's runs and points start.
    fn ends(&self) -> (usize, usize) {
        let runs = self
            .tiles
            .checked_sub(1)
            .map_or(0, |last| usize::from(self.held[last].end));
        let points = runs
            .checked_sub(1)
            .map_or(0, |last| usize::from(self.runs[last].end));
        (runs, points)
    }

    /// Keep only the tiles `keep` says, closing the gaps the others leave.
    fn keep(&mut self, keep: impl Fn(&Held) -> bool) {
        let (mut tiles, mut runs, mut points) = (0, 0, 0);
        let (mut run_from, mut point_from) = (0usize, 0usize);
        for t in 0..self.tiles {
            let held = self.held[t];
            let run_end = usize::from(held.end);
            let point_end = run_end
                .checked_sub(1)
                .filter(|_| run_end > run_from)
                .map_or(point_from, |last| usize::from(self.runs[last].end));
            if keep(&held) {
                let shift = point_from - points;
                self.points.copy_within(point_from..point_end, points);
                for r in run_from..run_end {
                    let mut run = self.runs[r];
                    run.end -= shift as u16;
                    self.runs[runs + r - run_from] = run;
                }
                runs += run_end - run_from;
                points += point_end - point_from;
                self.held[tiles] = Held {
                    end: runs as u16,
                    ..held
                };
                tiles += 1;
            }
            (run_from, point_from) = (run_end, point_end);
        }
        if tiles != self.tiles {
            self.tiles = tiles;
            self.generation = self.generation.wrapping_add(1);
        }
    }

    /// Every held road `detail` draws, nearest tile to `here` first.
    pub fn each(&self, detail: Detail, here: (f32, f32)) -> impl Iterator<Item = Placed<'_>> {
        let mut order: heapless::Vec<(f32, usize), TILES> = heapless::Vec::new();
        for (t, held) in self.held[..self.tiles].iter().enumerate() {
            let (dx, dy) = (held.corner.0 - here.0, held.corner.1 - here.1);
            let _ = order.push((dx * dx + dy * dy, t));
        }
        order.sort_unstable_by(|a, b| a.0.total_cmp(&b.0));
        order.into_iter().flat_map(move |(_, t)| {
            let held = &self.held[t];
            let first = t
                .checked_sub(1)
                .map_or(0, |p| usize::from(self.held[p].end));
            (first..usize::from(held.end)).filter_map(move |r| {
                let run = self.runs[r];
                let start = r
                    .checked_sub(1)
                    .map_or(0, |p| usize::from(self.runs[p].end));
                detail.keeps(run.class).then(|| Placed {
                    class: run.class,
                    held,
                    points: &self.points[start..usize::from(run.end)],
                })
            })
        })
    }
}

/// The free end of the arrays while a tile is extracted, and the road
/// being drawn out of it: roads that carry on where the last of their class
/// ended join it, so a street of many blocks is one polyline.
struct Room<'a> {
    roads: &'a mut Roads,
    runs: usize,
    points: usize,
    full: bool,
    corner: (i32, i32),
    scale: (f32, f32),
    tolerance: f32,
    /// The open road's class, and where its points start.
    open: Option<(RoadClass, usize)>,
    /// Its points since the last one kept, which is the first.
    stretch: heapless::Vec<(u16, u16), STRETCH>,
}

impl Room<'_> {
    /// Add a road of `class` through `points` (absolute, `(lat_e7, lon_e7)`).
    fn road(&mut self, class: RoadClass, points: impl Iterator<Item = (i32, i32)>) {
        let corner = self.corner;
        let mut local = points.map(|(lat, lon)| {
            (
                (lon / 10 - corner.1 / 10).clamp(0, i32::from(u16::MAX)) as u16,
                (lat / 10 - corner.0 / 10).clamp(0, i32::from(u16::MAX)) as u16,
            )
        });
        let Some(first) = local.next() else {
            return;
        };
        let joins =
            self.open.is_some_and(|(open, _)| open == class) && self.stretch.last() == Some(&first);
        if !joins {
            self.close();
            self.open = Some((class, self.points));
            let _ = self.stretch.push(first);
        }
        for point in local {
            if self.stretch.is_full() {
                self.simplify();
            }
            let _ = self.stretch.push(point);
        }
    }

    /// Keep the stretch's points a reader needs, leaving its last as the
    /// next stretch's first: each point past which a straight line from the
    /// last kept would stray more than `tolerance` from one it skipped.
    fn simplify(&mut self) {
        let Some(&last) = self.stretch.last() else {
            return;
        };
        let scale = self.scale;
        let metres = |&(x, y): &(u16, u16)| (f32::from(x) * scale.0, f32::from(y) * scale.1);
        if self.open.is_some_and(|(_, start)| start == self.points) {
            self.push(self.stretch[0]);
        }
        let mut anchor = 0;
        for candidate in 2..self.stretch.len() {
            let (a, b) = (
                metres(&self.stretch[anchor]),
                metres(&self.stretch[candidate]),
            );
            let strays = self.stretch[anchor + 1..candidate]
                .iter()
                .any(|p| strays_from(metres(p), a, b, self.tolerance));
            if strays {
                anchor = candidate - 1;
                self.push(self.stretch[anchor]);
            }
        }
        if self.stretch.len() > 1 {
            self.push(last);
        }
        self.stretch.clear();
        let _ = self.stretch.push(last);
    }

    /// End the open road: hold it if it has a segment and fits.
    fn close(&mut self) {
        self.simplify();
        self.stretch.clear();
        let Some((class, start)) = self.open.take() else {
            return;
        };
        if self.full || self.runs == RUNS || self.points - start < 2 {
            self.full |= self.runs == RUNS;
            self.points = start;
            return;
        }
        self.roads.runs[self.runs] = Run {
            end: self.points as u16,
            class,
        };
        self.runs += 1;
    }

    fn push(&mut self, point: (u16, u16)) {
        if self.points == POINTS {
            self.full = true;
            return;
        }
        self.roads.points[self.points] = point;
        self.points += 1;
    }
}

fn strays_from(p: (f32, f32), a: (f32, f32), b: (f32, f32), tolerance: f32) -> bool {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let length = dx * dx + dy * dy;
    let t = if length > 0.0 {
        (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / length).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let (qx, qy) = (a.0 + t * dx - p.0, a.1 + t * dy - p.1);
    qx * qx + qy * qy > tolerance * tolerance
}

#[cfg(test)]
pub(super) mod tests;
