//! Courses drawn at random, each with up to three spurs, of every shape a
//! spur can take, near a spur's edges included: a fork too narrow to tell
//! from the road, a way back drifting off, a dead end at an out-and-back's
//! far corner, depths either side of `SPUR_MIN_M` and `SPUR_M`. At each
//! spur, the scan and the upload must say what the course says with the
//! spur's path taken out, or what it said before spurs were cut
//! (`uncut`).

use super::super::super::{STEP_M, Turn, distance, swing};
use super::super::{ALONE_M, RAY_M, SPUR_M, SPUR_MIN_M};
use super::{heeded, join, length, path, scanned, uncut};
use crate::ride::course::stored::tests::found_along;
use crate::ride::thin::off_segment;
use crate::testing::Rng;
use std::string::String;
use std::vec::Vec;
use std::{format, vec};

type P = (f32, f32);
/// A course's turns as one path finds them, and metres of it cut as spurs.
type TurnsOf = fn(&[P]) -> (Vec<Turn>, f32);

const EVERY_M: [f32; 6] = [1.0, 3.0, 5.0, 8.0, 15.0, 10_000.0];
const NEAR_M: f32 = 6.0;
/// Where a turn starts, the rider hears it: a few metres either way, as a
/// U-turn's chord swings sooner where its legs lie a metre apart. Where it
/// ends only frees the next, and a cut that steps aside moves it most.
const STARTS_M: f32 = 10.0;
const ENDS_M: f32 = 15.0;
const SHALLOWEST_M: f32 = SPUR_MIN_M - 3.0;
const DEEPEST_M: f32 = SPUR_M + 10.0;
const ROUTE_APART_M: f32 = 30.0;
const DRIFT_M: f32 = 3.5;
/// The scan reads a course at whole steps from its start, so a spur cut
/// shifts the steps the course after it is read at, as the course without
/// its spurs is shifted by what it leaves out. What either says is also what
/// it says read from a start this much further back.
const PHASES_M: [f32; 8] = [0.0, 0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75];
/// A turn on the edge of being told says it or not from one start to one a
/// few centimetres away, so the shift a cut makes is read give or take
/// this.
const NEAR_PHASES_M: [f32; 7] = [-0.15, -0.1, -0.05, 0.0, 0.05, 0.1, 0.15];
const AROUND_M: f32 = 30.0;
const PLANNED_M: core::ops::RangeInclusive<f32> = 20.0..=100.0;
const PLANNED_DEG: f32 = 45.0;
const TURNS_BACK_DEG: f32 = 110.0;

fn uniform(rng: &mut Rng, low: f32, high: f32) -> f32 {
    low + (high - low) * (rng.next() >> 40) as f32 / (1u64 << 24) as f32
}

fn toward(p: P, bearing: f32, m: f32) -> P {
    let a = bearing.to_radians();
    (p.0 + m * libm::sinf(a), p.1 + m * libm::cosf(a))
}

fn bearing(a: P, b: P) -> f32 {
    libm::atan2f(b.0 - a.0, b.1 - a.1).to_degrees()
}

fn every(rng: &mut Rng) -> f32 {
    rng.pick(&EVERY_M)
}

/// A dead end off the course, `depth` deep at `bearing`, whose way back
/// comes back `drift` to its right, straight from the tip or alongside;
/// with a spur of its own on its way out, `inner` of the way along.
#[derive(Clone, Debug)]
struct Spur {
    bearing: f32,
    depth: f32,
    drift: f32,
    alongside: bool,
    out_every: f32,
    back_every: f32,
    inner: Option<(f32, std::boxed::Box<Spur>)>,
}

impl Spur {
    fn tip(&self, base: P) -> P {
        toward(base, self.bearing, self.depth)
    }

    fn returns_to(&self, base: P) -> P {
        toward(base, self.bearing + 90.0, self.drift)
    }

