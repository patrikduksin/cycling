use super::*;
use crate::ride::course::nav::Distance;
use crate::ride::homing::{Homing, Reported, TURN};
use crate::screens::ride::RidePage;
use crate::screens::workout::Page;

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
        speed_mm_s: None,
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
        reach: Reach::default(),
        radar: false,
        ..Track::default()
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
    following(course, (-100.0, 150.0), 500.0, Some(junction))
}

fn joining(course: &[Crumb], here: Point) -> Track<'_> {
    Track {
        available: true,
        here: Some(here),
        heading: Some(0),
        course,
        ..Track::default()
    }
}

fn on_course(course: &[Crumb], here: Point, along_m: f32) -> Track<'_> {
    let mut track = following(course, here, along_m, None);
    track.guidance = None;
    track
}

fn blue_runs(frame: &Frame, y: usize) -> usize {
    (0..WIDTH)
        .map(|x| pixel(frame, x, y, BLACK) == ACCENT)
        .fold((0, false), |(runs, was), is| {
            (runs + usize::from(is && !was), is)
        })
        .0
}

fn route_in(frame: &Frame, rows: core::ops::Range<usize>) -> Option<(usize, usize)> {
    rows.flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .find(|&(x, y)| matches!(pixel(frame, x, y, BLACK), ACCENT | CASING))
}

const BEHIND_THE_PUCK: core::ops::Range<usize> = 240..GROUND.end;

fn settled_at(speed_mm_s: u32) -> Reach {
    let mut zoom = Zoom::default();
    (0..60).for_each(|_| zoom.observe_riding_at(speed_mm_s));
    zoom.reach()
}

fn fill(frame: &mut Frame, track: &Track) {
    frame.route_leg.observe(track);
    super::fill(frame, track);
}

fn filled(track: &Track) -> Frame {
    let mut frame = Frame::default();
    fill(&mut frame, track);
    frame
}

#[test]
fn the_course_ahead_is_blue_on_its_casing_and_nothing_is_drawn_behind_the_puck() {
    let c = follower_course(&MOCKUP_COURSE);
    let frame = filled(&approach(&c));
    assert_eq!(frame.last_pose.rider, (120, 223));
    let at = |x, y| pixel(&frame, x, y, BLACK);
    assert_eq!(at(120, 150), ACCENT, "the route ahead");
    assert_eq!(at(127, 150), CASING, "its casing");
    assert_eq!(at(131, 150), GROUND_COLOR, "the ground beside it");
    assert_eq!(
        at(120, 270),
        GROUND_COLOR,
        "nothing of the course behind the puck"
    );
    assert_eq!(at(120, 60), WHITE, "the turn arrow's shaft, 250 m up");
    assert_eq!(at(141, 57), WHITE, "its head, on the route after the turn");
    assert_eq!(at(120, 212), WHITE, "the puck's disc");
    assert_eq!(at(120, 223), ACCENT, "its arrow");
    assert_eq!(at(120, 236), BLACK, "its ring");
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
    let track = off_course(&c);
    let frame = filled(&track);
    assert_eq!(frame.last_pose.rider, (120, 223));
    let at = |(x, y): Point| pixel(&frame, x as usize, y as usize, BLACK);
    let ring = frame
        .last_pose
        .projection(track.here.unwrap())
        .at((0.0, 250.0));
    let towards = |along: f32| {
        let (dx, dy) = (ring.0 - 120.0, ring.1 - 223.0);
        let length = libm::sqrtf(dx * dx + dy * dy);
        (120.5 + dx * along / length, 223.5 + dy * along / length)
    };
    assert_eq!(at(ring), WHITE, "the junction's centre");
    assert_eq!(at((ring.0 + 8.0, ring.1)), AMBER, "its amber ring");
    for along in [32.4, 47.4, 62.4] {
        assert_eq!(at(towards(along)), AMBER, "a chevron's tip {along} px out");
    }
    assert_eq!(at((120.0, 225.0)), AMBER, "the puck's arrow");
    assert_eq!(
        at((ring.0 + 25.0, ring.1)),
        ACCENT,
        "the route goes on from the junction"
    );
    assert_eq!(frame.arrow.strokes, 0, "no turn arrow while off course");
}

