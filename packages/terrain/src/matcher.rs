//! Which road the rider is on: GPS fix and heading to an edge, with
//! hysteresis so GPS noise next to a parallel road or at a junction does
//! not flip the match.
//!
//! Every edge within `RADIUS_M` scores
//! `(distance / 15 m)² + (heading error / 35°)²`, less a bonus for the edge
//! already matched (1) or one joined to it (0.5). The current match stays
//! until another edge beats it by `SWITCH` on two fixes in a row, or until it
//! is out of reach; an edge joined to it that scores better takes over at
//! once, since that is the road going on past a junction. Three fixes with
//! no road in reach end the match.
use crate::cache::{Cache, Lookup};
use crate::geo::{self, Point};
use crate::pack::{Header, TileKey};
use crate::tile::{Tile, TileId, flag};

pub const RADIUS_M: f32 = 40.0;
const DISTANCE_M: f32 = 15.0;
const HEADING_DEG: f32 = 35.0;
/// A wrong-way oneway still matches (GPS heading can be wrong), but costs this.
const WRONG_WAY: f32 = 4.0;
const SAME_BONUS: f32 = 1.0;
const JOINED_BONUS: f32 = 0.5;
const SWITCH: f32 = 1.0;
/// Past the end of the matched edge, a joined edge takes over only if it
/// costs at most this much more: about 49 degrees of heading error.
const PAST_NODE: f32 = 2.0;
/// A held direction reverses only past this heading error.
const REVERSE_DEG: f32 = 135.0;
/// Heading error counted at most this much while a direction is held.
const HELD_DEG: f32 = 60.0;
const SWITCH_FIXES: u8 = 2;
const LOST_FIXES: u8 = 3;
const KEEP: usize = 8;

/// The rider's place on the road graph.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Matched {
    pub tile: TileId,
    pub edge: u16,
    /// The point index the projection lies after (segment `segment` to
    /// `segment + 1`) and how far along that segment, 0..1.
    pub segment: u16,
    pub fraction: f32,
    /// Travelling from the edge's `from` node towards `to`; `None` until a
    /// heading tells.
    pub forward: Option<bool>,
    /// Distance from the fix to the road.
    pub offset_m: f32,
    /// Elevation of the projected point, decimetres.
    pub z_dm: i32,
    pub class: u8,
    pub flags: u8,
    /// 0..100.
    pub confidence: u8,
}

/// The closest segment of one edge: its points in metres from the fix.
#[derive(Clone, Copy, Debug)]
struct Nearest {
    segment: u16,
    fraction: f32,
    distance: f32,
    a: (f32, f32),
    b: (f32, f32),
}