    fn draw(&self, base: P, line: &mut Vec<Vec<P>>) {
        let tip = self.tip(base);
        match &self.inner {
            Some((along, inner)) => {
                let inner_base = toward(base, self.bearing, along * self.depth);
                line.push(path(&[base, inner_base], self.out_every));
                inner.draw(inner_base, line);
                line.push(path(&[inner.returns_to(inner_base), tip], self.out_every));
            }
            None => line.push(path(&[base, tip], self.out_every)),
        }
        let to = self.returns_to(base);
        let back = match self.alongside {
            true => vec![tip, toward(tip, self.bearing + 90.0, self.drift), to],
            false => vec![tip, to],
        };
        line.push(path(&back, self.back_every));
    }

    /// Its centre line from `base`, and those of its own spurs.
    fn lines(&self, base: P, lines: &mut Vec<(P, P)>) {
        lines.push((base, self.tip(base)));
        if let Some((along, inner)) = &self.inner {
            inner.lines(toward(base, self.bearing, along * self.depth), lines);
        }
    }
}

/// Spurs drawn one after another from a point `along_m` along the route.
#[derive(Clone, Debug)]
struct Site {
    along_m: f32,
    spurs: Vec<Spur>,
}

/// A course as drawn, and as it would be without its spurs.
struct Case {
    name: String,
    with: Vec<P>,
    without: Vec<P>,
    cut: Vec<Cut>,
}

/// Where a site's spurs leave the route and come back to it, metres along
/// the course as drawn, and how far apart the two are.
#[derive(Clone, Copy)]
struct Cut {
    from_m: f32,
    rejoin_m: f32,
    apart_m: f32,
    planned: bool,
}

struct Route {
    corners: Vec<P>,
    every: f32,
    sites: Vec<Site>,
    tags: Vec<String>,
}

impl Route {
    fn along(&self) -> Vec<f32> {
        let mut along = vec![0.0];
        for pair in self.corners.windows(2) {
            along.push(along[along.len() - 1] + distance(pair[0], pair[1]));
        }
        along
    }

    fn point(&self, m: f32) -> (P, f32, usize) {
        let along = self.along();
        let i = (1..along.len())
            .find(|&i| along[i] >= m)
            .unwrap_or(along.len() - 1);
        let (a, b) = (self.corners[i - 1], self.corners[i]);
        let t = ((m - along[i - 1]) / (along[i] - along[i - 1])).clamp(0.0, 1.0);
        let p = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
        (p, bearing(a, b), i)
    }

    fn planned(&self, site: &Site) -> bool {
        let [spur] = &site.spurs[..] else {
            return false;
        };
        let along = self.along();
        let total_m = along[self.corners.len() - 1];
        let near = |m: f32| (m - site.along_m).abs() <= ALONE_M;
        let crowded = self
            .sites
            .iter()
            .any(|other| other.along_m != site.along_m && near(other.along_m));
        let turns_back = (1..self.corners.len() - 1).any(|i| {
            let (a, b, c) = (self.corners[i - 1], self.corners[i], self.corners[i + 1]);
            let turned = libm::fabsf(swing(bearing(a, b), bearing(b, c)));
            turned >= TURNS_BACK_DEG && near(along[i])
        });
        if crowded || turns_back {
            return false;
        }
        let (base, _, _) = self.point(site.along_m);
        let ray = |m: f32| {
            let (p, _, _) = self.point(m.clamp(0.0, total_m));
            (distance(base, p) >= 1.0).then(|| bearing(base, p))
        };
        let rays = [
            ray(site.along_m - RAY_M),
            Some(spur.bearing),
            ray(site.along_m + RAY_M),
        ];
        let apart = |a: Option<f32>, b: Option<f32>| {
            a.zip(b)
                .is_none_or(|(a, b)| libm::fabsf(swing(a, b)) >= PLANNED_DEG)
        };
        spur.drift == 0.0
            && spur.inner.is_none()
            && PLANNED_M.contains(&spur.depth)
            && apart(rays[0], rays[1])
            && apart(rays[1], rays[2])
            && apart(rays[2], rays[0])
    }