#[test]
fn far_off_course_in_every_direction_three_chevrons_lead_back_clear_of_the_badge_and_radar() {
    let c = follower_course(&MOCKUP_COURSE);
    let junction = Spot {
        along_mm: TURN_MM,
        at: (0.0, 250.0),
    };
    for radar in [false, true] {
        for back_m in [400.0, 2_000.0, 150_000.0] {
            for k in 0..16 {
                let bearing = k as f32 * core::f32::consts::TAU / 16.0;
                let here = (
                    junction.at.0 - back_m * libm::sinf(bearing),
                    junction.at.1 - back_m * libm::cosf(bearing),
                );
                let mut track = following(&c, here, 650.0, Some(junction));
                let sign = Sign::OffCourse {
                    back: Distance::from_mm((back_m * 1000.0) as u32),
                };
                let badge = crate::screens::sign::on_page(RidePage::Map, sign, radar)
                    .unwrap()
                    .rect();
                track.guidance.as_mut().unwrap().sign = sign;
                track.radar = radar;
                let frame = filled(&track);
                let pose = frame.last_pose;
                let what = std::format!("radar {radar}, {back_m} m back on bearing {k}/16");
                let rider = (f32::from(pose.rider.0) + 0.5, f32::from(pose.rider.1) + 0.5);
                let towards = pose.projection(here).at(junction.at);
                let (dx, dy) = (towards.0 - rider.0, towards.1 - rider.1);
                let length = libm::sqrtf(dx * dx + dy * dy);
                let (ux, uy) = (dx / length, dy / length);
                let away = |(x, y): (usize, usize), (cx, cy): Point| {
                    let (dx, dy) = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
                    libm::sqrtf(dx * dx + dy * dy)
                };
                let chevrons: std::vec::Vec<(usize, usize)> = GROUND
                    .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
                    .filter(|&p| away(p, rider) > PUCK_CLEAR + 1.0)
                    .filter(|&(x, y)| pixel(&frame, x, y, BLACK) == AMBER)
                    .collect();
                let along: std::vec::Vec<f32> = chevrons
                    .iter()
                    .map(|&(x, y)| {
                        (x as f32 + 0.5 - rider.0) * ux + (y as f32 + 0.5 - rider.1) * uy
                    })
                    .collect();
                let span = along.iter().fold(0.0f32, |a, &b| a.max(b))
                    - along.iter().fold(f32::INFINITY, |a, &b| a.min(b));
                assert!(
                    span > 2.0 * CHEVRON_PITCH,
                    "{what}: three chevrons span over two pitches, these {span} px"
                );
                let hidden: std::vec::Vec<&(usize, usize)> = chevrons
                    .iter()
                    .filter(|&&(x, y)| badge.contains(x, y) || x < map_left(radar))
                    .collect();
                assert!(
                    hidden.is_empty(),
                    "{what}: chevrons under the badge or the radar strip at {hidden:?}"
                );
                assert!(
                    frame.junction_quarter_px.is_none(),
                    "{what}: the ring is off the map"
                );
            }
        }
    }
}

#[test]
fn without_a_course_the_ride_is_drawn_ridden_behind_the_rider() {
    let c = track_crumbs(&[(0.0, 0.0), (0.0, 300.0), (0.0, 500.0)]);
    let frame = filled(&free(&c));
    assert_eq!(frame.last_pose.rider, (120, 223));
    assert_eq!(pixel(&frame, 120, 270, BLACK), RIDDEN, "70 m behind");
    assert_eq!(pixel(&frame, 120, 223, BLACK), ACCENT, "the puck");
}

