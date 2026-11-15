use super::*;
use crate::ride::course::nav::Distance;
use crate::screens::ride::RidePage;
use crate::screens::workout::Page;
use pose::Step;

const MOCKUP_COURSE: [Point; 9] = [
    (0.0, -400.0),
    (0.0, 250.0),
    (25.0, 248.0),
    (60.0, 260.0),
    (95.0, 280.0),
    (150.0, 310.0),
    (183.0, 316.0),
    (184.0, 360.0),
    (180.0, 1_000.0),
];
const TURN_MM: u32 = 650_000;
const BADGE: Rect = Rect {
    left: 8,
    top: 30,
    right: 145,
    bottom: 90,
};

fn track_crumbs(points: &[Point]) -> std::vec::Vec<Crumb> {
    let mut along = 0.0;
    let mut last = points[0];
    points
        .iter()
        .map(|&(east, north)| {
            along += distance(last, (east, north));
            last = (east, north);
            Crumb { east, north, along }
        })
        .collect()
}

fn follower_course(points: &[Point]) -> std::vec::Vec<Crumb> {
    let mut crumbs = track_crumbs(points);
    let total = crumbs.last().unwrap().along;
    for c in &mut crumbs {
        c.along = total - c.along;
    }
    crumbs.reverse();
    crumbs
}

fn free(crumbs: &[Crumb]) -> Track<'_> {
    Track {
        available: true,
        crumbs,
        here: crumbs.last().map(|c| (c.east, c.north)),
        heading: Some(0),
        ..Track::default()
    }
}

fn following(course: &[Crumb], here: Point, along_m: f32, junction: Option<Spot>) -> Track<'_> {
    let total = course.last().unwrap().along;
    let end = course
        .partition_point(|c| c.along < total - along_m)
        .clamp(1, course.len() - 1);
    let sign = match junction {
        Some(_) => Sign::OffCourse {
            back: Distance::from_mm(140_000),
        },
        None => Sign::Chip {
            glyph: Glyph::Right,
            distance: Distance::from_mm(250_000),
        },
    };
    Track {
        available: true,
        crumbs: &[],
        here: Some(here),
        heading: Some(0),
        course,
        ahead: Some((end, Some(here))),
        guidance: Some(Guidance {
            sign,
            turn: Some((
                Spot {
                    along_mm: TURN_MM,
                    at: (0.0, 250.0),
                },
                Glyph::Right,
            )),
            junction,
        }),
        left_m: Some(12_400),
        badge: Some(BADGE),
        radar: false,
    }
}

fn approach(course: &[Crumb]) -> Track<'_> {
    following(course, (0.0, 0.0), 400.0, None)
}

fn off_course(course: &[Crumb]) -> Track<'_> {
    let junction = Spot {
        along_mm: TURN_MM,
        at: (0.0, 250.0),
    };
    let mut track = following(course, (0.0, 390.0), 650.0, Some(junction));
    track.badge = Some(Rect {
        right: 160,
        ..BADGE
    });
    track
}

fn filled(track: &Track) -> Frame {
    let mut frame = Frame::default();
    fill(&mut frame, track);
    frame
}

#[test]
fn the_course_ahead_is_blue_on_its_casing_and_the_ridden_part_dimmed() {
    let c = follower_course(&MOCKUP_COURSE);
    let frame = filled(&approach(&c));
    assert_eq!(
        (frame.last_pose.step, frame.last_pose.rider),
        (Step::M400, (80, 266))
    );
    let at = |x, y| pixel(&frame, x, y, BLACK);
    assert_eq!(at(80, 200), ACCENT, "the route ahead");
    assert_eq!(at(87, 200), CASING, "its casing");
    assert_eq!(at(91, 200), GROUND_COLOR, "the ground beside it");
    assert_eq!(at(80, 285), RIDDEN, "the ridden course below the puck");
    assert_eq!(at(84, 285), GROUND_COLOR, "which is narrower");
    assert_eq!(at(80, 121), WHITE, "the turn arrow's shaft");
    assert_eq!(at(101, 117), WHITE, "its head, on the route after the turn");
    assert_eq!(at(80, 255), WHITE, "the puck's disc");
    assert_eq!(at(80, 266), ACCENT, "its arrow");
    assert_eq!(at(80, 279), BLACK, "its ring");
    assert_eq!(
        at(30, 30),
        GROUND_COLOR,
        "the ground reaches the status bar"
    );
    assert_eq!(at(30, 295), BLACK, "the footer is not the map's");
}