    fn case(&self) -> Case {
        let along = self.along();
        let mut sites = self.sites.clone();
        sites.sort_by(|a, b| a.along_m.total_cmp(&b.along_m));
        let (mut with, mut without) = (Vec::new(), Vec::new());
        let mut cut = Vec::new();
        let mut chunk = vec![self.corners[0]];
        let mut next = 1;
        for site in &sites {
            while next < self.corners.len() && along[next] < site.along_m {
                chunk.push(self.corners[next]);
                next += 1;
            }
            let (base, _, _) = self.point(site.along_m);
            if chunk[chunk.len() - 1] != base {
                chunk.push(base);
            }
            let route = path(&chunk, self.every);
            with.push(route.clone());
            without.push(route);
            let from_m = length(&join(&with));
            let mut to = base;
            for spur in &site.spurs {
                spur.draw(to, &mut with);
                to = spur.returns_to(to);
            }
            without.push(path(&[base, to], self.every));
            cut.push(Cut {
                from_m,
                rejoin_m: length(&join(&with)),
                apart_m: distance(base, to),
                planned: self.planned(site),
            });
            chunk = vec![to];
            if next < self.corners.len() && along[next] <= site.along_m {
                next += 1;
            }
        }
        chunk.extend_from_slice(&self.corners[next..]);
        with.push(path(&chunk, self.every));
        without.push(path(&chunk, self.every));
        Case {
            name: self.tags.join(", "),
            with: join(&with),
            without: join(&without),
            cut,
        }
    }

    fn tangled(&self) -> bool {
        let mut lines = Vec::new();
        let mut bases = Vec::new();
        for site in &self.sites {
            let (mut base, _, _) = self.point(site.along_m);
            for spur in &site.spurs {
                let first = lines.len();
                spur.lines(base, &mut lines);
                bases.push((base, first, lines.len()));
                base = spur.returns_to(base);
            }
        }
        let segments: Vec<(P, P)> = self
            .corners
            .windows(2)
            .filter(|pair| pair[0] != pair[1])
            .map(|pair| (pair[0], pair[1]))
            .collect();
        let touches = |segment: &(P, P), p: P| off_segment(p, segment.0, segment.1) < 0.5;
        for (i, &(base, tip)) in lines.iter().enumerate() {
            let depth = distance(base, tip);
            let owner = bases
                .iter()
                .find(|(_, first, last)| (*first..*last).contains(&i))
                .map_or((i, i + 1), |&(_, first, last)| (first, last));
            let way = bearing(base, tip);
            for segment in &segments {
                if touches(segment, base) {
                    continue;
                }
                let near = (8..=depth as usize).any(|m| {
                    off_segment(toward(base, way, m as f32), segment.0, segment.1) < NEAR_M
                });
                if near {
                    return true;
                }
            }
            for (j, &(other_base, other_tip)) in lines.iter().enumerate() {
                if (owner.0..owner.1).contains(&j) {
                    continue;
                }
                let near = (8..=depth as usize).any(|m| {
                    off_segment(toward(base, way, m as f32), other_base, other_tip) < NEAR_M
                });
                if near {
                    return true;
                }
            }
        }
        let overlaps = segments.iter().enumerate().any(|(i, a)| {
            segments[i + 1..].iter().any(|b| {
                let retraced = a.0 == b.1 && a.1 == b.0;
                [(a.0, a.1), (a.1, a.0)].iter().any(|&(p, q)| {
                    [(b.0, b.1), (b.1, b.0)].iter().any(|&(r, t)| {
                        distance(p, r) < 0.5
                            && !retraced
                            && libm::cosf((bearing(p, q) - bearing(r, t)).to_radians()) > 0.9
                    })
                })
            })
        });
        overlaps
            || segments.iter().enumerate().any(|(i, a)| {
                segments[i + 1..].iter().skip(1).any(|b| {
                    let meet = [a.0, a.1]
                        .iter()
                        .any(|&p| distance(p, b.0) < 0.5 || distance(p, b.1) < 0.5);
                    !meet
                        && path(&[b.0, b.1], 2.0)
                            .into_iter()
                            .any(|p| off_segment(p, a.0, a.1) < ROUTE_APART_M)
                })
            })
    }
}

fn turn_deg(rng: &mut Rng) -> f32 {
    let size = uniform(rng, 20.0, 160.0);
    match rng.one_in(2) {
        true => size,
        false => -size,
    }
}