#[test]
fn the_course_ahead_moves_neither_the_scale_nor_the_rider() {
    let ahead = |past_the_turn: &[Point]| {
        let mut points = std::vec![(0.0, -400.0), (0.0, 250.0)];
        points.extend_from_slice(past_the_turn);
        follower_course(&points)
    };
    let courses = [
        ahead(&[(3_000.0, 250.0)]),
        ahead(&[(-3_000.0, 250.0)]),
        ahead(&[(0.0, 260.0), (0.0, 5_000.0)]),
        ahead(&[(20.0, 250.0), (20.0, -300.0)]),
    ];
    let poses: std::vec::Vec<_> = courses
        .iter()
        .flat_map(|c| {
            [false, true].map(|radar| {
                let mut track = approach(c);
                track.radar = radar;
                let frame = filled(&track);
                let puck = (100..150)
                    .flat_map(|x| (200..250).map(move |y| (x, y)))
                    .map(|(x, y)| pixel(&frame, x, y, BLACK))
                    .filter(|&c| c == WHITE || c == BLACK)
                    .count();
                (radar, frame.last_pose, puck)
            })
        })
        .collect();
    for pose in &poses {
        let first = poses.iter().find(|p| p.0 == pose.0).unwrap();
        assert_eq!(pose, first, "every course ahead");
    }
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
    let mut fast = free(&ride);
    fast.reach = settled_at(30_000);
    check(&fast, &["250 M", "8.33", "KM"], "riding fast");
    let start = track_crumbs(&[(0.0, 0.0)]);
    check(
        &free(&start),
        &["DRAWS AS YOU RIDE", "100 M", "8.33", "KM"],
        "first fix",
    );
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
fn on_an_out_and_back_only_the_way_out_is_blue_and_nothing_behind_the_puck() {
    let c = follower_course(&[
        (0.0, -400.0),
        (0.0, 2_000.0),
        (30.0, 2_000.0),
        (30.0, -400.0),
    ]);
    let frame = filled(&on_course(&c, (0.0, 0.0), 400.0));
    for y in [40, 100, 150, 200] {
        assert_eq!(blue_runs(&frame, y), 1, "one blue line at row {y}");
    }
    assert_eq!(pixel(&frame, 120, 150, BLACK), ACCENT, "the way out");
    assert_eq!(
        pixel(&frame, 140, 150, BLACK),
        GROUND_COLOR,
        "not the way back"
    );
    assert_eq!(route_in(&frame, BEHIND_THE_PUCK), None, "behind the puck");
}

#[test]
fn a_loop_finishing_across_the_rider_draws_no_second_line() {
    let c = follower_course(&[
        (0.0, -200.0),
        (0.0, 600.0),
        (600.0, 600.0),
        (600.0, 0.0),
        (-500.0, 0.0),
    ]);
    let frame = filled(&on_course(&c, (0.0, 0.0), 200.0));
    assert_eq!(pixel(&frame, 120, 150, BLACK), ACCENT, "the course ahead");
    for x in [40, 70, 170, 200] {
        assert_eq!(
            pixel(&frame, x, 223, BLACK),
            GROUND_COLOR,
            "the finish across the puck's row at x {x}"
        );
    }
    assert_eq!(route_in(&frame, BEHIND_THE_PUCK), None, "behind the puck");
}

#[test]
fn before_joining_the_course_runs_only_its_own_way_from_the_nearest_point() {
    let north = follower_course(&[(0.0, -1_000.0), (0.0, 1_000.0)]);
    let frame = filled(&joining(&north, (0.0, 0.0)));
    assert_eq!(
        pixel(&frame, 120, 150, BLACK),
        ACCENT,
        "ahead, the course's way"
    );
    assert_eq!(route_in(&frame, BEHIND_THE_PUCK), None, "not back along it");
    let south = follower_course(&[(0.0, 1_000.0), (0.0, -1_000.0)]);
    let frame = filled(&joining(&south, (0.0, 0.0)));
    assert_eq!(
        pixel(&frame, 120, 270, BLACK),
        ACCENT,
        "behind, the course's way"
    );
    assert_eq!(
        route_in(&frame, GROUND.start..205),
        None,
        "not ahead against it"
    );
}

#[test]
fn off_course_the_line_starts_at_the_junction_and_runs_on_from_it() {
    let c = follower_course(&MOCKUP_COURSE);
    let track = off_course(&c);
    let frame = filled(&track);
    let at = frame.last_pose.projection(track.here.unwrap());
    let shown = |p: Point| {
        let (x, y) = at.at(p);
        pixel(&frame, x as usize, y as usize, BLACK)
    };
    assert_eq!(shown((60.0, 260.0)), ACCENT, "on from the junction");
    for north in [100.0, 150.0, 200.0] {
        assert_eq!(
            shown((0.0, north)),
            GROUND_COLOR,
            "the course before the junction, {north} m north"
        );
    }
}

#[test]
fn the_ridden_track_is_drawn_only_without_a_course() {
    let ridden = track_crumbs(&[(-300.0, -300.0), (-150.0, -150.0), (0.0, 0.0)]);
    let c = follower_course(&[(0.0, -1_000.0), (0.0, 1_000.0)]);
    let mut before = joining(&c, (0.0, 0.0));
    before.crumbs = &ridden;
    let mut along = on_course(&c, (0.0, 0.0), 1_000.0);
    along.crumbs = &ridden;
    let ridden_px = |track: &Track| {
        let frame = filled(track);
        GROUND
            .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
            .filter(|&(x, y)| pixel(&frame, x, y, BLACK) == RIDDEN)
            .count()
    };
    assert_eq!(ridden_px(&before), 0, "before joining");
    assert_eq!(ridden_px(&along), 0, "following");
    assert!(ridden_px(&free(&ridden)) > 0, "a free ride");
}

const AHEAD_OF_THE_PUCK: core::ops::Range<usize> = GROUND.start..205;
const SOUTH: u16 = TURN / 2;

fn blue_columns(frame: &Frame, y: usize) -> std::vec::Vec<usize> {
    (0..WIDTH)
        .filter(|&x| pixel(frame, x, y, BLACK) == ACCENT)
        .collect()
}

fn blue_in(frame: &Frame, xs: core::ops::Range<usize>, ys: core::ops::Range<usize>) -> usize {
    ys.flat_map(|y| xs.clone().map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(frame, x, y, BLACK) == ACCENT)
        .count()
}

fn there_and_back(lane_m: f32) -> std::vec::Vec<Crumb> {
    follower_course(&[
        (0.0, -100.0),
        (0.0, 1_000.0),
        (lane_m, 1_000.0),
        (lane_m, -100.0),
    ])
}

#[test]
fn a_rider_between_the_way_out_and_back_sees_the_way_out_on_every_fix() {
    let c = there_and_back(6.0);
    let mut frame = Frame::default();
    for x in [2.0, 4.0, 2.0, 4.0] {
        fill(&mut frame, &joining(&c, (x, 0.0)));
        assert!(
            route_in(&frame, AHEAD_OF_THE_PUCK).is_some(),
            "{x} m east: ahead"
        );
        assert_eq!(
            route_in(&frame, BEHIND_THE_PUCK),
            None,
            "{x} m east: behind"
        );
    }
}

#[test]
fn on_one_road_out_and_back_the_way_the_rider_rides_is_the_route() {
    let c = there_and_back(0.0);
    for heading in [0, SOUTH] {
        let mut track = joining(&c, (0.0, 0.0));
        track.heading = Some(heading);
        let frame = filled(&track);
        assert!(
            route_in(&frame, AHEAD_OF_THE_PUCK).is_some(),
            "heading {heading}: ahead"
        );
        assert_eq!(
            route_in(&frame, BEHIND_THE_PUCK),
            None,
            "heading {heading}: behind"
        );
    }
}

#[test]
fn between_two_roads_the_same_way_the_route_holds_its_road_until_another_is_10_m_closer_on_2_fixes()
{
    let c = follower_course(&[
        (0.0, -100.0),
        (0.0, 1_000.0),
        (300.0, 1_000.0),
        (300.0, -100.0),
        (30.0, -100.0),
        (30.0, 1_000.0),
    ]);
    let road_x = |frame: &Frame| {
        let columns = blue_columns(frame, 150);
        assert!(!columns.is_empty(), "a route ahead");
        columns.iter().sum::<usize>() / columns.len()
    };
    let mut frame = Frame::default();
    for x in [14.0, 16.0, 14.0, 16.0] {
        fill(&mut frame, &joining(&c, (x, 0.0)));
        assert!(road_x(&frame) < 120, "{x} m east: the first road");
    }
    fill(&mut frame, &joining(&c, (22.0, 0.0)));
    assert!(
        road_x(&frame) < 120,
        "22 m east, one fix 14 m nearer the second road"
    );
    fill(&mut frame, &joining(&c, (24.0, 0.0)));
    assert!(
        road_x(&frame) > 120,
        "24 m east, the second fix nearer the second road"
    );
}

#[test]
fn standing_still_before_joining_the_route_keeps_its_leg() {
    let c = there_and_back(6.0);
    let mut frame = Frame::default();
    fill(&mut frame, &joining(&c, (2.0, 0.0)));
    let mut standing = joining(&c, (4.0, 0.0));
    standing.heading = None;
    fill(&mut frame, &standing);
    assert!(route_in(&frame, AHEAD_OF_THE_PUCK).is_some(), "ahead");
    assert_eq!(route_in(&frame, BEHIND_THE_PUCK), None, "behind");
}

#[test]
fn after_following_stops_the_route_keeps_the_leg_it_followed() {
    let c = there_and_back(6.0);
    let mut frame = Frame::default();
    fill(&mut frame, &on_course(&c, (0.0, 0.0), 100.0));
    let mut stopped = joining(&c, (4.0, 0.0));
    stopped.heading = None;
    fill(&mut frame, &stopped);
    assert!(route_in(&frame, AHEAD_OF_THE_PUCK).is_some(), "ahead");
    assert_eq!(route_in(&frame, BEHIND_THE_PUCK), None, "behind");
}

#[test]
fn a_route_that_jogs_after_its_turn_still_reaches_the_top_edge() {
    let c = follower_course(&MOCKUP_COURSE);
    let mut track = following(&c, (0.0, 20.0), 420.0, None);
    track.reach = settled_at(10_000);
    let frame = filled(&track);
    assert!(
        route_in(&frame, GROUND.start..GROUND.start + 1).is_some(),
        "the route at the map's top row"
    );
}

#[test]
fn a_turn_back_beyond_the_top_of_the_map_draws_none_of_its_way_back() {
    let c = follower_course(&[(0.0, -100.0), (0.0, 400.0), (20.0, -100.0)]);
    let frame = filled(&on_course(&c, (0.0, 0.0), 100.0));
    assert_eq!(pixel(&frame, 120, 150, BLACK), ACCENT, "the way to it");
    assert_eq!(route_in(&frame, BEHIND_THE_PUCK), None, "the way back");
}

#[test]
fn a_turn_back_on_the_map_draws_its_way_back() {
    let c = follower_course(&[(0.0, -100.0), (0.0, 150.0), (20.0, -100.0)]);
    let frame = filled(&on_course(&c, (0.0, 0.0), 100.0));
    assert_eq!(pixel(&frame, 120, 150, BLACK), ACCENT, "the way to it");
    assert!(
        route_in(&frame, BEHIND_THE_PUCK).is_some(),
        "the way back, past the puck"
    );
}

#[test]
fn off_course_from_a_junction_below_the_map_the_route_runs_up_past_the_rider() {
    let c = follower_course(&[(0.0, -200.0), (0.0, -150.0), (0.0, 0.0), (0.0, 200.0)]);
    let junction = Spot {
        along_mm: 0,
        at: (0.0, -200.0),
    };
    let frame = filled(&following(&c, (0.0, 0.0), 200.0, Some(junction)));
    assert_eq!(
        frame.junction_quarter_px, None,
        "the junction is off the map"
    );
    assert_eq!(pixel(&frame, 120, 150, BLACK), ACCENT, "the course ahead");
}

#[test]
fn a_turn_past_the_edge_of_the_map_draws_its_road_coming_back_onto_it() {
    let c = follower_course(&[
        (0.0, -100.0),
        (0.0, 0.0),
        (200.0, 80.0),
        (210.0, 120.0),
        (170.0, 160.0),
        (170.0, 240.0),
    ]);
    let mut track = on_course(&c, (0.0, 0.0), 100.0);
    let total = c.last().unwrap().along;
    let turn_mm = ((total - c[1].along) * 1_000.0) as u32;
    track.guidance = Some(Guidance {
        sign: Sign::Chip {
            glyph: Glyph::Left,
            distance: Distance::from_mm(turn_mm - 100_000),
        },
        turn: Some((
            Spot {
                along_mm: turn_mm,
                at: (170.0, 160.0),
            },
            Glyph::Left,
        )),
        junction: None,
    });
    let frame = filled(&track);
    assert!(frame.arrow.strokes > 0, "the turn's arrow");
    assert!(
        blue_in(&frame, 223..WIDTH, 75..115) > 0,
        "the road after the turn, back on the map"
    );
}

const RIDING_MM_S: u32 = 4_167;
const STANDING: Option<u32> = Some(0);
const RIDING: Option<u32> = Some(RIDING_MM_S);

fn degrees(d: f32) -> u16 {
    (d * f32::from(TURN) / 360.0) as u16
}

fn blue_at(frame: &Frame, here: Point, world: Point) -> usize {
    let (x, y) = frame.last_pose.projection(here).at(world);
    let (x, y) = (x as usize, y as usize);
    blue_in(frame, x - 4..x + 5, y - 4..y + 5)
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Way {
    Out,
    Back,
}

const ALONG_EACH_WAY_M: f32 = 60.0;

fn way_drawn(frame: &Frame, here: Point, lane_m: f32) -> Way {
    match (
        blue_at(frame, here, (0.0, here.1 + ALONG_EACH_WAY_M)),
        blue_at(frame, here, (lane_m, here.1 - ALONG_EACH_WAY_M)),
    ) {
        (0, 0) => panic!("neither way drawn"),
        (_, 0) => Way::Out,
        (0, _) => Way::Back,
        _ => panic!("both ways drawn"),
    }
}

struct Rider<'a> {
    homing: Homing<'a>,
    map: Cache<'static>,
    placement: Placement,
    now: u64,
}

impl<'a> Rider<'a> {
    fn new(storage: &'a mut [Crumb]) -> Self {
        Self {
            homing: Homing::new(storage),
            map: Cache::new(std::boxed::Box::leak(std::boxed::Box::default())),
            placement: Placement::default(),
            now: 0,
        }
    }

    fn reports(&mut self, (east, north): Point) {
        const METRES_PER_E7: f32 = 0.011_119_508;
        self.now += 1_000;
        let location_e7 = (
            libm::roundf(north / METRES_PER_E7) as i32,
            libm::roundf(east / METRES_PER_E7) as i32,
        );
        self.homing.observe(
            Some(Reported {
                location_e7,
                taken_loop_ms: self.now,
            }),
            self.now,
        );
    }

    fn shows(&mut self, course: &[Crumb], speed_mm_s: Option<u32>) -> Point {
        let here = self.homing.position().expect("a fix");
        let mut track = joining(course, here);
        track.placement = self.placement;
        track.here_taken_ms = self.homing.position_taken_ms();
        track.heading = self.homing.heading();
        track.speed_mm_s = speed_mm_s;
        self.map.update(&track);
        here
    }

    fn sees(&mut self, course: &[Crumb], lane_m: f32, speed_mm_s: Option<u32>) -> Way {
        let here = self.shows(course, speed_mm_s);
        way_drawn(self.map.frame, here, lane_m)
    }

    fn rides_and_sees(
        &mut self,
        course: &[Crumb],
        lane_m: f32,
        at: Point,
        speed_mm_s: Option<u32>,
    ) -> Way {
        self.reports(at);
        self.sees(course, lane_m, speed_mm_s)
    }

    fn places_a_course(&mut self) {
        self.placement = self.placement.next();
    }

    fn holds_the_way_back(&mut self, course: &[Crumb]) {
        for north in [0.0, 20.0, 40.0, 60.0] {
            assert_eq!(
                self.rides_and_sees(course, 6.0, (0.0, north), RIDING),
                Way::Out
            );
        }
        let back = [40.0, 30.0, 20.0, 10.0]
            .into_iter()
            .map(|north| self.rides_and_sees(course, 6.0, (0.0, north), RIDING))
            .last();
        assert_eq!(back, Some(Way::Back), "turned back");
    }
}

#[test]
fn headings_across_the_held_leg_never_switch_it_standing_or_riding() {
    let c = there_and_back(6.0);
    let mut frame = Frame::default();
    for (fix, d) in [85.0, 95.0, 85.0, 95.0, 85.0, 275.0, 85.0, 275.0]
        .into_iter()
        .enumerate()
    {
        let mut track = joining(&c, (3.0, 0.0));
        track.heading = Some(degrees(d));
        fill(&mut frame, &track);
        assert!(
            way_drawn(&frame, (3.0, 0.0), 6.0) == Way::Out,
            "standing, fix {fix} at {d}°"
        );
    }
    for (fix, d) in [85.0, 95.0, 85.0, 95.0, 85.0, 95.0].into_iter().enumerate() {
        let here = (3.0, 4.0 * fix as f32);
        let mut track = joining(&c, here);
        track.heading = Some(degrees(d));
        track.speed_mm_s = RIDING;
        fill(&mut frame, &track);
        assert!(
            way_drawn(&frame, here, 6.0) == Way::Out,
            "riding, fix {fix} at {d}°"
        );
    }
}

const JITTER: [Point; 5] = [(0.0, 0.0), (16.0, 1.4), (0.0, 2.8), (16.0, 1.4), (0.0, 0.0)];

#[test]
fn a_stop_s_gps_jitter_never_switches_the_held_leg() {
    for (lane_m, speed_mm_s) in [(6.0, None), (6.0, STANDING), (20.0, STANDING)] {
        let c = there_and_back(lane_m);
        let mut storage = [Crumb::default(); 64];
        let mut rider = Rider::new(&mut storage);
        let mut drawn = 0;
        for (fix, at) in JITTER.into_iter().enumerate() {
            rider.reports(at);
            if rider.homing.heading().is_none() {
                continue;
            }
            assert!(
                rider.sees(&c, lane_m, speed_mm_s) == Way::Out,
                "legs {lane_m} m apart at {speed_mm_s:?} mm/s, fix {fix} at {at:?}"
            );
            drawn += 1;
        }
        assert_eq!(drawn, 4, "every fix after the first has a heading");
    }
}

#[test]
fn standing_jitter_across_25_m_from_the_held_leg_never_switches_it() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    for at in [(0.0, 0.0), (24.0, -40.0), (24.0, -20.0)] {
        rider.reports(at);
    }
    assert!(
        rider.rides_and_sees(&c, 6.0, (24.0, 0.0), STANDING) == Way::Out,
        "24 m off"
    );
    assert!(
        rider.rides_and_sees(&c, 6.0, (26.0, -1.0), STANDING) == Way::Out,
        "26 m off"
    );
}

