use super::*;
use crate::ride::course::tests::{at as point_at, empty};
use crate::ride::course::{Builder, Course, east_per_e7, metres};
use std::vec;
use std::vec::Vec;

/// Points every `step` metres along straight legs between `corners`.
fn path(corners: &[(f32, f32)], step: f32) -> Vec<(f32, f32)> {
    let mut points = vec![corners[0]];
    for pair in corners.windows(2) {
        let steps = (distance(pair[0], pair[1]) / step).ceil() as usize;
        for i in 1..=steps {
            let t = i as f32 / steps as f32;
            points.push((
                pair[0].0 + t * (pair[1].0 - pair[0].0),
                pair[0].1 + t * (pair[1].1 - pair[0].1),
            ));
        }
    }
    points
}

/// Points every `step` metres round a circle about `centre`, from `from`
/// degrees clockwise from north, through `sweep` degrees: clockwise when
/// positive.
fn arc(centre: (f32, f32), radius: f32, from: f32, sweep: f32, step: f32) -> Vec<(f32, f32)> {
    let steps = (fabsf(sweep).to_radians() * radius / step).ceil() as usize;
    (0..=steps)
        .map(|i| {
            let a = (from + sweep * i as f32 / steps as f32).to_radians();
            (
                centre.0 + radius * libm::sinf(a),
                centre.1 + radius * libm::cosf(a),
            )
        })
        .collect()
}

fn join(parts: &[Vec<(f32, f32)>]) -> Vec<(f32, f32)> {
    let mut line: Vec<(f32, f32)> = Vec::new();
    for part in parts {
        let skip = usize::from(line.last() == part.first());
        line.extend_from_slice(&part[skip..]);
    }
    line
}