#[test]
fn off_course_chevrons_lead_back_to_the_junction_and_the_puck_turns_amber() {
    let c = follower_course(&MOCKUP_COURSE);
    let frame = filled(&off_course(&c));
    assert_eq!(
        (frame.last_pose.step, frame.last_pose.rider),
        (Step::M400, (80, 158))
    );
    let at = |x, y| pixel(&frame, x, y, BLACK);
    assert_eq!(at(80, 242), WHITE, "the junction's centre");
    assert_eq!(at(88, 242), AMBER, "its amber ring");
    assert_eq!(at(80, 199), AMBER, "the middle chevron's tip");
    assert_eq!(at(80, 184), AMBER, "the first chevron's tip");
    assert_eq!(at(80, 160), AMBER, "the puck's arrow");
    assert_eq!(at(150, 213), ACCENT, "the route goes on from the junction");
    assert_eq!(frame.arrow.strokes, 0, "no turn arrow while off course");
}

#[test]
fn off_course_beyond_the_widest_step_the_chevrons_still_lead_back_to_the_junction() {
    let c = follower_course(&MOCKUP_COURSE);
    let junction = Spot {
        along_mm: TURN_MM,
        at: (0.0, 250.0),
    };
    let here = (0.0, 3_500.0);
    let mut track = following(&c, here, 650.0, Some(junction));
    track.badge = Some(Rect {
        right: 160,
        ..BADGE
    });
    let frame = filled(&track);
    let pose = frame.last_pose;
    assert_eq!(pose.step, Step::Km3);
    let rider = (f32::from(pose.rider.0), f32::from(pose.rider.1));
    let towards = pose.projection(here).at(junction.at);
    let (dx, dy) = (towards.0 - rider.0, towards.1 - rider.1);
    let length = libm::sqrtf(dx * dx + dy * dy);
    let (ux, uy) = (dx / length, dy / length);
    let chevrons: std::vec::Vec<(usize, usize)> = GROUND
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .filter(|&(x, y)| {
            let (x, y) = (x as f32 - rider.0, y as f32 - rider.1);
            libm::sqrtf(x * x + y * y) > PUCK_CLEAR + 1.0
        })
        .filter(|&(x, y)| pixel(&frame, x, y, BLACK) == AMBER)
        .collect();
    let ahead: std::vec::Vec<f32> = chevrons
        .iter()
        .map(|&(x, y)| (x as f32 - rider.0) * ux + (y as f32 - rider.1) * uy)
        .collect();
    let beside = chevrons
        .iter()
        .map(|&(x, y)| ((x as f32 - rider.0) * uy - (y as f32 - rider.1) * ux).abs())
        .fold(0.0f32, f32::max);
    let reach = ahead.iter().fold(0.0f32, |a, &b| a.max(b))
        - ahead.iter().fold(f32::INFINITY, |a, &b| a.min(b));
    assert!(
        reach > 2.0 * CHEVRON_PITCH,
        "{pose:?}: three chevrons span over two pitches, these {reach} px"
    );
    assert!(
        ahead.iter().all(|&a| a > 0.0) && beside <= CHEVRON_HALF + CHEVRON_STROKE,
        "{pose:?}: amber off the straight line to the junction, {} px aside",
        beside
    );
    assert_eq!(
        frame.junction_quarter_px, None,
        "the ring is off the screen"
    );
}

#[test]
fn chevrons_that_would_sit_under_the_puck_or_the_ring_are_left_out() {
    let mut frame = Frame::default();
    chevrons(&mut frame, (80.0, 158.0), (80.0, 242.0));
    assert_eq!(frame.chevron_count, 6, "three, two strokes each");
    chevrons(&mut frame, (80.0, 192.0), (80.0, 242.0));
    assert_eq!(frame.chevron_count, 2, "only the middle one fits 50 px");
    chevrons(&mut frame, (80.0, 200.0), (80.0, 242.0));
    assert_eq!(frame.chevron_count, 0, "none fits 42 px");
}

#[test]
fn without_a_course_the_ride_is_drawn_ridden_behind_the_rider_mid_map() {
    let c = track_crumbs(&[(0.0, 0.0), (0.0, 300.0), (0.0, 500.0)]);
    let frame = filled(&free(&c));
    assert_eq!(
        (frame.last_pose.step, frame.last_pose.rider),
        (Step::Km1, (120, 156))
    );
    assert_eq!(pixel(&frame, 120, 240, BLACK), RIDDEN, "350 m behind");
    assert_eq!(pixel(&frame, 120, 156, BLACK), ACCENT, "the puck");
}