#[test]
fn turning_back_then_stopping_and_riding_across_never_switches_the_held_leg() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    for (at, speed_mm_s) in [
        ((0.0, 0.0), RIDING),
        ((0.0, 20.0), RIDING),
        ((0.0, 40.0), RIDING),
        ((0.0, 60.0), RIDING),
        ((0.0, 40.0), RIDING),
        ((0.0, 36.0), RIDING),
        ((0.0, 36.0), STANDING),
        ((16.0, 40.0), RIDING),
        ((0.0, 20.0), RIDING),
    ] {
        assert!(
            rider.rides_and_sees(&c, 6.0, at, speed_mm_s) == Way::Out,
            "at {at:?}, {speed_mm_s:?} mm/s"
        );
    }
}

fn fixes_to_turn_back(kmh: f32) -> Option<usize> {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    let speed_mm_s = Some((kmh / 3.6 * 1_000.0) as u32);
    let step_m = kmh / 3.6;
    for fix in 0..=30 {
        let at = (0.0, step_m * fix as f32);
        assert!(
            rider.rides_and_sees(&c, 6.0, at, speed_mm_s) == Way::Out,
            "{kmh} km/h out, fix {fix}"
        );
    }
    let mut switched = None;
    for fix in 1..=30 {
        let at = (0.0, step_m * (30 - fix) as f32);
        let back = rider.rides_and_sees(&c, 6.0, at, speed_mm_s) == Way::Back;
        match switched {
            None if back => switched = Some(fix),
            Some(on) => assert!(back, "{kmh} km/h: back on fix {on}, out again on {fix}"),
            None => {}
        }
    }
    std::println!("{kmh} km/h U-turn: the way back from fix {switched:?}");
    switched
}

