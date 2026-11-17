use super::*;
extern crate std;
use std::vec::Vec;

fn joining_by_every_leg(
    course: &[Crumb],
    heading: Option<u16>,
    centre: Point,
    held: Option<usize>,
) -> Option<Joining> {
    let closest =
        |legs: &mut dyn Iterator<Item = Leg>| legs.min_by(|a, b| a.gap_m.total_cmp(&b.gap_m));
    let legs = 1..course.len();
    let leg = |end: usize| leg(course, centre, end);
    let held = held.filter(|end| legs.contains(end)).map(leg);
    let Some(heading) = heading else {
        let best = held.or_else(|| closest(&mut legs.map(leg)))?;
        return Some(Joining {
            held,
            best,
            heading_against_held: false,
            held_leg_too_far: false,
        });
    };
    let (sin, cos) = rotation(heading);
    let our_way = |&end: &usize| {
        let (a, b) = (course[end - 1], course[end]);
        (a.east - b.east) * sin + (a.north - b.north) * cos > 0.0
    };
    let ours =
        closest(&mut legs.clone().filter(our_way).map(leg)).filter(|l| l.gap_m <= JOIN_WITHIN_M);
    let best = ours.or_else(|| closest(&mut legs.map(leg)))?;
    let heading_against_held = held.is_some_and(|held| {
        let (a, b) = (course[held.end - 1], course[held.end]);
        let off_its_way = (heading + TURN - bearing((b.east, b.north), (a.east, a.north))) % TURN;
        MORE_THAN_135_DEG_OFF.contains(&off_its_way)
    });
    let held_leg_too_far = held.is_some_and(|held| {
        best.end != held.end
            && (held.gap_m > JOIN_WITHIN_M || held.gap_m - best.gap_m > JOIN_SWITCH_M)
    });
    Some(Joining {
        held,
        best,
        heading_against_held,
        held_leg_too_far,
    })
}

type Bits = (usize, u32, u32, u32);

fn bits(leg: Leg) -> Bits {
    (
        leg.end,
        leg.at.0.to_bits(),
        leg.at.1.to_bits(),
        leg.gap_m.to_bits(),
    )
}

fn answer(joining: Option<Joining>) -> Option<(Option<Bits>, Bits, bool, bool)> {
    joining.map(|j| {
        (
            j.held.map(bits),
            bits(j.best),
            j.heading_against_held,
            j.held_leg_too_far,
        )
    })
}

pub(crate) struct Random(pub(crate) u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub(crate) fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub(crate) fn within(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * (self.next() >> 40) as f32 / (1u64 << 24) as f32
    }
}

pub(crate) fn crumbs(points: &[Point]) -> Vec<Crumb> {
    let mut along = 0.0;
    let mut course = Vec::new();
    for (k, &p) in points.iter().enumerate() {
        if k > 0 {
            along += distance(points[k - 1], p);
        }
        course.push(Crumb {
            east: p.0,
            north: p.1,
            along,
        });
    }
    course
}

pub(crate) fn wander(random: &mut Random, origin: Point, n: usize) -> Vec<Point> {
    let mut at = origin;
    let mut bearing = random.within(0.0, core::f32::consts::TAU);
    let mut points = std::vec![at];
    for _ in 1..n {
        bearing += match random.below(8) {
            0 => random.within(1.5, 3.0),
            _ => random.within(-0.4, 0.4),
        };
        if random.below(20) > 0 {
            let step = random.within(5.0, 60.0);
            at = (at.0 + step * bearing.sin(), at.1 + step * bearing.cos());
        }
        points.push(at);
    }
    points
}

pub(crate) fn out_and_back(random: &mut Random, origin: Point) -> Vec<Point> {
    let n = 30 + random.below(60) as usize;
    let mut points = wander(random, origin, n);
    let beside = match random.below(3) {
        0 => 0.0,
        _ => random.within(-4.0, 4.0),
    };
    let back: Vec<Point> = points
        .iter()
        .rev()
        .map(|p| (p.0 + beside, p.1 + beside))
        .collect();
    points.extend(back);
    points
}

