//! Courses drawn at random, each with up to three spurs, against the same
//! course with every spur's path taken out at its base: a spur must say
//! what that course says, on both the scan and the upload.
//!
//! The generator leaves out what is read otherwise by design, each where it
//! draws it. A road within `NEAR_M` of a spur, or leaving its base nearer
//! than `ALONG_COS`, cannot be told from a way back up to `RETRACE_M` off,
//! nor can a way back drifting more than `V_DRIFT_M` off a V. Dead ends
//! within a few metres of `SPUR_MIN_M` or `SPUR_M` deep, or within five of
//! `TIP_SPUR_M` past a far corner, sit on the edge of what is a spur. A V
//! is never drawn on a lead. A spur by an out-and-back's far corner, or on
//! its way back less than `SPUR_M` from it, makes the far end a dead end
//! too, and either could be the road. Corners turn at least 10 degrees from
//! where a turn's kind changes, which a cut that steps aside can cross.

use super::super::super::{Turn, distance};
use super::super::{RETRACE_M, SPUR_M, SPUR_MIN_M, TIP_SPUR_M};
use super::{heeded, join, length, path, scanned};
use crate::ride::course::stored::tests::found_along;
use crate::ride::thin::off_segment;
use crate::testing::Rng;
use std::string::String;
use std::vec::Vec;
use std::{format, vec};

type P = (f32, f32);

const EVERY_M: [f32; 6] = [1.0, 3.0, 5.0, 8.0, 15.0, 10_000.0];
const NEAR_M: f32 = 6.0;
/// Where a turn starts, the rider hears it: a few metres either way, as a
/// U-turn's chord swings sooner where its legs lie a metre apart. Where it
/// ends only frees the next, and a cut that steps aside moves it most.
const STARTS_M: f32 = 10.0;
const ENDS_M: f32 = 15.0;
const SHALLOWEST_M: f32 = SPUR_MIN_M + 3.0;
const SPUR_MAX_M: f32 = SPUR_M - 10.0;
const TIP_STUB_GAP_M: f32 = TIP_SPUR_M + 10.0;
const FAR_CORNER_M: f32 = 40.0;
/// A road leaving a spur's base nearer its line than 30 degrees runs within
/// `RETRACE_M` of it for metres.
const ALONG_COS: f32 = 0.866;
/// A spur this near a road at its base whose way back drifts further runs
/// within `RETRACE_M` of that road.
const V_DEG: f32 = 55.0;
const V_DRIFT_M: f32 = 2.0;

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

    /// Draw it from `base` onto `line`; where its way back ends.
    fn draw(&self, base: P, line: &mut Vec<Vec<P>>) -> P {
        let tip = self.tip(base);
        match &self.inner {
            Some((along, inner)) => {
                let inner_base = toward(base, self.bearing, along * self.depth);
                line.push(path(&[base, inner_base], self.out_every));
                let back = inner.draw(inner_base, line);
                line.push(path(&[back, tip], self.out_every));
            }
            None => line.push(path(&[base, tip], self.out_every)),
        }
        let to = self.returns_to(base);
        let back = match self.alongside {
            true => vec![tip, toward(tip, self.bearing + 90.0, self.drift), to],
            false => vec![tip, to],
        };
        line.push(path(&back, self.back_every));
        to
    }

    /// Its centre line from `base`, and those of its own spurs.
    fn lines(&self, base: P, depth: usize, lines: &mut Vec<(P, P, usize)>) {
        lines.push((base, self.tip(base), depth));
        if let Some((along, inner)) = &self.inner {
            inner.lines(
                toward(base, self.bearing, along * self.depth),
                depth + 1,
                lines,
            );
        }
    }
}

/// Spurs drawn one after another from a point `along_m` along the route.
#[derive(Clone, Debug)]
struct Site {
    along_m: f32,
    spurs: Vec<Spur>,
}

/// A course as drawn, and as it would be without its spurs, with the
/// stretches of it they take, as metres along it to where each leaves the
/// route and comes back, and how far apart the two are.
struct Case {
    name: String,
    with: Vec<P>,
    without: Vec<P>,
    cut: Vec<(f32, f32, f32)>,
}

/// The route, a point every `every` metres, with what is drawn on it.
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
                to = spur.draw(to, &mut with);
            }
            without.push(path(&[base, to], self.every));
            cut.push((from_m, length(&join(&with)), distance(base, to)));
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

    /// A spur runs within `NEAR_M` of the course other than at its base,
    /// or two parts of the route run that near.
    fn tangled(&self) -> bool {
        let mut lines = Vec::new();
        let mut bases = Vec::new();
        for site in &self.sites {
            let (mut base, _, _) = self.point(site.along_m);
            for spur in &site.spurs {
                let first = lines.len();
                spur.lines(base, 0, &mut lines);
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
        for (i, &(base, tip, _)) in lines.iter().enumerate() {
            let depth = distance(base, tip);
            let owner = bases
                .iter()
                .find(|(_, first, last)| (*first..*last).contains(&i))
                .map_or((i, i + 1), |&(_, first, last)| (first, last));
            let way = bearing(base, tip);
            for segment in &segments {
                if touches(segment, base) {
                    let ends = [segment.0, segment.1];
                    let same = ends
                        .iter()
                        .filter(|&&end| distance(end, base) > 0.5)
                        .any(|&end| {
                            libm::cosf((bearing(base, end) - way).to_radians()) > ALONG_COS
                        });
                    if same {
                        return true;
                    }
                    continue;
                }
                let near = (8..=depth as usize).any(|m| {
                    off_segment(toward(base, way, m as f32), segment.0, segment.1) < NEAR_M
                });
                if near {
                    return true;
                }
            }
            for (j, &(other_base, other_tip, _)) in lines.iter().enumerate() {
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
                            .any(|p| off_segment(p, a.0, a.1) < 30.0)
                })
            })
    }
}