#[test]
fn a_u_turn_before_joining_switches_to_the_way_back_and_keeps_it() {
    for (kmh, by_fix) in [(5.0, 24), (8.0, 11), (15.0, 8), (30.0, 4)] {
        let switched = fixes_to_turn_back(kmh);
        assert!(
            switched.is_some_and(|fix| fix <= by_fix),
            "{kmh} km/h: the way back from fix {switched:?}, by {by_fix}"
        );
    }
}

#[test]
fn riding_across_to_the_course_then_along_it_draws_one_way_then_the_way_out() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    rider.reports((0.0, 0.0));
    rider.reports((-80.0, 0.0));
    let across: std::vec::Vec<Way> = [-60.0, -40.0, -20.0, -4.0]
        .into_iter()
        .map(|east| rider.rides_and_sees(&c, 6.0, (east, 0.0), RIDING))
        .collect();
    assert!(
        across.windows(2).all(|w| w[0] == w[1]),
        "one way while riding across: {across:?}"
    );
    let along: std::vec::Vec<Way> = [4.0, 8.0, 12.0, 16.0, 20.0, 24.0, 28.0, 32.0]
        .into_iter()
        .map(|north| rider.rides_and_sees(&c, 6.0, (0.0, north), RIDING))
        .collect();
    let first_out = along
        .iter()
        .position(|&way| way == Way::Out)
        .expect("the way out");
    assert!(
        along[first_out..].iter().all(|&way| way == Way::Out),
        "the way out holds once drawn: {along:?}"
    );
    assert!(first_out <= 4, "the way out by fix {first_out}: {along:?}");
}