#[test]
fn rows_paint_what_the_per_pixel_original_says() {
    let c = follower_course(&MOCKUP_COURSE);
    let wander: std::vec::Vec<Point> = (0..400)
        .map(|i| {
            let a = i as f32 * 0.21;
            (
                libm::sinf(a) * 300.0 + i as f32,
                libm::cosf(a * 1.3) * 300.0,
            )
        })
        .collect();
    let w = track_crumbs(&wander);
    let mut turning = free(&w);
    turning.heading = Some(900);
    let cases = [
        approach(&c),
        following(&c, (0.0, 210.0), 610.0, None),
        off_course(&c),
        turning,
    ];
    for (n, track) in cases.iter().enumerate() {
        let frame = filled(track);
        for y in 0..HEIGHT {
            let mut pixels = [0x1234u16; WIDTH];
            paint(&frame, &mut Row::new(y, &mut pixels));
            for (x, &got) in pixels.iter().enumerate() {
                assert_eq!(got, pixel(&frame, x, y, 0x1234), "case {n} at {x},{y}");
            }
        }
    }
}

#[test]
fn a_long_course_keeps_to_its_budget_and_each_band_holds_what_reaches_it() {
    let zigzag: std::vec::Vec<Point> = (0..4_000)
        .map(|i| (if i % 2 == 0 { 0.0 } else { 30.0 }, i as f32 * 5.0))
        .collect();
    let c = follower_course(&zigzag);
    let mut track = following(&c, (0.0, 100.0), 100.0, None);
    track.guidance = None;
    let frame = filled(&track);
    assert!(frame.counts[Line::Route as usize] > 0);
    assert!(frame.counts[Line::Route as usize] <= ROUTE_SEGMENTS);
    for line in Line::ALL {
        for band in 0..BAND_COUNT {
            for &i in frame.band(line, band) {
                let (first, last) = bands(&frame.strokes[usize::from(i)], line.reach()).unwrap();
                assert!((first..=last).contains(&band));
            }
        }
    }
    for line in Line::ALL {
        assert!(usize::from(frame.band_offsets[line as usize][BAND_COUNT]) <= BAND_ENTRIES);
    }
}

#[test]
fn the_frame_is_refilled_only_when_the_track_changes() {
    let c = follower_course(&MOCKUP_COURSE);
    let mut frame = Frame::default();
    let mut cache = Cache::new(&mut frame);
    let t = approach(&c);
    cache.update(&t);
    let drawn = cache.frame.counts;
    assert!(drawn[Line::Route as usize] > 0);
    cache.frame.counts = [0; 2];
    cache.update(&t);
    assert_eq!(cache.frame.counts, [0; 2], "the same track is left alone");
    let mut moved = t;
    moved.heading = Some(100);
    cache.update(&moved);
    assert!(cache.frame.counts[Line::Route as usize] > 0);
}

#[test]
fn fills_of_one_track_held_at_another_step_hash_apart() {
    use core::hash::{Hash, Hasher};
    let c = follower_course(&MOCKUP_COURSE);
    let nearer = following(&c, (0.0, 55.0), 455.0, None);
    let (mut fresh_frame, mut held_frame) = (Frame::default(), Frame::default());
    let mut fresh = Cache::new(&mut fresh_frame);
    fresh.update(&nearer);
    let mut held = Cache::new(&mut held_frame);
    held.update(&approach(&c));
    held.update(&nearer);
    let (fresh, held) = (fresh.filled().unwrap(), held.filled().unwrap());
    assert_ne!(
        fresh.frame.last_pose.step, held.frame.last_pose.step,
        "zooming in waits"
    );
    let hash = |filled: &Filled| {
        let mut hasher = std::hash::DefaultHasher::new();
        filled.hash(&mut hasher);
        hasher.finish()
    };
    assert_ne!(
        hash(&fresh),
        hash(&held),
        "a skipped band keeps the old map"
    );
}

#[track_caller]
fn assert_shows(canvas: &Canvas, want: &[&str], what: &str) {
    use crate::screens::chrome::layout::{assert_shown, text};
    let texts: std::vec::Vec<_> = canvas.runs.iter().filter_map(text).collect();
    for (name, rect) in &texts {
        assert!(
            rect.right <= WIDTH && rect.bottom <= HEIGHT,
            "{what}: {name}"
        );
    }
    assert_shown(&texts, want, what);
}

