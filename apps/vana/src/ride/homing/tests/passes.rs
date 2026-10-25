//! Courses that pass the same place twice, ridden through `Homing` with
//! noisy fixes: every word names the next corner of the pass the rider is
//! on, or there is none.
use super::*;
use crate::ride::course::stored::tests::uploaded;
use crate::ride::course::turns::tests::{FOUND, Found};
use crate::ride::course::turns::{Kind, Side, TURNS, Turn};
use crate::testing::Rng;
use std::string::String;

/// The corners of `line`: metres along to each, and its swing, right
/// positive. The points of a curve, each a small swing the same way within
/// 5 m of the last, are one corner where it starts.
fn corners_of(line: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let way = |a: Point, b: Point| libm::atan2f(b.0 - a.0, b.1 - a.1).to_degrees();
    let (mut along, mut last) = (0.0, f32::MIN);
    let mut corners: Vec<(f32, f32)> = Vec::new();
    for w in line.windows(3) {
        along += distance(w[0], w[1]);
        let swing = turns::swing(way(w[0], w[1]), way(w[1], w[2]));
        if swing.abs() < 1.0 {
            continue;
        }
        match corners.last_mut() {
            Some(curve) if swing.abs() < 45.0 && along - last <= 5.0 && curve.1 * swing > 0.0 => {
                curve.1 += swing;
            }
            _ => corners.push((along, swing)),
        }
        last = along;
    }
    corners
}

/// Whether `kind` names the corner `at` of `corners`, or the pair of them
/// from it as one U-turn; how many corners it names.
fn names(kind: Kind, corners: &[(f32, f32)], at: usize) -> Option<usize> {
    let one = corners.get(at)?.1;
    let side = |degrees: f32| match degrees > 0.0 {
        true => Side::Right,
        false => Side::Left,
    };
    match kind {
        Kind::Turn(s) => (side(one) == s && (55.0..=150.0).contains(&one.abs())).then_some(1),
        Kind::Sharp(s) => (side(one) == s && (100.0..=170.0).contains(&one.abs())).then_some(1),
        Kind::UTurn if one.abs() >= 135.0 => Some(1),
        Kind::UTurn => corners.get(at + 1).and_then(|next| {
            let pair = side(one) == side(next.1)
                && next.0 - corners[at].0 <= 25.0
                && (one + next.1).abs() >= 135.0;
            pair.then_some(2)
        }),
    }
}

#[derive(Debug, PartialEq)]
enum Heard {
    Right,
    /// Every word right, but a corner of 60 degrees or more had none at it.
    Silent,
    Wrong,
}

/// What `said`, metres along the rider's way and the word, makes of the
/// rider's `corners`, with fixes up to `noise` m off: a word may name the
/// corner after one the rider is within 12 m and that of, or the one they
/// are at.
fn judge(said: &[(f32, Warning)], corners: &[(f32, f32)], noise: f32) -> Heard {
    let mut named = vec![false; corners.len()];
    for &(metres, warning) in said {
        let next = |metres: f32| {
            corners
                .iter()
                .position(|c| c.0 > metres + 12.0)
                .unwrap_or(corners.len())
        };
        let ok = [
            next(metres - noise - 12.0),
            next(metres - noise),
            next(metres + noise),
        ]
        .into_iter()
        .any(|at| match warning {
            Warning::Ahead { kind, .. } => names(kind, corners, at).is_some(),
            Warning::Now { kind, then } => match names(kind, corners, at) {
                None => false,
                Some(n) => {
                    let then = match then {
                        None => Some(0),
                        Some(then) => names(then, corners, at + n),
                    };
                    then.inspect(|m| named[at..at + n + m].iter_mut().for_each(|c| *c = true))
                        .is_some()
                }
            },
        });
        if !ok {
            return Heard::Wrong;
        }
    }
    let missed = corners
        .iter()
        .zip(&named)
        .any(|(c, &named)| c.1.abs() >= 60.0 && !named);
    match missed {
        true => Heard::Silent,
        false => Heard::Right,
    }
}