#[test]
fn a_stop_between_each_fix_the_other_way_never_turns_the_held_leg_back() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    for north in [0.0, 20.0, 40.0, 60.0] {
        assert_eq!(
            rider.rides_and_sees(&c, 6.0, (0.0, north), RIDING),
            Way::Out
        );
    }
    for north in [40.0, 36.0, 20.0] {
        for speed_mm_s in [RIDING, STANDING] {
            assert_eq!(
                rider.rides_and_sees(&c, 6.0, (0.0, north), speed_mm_s),
                Way::Out,
                "{north} m north at {speed_mm_s:?} mm/s"
            );
        }
    }
}

#[test]
fn a_stop_between_two_fixes_far_from_the_held_leg_starts_their_count_again() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    for north in [0.0, 20.0, 40.0, 60.0] {
        assert_eq!(
            rider.rides_and_sees(&c, 6.0, (0.0, north), RIDING),
            Way::Out
        );
    }
    for (at, speed_mm_s, way) in [
        ((30.0, 60.0), RIDING, Way::Out),
        ((30.0, 60.0), STANDING, Way::Out),
        ((30.0, 64.0), RIDING, Way::Out),
        ((30.0, 68.0), RIDING, Way::Back),
    ] {
        assert_eq!(
            rider.rides_and_sees(&c, 6.0, at, speed_mm_s),
            way,
            "at {at:?}, {speed_mm_s:?} mm/s"
        );
    }
}

