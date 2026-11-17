//! Courses with more turns than the list holds.
use super::turn_list::{TURN_LIST, View};
use super::*;
use crate::ride::course::nav::tests::said as told;
use crate::ride::course::stored::tests::found;
use crate::ride::course::stored::tests::uploaded;
use crate::ride::course::turns::tests::bends;
use crate::ride::course::turns::{Kind, Side, TURNS, Turn};

/// `count` corners 100 m apart, right then left in turn, drawn only at
/// the corners as a planner's route is.
fn zigzag(count: usize) -> Vec<(f32, f32)> {
    zigzag_apart(count, 100.0)
}

fn zigzag_apart(count: usize, apart: f32) -> Vec<(f32, f32)> {
    let corners: Vec<(f32, f32)> = (0..count)
        .map(|k| {
            (
                400.0 + apart * k as f32,
                if k % 2 == 0 { 90.0 } else { -90.0 },
            )
        })
        .collect();
    bends(1_000.0, &corners)
}

/// The words at each corner riding all of `line` with a fix every 7 m,
/// through a list of `room` turns, with the first `stored` turns found at
/// upload when given and the course flagged as having more.
fn at_corners(line: &[(f32, f32)], room: usize, stored: Option<usize>) -> Vec<Kind> {
    let (course, turns, _) = found(line);
    let points = course.points();
    let (mut crumbs, mut kept) = (vec![Crumb::default(); 64], vec![Crumb::default(); 2048]);
    let mut list = vec![Turn::default(); room];
    let mut rider = Rider::new(&mut crumbs, &mut kept, &mut list);
    rider.set_course(points);
    let length: f32 = line.windows(2).map(|w| distance(w[0], w[1])).sum();
    let mut now = 0;
    let mut said = Vec::new();
    for k in 0..(length / 7.0) as usize {
        let (east, north) = along_line(line, k as f32 * 7.0);
        now += 1_000;
        let event = rider.observe(Some(at(east, north)), now);
        said.extend(event.and_then(|event| told(event, rider.sign(now))));
        if rider.needs_course() {
            match stored {
                Some(count) => {
                    let count = count.min(turns.len());
                    rider.place(points, &mut super::stored(&turns[..count], true));
                }
                None => rider.set_course(points),
            }
        }
    }
    said.into_iter()
        .filter_map(|said| match said {
            Said::Now(kind) => Some(kind),
            Said::Soon(_) => None,
        })
        .collect()
}

fn right_left(count: usize) -> Vec<Kind> {
    (0..count)
        .map(|k| match k % 2 {
            0 => Kind::Turn(Side::Right),
            _ => Kind::Turn(Side::Left),
        })
        .collect()
}

/// 1,500 corners: the scan of the placed course waits while the list is
/// full and goes on as the rider passes the turns, so every corner is
/// named at it, past the 1,024th too.
#[test]
fn a_course_with_more_turns_than_the_list_names_every_one() {
    assert_eq!(at_corners(&zigzag(1_500), TURNS, None), right_left(1_500));
    assert_eq!(at_corners(&zigzag(60), 16, None), right_left(60));
}

/// Found at upload, a list that filled holds the first turns, and the scan
/// of the placed course names those after the last of them.
#[test]
fn turns_stored_from_a_full_list_are_followed_by_the_scan() {
    let (_, turns, upload) = found(&zigzag(1_500));
    assert!(upload.full && turns.len() == TURNS, "{upload:?}");
    assert_eq!(
        at_corners(&zigzag(1_500), TURNS, Some(TURNS)),
        right_left(1_500)
    );
    assert_eq!(at_corners(&zigzag(60), 16, Some(16)), right_left(60));
}

/// The first corner after the stored ones, 60 m on, is named too.
#[test]
fn the_corner_just_after_the_stored_turns_is_named() {
    for apart in [60.0, 80.0] {
        let line = zigzag_apart(33, apart);
        assert_eq!(
            at_corners(&line, 16, Some(16)),
            right_left(33),
            "{apart} m apart"
        );
    }
}

#[test]
fn compacting_the_turn_list_keeps_its_order_and_indices() {
    let corners: Vec<(f32, f32)> = (0..60)
        .map(|k| {
            (
                400.0 + 100.0 * k as f32,
                if k % 2 == 0 { 90.0 } else { -90.0 },
            )
        })
        .collect();
    let line = bends(1_000.0, &corners);
    let (course, all) = uploaded(&line);
    for (room, stored) in [(16, None), (16, Some(16)), (TURNS, None)] {
        let (mut crumbs, mut kept) = (vec![Crumb::default(); 64], vec![Crumb::default(); 2048]);
        let mut list = vec![Turn::default(); room];
        let mut rider = Rider::new(&mut crumbs, &mut kept, &mut list);
        rider.set_course(course.points());
        let length: f32 = line.windows(2).map(|w| distance(w[0], w[1])).sum();
        let (mut now, mut waited, mut compacted) = (0, false, false);
        for k in 0..(length / 7.0) as usize {
            let first = (rider.follower.turn_count > 0).then(|| rider.follower.turns[0].start_mm);
            let (east, north) = along_line(&line, k as f32 * 7.0);
            now += 1_000;
            rider.observe(Some(at(east, north)), now);
            waited |= rider.follower.scan.waiting();
            compacted |= first.is_some_and(|first| rider.follower.turns[0].start_mm > first);
            if rider.needs_course() {
                match stored {
                    Some(count) => {
                        rider.place(course.points(), &mut super::stored(&all[..count], true))
                    }
                    None => rider.set_course(course.points()),
                }
            }
            let view = View::of(&rider.follower);
            for (rule, keeps) in TURN_LIST {
                assert!(keeps(&view), "{rule} broken at {k} in a room of {room}");
            }
        }
        assert!(rider.follower.scan.done());
        if room < TURNS {
            assert!(waited && compacted, "room {room}: {waited} {compacted}");
        }
    }
}
