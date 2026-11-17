use super::*;
use crate::ride::course::Course;
use crate::ride::course::stored::tests::uploaded;
use crate::ride::course::turns::tests::{arc, join};
use std::boxed::Box;
use std::sync::OnceLock;

const STEP_M: f32 = 10.0;
pub(super) const OFFSETS_M: [f32; 3] = [0.0, 30.0, 60.0];
/// The scan is finished from the start, so nothing compacts the list;
/// `TURNS` is covered by the compaction test.
pub(super) const ROOM: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Size {
    /// #399's courses.
    Issue,
    /// Shrunk until the search closes.
    Short,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Shape {
    Straight,
    L,
    Hairpin,
}

impl Shape {
    pub(super) const ALL: [Shape; 3] = [Shape::Straight, Shape::L, Shape::Hairpin];
}

pub(super) struct Fixture {
    pub(super) name: &'static str,
    pub(super) course: Box<Course>,
    pub(super) stored: Vec<Turn>,
    pub(super) placed: Vec<Crumb>,
    pub(super) lattice: Vec<[LatticeSpot; 3]>,
}

#[derive(Clone, Copy)]
pub(super) struct LatticeSpot {
    pub(super) at: (f32, f32),
    pub(super) course_bearing: u16,
}

impl Fixture {
    fn new(name: &'static str, margin_m: f32, drawn: Vec<(f32, f32)>) -> Self {
        let (course, stored) = uploaded(&drawn);
        assert!(stored.len() <= ROOM, "{name}: {} turns", stored.len());
        let length: f32 = drawn.windows(2).map(|w| distance(w[0], w[1])).sum();
        let count = ((length + 2.0 * margin_m) / STEP_M) as usize + 1;
        let lattice = (0..count)
            .map(|k| {
                let OnExtendedLine {
                    at: (east, north),
                    way: (dx, dy),
                } = on_extended_line(&drawn, k as f32 * STEP_M - margin_m);
                let course_bearing = bearing((0.0, 0.0), (dx, dy));
                OFFSETS_M.map(|off| LatticeSpot {
                    at: (east - dy * off, north + dx * off),
                    course_bearing,
                })
            })
            .collect();
        let mut placed = vec![Crumb::default(); course.points().len()];
        let mut turns = vec![Turn::default(); ROOM];
        let mut follower = Follower::new(&mut placed, &mut turns);
        follower.handle(Input::Course {
            points: course.points(),
            frame: Some(frame()),
            read: &mut |_| None,
        });
        Self {
            name,
            course,
            stored,
            placed,
            lattice,
        }
    }
}

struct OnExtendedLine {
    at: (f32, f32),
    way: (f32, f32),
}

fn on_extended_line(line: &[(f32, f32)], metres: f32) -> OnExtendedLine {
    let legs: Vec<_> = line
        .windows(2)
        .map(|w| (w[0], w[1], distance(w[0], w[1])))
        .filter(|&(_, _, length)| length > 0.0)
        .collect();
    let mut walked = 0.0;
    for (k, &(a, b, length)) in legs.iter().enumerate() {
        let last = k + 1 == legs.len();
        if metres < walked + length || last {
            let way = ((b.0 - a.0) / length, (b.1 - a.1) / length);
            let t = metres - walked;
            return OnExtendedLine {
                at: (a.0 + way.0 * t, a.1 + way.1 * t),
                way,
            };
        }
        walked += length;
    }
    unreachable!("a course has a leg")
}

struct Lengths {
    margin_m: f32,
    straight: f32,
    l_leg: f32,
    hairpin_spur_at_m: f32,
    hairpin_spur_m: f32,
    hairpin_top_m: f32,
}

fn name(size: Size, shape: Shape) -> &'static str {
    match (size, shape) {
        (Size::Issue, Shape::Straight) => "straight",
        (Size::Issue, Shape::L) => "L with one left",
        (Size::Issue, Shape::Hairpin) => "hairpin beside a 100 m spur",
        (Size::Short, Shape::Straight) => "short straight",
        (Size::Short, Shape::L) => "short L",
        (Size::Short, Shape::Hairpin) => "short hairpin beside a 30 m spur",
    }
}

fn drawn(lengths: &Lengths, shape: Shape) -> Vec<(f32, f32)> {
    let Lengths {
        straight,
        l_leg,
        hairpin_spur_at_m,
        hairpin_spur_m,
        hairpin_top_m,
        ..
    } = *lengths;
    match shape {
        Shape::Straight => path(&[(0.0, 0.0), (0.0, straight)], 5.0),
        Shape::L => path(&[(0.0, 0.0), (0.0, l_leg), (-l_leg, l_leg)], 5.0),
        Shape::Hairpin => join(&[
            path(
                &[
                    (0.0, 0.0),
                    (0.0, hairpin_spur_at_m),
                    (hairpin_spur_m, hairpin_spur_at_m),
                    (0.0, hairpin_spur_at_m),
                    (0.0, hairpin_top_m),
                ],
                5.0,
            ),
            arc((-20.0, hairpin_top_m), 20.0, 90.0, -180.0, 5.0),
            path(&[(-40.0, hairpin_top_m), (-40.0, 0.0)], 5.0),
        ]),
    }
}

struct Fixtures {
    straight: Fixture,
    l: Fixture,
    hairpin: Fixture,
}

fn fixtures(size: Size) -> &'static Fixtures {
    static ISSUE: OnceLock<Fixtures> = OnceLock::new();
    static SHORT: OnceLock<Fixtures> = OnceLock::new();
    let (cell, lengths) = match size {
        Size::Issue => (
            &ISSUE,
            Lengths {
                margin_m: 50.0,
                straight: 200.0,
                l_leg: 150.0,
                hairpin_spur_at_m: 100.0,
                hairpin_spur_m: 100.0,
                hairpin_top_m: 150.0,
            },
        ),
        Size::Short => (
            &SHORT,
            Lengths {
                margin_m: 20.0,
                straight: 60.0,
                l_leg: 50.0,
                hairpin_spur_at_m: 30.0,
                hairpin_spur_m: 30.0,
                hairpin_top_m: 60.0,
            },
        ),
    };
    cell.get_or_init(|| {
        let made =
            |shape| Fixture::new(name(size, shape), lengths.margin_m, drawn(&lengths, shape));
        Fixtures {
            straight: made(Shape::Straight),
            l: made(Shape::L),
            hairpin: made(Shape::Hairpin),
        }
    })
}

pub(super) fn fixture(size: Size, shape: Shape) -> &'static Fixture {
    let fixtures = fixtures(size);
    match shape {
        Shape::Straight => &fixtures.straight,
        Shape::L => &fixtures.l,
        Shape::Hairpin => &fixtures.hairpin,
    }
}

pub(super) fn frame() -> Frame {
    Frame::at(at(0.0, 0.0))
}