fn turn_deg(rng: &mut Rng) -> f32 {
    let size = match rng.below(5) {
        0 => uniform(rng, 20.0, 45.0),
        1 => uniform(rng, 130.0, 140.0),
        _ => uniform(rng, 70.0, 110.0),
    };
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
            let end_m = match rng.one_in(3) {
                true => uniform(rng, 15.0, TIP_SPUR_M - 5.0),
                false => uniform(rng, TIP_STUB_GAP_M, 80.0),
            };
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
        0 => uniform(rng, SHALLOWEST_M, SHALLOWEST_M + 2.0),
        1 => uniform(rng, (most - 10.0).max(SHALLOWEST_M), most),
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
    let apart = |road: f32| libm::fabsf(super::super::super::swing(road, bearing));
    let v = apart(heading + 180.0).min(apart(ahead)) < V_DEG;
    let most = if v { V_DRIFT_M } else { RETRACE_M - 0.05 };
    let drift = match rng.below(3) {
        0 => 0.0,
        _ => uniform(rng, 0.0, most),
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
        0 => rng.pick(&[90.0, -90.0, 145.0, -145.0, 35.0, -35.0]),
        _ => {
            let size = uniform(rng, 30.0, 150.0);
            if rng.one_in(2) { size } else { -size }
        }
    }
}

/// Case `seed`: a route, and up to three spurs on it.
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
        let stub = uniform(&mut rng, SHALLOWEST_M, TIP_SPUR_M - 5.0);
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
        let first = depth(&mut rng, SPUR_MAX_M);
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
            let (way, depth) = (way(&mut rng), depth(&mut rng, SPUR_MAX_M));
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
    let apart = far_end.is_none_or(|(end, _)| {
        let far = (along[end - 1] - FAR_CORNER_M)..(along[end + 1] + SPUR_M);
        route
            .sites
            .iter()
            .all(|site| site.along_m == along[end] || !far.contains(&site.along_m))
    });
    (spaced && apart && !route.tangled()).then(|| route.case())
}

/// Metres along the course without its spurs to `m` along it drawn.
fn without_m(cut: &[(f32, f32, f32)], m: f32) -> f32 {
    let mut removed = 0.0;
    for &(from_m, to_m, apart) in cut {
        if m <= from_m {
            break;
        }
        if m < to_m {
            return from_m - removed;
        }
        removed += to_m - from_m - apart;
    }
    m - removed
}

fn alike(drawn: &[Turn], without: &[Turn], cut: &[(f32, f32, f32)]) -> bool {
    let (drawn, without) = (heeded(drawn), heeded(without));
    let placed = |mm: u32, want_mm: u32, near_m: f32| {
        (without_m(cut, mm as f32 / 1000.0) - want_mm as f32 / 1000.0).abs() <= near_m
    };
    drawn.len() == without.len()
        && drawn.iter().zip(&without).all(|(turn, want)| {
            (turn.kind, turn.twice, turn.cornered) == (want.kind, want.twice, want.cornered)
                && placed(turn.start_mm, want.start_mm, STARTS_M)
                && placed(turn.end_mm, want.end_mm, ENDS_M)
        })
}

fn told(turns: &[Turn]) -> String {
    let said: Vec<_> = heeded(turns)
        .iter()
        .map(|turn| format!("{:?}@{:.0}", turn.kind, turn.start_mm as f32 / 1000.0))
        .collect();
    said.join(" ")
}

/// Each case of `seeds` drawn whose spurs do not say what the course
/// without them says, and how many were drawn.
pub(super) fn unlike(seeds: core::ops::Range<u64>) -> (Vec<String>, usize) {
    let (mut failed, mut checked) = (Vec::new(), 0);
    for seed in seeds {
        let Some(case) = drawn(seed) else {
            continue;
        };
        checked += 1;
        let (scan, scan_without) = (scanned(&case.with).0, scanned(&case.without).0);
        let (upload, upload_without) = (found_along(&case.with).0, found_along(&case.without).0);
        let scan_ok = alike(&scan, &scan_without, &case.cut);
        let upload_ok = alike(&upload, &upload_without, &case.cut);
        if !scan_ok || !upload_ok {
            failed.push(format!(
                "{}: scan {scan_ok}, upload {upload_ok}\n  without: {}\n  scan:    {}\n  without: {}\n  upload:  {}",
                case.name,
                told(&scan_without),
                told(&scan),
                told(&upload_without),
                told(&upload)
            ));
        }
    }
    (failed, checked)
}
