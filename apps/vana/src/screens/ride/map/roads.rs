use super::pose::{CLOSEST, Reach};
use crate::ride::geo::Frame as RideFrame;
use terrain::{PackId, Road, RoadClass, TileKey};

pub const TILES: usize = terrain::WANTED;
pub const POINTS: usize = 8_192;
pub const RUNS: usize = 4_096;
const _: () = assert!(
    RUNS <= POINTS && POINTS <= u16::MAX as usize,
    "roads end at u16 indices"
);
const STRETCH: usize = 32;

const TOWN_PAST_AHEAD_M: f32 = 560.0;
const STREETS_BELOW_AHEAD_M: f32 = 500.0;
const TOWN_CLOSEST: Reach = Reach::new(STREETS_BELOW_AHEAD_M as u16, Detail::Town);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Detail {
    #[default]
    Streets,
    Town,
}

impl Detail {
    pub(super) fn at(self, ahead_m: f32) -> Self {
        match self {
            Self::Streets if ahead_m > TOWN_PAST_AHEAD_M => Self::Town,
            Self::Town if ahead_m < STREETS_BELOW_AHEAD_M => Self::Streets,
            held => held,
        }
    }

    pub fn keeps(self, class: RoadClass) -> bool {
        match self {
            Self::Streets => true,
            Self::Town => !matches!(
                class,
                RoadClass::Service | RoadClass::Track | RoadClass::Path
            ),
        }
    }

    fn half_pixel_m(self) -> f32 {
        let closest = match self {
            Self::Streets => CLOSEST,
            Self::Town => TOWN_CLOSEST,
        };
        0.5 / closest.px_per_m()
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Held {
    key: Option<TileKey>,
    detail: Detail,
    corner: (f32, f32),
    scale: (f32, f32),
    end: u16,
}

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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Source {
    pub pack: PackId,
    pub frame: RideFrame,
}

#[derive(Clone)]
pub struct Roads {
    source: Option<Source>,
    held: [Held; TILES],
    tiles: usize,
    runs: [Run; RUNS],
    points: [(u16, u16); POINTS],
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

pub struct Placed<'a> {
    pub class: RoadClass,
    held: &'a Held,
    points: &'a [(u16, u16)],
}

impl Placed<'_> {
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

    pub fn hold(&mut self, source: Option<Source>) {
        if self.source != source {
            self.source = source;
            self.keep(|_| false);
        }
    }

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

    pub fn extract<'t>(
        &mut self,
        view: &[TileKey],
        key: TileKey,
        detail: Detail,
        corner_e7: (i32, i32),
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
            corner_e7,
            scale,
            tolerance: detail.half_pixel_m(),
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
        let place = frame.place(corner_e7.0, corner_e7.1);
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

struct Room<'a> {
    roads: &'a mut Roads,
    runs: usize,
    points: usize,
    full: bool,
    corner_e7: (i32, i32),
    scale: (f32, f32),
    tolerance: f32,
    open: Option<(RoadClass, usize)>,
    stretch: heapless::Vec<(u16, u16), STRETCH>,
}

impl Room<'_> {
    fn road(&mut self, class: RoadClass, lat_lon_e7: impl Iterator<Item = (i32, i32)>) {
        let corner = self.corner_e7;
        let mut local = lat_lon_e7.map(|(lat, lon)| {
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