#[derive(Clone, Copy, Debug)]
struct Candidate {
    found: Matched,
    cost: f32,
    /// Shares a node with the current match: the road on past a junction.
    joined: bool,
    /// Its nearest point is one of its end nodes: the rider is near where it
    /// starts, not beside it.
    at_node: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Matcher {
    current: Option<Matched>,
    challenger: Option<(TileId, u16, u8)>,
    misses: u8,
}

/// The tiles a search around `fix` needs: its own, and each neighbour
/// within `RADIUS_M` of the shared boundary.
pub fn nearby(header: &Header, fix: Point, out: &mut [Option<TileKey>; 4]) {
    *out = [None; 4];
    let Some(key) = header.key(fix.lat, fix.lon) else {
        return;
    };
    let (south, west) = header.origin(key);
    let tile = header.tile as i32;
    let reach_lat = (RADIUS_M / (geo::LAT_UM * 1e-6)) as i32;
    // Longitude degrees shrink towards the poles; 60° S is the worst case.
    let reach_lon = reach_lat * 2;
    let d_row = if fix.lat - south < reach_lat {
        -1
    } else if south + tile - fix.lat < reach_lat {
        1
    } else {
        0
    };
    let d_col = if fix.lon - west < reach_lon {
        -1
    } else if west + tile - fix.lon < reach_lon {
        1
    } else {
        0
    };
    let mut n = 0;
    for (dr, dc) in [(0, 0), (d_row, 0), (0, d_col), (d_row, d_col)] {
        if let Some(k) = header.checked(i64::from(key.row) + dr, i64::from(key.col) + dc)
            && !out.contains(&Some(k))
        {
            out[n] = Some(k);
            n += 1;
        }
    }
}

impl Matcher {
    pub fn current(&self) -> Option<Matched> {
        self.current
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Match one fix. `Err(key)` asks for a tile first; nothing changes.
    pub fn update(
        &mut self,
        cache: &Cache<'_>,
        header: &Header,
        fix: Point,
        heading: Option<f32>,
    ) -> Result<Option<Matched>, TileKey> {
        let mut tiles = [None; 4];
        nearby(header, fix, &mut tiles);
        let mut best: [Option<Candidate>; KEEP] = [None; KEEP];
        for key in tiles.into_iter().flatten() {
            match cache.get(key) {
                Lookup::Absent => return Err(key),
                Lookup::Missing => {}
                Lookup::Ready(tile) => self.search(header, &tile, fix, heading, &mut best),
            }
        }
        let best_one = best
            .iter()
            .flatten()
            .min_by(|a, b| a.cost.total_cmp(&b.cost))
            .copied();
        let Some(top) = best_one else {
            self.misses = self.misses.saturating_add(1);
            self.challenger = None;
            if self.misses >= LOST_FIXES {
                self.current = None;
            }
            return Ok(self.current);
        };
        self.misses = 0;
        let same = |c: &Candidate, m: &Matched| c.found.tile == m.tile && c.found.edge == m.edge;
        let mut chosen = top;
        let previous = self.current;
        if let Some(current) = self.current
            && let Some(kept) = best.iter().flatten().find(|c| same(c, &current)).copied()
        {
            if top.joined && !same(&top, &current) {
                // Past the junction at the end of the current edge.
                self.challenger = None;
            } else if same(&top, &current) || top.cost + SWITCH >= kept.cost {
                self.challenger = None;
                chosen = kept;
            } else {
                let count = match self.challenger {
                    Some((tile, edge, count))
                        if tile == top.found.tile && edge == top.found.edge =>
                    {
                        count + 1
                    }
                    _ => 1,
                };
                if count < SWITCH_FIXES {
                    self.challenger = Some((top.found.tile, top.found.edge, count));
                    chosen = kept;
                } else {
                    self.challenger = None;
                }
            }
        }
        // Past the end of the matched edge: the road goes on along an edge
        // joined to it, if the rider is beside one and riding along it (a
        // bend, not a side arm the heading crosses).
        if chosen.at_node
            && let Some(on) = best
                .iter()
                .flatten()
                .filter(|c| c.joined && !c.at_node && c.cost <= chosen.cost + PAST_NODE)
                .min_by(|a, b| a.cost.total_cmp(&b.cost))
        {
            self.challenger = None;
            chosen = *on;
        }
        // Confidence: near the road, and clearly better than any other road
        // beside the rider. Roads that only start at a node near the rider
        // (the edge just left, a junction's other arms) say nothing about
        // which road the rider is on.
        let left = previous.filter(|p| {
            chosen.joined && !(p.tile == chosen.found.tile && p.edge == chosen.found.edge)
        });
        let runner_up = best
            .iter()
            .flatten()
            .filter(|c| !same(c, &chosen.found) && !c.at_node && left.is_none_or(|p| !same(c, &p)))
            .map(|c| c.cost)
            .fold(f32::INFINITY, f32::min);
        let near = (1.0 - chosen.found.offset_m / RADIUS_M).clamp(0.0, 1.0);
        let clear = ((runner_up - chosen.cost) / 2.0).clamp(0.3, 1.0);
        let steady = if heading.is_some() { 1.0 } else { 0.7 };
        let mut found = chosen.found;
        if found.forward.is_none()
            && let Some(previous) = self
                .current
                .filter(|m| m.tile == found.tile && m.edge == found.edge)
        {
            found.forward = previous.forward;
        }
        found.confidence = (near * clear * steady * 100.0) as u8;
        self.current = Some(found);
        Ok(self.current)
    }

    fn search(
        &self,
        header: &Header,
        tile: &Tile<'_>,
        fix: Point,
        heading: Option<f32>,
        best: &mut [Option<Candidate>; KEEP],
    ) {
        let id = tile.id();
        let lon_um = tile.lon_um;
        let here = (0.0, 0.0);
        for number in 0..tile.edge_count() as u16 {
            let edge = tile.edge(number);
            let points = Tile::points(&edge);
            let mut nearest: Option<Nearest> = None;
            let mut am = geo::edge_point(header, tile, &edge, 0)
                .0
                .metres_from(fix, lon_um);
            for index in 1..points {
                let b = geo::edge_point(header, tile, &edge, index);
                let bm = b.0.metres_from(fix, lon_um);
                // Cheap reject: both ends far on one side.
                if !(am.0 > RADIUS_M && bm.0 > RADIUS_M
                    || am.0 < -RADIUS_M && bm.0 < -RADIUS_M
                    || am.1 > RADIUS_M && bm.1 > RADIUS_M
                    || am.1 < -RADIUS_M && bm.1 < -RADIUS_M)
                {
                    let (t, d) = geo::project(here, am, bm);
                    if d <= RADIUS_M && nearest.is_none_or(|n| d < n.distance) {
                        nearest = Some(Nearest {
                            segment: index as u16 - 1,
                            fraction: t,
                            distance: d,
                            a: am,
                            b: bm,
                        });
                    }
                }
                am = bm;
            }
            let Some(Nearest {
                segment,
                fraction,
                distance,
                a: sa,
                b: sb,
            }) = nearest
            else {
                continue;
            };
            let at_node = (segment == 0 && fraction <= 0.0)
                || (usize::from(segment) + 2 == points && fraction >= 1.0);
            let along = geo::bearing(sb.0 - sa.0, sb.1 - sa.1);
            let mut cost = (distance / DISTANCE_M) * (distance / DISTANCE_M);
            let mut forward = None;
            if let Some(heading) = heading {
                let ahead = libm::fabsf(geo::turn(heading, along));
                let back = 180.0 - ahead;
                let oneway = edge.has(flag::ONEWAY);
                // On the edge already matched, the direction holds unless the
                // heading clearly reverses: near a hairpin a heading taken
                // over the last 15 m cuts the corner by up to 90 degrees.
                let held = self
                    .current
                    .filter(|m| m.tile == id && m.edge == number)
                    .and_then(|m| m.forward);
                let keep_forward = match held {
                    Some(true) => ahead <= REVERSE_DEG,
                    Some(false) => back > REVERSE_DEG,
                    None => ahead <= back,
                };
                let (error, fwd, penalty) = if keep_forward || oneway {
                    (
                        ahead,
                        true,
                        if oneway && ahead > 90.0 {
                            WRONG_WAY
                        } else {
                            0.0
                        },
                    )
                } else {
                    (back, false, 0.0)
                };
                let error = if held.is_some() {
                    error.min(HELD_DEG)
                } else {
                    error
                };
                cost += (error / HEADING_DEG) * (error / HEADING_DEG) + penalty;
                forward = Some(fwd);
            }
            let mut joined = false;
            if let Some(current) = self.current {
                if current.tile == id && current.edge == number {
                    cost -= SAME_BONUS;
                } else if current.tile == id {
                    let was = tile.edge(current.edge);
                    if [was.from, was.to]
                        .iter()
                        .any(|n| *n == edge.from || *n == edge.to)
                    {
                        cost -= JOINED_BONUS;
                        joined = true;
                    }
                }
            }
            let za = geo::z_dm(tile.point(&edge, usize::from(segment)));
            let zb = geo::z_dm(tile.point(&edge, usize::from(segment) + 1));
            let found = Matched {
                tile: id,
                edge: number,
                segment,
                fraction,
                forward,
                offset_m: distance,
                z_dm: za + ((zb - za) as f32 * fraction) as i32,
                class: edge.class,
                flags: edge.flags,
                confidence: 0,
            };
            insert(
                best,
                Candidate {
                    found,
                    cost,
                    joined,
                    at_node,
                },
            );
        }
    }
}

fn insert(best: &mut [Option<Candidate>; KEEP], candidate: Candidate) {
    let slot = best.iter().position(Option::is_none).or_else(|| {
        let (worst, value) = best
            .iter()
            .enumerate()
            .map(|(i, c)| (i, c.map_or(f32::INFINITY, |c| c.cost)))
            .max_by(|a, b| a.1.total_cmp(&b.1))?;
        (value > candidate.cost).then_some(worst)
    });
    if let Some(slot) = slot {
        best[slot] = Some(candidate);
    }
}