/// Ride `line`, drawn every 5 m, from its start to its end with a fix every
/// `step` m up to `noise` m off; the words and where along `line` each came,
/// and every other event.
fn ride_line(
    line: &[(f32, f32)],
    found: Found,
    noise: f32,
    step: f32,
    seed: u64,
) -> (Vec<(f32, Warning)>, Vec<Event>) {
    ride_way(line, line, found, noise, step, seed)
}

/// As `ride_line`, the rider riding `way` rather than the course `line`.
fn ride_way(
    line: &[(f32, f32)],
    way: &[(f32, f32)],
    found: Found,
    noise: f32,
    step: f32,
    seed: u64,
) -> (Vec<(f32, Warning)>, Vec<Event>) {
    ride_gapped(line, way, found, noise, step, seed, 0.0..0.0)
}

/// As `ride_way`, with no fix from `gap` metres along `way`, the clock
/// running on.
fn ride_gapped(
    line: &[(f32, f32)],
    way: &[(f32, f32)],
    found: Found,
    noise: f32,
    step: f32,
    seed: u64,
    gap: core::ops::Range<f32>,
) -> (Vec<(f32, Warning)>, Vec<Event>) {
    let drawn = path(line, 5.0);
    let (course, stored): (Vec<course::Point>, Option<Vec<Turn>>) = match found {
        Found::Scanned => (
            drawn
                .iter()
                .map(|&(e, n)| course::tests::at(f64::from(e), f64::from(n)))
                .collect(),
            None,
        ),
        Found::Uploaded => {
            let (course, turns) = uploaded(&drawn);
            (course.points().to_vec(), Some(turns))
        }
    };
    let length: f32 = way.windows(2).map(|w| distance(w[0], w[1])).sum();
    let mut rng = Rng::new(seed + 1);
    let mut jitter = || {
        let span = (2.0 * noise * 1000.0) as u64;
        rng.below(span + 1) as f32 / 1000.0 - noise
    };
    let (mut crumbs, mut kept) = (vec![Crumb::default(); 512], vec![Crumb::default(); 2048]);
    let mut list = vec![Turn::default(); TURNS];
    let mut homing = Homing::new(&mut crumbs)
        .with_line(&mut kept)
        .with_turns(&mut list);
    homing.set_course(&course);
    let (mut said, mut events) = (Vec::new(), Vec::new());
    let mut now = 0;
    for k in 0..=(length / step) as usize {
        let metres = k as f32 * step;
        let (east, north) = along_line(way, metres);
        let fix = (east + jitter(), north + jitter());
        if gap.contains(&metres) {
            now += 1_000;
            homing.observe(None, now);
            continue;
        }
        let one = [fix];
        let (words, more) =
            ride_course_stored(&mut homing, &course, stored.as_deref(), &one, &mut now);
        said.extend(words.into_iter().map(|w| (metres, w)));
        events.extend(more);
    }
    (said, events)
}

fn mirrored(line: &[(f32, f32)]) -> Vec<(f32, f32)> {
    line.iter().map(|&(e, n)| (-e, n)).collect()
}

#[derive(Default)]
struct Tally {
    wrong: Vec<String>,
    silent: usize,
    /// Runs told OFF TRACK, though the rider kept to the course.
    off: usize,
    runs: usize,
}

/// Ride each of `lines` every way.
fn gate(lines: &[(String, Vec<(f32, f32)>)], found: Found) -> Tally {
    let mut tally = Tally::default();
    for (name, line) in lines {
        let corners = corners_of(line);
        for noise in [0.0f32, 3.0, 6.0] {
            for step in [5.0f32, 7.0, 9.0] {
                for seed in 0..8 {
                    let (said, events) = ride_line(line, found, noise, step, seed);
                    tally.runs += 1;
                    tally.off += usize::from(events.contains(&Event::OffTrack));
                    match judge(&said, &corners, noise) {
                        Heard::Wrong => tally.wrong.push(std::format!(
                            "{found:?}: {name} noise {noise} step {step} seed {seed}: {said:?}"
                        )),
                        Heard::Silent => tally.silent += 1,
                        Heard::Right => {}
                    }
                }
            }
        }
    }
    tally
}

