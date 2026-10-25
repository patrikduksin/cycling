use super::*;
use crate::ride::course::tests::{at as point_at, empty};
use crate::ride::course::{Builder, Course, east_per_e7, metres};
use std::vec;
use std::vec::Vec;

/// Points every `step` metres along straight legs between `corners`.
pub(crate) fn path(corners: &[(f32, f32)], step: f32) -> Vec<(f32, f32)> {
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
pub(crate) fn arc(
    centre: (f32, f32),
    radius: f32,
    from: f32,
    sweep: f32,
    step: f32,
) -> Vec<(f32, f32)> {
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

pub(crate) fn join(parts: &[Vec<(f32, f32)>]) -> Vec<(f32, f32)> {
    let mut line: Vec<(f32, f32)> = Vec::new();
    for part in parts {
        let skip = usize::from(line.last() == part.first());
        line.extend_from_slice(&part[skip..]);
    }
    line
}

fn length(line: &[(f32, f32)]) -> f32 {
    line.windows(2).map(|pair| distance(pair[0], pair[1])).sum()
}

/// `line`'s turns, found as a placed course's are.
pub(crate) fn turns_of(line: &[(f32, f32)]) -> Vec<Turn> {
    let mut turns = vec![Turn::default(); TURNS];
    let at = |k: usize| line[k];
    let mut scan = Scan::default();
    let mut found = 0;
    while !scan.done() {
        found = scan.step(line.len(), at, SCAN_STEPS, &mut turns, found);
    }
    turns.truncate(found);
    turns
}

/// Ride `line` through `positions`, metres from its start, and say where
/// each warning came. Riding back is not riding the course its way, which
/// warns of nothing (`Homing`), so only the way on is asked.
fn ride_through(
    line: &[(f32, f32)],
    positions: impl IntoIterator<Item = f32>,
) -> Vec<(u32, Warning)> {
    ride_with(&turns_of(line), positions)
}

/// Ride a course of `turns` through `positions`, as `ride_through`.
pub(crate) fn ride_with(
    turns: &[Turn],
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
        if let Some(warning) = warner.step(turns, mm(metres)) {
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
pub(crate) fn ring(radius: f32, sweep: f32, spacing: f32) -> Vec<(f32, f32)> {
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
pub(crate) fn hairpin(radius: f32, spacing: f32) -> Vec<(f32, f32)> {
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
        // 20 to 32 m apart: one warning names both, the way they go.
        let both = Warning::Now {
            kind: LEFT,
            then: Some(RIGHT),
        };
        for across in [20.0, 25.0, 32.0] {
            let pair = ride(
                &corners(spacing, &[(400.0, false), (400.0 + across, true)]),
                7.0,
            );
            assert_eq!(
                kinds(&pair),
                [ahead(LEFT), both],
                "{spacing} m, {across} m: {pair:?}"
            );
        }
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
    builder.start(&mut course, &mut ());
    for &(east, north) in &full {
        builder.push(
            &mut course,
            point_at(f64::from(east), f64::from(north)),
            &mut (),
        );
    }
    builder.finish(&mut course, &mut ());
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
    let turns = turns_of(&line);
    let mut warner = Warner::default();
    assert_eq!(warner.step(&turns, 700_000), None);
    assert_eq!(Warner::default().step(&[], 0), None);
    assert!(turns_of(&[(0.0, 0.0), (0.0, 0.0)]).is_empty());
    assert!(turns_of(&[(0.0, 0.0)]).is_empty());
    // Points kilometres apart, and on top of one another.
    let far = [(0.0, 0.0), (0.0, 4.0e6), (4.0e6, 4.0e6), (4.0e6, 4.0e6)];
    let kinds: Vec<_> = turns_of(&far).iter().filter_map(|turn| turn.kind).collect();
    assert_eq!(kinds, [RIGHT]);
    // A rider beyond what a u32 of millimetres holds.
    assert_eq!(Warner::default().step(&turns_of(&far), u32::MAX), None);
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
    let turns = turns_of(&line);
    assert!(!turns.is_empty());
    for turn in &turns {
        assert!(turn.end_mm - turn.start_mm <= mm(SPAN_M), "{turn:?}");
    }
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
    let mut turns = vec![Turn::default(); TURNS];
    let mut scan = Scan::default();
    let found = scan.step(line.len(), at, SCAN_STEPS, &mut turns, 0);
    assert!(found <= 1);
    // Each point is passed once by each end of the chord, and its tail.
    let most = (8 * line.len() + 8 * SCAN_STEPS) as u32;
    assert!(calls.get() <= most, "{} reads", calls.get());
}

/// `line`, then `after` metres straight on from its end, as a course built
/// from a point every 5 m keeps it: thinned to fit when that is too many.
fn thinned(line: &[(f32, f32)], after: f32) -> (Vec<(f32, f32)>, f32) {
    let (course, tolerance) = built(line, after);
    (line_of(&course), tolerance)
}

/// The course `thinned` keeps, and the tolerance it was thinned at.
pub(crate) fn built(line: &[(f32, f32)], after: f32) -> (std::boxed::Box<Course>, f32) {
    build(&extended(line, after), &mut ())
}

/// `line`, then `after` metres straight on from its end, a point every 5 m.
pub(crate) fn extended(line: &[(f32, f32)], after: f32) -> Vec<(f32, f32)> {
    let (a, b) = (line[line.len() - 2], line[line.len() - 1]);
    let length = distance(a, b);
    let far = (
        b.0 + (b.0 - a.0) / length * after,
        b.1 + (b.1 - a.1) / length * after,
    );
    join(&[line.to_vec(), path(&[b, far], 5.0)])
}

/// The course built from `full`, followed by `follow`, and the tolerance
/// it was thinned at.
pub(crate) fn build(
    full: &[(f32, f32)],
    follow: &mut impl crate::ride::course::Follow,
) -> (std::boxed::Box<Course>, f32) {
    let mut course = empty();
    let mut builder = Builder::new();
    builder.start(&mut course, follow);
    for &(east, north) in full {
        builder.push(
            &mut course,
            point_at(f64::from(east), f64::from(north)),
            follow,
        );
    }
    builder.finish(&mut course, follow);
    (course, builder.tolerance)
}

/// How a course's turns are found: by the scan of the course as kept when
/// it is placed, or at upload from every point (`course::stored`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Found {
    Scanned,
    Uploaded,
}

pub(crate) const FOUND: [Found; 2] = [Found::Scanned, Found::Uploaded];

/// What a rider hears on `drawn`, then `after` metres on, uploaded and
/// kept as `built` keeps it, ridden from its start to `to` metres with a
/// fix every 7 m. A course `after` 0 m on is scanned as drawn.
pub(crate) fn heard(
    drawn: &[(f32, f32)],
    after: f32,
    found: Found,
    to: f32,
) -> Vec<(u32, Warning)> {
    let positions = (0..=(to / 7.0) as usize).map(|k| k as f32 * 7.0);
    match found {
        Found::Scanned if after > 0.0 => ride_through(&thinned(drawn, after).0, positions),
        Found::Scanned => ride_through(drawn, positions),
        Found::Uploaded => {
            let (_, turns) = crate::ride::course::stored::tests::uploaded(&extended(drawn, after));
            ride_with(&turns, positions)
        }
    }
}

/// Ride `line` from its start to `to` metres, a fix every 7 m.
fn ride_to(line: &[(f32, f32)], to: f32) -> Vec<(u32, Warning)> {
    ride_through(line, (0..=(to / 7.0) as usize).map(|k| k as f32 * 7.0))
}

#[derive(Debug, PartialEq)]
pub(crate) enum Verdict {
    Right,
    Silent,
    Wrong,
}

/// Whether what was said names a turn of `degrees`, right positive, rightly:
/// every word the way it goes, as one turn or as two the same way.
pub(crate) fn judge(said: &[(u32, Warning)], degrees: f32) -> Verdict {
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
/// what is said is right, or nothing is. Turns found at upload name every
/// hairpin a U-turn.
#[test]
fn a_thinned_course_names_its_roundabouts_and_hairpins_rightly_or_not_at_all() {
    for found in FOUND {
        let (mut silent, mut runs, mut u_turns, mut hairpins) = (Vec::new(), 0, 0, 0);
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
                            let degrees = swing(0.0, 180.0 - sweep);
                            let said = heard(&drawn, after, found, 900.0);
                            let verdict = judge(&said, degrees);
                            assert!(
                                verdict != Verdict::Wrong,
                                "{found:?}: r{radius} exit {sweep} ({degrees}), {spacing} m, curved {curved}, after {after}: {said:?}"
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
                    let said = kinds(&heard(&hairpin(radius, spacing), after, found, 1_200.0));
                    let twice = [
                        ahead(LEFT),
                        Warning::Now {
                            kind: LEFT,
                            then: Some(LEFT),
                        },
                    ];
                    let u_turn = said == [ahead(U_TURN), now(U_TURN)];
                    assert!(
                        u_turn || (said == twice && found == Found::Scanned),
                        "{found:?}: hairpin r{radius}, {spacing} m, after {after}: {said:?}"
                    );
                    (hairpins, u_turns) = (hairpins + 1, u_turns + usize::from(u_turn));
                }
            }
        }
        std::println!(
            "{found:?}: {} of {runs} silent where a turn of 75 degrees or more was due; {u_turns} of {hairpins} thinned hairpins U-TURN",
            silent.len()
        );
        let most = match found {
            Found::Scanned => 15,
            Found::Uploaded => 5,
        };
        assert!(silent.len() * 100 <= runs * most, "{found:?}: {silent:?}");
    }
}

/// As `ring`, but the way in and out each curve onto the ring on a 10 m
/// radius, as roads into a roundabout do.
pub(crate) fn curved_ring(radius: f32, sweep: f32, spacing: f32) -> Vec<(f32, f32)> {
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
    for apart in [25.0, 40.0, 60.0] {
        let rll = [
            (400.0, true),
            (400.0 + apart, false),
            (400.0 + 2.0 * apart, false),
        ];
        let lr = [(400.0, false), (400.0 + apart, true)];
        for (name, turns) in [("R L L", &rll[..]), ("L R", &lr[..])] {
            let route = words(&ride_to(&corners(1_000.0, turns), 700.0));
            let thin = words(&ride_to(&thinned(&corners(5.0, turns), 12_000.0).0, 700.0));
            rows.push((
                std::format!("{name} {apart} m apart: planner route; thinned, 12 km on"),
                std::format!("{route}; {thin}"),
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
/// The table in the pull request for turns found at upload: what each
/// thinned course says with its turns scanned when placed, and found at
/// upload. `cargo test -p vana --lib --release upload_table -- --ignored --nocapture`
#[test]
#[ignore = "prints the upload table"]
fn upload_table() {
    type Row = (std::string::String, Vec<(f32, f32)>, f32, f32);
    let mut rows: Vec<Row> = Vec::new();
    for after in [12_000.0, 150_000.0] {
        let km = after / 1000.0;
        for radius in [15.0, 20.0, 30.0, 40.0] {
            for sweep in [90.0, 270.0, 330.0] {
                rows.push((
                    std::format!("{km} km on: roundabout r{radius}, exit at {sweep} degrees"),
                    ring(radius, sweep, 5.0),
                    after,
                    900.0,
                ));
            }
        }
        for radius in [20.0, 30.0, 40.0] {
            rows.push((
                std::format!("{km} km on: hairpin r{radius}"),
                hairpin(radius, 5.0),
                after,
                1_200.0,
            ));
        }
        for apart in [25.0, 40.0] {
            let rll = [
                (400.0, true),
                (400.0 + apart, false),
                (400.0 + 2.0 * apart, false),
            ];
            rows.push((
                std::format!("{km} km on: R, L, L {apart} m apart"),
                corners(5.0, &rll),
                after,
                700.0,
            ));
        }
    }
    for (name, drawn, after, to) in rows {
        let [scanned, uploaded] = FOUND.map(|found| words(&heard(&drawn, after, found, to)));
        std::println!("| {name} | {scanned} | {uploaded} |");
    }
}

/// way up. A city block, to the right, or to the left mirrored.
pub(crate) fn block(spacing: f32, apart: f32, kerb: f32, right: bool) -> Vec<(f32, f32)> {
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
    for found in FOUND {
        let (mut runs, mut silent) = (0, 0);
        // 0 km on is the course as drawn; after 12 or 150 km it is thinned.
        for after in [0.0, 12_000.0, 150_000.0] {
            for right in [true, false] {
                for spacing in [3.0, 5.0, 8.0] {
                    for kerb in [0.0, 2.0, 4.0, 6.0, 8.0] {
                        for apart in [
                            15.0, 20.0, 25.0, 30.0, 35.0, 40.0, 45.0, 50.0, 60.0, 70.0, 80.0,
                        ] {
                            let said =
                                heard(&block(spacing, apart, kerb, right), after, found, 700.0);
                            runs += 1;
                            silent += usize::from(said.is_empty());
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
                                "{found:?}: {apart} m apart, kerb {kerb}, {spacing} m, right {right}, after {after}: {said:?}"
                            );
                        }
                    }
                }
            }
        }
        std::println!("{found:?}: 0 of {runs} wrong, {silent} silent");
    }
}

/// Whether each warning in `said` names the corners of `turns` (metres
/// along, right) the rider has yet to reach, more than 9 m on (nearer, they
/// are turning it already): the next one, and with THEN
/// the one after; a U-TURN only for two the same way within 25 m.
fn names_next_corners(said: &[(u32, Warning)], turns: &[(f32, f32)]) -> bool {
    let side = |degrees: f32| {
        if degrees > 0.0 {
            Side::Right
        } else {
            Side::Left
        }
    };
    let fits = |kind: Kind, at: usize| match kind {
        Kind::Turn(s) | Kind::Sharp(s) => turns.get(at).is_some_and(|t| side(t.1) == s),
        Kind::UTurn => match (turns.get(at), turns.get(at + 1)) {
            (Some(a), Some(b)) => side(a.1) == side(b.1) && b.0 - a.0 <= 25.0,
            _ => false,
        },
    };
    said.iter().all(|&(metres, warning)| {
        let at = turns
            .iter()
            .position(|t| t.0 > metres as f32 + 9.0)
            .unwrap_or(turns.len());
        match warning {
            Warning::Ahead { kind, .. } | Warning::Now { kind, then: None } => fits(kind, at),
            Warning::Now {
                kind,
                then: Some(then),
            } => fits(kind, at) && fits(then, at + if kind == Kind::UTurn { 2 } else { 1 }),
        }
    })
}

/// A road north, turning by the given degrees (right positive) at each
/// distance along it, then 400 m on; a point every `spacing` metres.
pub(crate) fn bends(spacing: f32, turns: &[(f32, f32)]) -> Vec<(f32, f32)> {
    let (mut heading, mut done) = (0.0f32, 0.0);
    let mut points = vec![(0.0f32, 0.0f32)];
    let end = turns.last().map_or(800.0, |turn| turn.0 + 400.0);
    for &(at, degrees) in turns.iter().chain([(end, 0.0)].iter()) {
        let radians = heading.to_radians();
        let p = *points.last().unwrap();
        let leg = at - done;
        points.push((
            p.0 + leg * libm::sinf(radians),
            p.1 + leg * libm::cosf(radians),
        ));
        done = at;
        heading += degrees;
    }
    path(&points, spacing)
}

/// Whether each course of `turns` (metres along, degrees), drawn every
/// `spacings` metres, as drawn and thinned 12 and 150 km on, names its
/// corners rightly: the wrong runs, and how many were silent, of how many.
fn ride_corners(
    courses: &[Vec<(f32, f32)>],
    spacings: &[f32],
    found: Found,
) -> (Vec<std::string::String>, usize, usize) {
    let (mut runs, mut silent, mut wrong) = (0, 0, Vec::new());
    for turns in courses {
        for &spacing in spacings {
            let full = bends(spacing, turns);
            for after in [0.0, 12_000.0, 150_000.0] {
                let said = heard(&full, after, found, 800.0);
                if !names_next_corners(&said, turns) {
                    wrong.push(std::format!(
                        "{found:?}: {turns:?}, {spacing} m, {after} m on: {said:?}"
                    ));
                }
                runs += 1;
                silent += usize::from(said.is_empty());
            }
        }
    }
    (wrong, silent, runs)
}

/// Two to four street corners of either way, 15 to 80 m apart, drawn every
/// 3, 5 or 8 m or only at the corners (a planner's route), as drawn and
/// thinned: every warning names the next corner's way, or there is none.
#[test]
fn corners_of_either_way_close_together_are_named_the_way_they_go() {
    let mut courses = Vec::new();
    for count in 2..=4usize {
        for ways in 0..1u32 << count {
            for apart in [15.0, 20.0, 25.0, 30.0, 40.0, 50.0, 60.0, 80.0] {
                courses.push(
                    (0..count)
                        .map(|k| {
                            let right = ways >> k & 1 == 1;
                            (400.0 + apart * k as f32, if right { 90.0 } else { -90.0 })
                        })
                        .collect(),
                );
            }
        }
    }
    for found in FOUND {
        let (wrong, silent, runs) = ride_corners(&courses, &[3.0, 5.0, 8.0, 1_000.0], found);
        std::println!(
            "{found:?}: {} of {runs} wrong, {silent} silent",
            wrong.len()
        );
        assert!(wrong.is_empty(), "{:#?}", &wrong[..wrong.len().min(8)]);
    }
}

/// The shapes a review found named the wrong way: a right, a left and two
/// rights 20 m apart; a staggered junction, a left and a right 15 m apart,
/// then a right; and a chicane of 75 degree corners 30 m apart.
#[test]
fn staggered_junctions_and_chicanes_are_named_rightly_or_not_at_all() {
    let mut courses = vec![
        vec![(400.0, 90.0), (420.0, -90.0), (440.0, 90.0), (460.0, 90.0)],
        vec![
            (400.0, -90.0),
            (420.0, 90.0),
            (440.0, -90.0),
            (460.0, -90.0),
        ],
    ];
    for then in [30.0, 40.0, 50.0, 60.0] {
        for way in [1.0, -1.0] {
            courses.push(vec![
                (400.0, -90.0 * way),
                (415.0, 90.0 * way),
                (415.0 + then, 90.0 * way),
            ]);
        }
    }
    for way in [1.0, -1.0] {
        courses.push(
            [1.0, -1.0, -1.0, 1.0]
                .iter()
                .enumerate()
                .map(|(k, side)| (400.0 + 30.0 * k as f32, 75.0 * side * way))
                .collect(),
        );
    }
    for found in FOUND {
        let (wrong, silent, runs) = ride_corners(&courses, &[3.0, 5.0, 8.0, 1_000.0], found);
        std::println!(
            "{found:?}: {} of {runs} wrong, {silent} silent",
            wrong.len()
        );
        assert!(wrong.is_empty(), "{:#?}", &wrong[..wrong.len().min(8)]);
    }
}

/// How long finding a 300 km course's turns takes, and how many it keeps:
/// `cargo test --release -p vana --lib scan_time -- --ignored --nocapture`
#[test]
#[ignore = "a measurement"]
fn scan_time() {
    // 300 km of roads, a corner either way every kilometre.
    let mut turns = Vec::new();
    let mut rng = crate::testing::Rng::new(9);
    for k in 0..300 {
        let way = if rng.below(2) == 0 { 90.0 } else { -90.0 };
        turns.push((400.0 + 1_000.0 * k as f32, way));
    }
    let (course, tolerance) = built(&bends(5.0, &turns), 0.1);
    let line = line_of(&course);
    let length = length(&line);
    let start = std::time::Instant::now();
    let found = turns_of(&line);
    let took = start.elapsed();
    std::println!(
        "{:.0} km in {} points (thinned at {tolerance} m): {} turns kept in {took:?}, {} steps",
        length / 1000.0,
        line.len(),
        found.len(),
        (length / STEP_M) as u64
    );
    std::println!(
        "a turn is {} bytes; {TURNS} take {} bytes",
        core::mem::size_of::<Turn>(),
        TURNS * core::mem::size_of::<Turn>()
    );
}

/// The point `metres` along `line`.
fn point_along(line: &[(f32, f32)], metres: f32) -> (f32, f32) {
    let mut walked = 0.0;
    for pair in line.windows(2) {
        let step = distance(pair[0], pair[1]);
        if walked + step >= metres && step > 0.0 {
            let t = (metres - walked) / step;
            return (
                pair[0].0 + t * (pair[1].0 - pair[0].0),
                pair[0].1 + t * (pair[1].1 - pair[0].1),
            );
        }
        walked += step;
    }
    *line.last().unwrap()
}

/// Metres along `line` to the point of it nearest `p`.
fn along_to(line: &[(f32, f32)], p: (f32, f32)) -> f32 {
    let (mut best, mut walked) = ((f32::MAX, 0.0), 0.0);
    for pair in line.windows(2) {
        let step = distance(pair[0], pair[1]);
        let off = crate::ride::thin::off_segment(p, pair[0], pair[1]);
        if step > 0.0 && off < best.0 {
            let t = (((p.0 - pair[0].0) * (pair[1].0 - pair[0].0)
                + (p.1 - pair[0].1) * (pair[1].1 - pair[0].1))
                / (step * step))
                .clamp(0.0, 1.0);
            best = (off, walked + t * step);
        }
        walked += step;
    }
    best.1
}

/// A recorded track as a course: each point of `full` up to `noise`
/// metres off, east and north, as GPS puts it.
fn jittered(full: &[(f32, f32)], noise: f32, seed: u64) -> Vec<(f32, f32)> {
    let mut rng = crate::testing::Rng::new(seed);
    let span = (2.0 * noise * 1000.0) as u64;
    let mut jitter = || rng.below(span + 1) as f32 / 1000.0 - noise;
    full.iter()
        .map(|&(east, north)| (east + jitter(), north + jitter()))
        .collect()
}

/// Wrong runs and silent runs, of how many, on recorded tracks of corners,
/// roundabouts and a hairpin up to `noise` metres off, as drawn and 12 km
/// on, with their turns `found` as each path finds them. A run is wrong
/// when a word sends the rider the wrong way: the wrong side, the wrong
/// corner next, or a U-TURN that is not one.
fn ride_jittered(noise: f32, found: Found) -> (Vec<std::string::String>, usize, usize) {
    enum Judged {
        Corners(Vec<(f32, f32)>),
        Turn(f32),
    }
    let mut shapes: Vec<(Vec<(f32, f32)>, Judged)> = Vec::new();
    for spacing in [3.0, 5.0, 8.0] {
        for turns in [
            vec![(400.0, 90.0)],
            vec![(400.0, -90.0)],
            vec![(400.0, 90.0), (440.0, -90.0)],
        ] {
            shapes.push((bends(spacing, &turns), Judged::Corners(turns)));
        }
        for (radius, sweep) in [(20.0, 90.0), (20.0, 270.0), (30.0, 90.0), (30.0, 270.0)] {
            let degrees = swing(0.0, 180.0 - sweep);
            shapes.push((ring(radius, sweep, spacing), Judged::Turn(degrees)));
        }
        shapes.push((hairpin(30.0, spacing), Judged::Turn(-180.0)));
    }
    let (mut runs, mut silent, mut wrong) = (0, 0, Vec::new());
    for (k, (drawn, judged)) in shapes.iter().enumerate() {
        for after in [0.0, 12_000.0] {
            for seed in 0..15u64 {
                let full = jittered(&extended(drawn, after), noise, seed * 100 + k as u64 + 1);
                let positions = (0..=(1_200.0 / 7.0) as usize).map(|k| k as f32 * 7.0);
                let (line, said) = match found {
                    Found::Scanned => {
                        let line = line_of(&build(&full, &mut ()).0);
                        let said = ride_through(&line, positions);
                        (line, said)
                    }
                    Found::Uploaded => {
                        let (course, turns) = crate::ride::course::stored::tests::uploaded(&full);
                        (line_of(&course), ride_with(&turns, positions))
                    }
                };
                let right = match judged {
                    Judged::Corners(turns) => {
                        // Where each corner is along the course as kept, which
                        // jitter makes longer than the road.
                        let road = bends(1.0, turns);
                        let kept: Vec<(f32, f32)> = turns
                            .iter()
                            .map(|&(at, degrees)| {
                                (along_to(&line, point_along(&road, at)), degrees)
                            })
                            .collect();
                        names_next_corners(&said, &kept)
                    }
                    Judged::Turn(degrees) => said.iter().all(|&(_, warning)| {
                        let fits = |kind: Kind| match kind {
                            Kind::UTurn => fabsf(*degrees) >= 135.0,
                            Kind::Turn(side) | Kind::Sharp(side) => {
                                (side == Side::Right) == (*degrees > 0.0)
                            }
                        };
                        match warning {
                            Warning::Ahead { kind, .. } => fits(kind),
                            Warning::Now { kind, then } => fits(kind) && then.is_none_or(fits),
                        }
                    }),
                };
                if !right {
                    wrong.push(std::format!(
                        "{found:?} ±{noise} m, shape {k}, after {after}, seed {seed}: {} {:?}",
                        words(&said),
                        said.iter().map(|s| s.0).collect::<Vec<_>>()
                    ));
                }
                runs += 1;
                silent += usize::from(said.is_empty());
            }
        }
    }
    (wrong, silent, runs)
}

/// Recorded tracks as courses, up to 1 to 4 m off: turns found at upload
/// are wrong no more often than the scan of the course as kept, and never
/// within 2 m.
#[test]
fn a_recorded_track_names_its_turns_no_worse_than_the_course_as_kept() {
    for noise in [1.0, 2.0, 3.0, 4.0] {
        let [scanned, uploaded] = FOUND.map(|found| ride_jittered(noise, found));
        std::println!(
            "±{noise} m: scanned {} wrong, {} silent; uploaded {} wrong, {} silent; of {} runs",
            scanned.0.len(),
            scanned.1,
            uploaded.0.len(),
            uploaded.1,
            uploaded.2
        );
        let first = |wrong: &[std::string::String]| wrong[..wrong.len().min(8)].to_vec();
        // The seeds are fixed, but at ±3 m and more both paths are wrong in a
        // few percent of runs, and which runs is chance: a margin of 1% of
        // the runs keeps a draw that is level from failing.
        let margin = uploaded.2 / 100;
        assert!(
            uploaded.0.len() <= scanned.0.len() + margin,
            "{:#?}",
            first(&uploaded.0)
        );
        if noise <= 2.0 {
            assert!(uploaded.0.is_empty(), "{:#?}", first(&uploaded.0));
        }
    }
}

/// Drawn courses with a point every 3.0 to 4.5 m: no corner is cut, so two
/// corners the same way are never one U-TURN, on city blocks and on mixed
/// corners, and the shapes a review found.
#[test]
fn corners_drawn_every_3_to_4_5_m_are_never_cut() {
    let spacings: Vec<f32> = (30..=45).map(|k| k as f32 / 10.0).collect();
    let mut wrong = Vec::new();
    for &spacing in &spacings {
        for right in [true, false] {
            for kerb in [0.0, 4.0] {
                for apart in [15.0, 20.0, 25.0, 30.0, 40.0, 60.0] {
                    let said = heard(
                        &block(spacing, apart, kerb, right),
                        0.0,
                        Found::Uploaded,
                        700.0,
                    );
                    let side = if right { Side::Right } else { Side::Left };
                    let fits = |kind: Kind| match kind {
                        Kind::UTurn => apart <= 25.0 && kerb == 0.0 || apart <= 20.0,
                        Kind::Turn(s) | Kind::Sharp(s) => s == side,
                    };
                    let bad = said.iter().any(|&(_, warning)| match warning {
                        Warning::Ahead { kind, .. } | Warning::Now { kind, then: None } => {
                            !fits(kind)
                        }
                        Warning::Now {
                            kind,
                            then: Some(then),
                        } => !fits(kind) || !fits(then),
                    });
                    if bad {
                        wrong.push(std::format!(
                            "block {apart} m, kerb {kerb}, {spacing} m, right {right}: {}",
                            words(&said)
                        ));
                    }
                }
            }
        }
    }
    let mut courses = vec![
        vec![(400.0, 90.0), (425.0, 90.0), (450.0, 90.0)],
        vec![(400.0, -90.0), (415.0, 90.0), (440.0, 90.0)],
        vec![(400.0, -90.0), (415.0, 90.0), (445.0, 90.0)],
    ];
    for ways in 0..8u32 {
        for apart in [20.0, 25.0, 30.0] {
            courses.push(
                (0..3)
                    .map(|k| {
                        (
                            400.0 + apart * k as f32,
                            if ways >> k & 1 == 1 { 90.0 } else { -90.0 },
                        )
                    })
                    .collect(),
            );
        }
    }
    for turns in &courses {
        for &spacing in &spacings {
            let said = heard(&bends(spacing, turns), 0.0, Found::Uploaded, 800.0);
            // A U-TURN only where the way back is within 21 m of the way out.
            let wide = turns.windows(2).all(|pair| pair[1].0 - pair[0].0 > 21.0);
            let u_turn = said.iter().any(|&(_, warning)| match warning {
                Warning::Ahead { kind, .. } => kind == Kind::UTurn,
                Warning::Now { kind, then } => kind == Kind::UTurn || then == Some(Kind::UTurn),
            });
            if !names_next_corners(&said, turns) || (wide && u_turn) {
                wrong.push(std::format!("{turns:?}, {spacing} m: {}", words(&said)));
            }
        }
    }
    assert!(
        wrong.is_empty(),
        "{} wrong: {:#?}",
        wrong.len(),
        &wrong[..wrong.len().min(8)]
    );
}

/// A roundabout drawn as a polygon of `side` m sides, the ways in and out
/// drawn only at their ends, as a coarse planner's route may draw one.
fn coarse_ring(radius: f32, sweep: f32, side: f32) -> Vec<(f32, f32)> {
    let ring = arc((0.0, 0.0), radius, 180.0, -sweep, side);
    let out = *ring.last().unwrap();
    let away = (out.0 / radius * 300.0, out.1 / radius * 300.0);
    join(&[vec![(0.0, -300.0), (0.0, -radius)], ring, vec![out, away]])
}

/// Every exit of a roundabout drawn with sides of 20 to 40 m, as drawn and
/// 12 km on: a polygon of four sides is a city block's shape, so what is
/// said is right or nothing is.
#[test]
fn a_roundabout_drawn_coarsely_never_names_a_wrong_way() {
    for found in FOUND {
        let (mut wrong, mut silent, mut runs) = (Vec::new(), 0, 0);
        for side in [20.0, 25.0, 30.0, 40.0] {
            for radius in [15.0, 20.0, 25.0, 30.0, 35.0, 40.0] {
                for sweep in (3..=22).map(|k| k as f32 * 15.0) {
                    let drawn = coarse_ring(radius, sweep, side);
                    let degrees = swing(0.0, 180.0 - sweep);
                    for after in [0.0, 12_000.0] {
                        let said = heard(&drawn, after, found, 900.0);
                        runs += 1;
                        match judge(&said, degrees) {
                            Verdict::Wrong => wrong.push(std::format!(
                                "{found:?}: side {side}, r{radius} exit {sweep}, after {after}: {said:?}"
                            )),
                            Verdict::Silent => silent += 1,
                            Verdict::Right => {}
                        }
                    }
                }
            }
        }
        std::println!(
            "{found:?}: {} of {runs} wrong, {silent} silent",
            wrong.len()
        );
        assert!(wrong.is_empty(), "{:#?}", &wrong[..wrong.len().min(8)]);
    }
}
