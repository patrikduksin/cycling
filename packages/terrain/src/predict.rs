//! The most likely road ahead, and its elevation every `SAMPLE_M`.
//!
//! From the matched point the walk follows the edge in the travel
//! direction. At a junction it scores every way on (lower is better):
//!
//! - the turn: `|angle| / 30°`, measured between 25 m before and 25 m after
//!   the junction, so a kink in the drawing is not a turn;
//! - staying on the same named road (`ref` or name): -2;
//! - the road class: +0.5 per class more minor, -0.2 per class more major;
//! - leaving the road for a slip road +1.5, for a track or path +2, for a
//!   cycleway +1;
//! - on a roundabout, exits are scored by their turn from the heading the
//!   rider entered with, and going on round costs 1.2.
//!
//! Wrong-way oneways and the road just ridden are not candidates. When the
//! best two differ by less than `FORK_MARGIN`, the road ahead is a real
//! choice: the walk stops there and says so. Each decision lowers the
//! confidence a little, more when it was close; the walk stops when the
//! confidence falls below `MIN_CONFIDENCE`.
use crate::cache::{Cache, Lookup, MAX_SPAN};
use crate::geo::{self, Point};
use crate::matcher::Matched;
use crate::pack::{Header, TileKey};
use crate::tile::{Tile, class, flag};

pub const SAMPLE_M: f32 = 20.0;
/// Samples for 10 km, both ends included.
pub const MAX_SAMPLES: usize = 501;
pub const FORK_MARGIN: f32 = 1.0;
const MIN_CONFIDENCE: f32 = 0.3;
const HEADING_SPAN_M: f32 = 25.0;
const MAX_EDGES: usize = 2000;
const VISITED: usize = 128;
const HISTORY: usize = 16;
const MAX_TILES: usize = 96;
/// Any turn sharper than 120 degrees costs the same: a hairpin is a turn.
const MAX_TURN_COST: f32 = 4.0;
const SAME_ROAD: f32 = 3.0;

/// Why the road ahead stops where it does.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum End {
    /// The full horizon was followed.
    #[default]
    Horizon,
    /// Two ways ahead are about as likely: the road splits here.
    Fork,
    /// The road ends.
    DeadEnd,
    /// The road leaves the pack.
    MapEdge,
    /// Too many close decisions, or no travel direction yet.
    Uncertain,
    /// The road ahead touches more tiles than the cache holds.
    Memory,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Prediction {
    /// Samples written, from the rider (sample 0) on.
    pub len: usize,
    pub end: End,
    /// Distance followed.
    pub end_m: f32,
    /// 0..100.
    pub confidence: u8,
}

/// Writes an elevation every `SAMPLE_M` of distance, interpolating
/// between road points.
struct Sampler<'s> {
    out: &'s mut [i16; MAX_SAMPLES],
    len: usize,
    last: (f32, i32),
}

impl Sampler<'_> {
    fn push(&mut self, distance: f32, z: i32) {
        while self.len < MAX_SAMPLES {
            let at = self.len as f32 * SAMPLE_M;
            if at > distance {
                break;
            }
            let (d0, z0) = self.last;
            let span = distance - d0;
            let value = if span > 0.0 {
                z0 as f32 + (z - z0) as f32 * (at - d0) / span
            } else {
                z as f32
            };
            self.out[self.len] = value.clamp(i16::MIN as f32, i16::MAX as f32) as i16;
            self.len += 1;
        }
        self.last = (distance, z);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Way {
    tile: TileKey,
    edge: u16,
    forward: bool,
}

/// What `predict` needs before it can follow the road.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Need {
    /// A tile not cached yet.
    Tile(TileKey),
    /// A new match: the cache now holds other bytes, or no roads, for the
    /// start's tile, as after a pack transfer.
    Match,
}

impl From<TileKey> for Need {
    fn from(key: TileKey) -> Self {
        Self::Tile(key)
    }
}

struct Choice {
    way: Way,
    cost: f32,
}