/// Every course of `lines`, its turns found both ways: no wrong word, and
/// no OFF TRACK.
fn pass(what: &str, lines: &[(String, Vec<(f32, f32)>)]) {
    let (mut wrong, mut off) = (Vec::new(), 0);
    for found in FOUND {
        let tally = gate(lines, found);
        std::println!(
            "{what}, {found:?}: {} of {} wrong, {} silent, {} off track",
            tally.wrong.len(),
            tally.runs,
            tally.silent,
            tally.off
        );
        wrong.extend(tally.wrong);
        off += tally.off;
    }
    assert!(wrong.is_empty(), "{:#?}", &wrong[..wrong.len().min(6)]);
    assert_eq!(off, 0, "told OFF TRACK on the course");
}

/// Figure-of-eight courses. One rides a stem north to a junction, round a
/// block of `side` back onto the stem `side` before the junction, and
/// through the junction again, the other way or straight on: the stem is
/// ridden twice, the second time `apart` m to the side. The other crosses
/// itself at a crossroads.
fn figure_eights() -> Vec<(String, Vec<(f32, f32)>)> {
    let mut lines = Vec::new();
    for side in [60.0f32, 100.0] {
        for apart in [0.0f32, 2.0, 4.0] {
            for straight in [false, true] {
                let exit = match straight {
                    true => (apart, 800.0),
                    false => (400.0, 400.0),
                };
                let line = vec![
                    (0.0, 0.0),
                    (0.0, 400.0),
                    (-side, 400.0),
                    (-side, 400.0 - side),
                    (apart, 400.0 - side),
                    (apart, 400.0),
                    exit,
                ];
                let name = std::format!("stem block {side} apart {apart} straight {straight}");
                lines.push((std::format!("{name} left"), line.clone()));
                lines.push((std::format!("{name} right"), mirrored(&line)));
            }
        }
    }
    let crossing = vec![
        (0.0, 0.0),
        (0.0, 500.0),
        (100.0, 500.0),
        (100.0, 400.0),
        (-300.0, 400.0),
    ];
    lines.push(("crossroads right".into(), crossing.clone()));
    lines.push(("crossroads left".into(), mirrored(&crossing)));
    lines
}

/// Out and back: north, right at a junction, `out` m on, back `apart` m to
/// the side, and left at the junction home.
fn out_and_backs() -> Vec<(String, Vec<(f32, f32)>)> {
    let mut lines = Vec::new();
    for out in [100.0f32, 200.0, 400.0] {
        for apart in [0.0f32, 4.0] {
            let line = match apart > 0.0 {
                true => vec![
                    (0.0, 0.0),
                    (0.0, 400.0),
                    (out, 400.0),
                    (out, 400.0 + apart),
                    (apart, 400.0 + apart),
                    (apart, 0.0),
                ],
                false => vec![
                    (0.0, 0.0),
                    (0.0, 400.0),
                    (out, 400.0),
                    (0.0, 400.0),
                    (0.0, 0.0),
                ],
            };
            let name = std::format!("out {out} apart {apart}");
            lines.push((std::format!("{name} right"), line.clone()));
            lines.push((std::format!("{name} left"), mirrored(&line)));
        }
    }
    lines
}

#[test]
fn a_figure_of_eight_names_the_corners_of_the_pass_the_rider_is_on() {
    pass("figure of eight", &figure_eights());
}

#[test]
fn an_out_and_back_names_the_corners_of_the_leg_the_rider_is_on() {
    pass("out and back", &out_and_backs());
}