/// A route of a few corners, a lead or a long out-and-back along it.
fn route(rng: &mut Rng, tags: &mut Vec<String>) -> (Vec<P>, Option<usize>, Option<(usize, f32)>) {
    let mut corners = vec![(0.0, 0.0)];
    let mut heading = uniform(rng, 0.0, 360.0);
    let segments = rng.range(2, 5);
    let mut lead = None;
    let mut far_end = None;
    for k in 0..segments {
        if k > 0 {
            heading += turn_deg(rng);
        }
        let last = corners[corners.len() - 1];
        let long = uniform(rng, 120.0, 450.0);
        if k > 0 && lead.is_none() && far_end.is_none() && rng.one_in(5) {
            let side = rng.pick(&[90.0, -90.0]);
            let lead_m = uniform(rng, 8.0, 25.0);
            let at = toward(last, heading + side, lead_m);
            corners.push(at);
            lead = Some((corners.len() - 1, heading));
            tags.push(format!("a {lead_m:.0} m lead"));
            corners.push(toward(at, heading, long));
            continue;
        }
        if k > 0 && far_end.is_none() && lead.is_none() && rng.one_in(5) {
            let side = rng.pick(&[90.0, -90.0]);
            let total = uniform(rng, 200.0, 600.0);
            let end_m = uniform(rng, 15.0, 80.0);
            let corner = toward(last, heading + side, total - end_m);
            let end = toward(corner, heading + side + rng.pick(&[90.0, -90.0]), end_m);
            corners.extend([corner, end, corner, last]);
            far_end = Some((corners.len() - 3, bearing(corner, end)));
            tags.push(format!(
                "a {total:.0} m out-and-back, its last {end_m:.1} m"
            ));
        }
        corners.push(toward(last, heading, long));
    }
    (corners, lead.map(|(i, _)| i), far_end)
}

fn depth(rng: &mut Rng, most: f32) -> f32 {
    match rng.below(10) {
        0 => uniform(rng, SHALLOWEST_M, SPUR_MIN_M + 3.0),
        1 => uniform(rng, (most - 20.0).max(SHALLOWEST_M), most),
        _ => uniform(rng, SHALLOWEST_M, most),
    }
}

/// A spur `way` off `heading`, `depth` deep, where the course goes on at
/// `ahead`.
fn spur(
    rng: &mut Rng,
    (heading, way, ahead): (f32, f32, f32),
    depth: f32,
    tags: &mut Vec<String>,
) -> Spur {
    let bearing = heading + way;
    let drift = match rng.below(3) {
        0 => 0.0,
        _ => uniform(rng, 0.0, DRIFT_M),
    };
    let right = toward((0.0, 0.0), bearing + 90.0, 1.0);
    let on = toward((0.0, 0.0), ahead, 1.0);
    let onward = right.0 * on.0 + right.1 * on.1 >= 0.0;
    let drift = match onward != rng.one_in(4) {
        true => drift,
        false => -drift,
    };
    let (out_every, back_every) = match rng.one_in(2) {
        true => {
            let every = every(rng);
            (every, every)
        }
        false => (every(rng), every(rng)),
    };
    let alongside = rng.one_in(2);
    let back = if alongside { "alongside" } else { "back" };
    tags.push(format!(
        "{depth:.1} m at {way:.0}, {back} {drift:.2} m, every {out_every}/{back_every} m"
    ));
    Spur {
        bearing,
        depth,
        drift,
        alongside,
        out_every,
        back_every,
        inner: None,
    }
}

fn way(rng: &mut Rng) -> f32 {
    match rng.below(4) {
        0 => rng.pick(&[90.0, -90.0, 145.0, -145.0, 35.0, -35.0, 165.0, -15.0]),
        _ => {
            let size = uniform(rng, 5.0, 175.0);
            if rng.one_in(2) { size } else { -size }
        }
    }
}