fn page<'a>(map: Option<Filled<'a>>) -> View<'a> {
    let mut view = View::new(Page::Ride, 0, None);
    view.ride_page = RidePage::Map;
    view.show_map(map);
    view.totals.add(1000, 8_333_000, &Default::default(), None);
    view.prepare();
    view
}

#[test]
fn every_state_lays_out_its_text() {
    let check = |track: &Track, want: &[&str], what: &str| {
        let mut frame = Frame::default();
        let mut cache = Cache::new(&mut frame);
        cache.update(track);
        assert_shows(page(cache.filled()).canvas(), want, what);
    };
    assert_shows(page(None).canvas(), &["NO TRACK"], "unavailable");
    check(&Track::default(), &["NO TRACK"], "no memory");
    let waiting = Track {
        available: true,
        ..Track::default()
    };
    check(&waiting, &["NO GPS YET", "MAP STARTS WITH GPS"], "no gps");
    let c = follower_course(&MOCKUP_COURSE);
    check(&approach(&c), &["100 M", "12.40", "KM"], "following");
    let ride = track_crumbs(&[(0.0, 0.0), (0.0, 800.0)]);
    check(&free(&ride), &["250 M", "8.33", "KM"], "riding");
    let start = track_crumbs(&[(0.0, 0.0)]);
    check(
        &free(&start),
        &["DRAWS AS YOU RIDE", "250 M", "8.33", "KM"],
        "first fix",
    );
}

#[test]
fn the_scale_bar_reads_each_step() {
    let bar = |step: Step| {
        let (px, label) = step.bar();
        (libm::roundf(px as f32 / step.px_per_m()), label)
    };
    assert_eq!(bar(Step::M300), (100.0, &b"100 M"[..]));
    assert_eq!(bar(Step::M400), (100.0, &b"100 M"[..]));
    assert_eq!(bar(Step::Km1), (250.0, &b"250 M"[..]));
    assert_eq!(bar(Step::Km3), (1_000.0, &b"1 KM"[..]));
}

#[test]
fn a_junction_far_off_the_screen_paints_nothing_of_it() {
    let c = follower_course(&[(0.0, 0.0), (0.0, 1_000.0)]);
    let junction = Spot {
        along_mm: 0,
        at: (0.0, 0.0),
    };
    let track = following(&c, (0.0, 150_000.0), 0.0, Some(junction));
    let frame = filled(&track);
    for y in 0..HEIGHT {
        let mut pixels = [BLACK; WIDTH];
        paint(&frame, &mut Row::new(y, &mut pixels));
        for (x, &got) in pixels.iter().enumerate() {
            assert_eq!(got, pixel(&frame, x, y, BLACK), "at {x},{y}");
        }
    }
}

#[test]
fn the_route_ahead_is_drawn_whatever_the_ridden_part_takes() {
    let mut frame = Frame {
        show: Show::Map,
        ..Frame::default()
    };
    let pose = Pose {
        step: Step::Km1,
        rider: (120, 156),
        heading: 0,
    };
    let at = pose.projection((0.0, 0.0));
    let out_and_back = core::iter::once((0.0, 0.0))
        .chain(core::iter::once((0.0, 200.0)))
        .chain((0..RIDDEN_SEGMENTS).map(|k| (0.0, if k % 2 == 0 { -800.0 } else { 800.0 })));
    frame.add(Line::Ridden, &at, out_and_back);
    frame.add(Line::Route, &at, [(0.0, 0.0), (0.0, 1_000.0)].into_iter());
    bucket(&mut frame);
    let mut pixels = [BLACK; WIDTH];
    paint(&frame, &mut Row::new(50, &mut pixels));
    assert_eq!(pixels[120], ACCENT, "the route ahead, over the ridden part");
}

fn last_leg(course: &[Crumb], north: f32) -> Track<'_> {
    let mut track = following(course, (0.0, north), north, None);
    let guidance = track.guidance.as_mut().unwrap();
    guidance.turn = None;
    guidance.sign = Sign::Chip {
        glyph: Glyph::Finish,
        distance: Distance::from_mm(2_600_000),
    };
    track
}

