use super::*;
use crate::ride::course::nav::Distance;
use crate::ride::homing::TURN;
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
    for radar in [false, true] {
        for back_m in [1_700.0, 2_500.0, 3_250.0, 20_000.0, 150_000.0] {
            let here = (0.0, 250.0 + back_m);
            let mut track = following(&c, here, 650.0, Some(junction));
            let sign = Sign::OffCourse {
                back: Distance::from_mm((back_m * 1000.0) as u32),
            };
            let badge = crate::screens::sign::on_page(RidePage::Map, sign, radar)
                .unwrap()
                .rect();
            track.guidance.as_mut().unwrap().sign = sign;
            track.badge = Some(badge);
            track.radar = radar;
            let frame = filled(&track);
            let pose = frame.last_pose;
            let what = std::format!("radar {radar}, {back_m} m back, {pose:?}");
            assert_eq!(pose.step, Step::Km3, "{what}");
            let rider = (f32::from(pose.rider.0), f32::from(pose.rider.1));
            let towards = pose.projection(here).at(junction.at);
            let (dx, dy) = (towards.0 - rider.0, towards.1 - rider.1);
            let length = libm::sqrtf(dx * dx + dy * dy);
            let (ux, uy) = (dx / length, dy / length);
            let ring = JUNCTION_RINGS_QUARTER_PX[0].0 as f32 / 4.0 + 1.0;
            let chevrons: std::vec::Vec<(usize, usize)> = GROUND
                .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
                .filter(|&(x, y)| {
                    let away = |(cx, cy): Point| {
                        let (x, y) = (x as f32 - cx, y as f32 - cy);
                        libm::sqrtf(x * x + y * y)
                    };
                    away(rider) > PUCK_CLEAR + 1.0 && away(towards) > ring
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
                "{what}: three chevrons span over two pitches, these {reach} px"
            );
            assert!(
                ahead.iter().all(|&a| a > 0.0) && beside <= CHEVRON_HALF + CHEVRON_STROKE,
                "{what}: amber off the straight line to the junction, {beside} px aside"
            );
            let hidden: std::vec::Vec<&(usize, usize)> = chevrons
                .iter()
                .filter(|&&(x, y)| badge.contains(x, y) || x < map_left(radar))
                .collect();
            assert!(
                hidden.is_empty(),
                "{what}: chevrons under the badge or the radar strip at {hidden:?}"
            );
            assert_eq!(
                frame.junction_quarter_px.is_none(),
                back_m > 3_200.0,
                "{what}: the ring sinks off the screen past 3.2 km"
            );
        }
    }
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
                let stroke = &frame.strokes[usize::from(i)];
                let (first, last) = bands(stroke, line.reach()).unwrap();
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

struct Approach {
    along_m: f32,
    frame: Frame,
    up_to_turn: Frame,
    past_turn: Frame,
    badge: Rect,
}

fn ride_to_the_last_turn(points: &[Point]) -> std::vec::Vec<Approach> {
    let course = follower_course(points);
    let total = course.last().unwrap().along;
    let corners = track_crumbs(points);
    let mut frame = Frame::default();
    let mut fixes = std::vec::Vec::new();
    let last_turn = corners[corners.len() - 2].along;
    for k in 1.. {
        let along_m = 10.0 * k as f32;
        if along_m >= last_turn {
            return fixes;
        }
        let leg = corners.partition_point(|c| c.along <= along_m);
        let (from, to) = (corners[leg - 1], corners[leg]);
        let t = (along_m - from.along) / (to.along - from.along);
        let here = (
            from.east + t * (to.east - from.east),
            from.north + t * (to.north - from.north),
        );
        let bearing = |a: Crumb, b: Crumb| libm::atan2f(b.east - a.east, b.north - a.north);
        let heading = bearing(from, to) / core::f32::consts::TAU * f32::from(TURN);
        let after = corners[leg + 1];
        let cross = (to.east - from.east) * (after.north - to.north)
            - (to.north - from.north) * (after.east - to.east);
        let glyph = if cross > 0.0 {
            Glyph::Left
        } else {
            Glyph::Right
        };
        let sign = Sign::Chip {
            glyph,
            distance: Distance::from_mm(((to.along - along_m) * 1000.0) as u32),
        };
        let badge = crate::screens::sign::on_page(RidePage::Map, sign, false)
            .unwrap()
            .rect();
        let turn = Spot {
            along_mm: (to.along * 1000.0) as u32,
            at: (to.east, to.north),
        };
        let end = course
            .partition_point(|c| c.along < total - along_m)
            .clamp(1, course.len() - 1);
        let track = Track {
            available: true,
            crumbs: &[],
            here: Some(here),
            heading: Some(libm::roundf(heading.rem_euclid(f32::from(TURN))) as u16),
            course: &course,
            ahead: Some((end, Some(here))),
            guidance: Some(Guidance {
                sign,
                turn: Some((turn, glyph)),
                junction: None,
            }),
            left_m: Some(1_000),
            badge: Some(badge),
            radar: false,
        };
        fill(&mut frame, &track);
        let at = frame.last_pose.projection(here);
        let split = track.split().unwrap();
        let only = |route: &mut dyn Iterator<Item = Point>, keep_marks: bool| {
            let mut alone = frame.clone();
            alone.counts[Line::Route as usize] = 0;
            alone.add(Line::Route, &at, route);
            if !keep_marks {
                alone.counts[Line::Ridden as usize] = 0;
                alone.arrow.strokes = 0;
                alone.puck = None;
            }
            bucket(&mut alone);
            alone
        };
        let ahead = || track.ahead_points(split);
        let up_to_turn = only(
            &mut ahead()
                .take_while(|&(_, before)| before)
                .map(|(p, _)| p)
                .chain([turn.at]),
            true,
        );
        let past_turn = only(
            &mut [turn.at]
                .into_iter()
                .chain(ahead().skip_while(|&(_, before)| before).map(|(p, _)| p)),
            false,
        );
        fixes.push(Approach {
            along_m,
            frame: frame.clone(),
            up_to_turn,
            past_turn,
            badge,
        });
    }
    unreachable!()
}

const LONG_LEFT: [Point; 4] = [
    (0.0, 0.0),
    (0.0, 4_200.0),
    (-700.0, 4_200.0),
    (-700.0, 8_000.0),
];
const TWO_150_LEFT_THEN_RIGHT: [Point; 6] = [
    (0.0, 0.0),
    (0.0, 1_500.0),
    (-150.0, 1_500.0),
    (-150.0, 1_650.0),
    (0.0, 1_650.0),
    (0.0, 4_000.0),
];

fn gap_to(badge: Rect, frame: &Frame) -> Option<(usize, (usize, usize))> {
    GROUND
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .filter(|&(x, y)| pixel(frame, x, y, BLACK) != GROUND_COLOR)
        .map(|(x, y)| {
            let dx = badge
                .left
                .saturating_sub(x)
                .max((x + 1).saturating_sub(badge.right));
            let dy = badge
                .top
                .saturating_sub(y)
                .max((y + 1).saturating_sub(badge.bottom));
            (dx.max(dy), (x, y))
        })
        .min()
}

#[test]
fn on_the_way_to_a_turn_nothing_up_to_it_comes_near_the_badge() {
    for (name, course) in [
        ("long left", &LONG_LEFT[..]),
        ("two150lr", &TWO_150_LEFT_THEN_RIGHT[..]),
    ] {
        let near: std::vec::Vec<(f32, Pose, (usize, (usize, usize)))> =
            ride_to_the_last_turn(course)
                .iter()
                .filter_map(|fix| {
                    let gap = gap_to(fix.badge, &fix.up_to_turn)?;
                    (gap.0 + 1 < pose::CLEARANCE as usize).then_some((
                        fix.along_m,
                        fix.frame.last_pose,
                        gap,
                    ))
                })
                .collect();
        assert!(
            near.is_empty(),
            "{name}: {} fixes come within {} px of the badge, e.g. (m, pose, (gap, px)) {:?}",
            near.len(),
            pose::CLEARANCE,
            &near
        );
    }
}

fn under_the_rounded_badge(badge: Rect, (x, y): (usize, usize)) -> bool {
    let r = BADGE_RADIUS as f32;
    let (cx, cy) = (x as f32 + 0.5, y as f32 + 0.5);
    let nearest_x = cx.clamp(badge.left as f32 + r, badge.right as f32 - r);
    let nearest_y = cy.clamp(badge.top as f32 + r, badge.bottom as f32 - r);
    let (dx, dy) = (cx - nearest_x, cy - nearest_y);
    badge.contains(x, y) && dx * dx + dy * dy <= r * r
}

#[test]
fn on_a_long_left_approach_the_line_past_the_turn_is_under_the_badge_on_one_stretch_a_step() {
    let is_line = |c: u16| {
        let (r, b) = (u32::from(c >> 11) << 3, u32::from(c & 31) << 3);
        r < 0x60 && b >= 0x90
    };
    let covered: std::vec::Vec<(usize, Step, f32)> = ride_to_the_last_turn(&LONG_LEFT)
        .iter()
        .enumerate()
        .filter(|(_, fix)| {
            GROUND
                .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
                .filter(|&p| under_the_rounded_badge(fix.badge, p))
                .any(|(x, y)| is_line(pixel(&fix.past_turn, x, y, BLACK)))
        })
        .map(|(k, fix)| (k, fix.frame.last_pose.step, fix.along_m))
        .collect();
    assert!(covered.len() <= 34, "{} fixes: {covered:?}", covered.len());
    for step in Step::ALL {
        let fixes: std::vec::Vec<usize> = covered
            .iter()
            .filter(|c| c.1 == step)
            .map(|c| c.0)
            .collect();
        assert!(
            fixes.windows(2).all(|w| w[1] == w[0] + 1),
            "at {step:?} the column flickers on the line past the turn: {covered:?}"
        );
    }
}