/// Case `seed`: a route and up to three spurs on it, or none where it draws
/// what the generator leaves out.
fn drawn(seed: u64) -> Option<Case> {
    let mut rng = Rng::new(seed);
    let mut tags = vec![format!("case {seed}")];
    let (corners, lead, far_end) = route(&mut rng, &mut tags);
    let mut route = Route {
        corners,
        every: every(&mut rng),
        sites: Vec::new(),
        tags: Vec::new(),
    };
    tags.push(format!("route every {} m", route.every));
    let along = route.along();
    let total_m = along[along.len() - 1];
    let mut spurs_left = rng.below(4) as usize;
    if let Some((end, way)) = far_end
        && spurs_left > 0
    {
        let stub = uniform(&mut rng, SHALLOWEST_M, 45.0);
        tags.push("at the far end".into());
        let side = rng.pick(&[90.0, -90.0]);
        let stub = spur(&mut rng, (way, side, way), stub, &mut tags);
        route.sites.push(Site {
            along_m: along[end],
            spurs: vec![stub],
        });
        spurs_left -= 1;
    }
    while spurs_left > 0 {
        let (along_m, kind) = match (lead, rng.below(8)) {
            (_, 0) => (0.0, "at the start"),
            (_, 1) => (total_m - uniform(&mut rng, 25.0, 80.0), "near the end"),
            (Some(i), 2) => (along[i], "past a lead"),
            (_, 2..=4) => {
                let i = rng.range(1, along.len() as u64 - 2) as usize;
                let off = match rng.below(3) {
                    0 => 0.0,
                    1 => uniform(&mut rng, 3.0, 30.0),
                    _ => -uniform(&mut rng, 3.0, 30.0),
                };
                (along[i] + off, "by a corner")
            }
            _ => (uniform(&mut rng, 30.0, total_m - 30.0), "along a road"),
        };
        let along_m = along_m.clamp(0.0, total_m - 25.0);
        let (_, heading, _) = route.point(along_m);
        let (_, ahead, _) = route.point(along_m + 0.5);
        tags.push(kind.into());
        let first_way = match kind {
            "past a lead" => 0.0,
            _ => way(&mut rng),
        };
        let first = depth(&mut rng, DEEPEST_M);
        let mut site = Site {
            along_m,
            spurs: vec![spur(
                &mut rng,
                (heading, first_way, ahead),
                first,
                &mut tags,
            )],
        };
        spurs_left -= 1;
        if spurs_left > 0 && rng.one_in(3) {
            tags.push("then another".into());
            let (way, depth) = (way(&mut rng), depth(&mut rng, DEEPEST_M));
            site.spurs
                .push(spur(&mut rng, (heading, way, ahead), depth, &mut tags));
            spurs_left -= 1;
        } else if spurs_left > 0 && site.spurs[0].depth >= 40.0 && rng.one_in(3) {
            let outer = site.spurs[0].bearing;
            let along = uniform(&mut rng, 0.25, 0.75);
            tags.push(format!("off its way out {along:.2} along"));
            let (side, depth) = (rng.pick(&[90.0, -90.0]), depth(&mut rng, 40.0));
            let inner = spur(&mut rng, (outer, side, outer), depth, &mut tags);
            site.spurs[0].inner = Some((along, std::boxed::Box::new(inner)));
            spurs_left -= 1;
        }
        route.sites.push(site);
    }
    route.tags = tags;
    let spaced = route.sites.iter().enumerate().all(|(i, a)| {
        route.sites[i + 1..]
            .iter()
            .all(|b| (a.along_m - b.along_m).abs() > 1.0)
    });
    (spaced && !route.tangled()).then(|| route.case())
}

fn led_by(line: &[P], lead_m: f32) -> Vec<P> {
    let (first, second) = (line[0], line[1]);
    let along = distance(first, second);
    let lead = (
        first.0 - lead_m * (second.0 - first.0) / along,
        first.1 - lead_m * (second.1 - first.1) / along,
    );
    let led = (lead_m > 0.0).then_some(lead);
    led.into_iter().chain(line.iter().copied()).collect()
}

fn led(turns: Vec<Turn>, lead_m: f32) -> Vec<Turn> {
    let lead_mm = (lead_m * 1000.0) as u32;
    turns
        .into_iter()
        .map(|turn| Turn {
            start_mm: turn.start_mm.saturating_sub(lead_mm),
            end_mm: turn.end_mm.saturating_sub(lead_mm),
            ..turn
        })
        .collect()
}