/// Follow the road from `start` for up to `horizon_m`, writing elevations
/// relative to the rider into `samples`.
pub fn predict(
    cache: &Cache<'_>,
    header: &Header,
    start: &Matched,
    horizon_m: f32,
    samples: &mut [i16; MAX_SAMPLES],
) -> Result<Prediction, Need> {
    let Some(forward) = start.forward else {
        return Ok(Prediction {
            end: End::Uncertain,
            ..Prediction::default()
        });
    };
    let Some(tile) = ready(cache, start.tile.key)?.filter(|tile| tile.id() == start.tile) else {
        return Err(Need::Match);
    };
    let edge = tile.edge(start.edge);
    let z0 = start.z_dm;
    let mut sampler = Sampler {
        out: samples,
        len: 0,
        last: (0.0, 0),
    };
    sampler.push(0.0, 0);
    // The projected start point.
    let a = geo::edge_point(header, &tile, &edge, usize::from(start.segment)).0;
    let b = geo::edge_point(header, &tile, &edge, usize::from(start.segment) + 1).0;
    let mut here = Point {
        lat: a.lat + ((b.lat - a.lat) as f32 * start.fraction) as i32,
        lon: a.lon + ((b.lon - a.lon) as f32 * start.fraction) as i32,
    };
    let mut distance = 0.0f32;
    let mut history = [(here, 0.0f32); HISTORY];
    let mut history_len = 1usize;
    let mut confidence = f32::from(start.confidence) / 100.0;
    let mut way = Way {
        tile: start.tile.key,
        edge: start.edge,
        forward,
    };
    let mut first_index = if forward {
        usize::from(start.segment) + 1
    } else {
        usize::from(start.segment)
    };
    let mut visited = [None; VISITED];
    let mut visited_next = 0;
    let mut tiles = [None::<TileKey>; MAX_TILES];
    // The road ahead may hold all but room for two of the largest tiles,
    // which the matcher's neighbours and the next tile need.
    let page_budget = cache.capacity().saturating_sub(2 * MAX_SPAN).max(1);
    let mut pages = 0;
    let mut roundabout_entry: Option<f32> = None;
    let mut approach = 0.0f32;
    let mut current_class = edge.class;
    let mut current_group = edge.group;
    let mut current_flags = edge.flags;

    let finish = |sampler: &Sampler<'_>, end: End, distance: f32, confidence: f32| Prediction {
        len: sampler.len,
        end,
        end_m: distance,
        confidence: (confidence * 100.0).clamp(0.0, 100.0) as u8,
    };

    for _ in 0..MAX_EDGES {
        let tile = ready(cache, way.tile)?.ok_or(way.tile)?;
        if !tiles.contains(&Some(way.tile)) {
            pages += cache.pages(way.tile);
            let Some(free) = tiles
                .iter()
                .position(Option::is_none)
                .filter(|_| pages <= page_budget)
            else {
                return Ok(finish(&sampler, End::Memory, distance, confidence));
            };
            tiles[free] = Some(way.tile);
        }
        visited[visited_next] = Some((way.tile, way.edge));
        visited_next = (visited_next + 1) % VISITED;
        let edge = tile.edge(way.edge);
        let last = Tile::points(&edge) - 1;
        // Walk this edge's points in the travel direction.
        let mut index = first_index;
        loop {
            let in_range = if way.forward { index <= last } else { true };
            if !in_range {
                break;
            }
            let (point, z) = geo::edge_point(header, &tile, &edge, index);
            let (east, north) = point.metres_from(here, tile.lon_um);
            let step = geo::length(east, north);
            if distance + step >= horizon_m {
                // Stop exactly at the horizon, part way along this segment.
                let t = if step > 0.0 {
                    ((horizon_m - distance) / step).clamp(0.0, 1.0)
                } else {
                    1.0
                };
                let (_, z_before) = sampler.last;
                let z_there = z_before as f32 + ((z - z0) - z_before) as f32 * t;
                sampler.push(horizon_m, z_there as i32);
                return Ok(finish(&sampler, End::Horizon, horizon_m, confidence));
            }
            distance += step;
            here = point;
            sampler.push(distance, z - z0);
            if history_len < HISTORY {
                history_len += 1;
            } else {
                history.copy_within(1.., 0);
            }
            history[history_len - 1] = (point, distance);
            if sampler.len >= MAX_SAMPLES {
                return Ok(finish(&sampler, End::Horizon, distance, confidence));
            }
            if way.forward {
                index += 1;
            } else if index == 0 {
                break;
            } else {
                index -= 1;
            }
        }
        // At the junction: every way on.
        let node = if way.forward { edge.to } else { edge.from };
        let at = tile.node(node);
        let incoming = {
            // The road HEADING_SPAN_M back, interpolated on the segment that
            // spans it (else the oldest point kept), so a long last segment
            // still gives its own direction and a bend further back does not
            // count.
            let points = &history[..history_len];
            let back = points
                .windows(2)
                .rev()
                .find(|pair| distance - pair[0].1 >= HEADING_SPAN_M)
                .map_or(points[0].0, |pair| {
                    let ((p0, d0), (p1, d1)) = (pair[0], pair[1]);
                    let want = distance - HEADING_SPAN_M;
                    let t = if d1 > d0 {
                        ((want - d0) / (d1 - d0)).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    Point {
                        lat: p0.lat + ((p1.lat - p0.lat) as f32 * t) as i32,
                        lon: p0.lon + ((p1.lon - p0.lon) as f32 * t) as i32,
                    }
                });
            let (east, north) = here.metres_from(back, tile.lon_um);
            if geo::length(east, north) > 0.0 {
                geo::bearing(east, north)
            } else {
                approach
            }
        };
        // On a roundabout, exits are judged against the heading the rider
        // came in with, taken where the approach road met the ring.
        if edge.has(flag::ROUNDABOUT) {
            roundabout_entry.get_or_insert(approach);
        } else {
            roundabout_entry = None;
            approach = incoming;
        }
        let mut choices: [Option<Choice>; 12] = Default::default();
        let mut count = 0;
        let mut blocked_edge = false;
        let mut consider =
            |cache_tile: &Tile<'_>, node: u16, choices: &mut [Option<Choice>; 12]| {
                for (number, ends_here) in cache_tile.incident(node) {
                    let candidate = Way {
                        tile: cache_tile.key,
                        edge: number,
                        forward: !ends_here,
                    };
                    if candidate.tile == way.tile && candidate.edge == way.edge {
                        continue;
                    }
                    let next = cache_tile.edge(number);
                    if next.has(flag::ONEWAY) && !candidate.forward {
                        continue;
                    }
                    if visited.contains(&Some((candidate.tile, candidate.edge))) {
                        continue;
                    }
                    let heading = outgoing(header, cache_tile, number, candidate.forward);
                    let reference = roundabout_entry.unwrap_or(incoming);
                    let mut cost = if roundabout_entry.is_some() && next.has(flag::ROUNDABOUT) {
                        1.2
                    } else {
                        (libm::fabsf(geo::turn(reference, heading)) / 30.0).min(MAX_TURN_COST)
                    };
                    if current_group != 0 && next.group == current_group {
                        cost -= SAME_ROAD;
                    }
                    let step = i32::from(next.class) - i32::from(current_class);
                    cost += if step > 0 {
                        0.8 * step as f32
                    } else {
                        0.2 * step as f32
                    };
                    if next.has(flag::LINK) && current_flags & flag::LINK == 0 {
                        cost += 1.5;
                    }
                    let off_road = |c: u8| c == class::TRACK || c == class::PATH;
                    if off_road(next.class) && !off_road(current_class) {
                        cost += 2.0;
                    }
                    if next.class == class::CYCLEWAY && current_class != class::CYCLEWAY {
                        cost += 1.0;
                    }
                    if count < choices.len() {
                        choices[count] = Some(Choice {
                            way: candidate,
                            cost,
                        });
                        count += 1;
                    }
                }
            };
        consider(&tile, node, &mut choices);
        for (d_row, d_col, nx, ny) in boundary_twins(at.x, at.y, header.tile as u16) {
            let Some(key) = header.checked(
                i64::from(way.tile.row) + d_row,
                i64::from(way.tile.col) + d_col,
            ) else {
                blocked_edge = true;
                continue;
            };
            match cache.get(key) {
                Lookup::Absent => return Err(Need::Tile(key)),
                Lookup::Missing => blocked_edge = true,
                Lookup::Ready(other) => {
                    if let Some(twin) = other.node_at(nx, ny) {
                        consider(&other, twin, &mut choices);
                    }
                }
            }
        }
        let mut best: Option<&Choice> = None;
        let mut second = f32::INFINITY;
        for choice in choices.iter().flatten() {
            match best {
                Some(b) if choice.cost >= b.cost => second = second.min(choice.cost),
                _ => {
                    if let Some(b) = best {
                        second = second.min(b.cost);
                    }
                    best = Some(choice);
                }
            }
        }
        let Some(best) = best else {
            let end = if blocked_edge {
                End::MapEdge
            } else {
                End::DeadEnd
            };
            return Ok(finish(&sampler, end, distance, confidence));
        };
        if second.is_finite() {
            let margin = second - best.cost;
            if margin < FORK_MARGIN {
                return Ok(finish(&sampler, End::Fork, distance, confidence));
            }
            confidence *= 0.85 + 0.15 * (margin / 4.0).min(1.0);
            if confidence < MIN_CONFIDENCE {
                return Ok(finish(&sampler, End::Uncertain, distance, confidence));
            }
        }
        way = best.way;
        let next_tile = ready(cache, way.tile)?.ok_or(way.tile)?;
        let next = next_tile.edge(way.edge);
        // A roundabout belongs to no road: the one the rider entered on
        // stays the one to prefer when leaving it.
        if !next.has(flag::ROUNDABOUT) {
            current_class = next.class;
            current_flags = next.flags;
            current_group = next.group;
        }
        let last = Tile::points(&next) - 1;
        first_index = if way.forward { 1 } else { last - 1 };
    }
    Ok(finish(&sampler, End::Uncertain, distance, confidence))
}