fn under_the_badge(frame: &Frame, badge: Rect) -> std::vec::Vec<(usize, usize)> {
    let reach = 7;
    let rows = GROUND.start.max(badge.top - reach)..badge.bottom + reach;
    let columns = badge.left.saturating_sub(reach)..badge.right + reach;
    rows.flat_map(|y| columns.clone().map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(frame, x, y, BLACK) != GROUND_COLOR)
        .collect()
}

#[test]
fn on_the_last_leg_the_route_and_the_finish_clear_the_badge() {
    let straight: std::vec::Vec<Point> = (0..=30).map(|k| (0.0, k as f32 * 100.0)).collect();
    let c = follower_course(&straight);
    for north in [400.0, 2_000.0, 2_700.0] {
        let frame = filled(&last_leg(&c, north));
        let covered = under_the_badge(&frame, BADGE);
        assert!(
            covered.is_empty(),
            "{north} m: {:?} puts {} px under the badge, e.g. {:?}",
            frame.last_pose,
            covered.len(),
            covered.first()
        );
    }
}

mod santiago {
    use super::*;
    use crate::ride::homing::TURN;
    use crate::screens::ride::map::roads::tests::{PLAZA, cached};
    use std::time::Instant;
    use std::{boxed::Box, println, string::String, vec::Vec};
    use terrain::PAGE_BYTES;

    /// A course through `here` heading `heading`: straight on for `turn_m`,
    /// then right, with the turn shown on the map's badge.
    pub(super) struct Approach {
        pub(super) course: Vec<Crumb>,
        here: Point,
        heading: u16,
        turn_m: f32,
    }

    impl Approach {
        pub(super) fn new(here: Point, heading: u16, turn_m: f32) -> Self {
            let a = f32::from(heading) / f32::from(TURN) * core::f32::consts::TAU;
            let (sin, cos) = (libm::sinf(a), libm::cosf(a));
            let to = |(right, ahead): Point| {
                (
                    here.0 + right * cos + ahead * sin,
                    here.1 - right * sin + ahead * cos,
                )
            };
            let mut points = Vec::new();
            let mut along = -400.0;
            while along < turn_m {
                points.push(to((0.0, along)));
                along += 20.0;
            }
            points.extend([to((0.0, turn_m)), to((1_000.0, turn_m))]);
            Self {
                course: follower_course(&points),
                here,
                heading,
                turn_m,
            }
        }