/// Metres along the course without its spurs to `m` along it drawn.
fn without_m(cut: &[Cut], m: f32) -> f32 {
    let mut removed = 0.0;
    for cut in cut {
        if m <= cut.from_m {
            break;
        }
        if m < cut.rejoin_m {
            return cut.from_m - removed;
        }
        removed += cut.rejoin_m - cut.from_m - cut.apart_m;
    }
    m - removed
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Said {
    Without,
    Uncut,
    Neither,
}

fn stretches(cut: &[Cut]) -> Vec<(f32, f32, bool)> {
    let mut stretches: Vec<(f32, f32, usize, bool)> = Vec::new();
    for cut in cut {
        let (from_m, to_m) = (cut.from_m - AROUND_M, cut.rejoin_m + AROUND_M);
        match stretches.last_mut() {
            Some(last) if from_m <= last.1 => {
                last.1 = to_m;
                last.2 += 1;
            }
            _ => stretches.push((from_m, to_m, 1, cut.planned)),
        }
    }
    stretches
        .into_iter()
        .map(|(from_m, to_m, sites, planned)| (from_m, to_m, sites == 1 && planned))
        .collect()
}

/// A turn in a stretch, and whether it starts within `STARTS_M` of the
/// stretch or only within twice that, where a turn just outside it in one
/// course may be just inside it in another.
type Grouped = Vec<Vec<(Turn, bool)>>;

fn grouped(turns: &[Turn], bounds: &[(f32, f32)], along: impl Fn(u32) -> f32) -> Grouped {
    let mut groups = vec![Vec::new(); bounds.len() + 1];
    for turn in heeded(turns) {
        let m = along(turn.start_mm);
        let within = |slack_m: f32| {
            move |&(from_m, to_m): &(f32, f32)| (from_m - slack_m..=to_m + slack_m).contains(&m)
        };
        for (k, bound) in bounds.iter().enumerate() {
            if within(2.0 * STARTS_M)(bound) {
                groups[k].push((turn, within(STARTS_M)(bound)));
            }
        }
        if !bounds.iter().any(within(-2.0 * STARTS_M)) {
            let core = !bounds.iter().any(within(-STARTS_M));
            groups[bounds.len()].push((turn, core));
        }
    }
    groups
}

fn same(said: &[(Turn, bool)], want: &[(Turn, bool)], onto: impl Fn(u32) -> f32) -> bool {
    let metres = |mm: u32| mm as f32 / 1000.0;
    let alike =
        |turn: &Turn, other: &Turn, onto: &dyn Fn(u32) -> f32, other_onto: &dyn Fn(u32) -> f32| {
            turn.kind == other.kind
                && (onto(turn.start_mm) - other_onto(other.start_mm)).abs() <= STARTS_M
        };
    let kept = |list: &[(Turn, bool)],
                others: &[(Turn, bool)],
                onto: &dyn Fn(u32) -> f32,
                other_onto: &dyn Fn(u32) -> f32| {
        list.iter()
            .filter(|(turn, core)| {
                *core
                    || others
                        .iter()
                        .any(|(other, _)| alike(turn, other, onto, other_onto))
            })
            .map(|(turn, _)| *turn)
            .collect::<Vec<_>>()
    };
    let (said, want) = (
        kept(said, want, &onto, &metres),
        kept(want, said, &metres, &onto),
    );
    let placed = |mm: u32, want_mm: u32, near_m: f32| (onto(mm) - metres(want_mm)).abs() <= near_m;
    said.len() == want.len()
        && said.iter().zip(&want).all(|(turn, want)| {
            (turn.kind, turn.twice, turn.cornered) == (want.kind, want.twice, want.cornered)
                && placed(turn.start_mm, want.start_mm, STARTS_M)
                && placed(turn.end_mm, want.end_mm, ENDS_M)
        })
}

fn judged(
    drawn: &[Turn],
    without: &[Vec<Turn>],
    uncut: &[Vec<Turn>],
    cut: &[Cut],
) -> Vec<(Said, bool)> {
    let stretches = stretches(cut);
    let drawn_bounds: Vec<_> = stretches.iter().map(|s| (s.0, s.1)).collect();
    let without_bounds: Vec<_> = stretches
        .iter()
        .map(|s| (without_m(cut, s.0), without_m(cut, s.1)))
        .collect();
    let metres = |mm: u32| mm as f32 / 1000.0;
    let drawn = grouped(drawn, &drawn_bounds, metres);
    let uncut: Vec<_> = uncut
        .iter()
        .map(|uncut| grouped(uncut, &drawn_bounds, metres))
        .collect();
    let without: Vec<_> = without
        .iter()
        .map(|without| grouped(without, &without_bounds, metres))
        .collect();
    let planned = stretches.iter().map(|s| s.2).chain([false]);
    drawn
        .iter()
        .enumerate()
        .zip(planned)
        .map(|((k, drawn), planned)| {
            let onto = |mm| without_m(cut, metres(mm));
            let said = if without.iter().any(|without| same(drawn, &without[k], onto)) {
                Said::Without
            } else if uncut.iter().any(|uncut| same(drawn, &uncut[k], metres)) {
                Said::Uncut
            } else {
                Said::Neither
            };
            (said, planned)
        })
        .collect()
}

fn told(turns: &[Turn]) -> String {
    let said: Vec<_> = heeded(turns)
        .iter()
        .map(|turn| {
            let flags = match (turn.twice, turn.cornered) {
                (true, true) => "+2c",
                (true, false) => "+2",
                (false, true) => "+c",
                (false, false) => "",
            };
            format!(
                "{:?}{flags}@{:.0}",
                turn.kind,
                turn.start_mm as f32 / 1000.0
            )
        })
        .collect();
    said.join(" ")
}

#[derive(Default)]
pub(super) struct Judged {
    pub(super) failed: Vec<String>,
    pub(super) courses: usize,
    pub(super) without: usize,
    pub(super) uncut: usize,
    pub(super) planned: usize,
}

pub(super) fn judge(seeds: core::ops::Range<u64>) -> Judged {
    let mut tally = Judged::default();
    for seed in seeds {
        let Some(case) = drawn(seed) else {
            continue;
        };
        tally.courses += 1;
        let paths: [(&str, TurnsOf); 2] = [("scan", scanned), ("upload", found_along)];
        for (path, turns_of) in paths {
            let (drawn, cut_m) = turns_of(&case.with);
            let removed_m: f32 = case
                .cut
                .iter()
                .map(|cut| cut.rejoin_m - cut.from_m - cut.apart_m)
                .sum();
            let phases = |shift_m: f32| {
                let near = NEAR_PHASES_M.map(|by_m| (shift_m + by_m).rem_euclid(STEP_M));
                PHASES_M.into_iter().chain(near)
            };
            let read = |line: &[P], lead_m: f32| led(turns_of(&led_by(line, lead_m)).0, lead_m);
            let without: Vec<_> = phases(removed_m - cut_m)
                .map(|lead_m| read(&case.without, lead_m))
                .collect();
            let uncut: Vec<_> = phases(-cut_m)
                .map(|lead_m| uncut(|| read(&case.with, lead_m)))
                .collect();
            let stretches = judged(&drawn, &without, &uncut, &case.cut);
            for (k, (said, planned)) in stretches.iter().enumerate() {
                let spur = k + 1 < stretches.len();
                match said {
                    Said::Without if spur => tally.without += 1,
                    Said::Uncut if spur => tally.uncut += 1,
                    _ => {}
                }
                tally.planned += usize::from(*planned);
                let wrong = *said == Said::Neither || (*planned && *said != Said::Without);
                if wrong {
                    tally.failed.push(format!(
                        "{} ({path}, stretch {k}, {said:?}, planned {planned})\n  drawn:   {}\n  without: {}\n  uncut:   {}",
                        case.name,
                        told(&drawn),
                        told(&without[0]),
                        told(&uncut[0])
                    ));
                }
            }
        }
    }
    tally
}