fn ready<'c>(cache: &'c Cache<'_>, key: TileKey) -> Result<Option<Tile<'c>>, TileKey> {
    match cache.get(key) {
        Lookup::Ready(tile) => Ok(Some(tile)),
        Lookup::Missing => Ok(None),
        Lookup::Absent => Err(key),
    }
}

/// Bearing leaving a junction along an edge, over its first 25 m.
fn outgoing(header: &Header, tile: &Tile<'_>, number: u16, forward: bool) -> f32 {
    let edge = tile.edge(number);
    let last = Tile::points(&edge) - 1;
    let at = |i: usize| geo::edge_point(header, tile, &edge, if forward { i } else { last - i }).0;
    let origin = at(0);
    let mut far = at(1);
    for i in 1..=last {
        far = at(i);
        let (east, north) = far.metres_from(origin, tile.lon_um);
        if geo::length(east, north) >= HEADING_SPAN_M {
            break;
        }
    }
    let (east, north) = far.metres_from(origin, tile.lon_um);
    geo::bearing(east, north)
}

/// The same boundary point in each tile that shares it.
fn boundary_twins(x: u16, y: u16, size: u16) -> impl Iterator<Item = (i64, i64, u16, u16)> {
    let cols = [(0i64, x), (-1, size), (1, 0)];
    let rows = [(0i64, y), (-1, size), (1, 0)];
    let col_ok = move |i: usize| i == 0 || (i == 1 && x == 0) || (i == 2 && x == size);
    let row_ok = move |i: usize| i == 0 || (i == 1 && y == 0) || (i == 2 && y == size);
    (0..3)
        .flat_map(move |r| (0..3).map(move |c| (r, c)))
        .filter(move |&(r, c)| (r, c) != (0, 0) && row_ok(r) && col_ok(c))
        .map(move |(r, c)| (rows[r].0, cols[c].0, cols[c].1, rows[r].1))
}