/// A rider who cuts across to the course a few hundred metres on is
/// followed from there, and hears the corners after it.
#[test]
fn a_shortcut_onto_the_course_further_on_is_followed_from_there() {
    let line = [
        (0.0, 0.0),
        (0.0, 400.0),
        (300.0, 400.0),
        (300.0, 800.0),
        (600.0, 800.0),
        (600.0, 1200.0),
    ];
    let way = [
        (0.0, 0.0),
        (0.0, 300.0),
        (300.0, 600.0),
        (300.0, 800.0),
        (600.0, 800.0),
        (600.0, 1200.0),
    ];
    for found in FOUND {
        for noise in [0.0, 3.0, 6.0] {
            for seed in 0..8 {
                let (said, _) = ride_way(&line, &way, found, noise, 7.0, seed);
                let now: Vec<Kind> = said
                    .iter()
                    .filter_map(|&(_, warning)| match warning {
                        Warning::Now { kind, .. } => Some(kind),
                        Warning::Ahead { .. } => None,
                    })
                    .collect();
                assert_eq!(
                    now,
                    [Kind::Turn(Side::Right), Kind::Turn(Side::Left)],
                    "{found:?} noise {noise} seed {seed}: {said:?}"
                );
            }
        }
    }
}

/// Courses that come back alongside themselves, with the fixes lost for 100
/// to 300 m: across an out-and-back's far end, or just before a hairpin.
/// Every word names the corner ahead on the pass the rider is on, no run is
/// told OFF TRACK, and each reaches the course's end.
#[test]
fn a_gap_in_the_fixes_beside_another_pass_keeps_to_the_rider_s_pass() {
    let mut lines = Vec::new();
    for out in [400.0f32, 800.0] {
        for apart in [4.0f32, 10.0, 20.0] {
            let line = vec![
                (0.0, 0.0),
                (0.0, 600.0),
                (out, 600.0),
                (out, 600.0 + apart),
                (apart, 600.0 + apart),
                (apart, 0.0),
            ];
            let turnaround = 600.0 + out;
            lines.push((std::format!("out {out} apart {apart}"), line, turnaround));
        }
    }
    let pin = 10.0;
    let hairpin = crate::ride::course::turns::tests::join(&[
        path(
            &[(0.0, 0.0), (0.0, 600.0), (400.0, 600.0), (400.0, 1000.0)],
            5.0,
        ),
        crate::ride::course::turns::tests::arc((400.0 + pin, 1000.0), pin, -90.0, 180.0, 3.0),
        path(
            &[(400.0 + 2.0 * pin, 1000.0), (400.0 + 2.0 * pin, 400.0)],
            5.0,
        ),
    ]);
    lines.push(("hairpin 20 m".into(), hairpin, 1400.0));
    let (mut wrong, mut off, mut unended, mut runs) = (Vec::new(), 0, 0, 0);
    for (name, line, at) in &lines {
        let corners = corners_of(line);
        for found in FOUND {
            for gap in [100.0f32, 150.0, 200.0, 300.0] {
                // Across the far end, and ending just before it.
                for from in [at - gap / 2.0, at - gap - 10.0] {
                    for noise in [0.0f32, 3.0] {
                        for seed in 0..4 {
                            let (said, events) =
                                ride_gapped(line, line, found, noise, 7.0, seed, from..from + gap);
                            runs += 1;
                            off += usize::from(events.contains(&Event::OffTrack));
                            unended += usize::from(!events.contains(&Event::CourseEnd));
                            if judge(&said, &corners, noise) == Heard::Wrong {
                                wrong.push(std::format!(
                                    "{found:?}: {name} gap {gap} from {from} noise {noise} seed {seed}: {said:?} {events:?}"
                                ));
                            }
                        }
                    }
                }
            }
        }
    }
    std::println!(
        "gaps: {} of {runs} wrong, {off} off track, {unended} never at the end",
        wrong.len()
    );
    assert!(wrong.is_empty(), "{:#?}", &wrong[..wrong.len().min(6)]);
    assert_eq!((off, unended), (0, 0), "told OFF TRACK or never at the end");
}