/// Where on `line` the rider is, `metres` from its start.
fn place(line: &[(f32, f32)], metres: f32) -> Place {
    let mut walked = 0.0;
    for next in 1..line.len() {
        let (a, b) = (line[next - 1], line[next]);
        let length = distance(a, b);
        if walked + length >= metres || next + 1 == line.len() {
            let t = if length > 0.0 {
                ((metres - walked) / length).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let here = (a.0 + t * (b.0 - a.0), a.1 + t * (b.1 - a.1));
            return Place {
                next,
                here,
                along_mm: mm(metres),
            };
        }
        walked += length;
    }
    unreachable!("a line of two points or more")
}

fn length(line: &[(f32, f32)]) -> f32 {
    line.windows(2).map(|pair| distance(pair[0], pair[1])).sum()
}

/// Ride `line` through `positions`, metres from its start, and say where
/// each warning came. Riding back is not riding the course its way, which
/// warns of nothing (`Homing`), so only the way on is asked.
fn ride_through(
    line: &[(f32, f32)],
    positions: impl IntoIterator<Item = f32>,
) -> Vec<(u32, Warning)> {
    let mut warner = Warner::default();
    let mut said = Vec::new();
    let mut last = f32::MIN;
    for metres in positions {
        let on = metres >= last;
        last = metres;
        if !on {
            continue;
        }
        let at = |k: usize| line[k];
        if let Some(warning) = warner.step(line.len(), at, place(line, metres)) {
            said.push((metres as u32, warning));
        }
    }
    said
}

/// Ride `line` start to end, a fix every `step` metres.
fn ride(line: &[(f32, f32)], step: f32) -> Vec<(u32, Warning)> {
    let fixes = (length(line) / step) as usize;
    ride_through(line, (0..=fixes).map(|k| k as f32 * step))
}

fn kinds(said: &[(u32, Warning)]) -> Vec<Warning> {
    said.iter().map(|&(_, warning)| warning).collect()
}

fn ahead(kind: Kind) -> Warning {
    Warning::Ahead { kind, metres: 150 }
}

fn now(kind: Kind) -> Warning {
    Warning::Now { kind, then: None }
}

const RIGHT: Kind = Kind::Turn(Side::Right);
const LEFT: Kind = Kind::Turn(Side::Left);
const U_TURN: Kind = Kind::UTurn;

/// A road north, turning right or left 90 degrees at each distance along
/// it, then 400 m on; a point every `spacing` metres.
fn corners(spacing: f32, turns: &[(f32, bool)]) -> Vec<(f32, f32)> {
    let (mut heading, mut done) = (0.0f32, 0.0);
    let mut points = vec![(0.0f32, 0.0f32)];
    let end = turns.last().map_or(800.0, |turn| turn.0 + 400.0);
    for &(at, right) in turns.iter().chain([(end, true)].iter()) {
        let radians = heading.to_radians();
        let p = *points.last().unwrap();
        let leg = at - done;
        points.push((
            p.0 + leg * libm::sinf(radians),
            p.1 + leg * libm::cosf(radians),
        ));
        done = at;
        heading += if right { 90.0 } else { -90.0 };
    }
    path(&points, spacing)
}

/// A ring of `radius` metres about (0, 0), ridden anticlockwise from the
/// south, leaving after `sweep` degrees round it.
fn ring(radius: f32, sweep: f32, spacing: f32) -> Vec<(f32, f32)> {
    let ring = arc((0.0, 0.0), radius, 180.0, -sweep, spacing.min(radius / 3.0));
    let out = *ring.last().unwrap();
    let away = (out.0 / radius * 300.0, out.1 / radius * 300.0);
    join(&[
        path(&[(0.0, -300.0), (0.0, -radius)], spacing),
        ring,
        path(&[out, away], spacing),
    ])
}

/// Up 400 m, round a left hairpin of `radius`, and back down.
fn hairpin(radius: f32, spacing: f32) -> Vec<(f32, f32)> {
    join(&[
        path(&[(0.0, 0.0), (0.0, 400.0)], spacing),
        arc(
            (-radius, 400.0),
            radius,
            90.0,
            -180.0,
            spacing.min(radius / 3.0),
        ),
        path(&[(-2.0 * radius, 400.0), (-2.0 * radius, 0.0)], spacing),
    ])
}

#[test]
fn a_corner_is_warned_of_150_m_before_and_again_at_it() {
    for spacing in [3.0, 5.0, 8.0, 50.0] {
        let said = ride(&corners(spacing, &[(400.0, true)]), 7.0);
        assert_eq!(
            kinds(&said),
            [ahead(RIGHT), now(RIGHT)],
            "{spacing} m: {said:?}"
        );
        assert!((235..=255).contains(&said[0].0), "{said:?}");
        assert!((355..=380).contains(&said[1].0), "{said:?}");
        let left = ride(&corners(spacing, &[(400.0, false)]), 7.0);
        assert_eq!(kinds(&left), [ahead(LEFT), now(LEFT)], "{spacing} m");
    }
}

#[test]
fn sharp_turns_and_u_turns_say_so() {
    let sharp = path(&[(0.0, 0.0), (0.0, 400.0), (300.0, 100.0)], 10.0);
    let kind = Kind::Sharp(Side::Right);
    assert_eq!(kinds(&ride(&sharp, 7.0)), [ahead(kind), now(kind)]);
    let back = path(&[(0.0, 0.0), (0.0, 400.0), (0.0, 100.0)], 10.0);
    assert_eq!(kinds(&ride(&back, 7.0)), [ahead(U_TURN), now(U_TURN)]);
}

#[test]
fn a_gentle_bend_says_nothing() {
    // 45 degrees right over 160 m, between straights.
    let line = join(&[
        path(&[(0.0, 0.0), (0.0, 300.0)], 10.0),
        arc((200.0, 300.0), 200.0, 270.0, 45.0, 5.0),
        path(&[(58.6, 441.4), (270.0, 653.0)], 10.0),
    ]);
    assert!(ride(&line, 7.0).is_empty());
}

#[test]
fn hairpins_wide_and_tight_are_u_turns_however_drawn() {
    for radius in [10.0, 20.0, 30.0] {
        for spacing in [3.0, 5.0, 8.0] {
            let said = ride(&hairpin(radius, spacing), 7.0);
            assert_eq!(
                kinds(&said),
                [ahead(U_TURN), now(U_TURN)],
                "r{radius} at {spacing} m: {said:?}"
            );
        }
    }
    let rough = path(&[(0.0, 0.0), (-10.0, 410.0), (-20.0, 0.0)], 50.0);
    assert_eq!(kinds(&ride(&rough, 7.0)), [ahead(U_TURN), now(U_TURN)]);
}

#[test]
fn a_roundabout_of_any_size_is_one_turn_the_way_it_leaves() {
    // Entering from the south, anticlockwise: the first exit is east, the
    // second north, the third west.
    for radius in [10.0, 15.0, 20.0, 30.0, 40.0] {
        for spacing in [3.0, 5.0] {
            let first = ride(&ring(radius, 90.0, spacing), 7.0);
            assert_eq!(
                kinds(&first),
                [ahead(RIGHT), now(RIGHT)],
                "r{radius}: {first:?}"
            );
            let third = ride(&ring(radius, 270.0, spacing), 7.0);
            assert_eq!(
                kinds(&third),
                [ahead(LEFT), now(LEFT)],
                "r{radius}: {third:?}"
            );
            let straight = ride(&ring(radius, 180.0, spacing), 7.0);
            assert!(straight.is_empty(), "r{radius}: {straight:?}");
            // Round to leave 30 degrees left of the way in: nothing, and
            // never a late word from partway round.
            let slight = ride(&ring(radius, 210.0, spacing), 7.0);
            assert!(slight.is_empty(), "r{radius}: {slight:?}");
        }
    }
}

#[test]
fn corners_close_together_are_named_together_or_one_by_one() {
    for spacing in [3.0, 5.0, 8.0] {
        // A jog 20 or 25 m across is no turn.
        for across in [20.0, 25.0] {
            let jog = ride(
                &corners(spacing, &[(400.0, false), (400.0 + across, true)]),
                7.0,
            );
            assert!(jog.is_empty(), "{spacing} m, {across} m: {jog:?}");
        }
        // 32 m apart: one warning names both.
        let pair = ride(&corners(spacing, &[(400.0, false), (432.0, true)]), 7.0);
        let both = Warning::Now {
            kind: LEFT,
            then: Some(RIGHT),
        };
        assert_eq!(kinds(&pair), [ahead(LEFT), both], "{spacing} m: {pair:?}");
        // 40 m apart: each its own, the second only at it.
        let apart = ride(&corners(spacing, &[(400.0, false), (440.0, true)]), 7.0);
        assert_eq!(
            kinds(&apart),
            [ahead(LEFT), now(LEFT), now(RIGHT)],
            "{spacing} m: {apart:?}"
        );
        let same = ride(&corners(spacing, &[(400.0, true), (440.0, true)]), 7.0);
        assert_eq!(
            kinds(&same),
            [ahead(RIGHT), now(RIGHT), now(RIGHT)],
            "{spacing} m: {same:?}"
        );
        // Six corners 40 m apart, right and left in turn.
        let turns: Vec<_> = (0..6)
            .map(|k| (400.0 + 40.0 * k as f32, k % 2 == 0))
            .collect();
        let said = ride(&corners(spacing, &turns), 7.0);
        let mut want = vec![ahead(RIGHT)];
        want.extend([RIGHT, LEFT, RIGHT, LEFT, RIGHT, LEFT].map(now));
        assert_eq!(kinds(&said), want, "{spacing} m: {said:?}");
        // Two far apart: each warned of ahead and at it.
        let block = ride(&corners(spacing, &[(400.0, false), (600.0, true)]), 7.0);
        assert_eq!(
            kinds(&block),
            [ahead(LEFT), now(LEFT), ahead(RIGHT), now(RIGHT)],
            "{spacing} m"
        );
    }
}

#[test]
fn a_jittery_track_says_only_its_corner() {
    for seed in 1..=8 {
        let mut rng = crate::testing::Rng::new(seed);
        let mut line = corners(5.0, &[(400.0, true)]);
        for point in &mut line[1..] {
            point.0 += rng.below(4_001) as f32 / 1000.0 - 2.0;
            point.1 += rng.below(4_001) as f32 / 1000.0 - 2.0;
        }
        let said = ride(&line, 7.0);
        assert_eq!(
            kinds(&said),
            [ahead(RIGHT), now(RIGHT)],
            "seed {seed}: {said:?}"
        );
    }
}

#[test]
fn riding_back_past_a_turn_and_on_again_warns_of_it_again() {
    let line = corners(5.0, &[(600.0, false), (900.0, true)]);
    let out = (0..=90).map(|k| k as f32 * 7.0);
    let back = (0..=60).map(|k| 630.0 - k as f32 * 7.0);
    let on = (0..=150).map(|k| 210.0 + k as f32 * 7.0);
    let said = ride_through(&line, out.chain(back).chain(on));
    assert_eq!(
        kinds(&said),
        [
            ahead(LEFT),
            now(LEFT),
            ahead(LEFT),
            now(LEFT),
            ahead(RIGHT),
            now(RIGHT)
        ],
        "{said:?}"
    );
}

#[test]
fn a_rider_inside_a_roundabout_hears_nothing_of_it() {
    let line = ring(20.0, 270.0, 3.0);
    // Starting to follow 20 m round the ring.
    let said = ride_through(&line, (0..60).map(|k| 320.0 + k as f32 * 7.0));
    assert!(said.is_empty(), "{said:?}");
}

/// `course`'s points in metres east and north of its start.
fn line_of(course: &Course) -> Vec<(f32, f32)> {
    let points = course.points();
    let scale = east_per_e7(points[0]);
    points
        .iter()
        .map(|&p| metres(points[0], p, scale))
        .collect()
}

#[test]
fn a_long_course_thinned_to_fit_warns_of_its_hairpins_as_drawn() {
    // 200 km meandering 25 m either side every 500 m, then ten switchbacks
    // of 300 m, a point every 5 m: far more than a course keeps.
    let mut road = Vec::new();
    for k in 0..40_000 {
        let north = k as f32 * 5.0;
        let east = 25.0 * libm::sinf(north / 500.0 * core::f32::consts::TAU);
        road.push((east, north));
    }
    let top = road.last().unwrap().1;
    let mut switchbacks = vec![road.clone()];
    for k in 0..10 {
        let (east, side) = (-20.0 * k as f32, if k % 2 == 0 { 1.0 } else { -1.0 });
        let (from, to) = if k % 2 == 0 {
            (top, top + 300.0)
        } else {
            (top + 300.0, top)
        };
        switchbacks.push(path(&[(east, from), (east, to)], 5.0));
        switchbacks.push(arc((east - 10.0, to), 10.0, 90.0, -180.0 * side, 2.0));
    }
    switchbacks.push(path(&[(-200.0, top), (-200.0, top + 300.0)], 5.0));
    let full = join(&switchbacks);
    let mut course = empty();
    let mut builder = Builder::new();
    builder.start(&mut course);
    for &(east, north) in &full {
        builder.push(&mut course, point_at(f64::from(east), f64::from(north)));
    }
    builder.finish(&mut course);
    let thinned = line_of(&course);
    assert!(
        builder.tolerance >= 16.0,
        "thinned hard: {}",
        builder.tolerance
    );
    assert!(thinned.len() * 20 < full.len(), "{} points", thinned.len());

    let want = [ahead(U_TURN), now(U_TURN)].repeat(10);
    let full_said = ride(&full, 7.0);
    assert_eq!(kinds(&full_said), want, "{full_said:?}");
    let thinned_said = ride(&thinned, 7.0);
    assert_eq!(kinds(&thinned_said), want, "{thinned_said:?}");
    // The thinned meander is shorter; from the first hairpin on they agree.
    let (fine_0, thin_0) = (full_said[0].0, thinned_said[0].0);
    for (fine, thin) in full_said.iter().zip(&thinned_said) {
        let (fine_m, thin_m) = (fine.0 - fine_0, thin.0 - thin_0);
        assert!(fine_m.abs_diff(thin_m) <= 30, "{fine:?} {thin:?}");
    }
}

#[test]
fn a_rider_past_the_end_or_on_a_tiny_course_hears_nothing() {
    let line = path(&[(0.0, 0.0), (0.0, 400.0), (300.0, 400.0)], 10.0);
    let at = |k: usize| line[k];
    let mut warner = Warner::default();
    let off_end = Place {
        next: line.len(),
        here: (300.0, 400.0),
        along_mm: 700_000,
    };
    assert_eq!(warner.step(line.len(), at, off_end), None);
    let start = Place {
        next: 0,
        here: (0.0, 0.0),
        along_mm: 0,
    };
    assert_eq!(warner.step(line.len(), at, start), None);
    let two = [(0.0, 0.0), (0.0, 0.0)];
    let place = Place {
        next: 1,
        here: (0.0, 0.0),
        along_mm: 0,
    };
    assert_eq!(warner.step(2, |k| two[k], place), None);
    // Points kilometres apart, and on top of one another.
    let far = [(0.0, 0.0), (0.0, 4.0e6), (4.0e6, 4.0e6), (4.0e6, 4.0e6)];
    let place = Place {
        next: 1,
        here: (0.0, 3.99996e6),
        along_mm: u32::MAX - 10,
    };
    assert_eq!(
        next(4, |k| far[k], place, 0, u32::MAX).and_then(|group| group.kind),
        Some(RIGHT)
    );
}

#[test]
fn a_staircase_is_cut_into_groups_of_bounded_length() {
    // 20 m north, 20 m east, forty times over, between straights.
    let mut corners = vec![(0.0, 0.0), (0.0, 300.0)];
    for k in 0..40 {
        let (east, north) = (20.0 * k as f32, 300.0 + 20.0 * k as f32);
        corners.extend([(east + 20.0, north), (east + 20.0, north + 20.0)]);
    }
    let top = *corners.last().unwrap();
    corners.push((top.0, top.1 + 300.0));
    let line = path(&corners, 5.0);
    let at = |k: usize| line[k];
    let mut groups = 0;
    for k in 0..40 {
        let place = place(&line, 250.0 + 40.0 * k as f32);
        if let Some(group) = next(line.len(), at, place, place.along_mm, u32::MAX) {
            assert!(group.end_mm - group.start_mm <= mm(SPAN_M), "{group:?}");
            groups += 1;
        }
    }
    assert!(groups > 0);
    // However the staircase is named, its warnings come one at a time.
    let said = ride(&line, 7.0);
    for pair in said.windows(2) {
        assert!(pair[1].0 > pair[0].0, "{said:?}");
    }
}

#[test]
fn thousands_of_points_in_one_spot_cost_a_bounded_scan() {
    let mut line = vec![(0.0, 0.0); 2_000];
    line.extend(path(&[(0.0, 0.0), (0.0, 400.0), (300.0, 400.0)], 10.0));
    let calls = core::cell::Cell::new(0u32);
    let at = |k: usize| {
        calls.set(calls.get() + 1);
        line[k]
    };
    let place = Place {
        next: 1,
        here: (0.0, 0.0),
        along_mm: 0,
    };
    assert_eq!(Warner::default().step(line.len(), at, place), None);
    let most = (2 * GROUPS * 2 * (WALK_POINTS + SAMPLES + 2)) as u32;
    assert!(calls.get() <= most, "{} reads", calls.get());
}

/// `line`, then `after` metres straight on from its end, as a course built
/// from a point every 5 m keeps it: thinned to fit when that is too many.
fn thinned(line: &[(f32, f32)], after: f32) -> (Vec<(f32, f32)>, f32) {
    let (a, b) = (line[line.len() - 2], line[line.len() - 1]);
    let length = distance(a, b);
    let far = (
        b.0 + (b.0 - a.0) / length * after,
        b.1 + (b.1 - a.1) / length * after,
    );
    let full = join(&[line.to_vec(), path(&[b, far], 5.0)]);
    let mut course = empty();
    let mut builder = Builder::new();
    builder.start(&mut course);
    for &(east, north) in &full {
        builder.push(&mut course, point_at(f64::from(east), f64::from(north)));
    }
    builder.finish(&mut course);
    (line_of(&course), builder.tolerance)
}

/// Ride `line` from its start to `to` metres, a fix every 7 m.
fn ride_to(line: &[(f32, f32)], to: f32) -> Vec<(u32, Warning)> {
    ride_through(line, (0..=(to / 7.0) as usize).map(|k| k as f32 * 7.0))
}

#[derive(Debug, PartialEq)]
enum Verdict {
    Right,
    Silent,
    Wrong,
}

/// Whether what was said names a turn of `degrees`, right positive, rightly:
/// every word the way it goes, as one turn or as two the same way.
fn judge(said: &[(u32, Warning)], degrees: f32) -> Verdict {
    let a = fabsf(degrees);
    let side = |s: Side| (s == Side::Right) == (degrees > 0.0);
    let fits = |kind: Kind| match kind {
        Kind::UTurn => a >= 135.0,
        Kind::Sharp(s) => side(s) && (100.0..=170.0).contains(&a),
        Kind::Turn(s) => side(s) && (55.0..=150.0).contains(&a),
    };
    let right = said.iter().all(|&(_, warning)| match warning {
        Warning::Ahead { kind, .. } => fits(kind),
        Warning::Now { kind, then } => fits(kind) && then.is_none_or(fits),
    });
    match said.is_empty() {
        true => Verdict::Silent,
        false if right => Verdict::Right,
        false => Verdict::Wrong,
    }
}

/// Every exit of a roundabout, and hairpins, on a course thinned to fit:
/// what is said is right, or nothing is.
#[test]
fn a_thinned_course_names_its_roundabouts_and_hairpins_rightly_or_not_at_all() {
    let (mut silent, mut runs) = (Vec::new(), 0);
    // 0 m on is the course as drawn; after 12 or 150 km it is thinned.
    for after in [0.0, 12_000.0, 150_000.0] {
        for spacing in [3.0, 5.0, 8.0] {
            for curved in [false, true] {
                for radius in [15.0, 20.0, 25.0, 30.0, 35.0, 40.0] {
                    for sweep in (3..=22).map(|k| k as f32 * 15.0) {
                        let drawn = match curved {
                            true => curved_ring(radius, sweep, spacing),
                            false => ring(radius, sweep, spacing),
                        };
                        let line = match after > 0.0 {
                            true => thinned(&drawn, after).0,
                            false => drawn,
                        };
                        let degrees = swing(0.0, 180.0 - sweep);
                        let said = ride_to(&line, 900.0);
                        let verdict = judge(&said, degrees);
                        assert!(
                            verdict != Verdict::Wrong,
                            "r{radius} exit {sweep} ({degrees}), {spacing} m, curved {curved}, after {after}: {said:?}"
                        );
                        runs += 1;
                        if verdict == Verdict::Silent && fabsf(degrees) >= 75.0 {
                            silent.push((after, spacing, curved, radius, sweep));
                        }
                    }
                }
            }
            if after == 0.0 {
                continue;
            }
            for radius in [20.0, 25.0, 30.0, 35.0, 40.0] {
                let (line, _) = thinned(&hairpin(radius, spacing), after);
                let said = ride_to(&line, 1_200.0);
                let twice = [
                    ahead(LEFT),
                    Warning::Now {
                        kind: LEFT,
                        then: Some(LEFT),
                    },
                ];
                let said = kinds(&said);
                assert!(
                    said == [ahead(U_TURN), now(U_TURN)] || said == twice,
                    "hairpin r{radius}, {spacing} m, after {after}: {said:?}"
                );
            }
        }
    }
    std::println!(
        "{} of {runs} silent where a turn of 75 degrees or more was due",
        silent.len()
    );
    assert!(silent.len() * 10 <= runs, "{silent:?}");
}

/// As `ring`, but the way in and out each curve onto the ring on a 10 m
/// radius, as roads into a roundabout do.
fn curved_ring(radius: f32, sweep: f32, spacing: f32) -> Vec<(f32, f32)> {
    let bend = 10.0;
    let step = spacing.min(radius / 3.0).min(bend / 3.0);
    let ring = arc((0.0, 0.0), radius, 180.0, -sweep, spacing.min(radius / 3.0));
    let exit = *ring.last().unwrap();
    // The exit's bearing from the centre, clockwise from north.
    let out = 180.0 - sweep;
    let (sin, cos) = (libm::sinf(out.to_radians()), libm::cosf(out.to_radians()));
    let turn = (exit.0 + bend * sin, exit.1 + bend * cos);
    let leave = arc(turn, bend, out + 180.0, 90.0, step);
    let off = *leave.last().unwrap();
    join(&[
        path(&[(-bend, -300.0), (-bend, -radius - bend)], spacing),
        arc((0.0, -radius - bend), bend, 270.0, 90.0, step),
        ring,
        leave,
        path(&[off, (off.0 + 300.0 * sin, off.1 + 300.0 * cos)], spacing),
    ])
}

/// What `said` names, shortly: R ahead, R, L THEN R.
fn words(said: &[(u32, Warning)]) -> std::string::String {
    let word = |kind: Kind| match kind {
        Kind::Turn(Side::Left) => "L",
        Kind::Turn(Side::Right) => "R",
        Kind::Sharp(Side::Left) => "SL",
        Kind::Sharp(Side::Right) => "SR",
        Kind::UTurn => "U",
    };
    let said: Vec<_> = said
        .iter()
        .map(|&(_, warning)| match warning {
            Warning::Ahead { kind, .. } => std::format!("{} ahead", word(kind)),
            Warning::Now { kind, then: None } => word(kind).into(),
            Warning::Now {
                kind,
                then: Some(then),
            } => std::format!("{} THEN {}", word(kind), word(then)),
        })
        .collect();
    if said.is_empty() {
        "nothing".into()
    } else {
        said.join(", ")
    }
}

/// The table in the pull request: what each course says, as Markdown rows.
/// `cargo test -p vana --lib cue_table -- --ignored --nocapture`
#[test]
#[ignore = "prints the cue table"]
fn cue_table() {
    let mut rows: Vec<(std::string::String, std::string::String)> = Vec::new();
    for spacing in [3.0, 5.0, 8.0] {
        let courses = [
            ("right turn", corners(spacing, &[(400.0, true)])),
            (
                "left, right 40 m on",
                corners(spacing, &[(400.0, false), (440.0, true)]),
            ),
            (
                "right, right 40 m on",
                corners(spacing, &[(400.0, true), (440.0, true)]),
            ),
            (
                "R L R L R L 40 m apart",
                corners(
                    spacing,
                    &(0..6)
                        .map(|k| (400.0 + 40.0 * k as f32, k % 2 == 0))
                        .collect::<Vec<_>>(),
                ),
            ),
            (
                "jog left, right 20 m on",
                corners(spacing, &[(400.0, false), (420.0, true)]),
            ),
            ("hairpin r10", hairpin(10.0, spacing)),
            ("hairpin r20", hairpin(20.0, spacing)),
        ];
        for (name, line) in courses {
            rows.push((
                std::format!("{name} ({spacing} m)"),
                words(&ride(&line, 7.0)),
            ));
        }
    }
    for radius in [10.0, 20.0, 30.0, 40.0] {
        let exits = [90.0, 180.0, 270.0].map(|sweep| words(&ride(&ring(radius, sweep, 3.0), 7.0)));
        rows.push((
            std::format!("roundabout r{radius}: first; straight; third exit"),
            exits.join("; "),
        ));
    }
    for after in [12_000.0, 150_000.0] {
        let km = after / 1000.0;
        for radius in [15.0, 20.0, 30.0, 40.0] {
            let exits = [90.0, 180.0, 270.0].map(|sweep| {
                words(&ride_to(
                    &thinned(&ring(radius, sweep, 5.0), after).0,
                    900.0,
                ))
            });
            rows.push((
                std::format!(
                    "thinned, {km} km on: roundabout r{radius}: first; straight; third exit"
                ),
                exits.join("; "),
            ));
        }
        for radius in [20.0, 30.0, 40.0] {
            let said = ride_to(&thinned(&hairpin(radius, 5.0), after).0, 1_200.0);
            rows.push((
                std::format!("thinned, {km} km on: hairpin r{radius}"),
                words(&said),
            ));
        }
    }
    let blocks = [
        (30.0, 0.0),
        (37.0, 0.0),
        (40.0, 4.0),
        (45.0, 8.0),
        (50.0, 8.0),
    ];
    for (apart, kerb) in blocks {
        for right in [true, false] {
            let way = if right { "right" } else { "left" };
            let full = block(5.0, apart, kerb, right);
            let thin = thinned(&full, 12_000.0).0;
            rows.push((
                std::format!("block, two {way}s {apart} m apart, kerb {kerb} m: drawn; thinned"),
                std::format!(
                    "{}; {}",
                    words(&ride_to(&full, 700.0)),
                    words(&ride_to(&thin, 700.0))
                ),
            ));
        }
    }
    for (name, said) in rows {
        std::println!("| {name} | {said} |");
    }
}

/// Two corners the same way, `apart` metres apart, each rounded to a kerb
/// of `kerb` metres: 400 m north, across, and 400 m back south beside the
/// way up. A city block, to the right, or to the left mirrored.
fn block(spacing: f32, apart: f32, kerb: f32, right: bool) -> Vec<(f32, f32)> {
    let line = if kerb == 0.0 {
        path(
            &[(0.0, 0.0), (0.0, 400.0), (apart, 400.0), (apart, 0.0)],
            spacing,
        )
    } else {
        let step = spacing.min(kerb / 2.0);
        join(&[
            path(&[(0.0, 0.0), (0.0, 400.0 - kerb)], spacing),
            arc((kerb, 400.0 - kerb), kerb, 270.0, 90.0, step),
            path(&[(kerb, 400.0), (apart - kerb, 400.0)], spacing),
            arc((apart - kerb, 400.0 - kerb), kerb, 0.0, 90.0, step),
            path(&[(apart, 400.0 - kerb), (apart, 0.0)], spacing),
        ])
    };
    if right {
        line
    } else {
        line.into_iter()
            .map(|(east, north)| (-east, north))
            .collect()
    }
}

/// A city block's two corners are named one by one or together, the way
/// they go, and never a U-TURN unless the way back is within 25 m of the
/// way out, where the rider is still on the course.
#[test]
fn a_city_block_is_two_turns_the_way_they_go_and_never_a_u_turn() {
    for thin in [false, true] {
        for right in [true, false] {
            for spacing in [3.0, 5.0, 8.0] {
                for kerb in [0.0, 2.0, 4.0, 6.0, 8.0] {
                    for apart in [
                        15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 60.0, 70.0, 80.0,
                    ] {
                        let full = block(spacing, apart, kerb, right);
                        let line = if thin {
                            thinned(&full, 12_000.0).0
                        } else {
                            full
                        };
                        let said = ride_to(&line, 700.0);
                        let side = if right { Side::Right } else { Side::Left };
                        let fits = |kind: Kind| match kind {
                            Kind::UTurn => apart <= 25.0,
                            Kind::Turn(s) | Kind::Sharp(s) => s == side,
                        };
                        let wrong = said.iter().any(|&(_, warning)| match warning {
                            Warning::Ahead { kind, .. } | Warning::Now { kind, then: None } => {
                                !fits(kind)
                            }
                            Warning::Now {
                                kind,
                                then: Some(then),
                            } => !fits(kind) || !fits(then),
                        });
                        assert!(
                            !wrong,
                            "{apart} m apart, kerb {kerb}, {spacing} m, right {right}, thin {thin}: {said:?}"
                        );
                    }
                }
            }
        }
    }
}