#[test]
fn a_course_removed_and_loaded_again_forgets_the_leg_it_held() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    rider.holds_the_way_back(&c);
    rider.places_a_course();
    rider.shows(&[], RIDING);
    for north in [30.0, 40.0] {
        rider.reports((0.0, north));
        rider.shows(&[], RIDING);
    }
    rider.places_a_course();
    assert_eq!(
        rider.sees(&c, 6.0, RIDING),
        Way::Out,
        "riding north on the way out"
    );
}

#[test]
fn a_new_ride_forgets_the_leg_the_last_one_held() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    rider.holds_the_way_back(&c);
    rider.homing.reset();
    rider.map.start_ride();
    assert_eq!(
        rider.rides_and_sees(&c, 6.0, (0.0, 0.0), STANDING),
        Way::Out,
        "the first fix, on the way out"
    );
}

#[test]
fn drifting_at_6_kmh_onto_a_road_26_m_away_and_riding_it_moves_the_route_onto_it() {
    let c = follower_course(&[
        (0.0, -1_000.0),
        (0.0, 1_000.0),
        (1_000.0, 1_000.0),
        (1_000.0, -1_000.0),
        (26.0, -1_000.0),
        (26.0, 1_000.0),
    ]);
    const SLOW: Option<u32> = Some(1_667);
    let road = |rider: &Rider, here: Point| {
        let (first, second) = (
            blue_at(rider.map.frame, here, (0.0, here.1 + 60.0)),
            blue_at(rider.map.frame, here, (26.0, here.1 + 60.0)),
        );
        assert!((first > 0) != (second > 0), "one road at {here:?}");
        if first > 0 { 0 } else { 26 }
    };
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    for north in [0.0, 20.0, 40.0, 60.0] {
        rider.reports((0.0, north));
        let here = rider.shows(&c, RIDING);
        assert_eq!(road(&rider, here), 0, "riding the first road");
    }
    let step_m = 6.0 / 3.6;
    let drift = (1..=16).map(|fix| (fix as f32 * step_m, 60.0));
    let along = (1..=20).map(|fix| (26.0, 60.0 + fix as f32 * step_m));
    let roads: std::vec::Vec<u32> = drift
        .chain(along)
        .map(|at| {
            rider.reports(at);
            let here = rider.shows(&c, SLOW);
            road(&rider, here)
        })
        .collect();
    let moved = roads
        .iter()
        .position(|&x| x == 26)
        .expect("the second road");
    assert!(
        roads[moved..].iter().all(|&x| x == 26),
        "the second road holds: {roads:?}"
    );
    assert!(
        moved < 16,
        "the second road before the drift reaches it, not fix {moved}: {roads:?}"
    );
}

#[test]
fn fixes_without_a_speed_never_count_travel_the_fixes_with_one_left_short() {
    let c = there_and_back(6.0);
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    for north in [0.0, 20.0, 40.0, 60.0] {
        assert_eq!(
            rider.rides_and_sees(&c, 6.0, (0.0, north), RIDING),
            Way::Out
        );
    }
    const CRAWL: Option<u32> = Some(1_400);
    for (north, speed_mm_s) in [
        (60.0, STANDING),
        (61.5, CRAWL),
        (63.0, None),
        (61.5, CRAWL),
        (60.0, None),
        (61.5, CRAWL),
        (63.0, None),
    ] {
        assert_eq!(
            rider.rides_and_sees(&c, 6.0, (30.0, north), speed_mm_s),
            Way::Out,
            "1.5 m to {north} m north at {speed_mm_s:?} mm/s"
        );
    }
}

#[test]
fn a_course_replaced_by_the_same_one_1_m_east_forgets_the_leg_it_held() {
    let c = there_and_back(6.0);
    let mut moved = c.clone();
    for crumb in &mut moved {
        crumb.east += 1.0;
    }
    let mut storage = [Crumb::default(); 64];
    let mut rider = Rider::new(&mut storage);
    rider.holds_the_way_back(&c);
    for north in [30.0, 50.0, 70.0] {
        rider.reports((0.0, north));
    }
    rider.places_a_course();
    assert_eq!(
        rider.sees(&moved, 6.0, STANDING),
        Way::Out,
        "stopped, heading north on the way out"
    );
}