        pub(super) fn track(&self) -> Track<'_> {
            let total = self.course.last().unwrap().along;
            let end = self
                .course
                .partition_point(|c| c.along < total - 400.0)
                .clamp(1, self.course.len() - 1);
            let turn = self
                .course
                .iter()
                .find(|c| (c.along - (total - 400.0 - self.turn_m)).abs() < 0.5);
            let turn = turn.map(|c| (c.east, c.north)).unwrap();
            Track {
                available: true,
                crumbs: &[],
                here: Some(self.here),
                heading: Some(self.heading),
                course: &self.course,
                ahead: Some((end, Some(self.here))),
                guidance: Some(Guidance {
                    sign: Sign::Chip {
                        glyph: Glyph::Right,
                        distance: Distance::from_mm((self.turn_m * 1000.0) as u32),
                    },
                    turn: Some((
                        Spot {
                            along_mm: ((400.0 + self.turn_m) * 1000.0) as u32,
                            at: turn,
                        },
                        Glyph::Right,
                    )),
                    junction: None,
                }),
                left_m: Some(8_000),
                badge: Some(BADGE),
                radar: false,
            }
        }
    }

    /// Fill `cache` from `track` and fetch roads until every tile of the
    /// view is held.
    pub(super) fn settle(
        cache: &mut Cache,
        engine: &mut terrain::Lookahead,
        frame: RideFrame,
        track: &Track,
    ) {
        for _ in 0..40 {
            cache.update(track);
            cache.fetch_roads(engine, frame);
        }
        cache.update(track);
    }

    fn painted(frame: &Frame) -> Vec<u16> {
        let mut screen = std::vec![0x1234u16; WIDTH * HEIGHT];
        for (y, pixels) in screen.as_chunks_mut::<WIDTH>().0.iter_mut().enumerate() {
            paint(frame, &mut Row::new(y, pixels));
        }
        screen
    }

    #[test]
    fn central_santiago_paints_its_roads_under_the_route_as_the_reference_does() {
        let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
        let mut engine = cached(&mut arena);
        let ride = RideFrame::at(PLAZA);
        let approach = Approach::new((0.0, -150.0), 0, 250.0);
        let track = approach.track();
        let mut frame = Box::new(Frame::default());
        let mut cache = Cache::new(&mut frame);
        settle(&mut cache, &mut engine, ride, &track);
        let frame = &*cache.frame;
        assert_eq!(frame.last_pose.step, Step::M400);
        assert!(
            frame.roads_drawn[0] > 10 && frame.roads_drawn[1] > 2,
            "{:?}",
            frame.roads_drawn
        );
        let screen = painted(frame);
        for y in GROUND.start..GROUND.end {
            for x in 0..WIDTH {
                assert_eq!(
                    screen[y * WIDTH + x],
                    pixel(frame, x, y, 0x1234),
                    "at {x},{y}"
                );
            }
        }
        let count = |color: u16| screen.iter().filter(|&&p| p == color).count();
        assert!(count(MINOR_ROAD) > 2_000 && count(MAJOR_ROAD) > 1_000);
        let (rider_x, rider_y) = (
            frame.last_pose.rider.0 as usize,
            frame.last_pose.rider.1 as usize,
        );
        let under_route = screen[(rider_y - 40) * WIDTH + rider_x];
        assert_eq!(under_route, ACCENT, "the route paints over the roads");
    }

    #[test]
    fn without_a_pack_the_course_is_drawn_on_bare_ground() {
        let mut empty: [u8; 0] = [];
        let mut engine = terrain::Lookahead::new(&mut empty, 10_000.0);
        let approach = Approach::new((0.0, -150.0), 0, 250.0);
        let track = approach.track();
        let mut frame = Box::new(Frame::default());
        let mut cache = Cache::new(&mut frame);
        settle(&mut cache, &mut engine, RideFrame::at(PLAZA), &track);
        let screen = painted(cache.frame);
        assert!(!screen.iter().any(|&p| p == MINOR_ROAD || p == MAJOR_ROAD));
        assert!(screen.iter().filter(|&&p| p == ACCENT).count() > 1_000);
    }

    /// Prints how much road each step's view of central Santiago draws,
    /// over a grid of places and headings, and the densest of each step.
    /// `cargo test -p vana --release --lib densest -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn densest_views_of_central_santiago() {
        let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
        let mut engine = cached(&mut arena);
        let ride = RideFrame::at(PLAZA);
        println!(
            "step  turn m  east  north  heading  minor  major  segments  fill µs  paint µs  without roads µs"
        );
        for (step, turn_m) in [
            (Step::M300, 60.0),
            (Step::M400, 250.0),
            (Step::Km1, 500.0),
            (Step::Km3, 1_500.0),
        ] {
            let mut densest = (0, String::new());
            for east in (-1_600..=1_600).step_by(400) {
                for north in (-2_000..=1_200).step_by(400) {
                    for heading in [0, TURN / 8, TURN / 4, 3 * TURN / 8] {
                        let approach = Approach::new((east as f32, north as f32), heading, turn_m);
                        let track = approach.track();
                        let mut frame = Box::new(Frame::default());
                        let mut cache = Cache::new(&mut frame);
                        settle(&mut cache, &mut engine, ride, &track);
                        assert_eq!(cache.frame.last_pose.step, step);
                        let started = Instant::now();
                        fill(cache.frame, &track);
                        let fill_us = started.elapsed().as_secs_f64() * 1e6;
                        let started = Instant::now();
                        for y in 0..HEIGHT {
                            let mut pixels = [0u16; WIDTH];
                            paint(cache.frame, &mut Row::new(y, &mut pixels));
                        }
                        let paint_us = started.elapsed().as_secs_f64() * 1e6;
                        let mut bare = Box::new(Frame::default());
                        fill(&mut bare, &track);
                        let started = Instant::now();
                        for y in 0..HEIGHT {
                            let mut pixels = [0u16; WIDTH];
                            paint(&bare, &mut Row::new(y, &mut pixels));
                        }
                        let bare_us = started.elapsed().as_secs_f64() * 1e6;
                        let f = &*cache.frame;
                        let segments = f.roads_drawn[0] + f.roads_drawn[1];
                        let line = std::format!(
                            "{step:?}  {turn_m}  {east}  {north}  {heading}  {}  {}  {segments}  {fill_us:.0}  {paint_us:.0}  {bare_us:.0}",
                            f.roads_drawn[0],
                            f.roads_drawn[1]
                        );
                        if segments > densest.0 {
                            densest = (segments, line);
                        }
                    }
                }
            }
            println!("{}", densest.1);
        }
    }
}