pub(crate) fn lanes(random: &mut Random, origin: Point) -> Vec<Point> {
    let gap = random.within(5.0, 30.0);
    let length = random.within(100.0, 600.0);
    let mut points = Vec::new();
    for lane in 0..2 + random.below(8) {
        let east = origin.0 + lane as f32 * gap;
        let (from, to) = match lane % 2 {
            0 => (origin.1, origin.1 + length),
            _ => (origin.1 + length, origin.1),
        };
        for k in 0..=4 {
            points.push((east, from + (to - from) * k as f32 / 4.0));
        }
    }
    points
}

pub(crate) fn crossing(random: &mut Random, origin: Point) -> Vec<Point> {
    let r = random.within(30.0, 400.0);
    let n = 16 + random.below(48) as usize;
    (0..=n)
        .map(|k| {
            let t = k as f32 / n as f32 * core::f32::consts::TAU;
            (origin.0 + r * t.sin(), origin.1 + r * (2.0 * t).sin() / 2.0)
        })
        .collect()
}

#[test]
fn the_nearest_leg_is_the_one_every_leg_scanned_would_find() {
    let mut random = Random(0x5EED_1399);
    let mut compared = 0;
    for course_no in 0..240 {
        let origin = match course_no % 3 {
            0 => (0.0, 0.0),
            1 => (random.within(-5e3, 5e3), random.within(-5e3, 5e3)),
            _ => (random.within(-2e5, 2e5), random.within(-2e5, 2e5)),
        };
        let points = match course_no % 4 {
            0 => {
                let n = 2 + random.below(200) as usize;
                wander(&mut random, origin, n)
            }
            1 => out_and_back(&mut random, origin),
            2 => lanes(&mut random, origin),
            _ => crossing(&mut random, origin),
        };
        let course = crumbs(&points);
        for _ in 0..120 {
            let near = points[random.below(points.len() as u64) as usize];
            let centre = match random.below(6) {
                0 => near,
                1 => (
                    near.0 + random.within(-500.0, 500.0),
                    near.1 + random.within(-500.0, 500.0),
                ),
                2 => (
                    near.0 + random.within(-5e3, 5e3),
                    near.1 + random.within(-5e3, 5e3),
                ),
                _ => (
                    near.0 + random.within(-40.0, 40.0),
                    near.1 + random.within(-40.0, 40.0),
                ),
            };
            let heading = match random.below(4) {
                0 => None,
                _ => Some(random.below(u64::from(TURN)) as u16),
            };
            let held = match random.below(4) {
                0 => None,
                1 => Some(random.below(course.len() as u64 + 3) as usize),
                _ => Some(1 + random.below(course.len() as u64 - 1) as usize),
            };
            let before = answer(joining_by_every_leg(&course, heading, centre, held));
            assert_eq!(
                answer(joining(&course, heading, centre, held)),
                before,
                "course {course_no}, centre {centre:?}, heading {heading:?}, held {held:?}"
            );
            let drawn = before.map(|(held, best, ..)| {
                let (end, east, north, _) = held.unwrap_or(best);
                (end, east, north)
            });
            let now = at(&course, heading, centre, held)
                .map(|(end, at)| (end, at.0.to_bits(), at.1.to_bits()));
            assert_eq!(now, drawn, "at: course {course_no}, centre {centre:?}");
            compared += 1;
        }
    }
    assert_eq!(compared, 240 * 120);
}

#[test]
fn a_course_half_a_world_from_the_ride_s_origin_still_finds_the_leg_every_scan_would() {
    let frame = crate::ride::geo::Frame::at((0, 0));
    let points = [
        frame.place(0, -1_798_640_960),
        frame.place(0, 1_798_640_961),
        frame.place(450, 1_798_641_088),
    ];
    let course = crumbs(&points);
    let centre = frame.place(0, 1_798_643_648);
    let old = joining_by_every_leg(&course, Some(3072), centre, Some(2));
    let new = joining(&course, Some(3072), centre, Some(2));
    assert_eq!(answer(new), answer(old));
}