mod santiago {
    use super::*;
    use crate::ride::homing::TURN;
    use crate::screens::ride::map::roads::tests::{PLAZA, SANTIAGO, cached, header};
    use std::time::Instant;
    use std::{boxed::Box, println, string::String, vec::Vec};
    use terrain::pack::HEADER_BYTES;
    use terrain::{PAGE_BYTES, TileKey};

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
                here_taken_ms: None,
                heading: Some(self.heading),
                speed_mm_s: None,
                course: &self.course,
                placement: Placement::default(),
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
                reach: Reach::default(),
                radar: false,
            }
        }
    }

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
        let track = Track {
            reach: settled_at(8_375),
            ..approach.track()
        };
        let mut frame = Box::new(Frame::default());
        let mut cache = Cache::new(&mut frame);
        settle(&mut cache, &mut engine, ride, &track);
        let frame = &*cache.frame;
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

    #[test]
    fn zeroed_memory_is_an_empty_map_that_fills_and_fetches_as_a_new_one() {
        let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
        let mut engine = cached(&mut arena);
        let ride = RideFrame::at(PLAZA);
        let approach = Approach::new((0.0, -150.0), 0, 250.0);
        let track = approach.track();
        let mut zeroed: Box<Frame> = unsafe { Box::new_zeroed().assume_init() };
        let mut fresh = Box::new(Frame::default());
        assert_eq!(painted(&zeroed), painted(&fresh));
        for frame in [&mut zeroed, &mut fresh] {
            settle(&mut Cache::new(frame), &mut engine, ride, &track);
        }
        assert!(road_pixels(&zeroed) > 1_000);
        assert_eq!(painted(&zeroed), painted(&fresh));
    }

    #[test]
    fn the_margin_never_crowds_a_tile_on_the_screen_out_of_the_fetch() {
        let chile = terrain::Header {
            tile: 7_812,
            south: -56_000_000,
            west: -76_000_000,
            rows: 4_942,
            cols: 1_280,
            ..header()
        };
        let ride = RideFrame::at((-531_600_000, -709_000_000));
        let leg = 2_000.0 / core::f32::consts::SQRT_2;
        let course = [
            Crumb {
                east: leg,
                north: leg,
                along: 0.0,
            },
            Crumb {
                east: 0.0,
                north: 0.0,
                along: 2_000.0,
            },
        ];
        let track = Track {
            available: true,
            here: Some((0.0, 0.0)),
            heading: Some(TURN / 8),
            course: &course,
            ahead: Some((1, Some((0.0, 0.0)))),
            reach: settled_at(30_000),
            ..Track::default()
        };
        let mut frame = Box::new(Frame::default());
        fill(&mut frame, &track);
        let under = |margin: f32| -> Vec<TileKey> {
            let corners = frame.view(margin).unwrap().map(|p| ride.locate(p));
            chile.tiles_under(&corners).collect()
        };
        assert!(
            under(ROAD_FETCH_MARGIN).len() > roads::TILES,
            "the view and its margin touch more tiles than are held"
        );
        let fetched = frame.road_tiles(&chile, ride).unwrap();
        for key in under(0.0) {
            assert!(fetched.contains(&key), "{key:?} is on the screen");
        }
    }

    fn without_roads() -> Vec<u8> {
        let header = header();
        let mut pack = SANTIAGO.to_vec();
        pack[header.directory as usize..header.data as usize].fill(0);
        pack[52] += 1;
        let crc = terrain::tile::crc32(&pack[..HEADER_BYTES - 4]);
        pack[HEADER_BYTES - 4..HEADER_BYTES].copy_from_slice(&crc.to_le_bytes());
        pack
    }

    fn ride_on(
        cache: &mut Cache,
        engine: &mut terrain::Lookahead,
        mut pack: Option<&[u8]>,
        ride: RideFrame,
        track: &Track,
    ) {
        for _ in 0..400 {
            cache.update(track);
            if let Some(source) = pack.as_mut() {
                engine.step(source);
            }
            cache.fetch_roads(engine, ride);
        }
        cache.update(track);
    }

    fn road_pixels(frame: &Frame) -> usize {
        painted(frame)
            .iter()
            .filter(|&&p| p == MINOR_ROAD || p == MAJOR_ROAD)
            .count()
    }

    #[test]
    fn a_replaced_or_removed_pack_takes_its_roads_off_the_map() {
        let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
        let mut engine = cached(&mut arena);
        let ride = RideFrame::at(PLAZA);
        let approach = Approach::new((0.0, -150.0), 0, 250.0);
        let track = approach.track();
        let mut frame = Box::new(Frame::default());
        let mut cache = Cache::new(&mut frame);
        settle(&mut cache, &mut engine, ride, &track);
        assert!(road_pixels(cache.frame) > 1_000);
        let empty = without_roads();
        engine.reset();
        ride_on(&mut cache, &mut engine, Some(&empty), ride, &track);
        assert_eq!(
            road_pixels(cache.frame),
            0,
            "a newer pack without roads here draws none"
        );
        engine.reset();
        ride_on(&mut cache, &mut engine, Some(SANTIAGO), ride, &track);
        assert!(
            road_pixels(cache.frame) > 1_000,
            "the pack's roads are back"
        );
        engine.reset();
        ride_on(&mut cache, &mut engine, None, ride, &track);
        assert_eq!(road_pixels(cache.frame), 0, "with no pack, bare ground");
    }

    #[test]
    #[ignore]
    fn densest_views_of_central_santiago() {
        let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
        let mut engine = cached(&mut arena);
        let ride = RideFrame::at(PLAZA);
        println!(
            "ahead m  turn m  east  north  heading  minor  major  segments  fill µs  paint µs  without roads µs"
        );
        for (speed_mm_s, turn_m) in [(1_400, 60.0), (7_500, 250.0), (25_000, 500.0)] {
            let reach = settled_at(speed_mm_s);
            let mut densest = (0, String::new());
            for east in (-1_600..=1_600).step_by(400) {
                for north in (-2_000..=1_200).step_by(400) {
                    for heading in [0, TURN / 8, TURN / 4, 3 * TURN / 8] {
                        let approach = Approach::new((east as f32, north as f32), heading, turn_m);
                        let track = Track {
                            reach,
                            ..approach.track()
                        };
                        let mut frame = Box::new(Frame::default());
                        let mut cache = Cache::new(&mut frame);
                        settle(&mut cache, &mut engine, ride, &track);
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
                            "{reach:?}  {turn_m}  {east}  {north}  {heading}  {}  {}  {segments}  {fill_us:.0}  {paint_us:.0}  {bare_us:.0}",
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
