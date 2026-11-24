//! The Stats page in both looks.
use crate::compose::data::Metric;
use crate::compose::node::Flags;
use crate::ride::log::{Sample, Status};
use crate::screens::Device;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{Battery, Gps};
use crate::screens::panels::ALL;
use crate::screens::ride::RidePage;
use crate::screens::theme::{DARK, LIGHT, Theme};
use crate::screens::workout::{Page, Text, View};
use std::string::String;
use std::vec::Vec;

/// One extreme reading of each kind the page shows.
#[derive(Clone, Copy)]
struct Case {
    speed_mm_s: Option<u32>,
    watts: Option<u16>,
    bpm: Option<u16>,
    cadence_tenths: Option<u16>,
    distance_mm: u64,
    secs: u64,
    grade_tenths: Option<i16>,
}

const RIDE: Case = Case {
    speed_mm_s: Some(8_722),
    watts: Some(200),
    bpm: Some(142),
    cadence_tenths: Some(920),
    distance_mm: 42_700_000,
    secs: 5_025,
    grade_tenths: Some(24),
};

const WIDEST: Case = Case {
    speed_mm_s: Some(27_750),
    watts: Some(1999),
    bpm: Some(199),
    cadence_tenths: Some(1990),
    distance_mm: 999_990_000,
    secs: 359_999,
    grade_tenths: Some(-299),
};

fn cases() -> Vec<Case> {
    let mut cases = std::vec![RIDE, WIDEST];
    let mut vary = |set: &dyn Fn(&mut Case)| {
        let mut case = WIDEST;
        set(&mut case);
        cases.push(case);
    };
    for v in [None, Some(0), Some(2_750)] {
        vary(&|c| c.speed_mm_s = v);
    }
    for v in [None, Some(0), Some(999)] {
        vary(&|c| c.watts = v);
    }
    for v in [None, Some(40)] {
        vary(&|c| c.bpm = v);
    }
    for v in [0, 15_000_000] {
        vary(&|c| c.distance_mm = v);
    }
    for v in [None, Some(0), Some(199)] {
        vary(&|c| c.grade_tenths = v);
    }
    cases
}

fn view(case: Case, device: &'static Device, flags: Flags, light: bool) -> View<'static> {
    let mut view = View::new(Page::Ride, 0, Some(crate::clock::Local(86_399)));
    view.device = device;
    view.ride_page = RidePage::Main;
    view.light = light;
    view.course = flags.contains(Flags::COURSE);
    view.radar_paired = flags.contains(Flags::RADAR);
    view.state = Status::Recording;
    view.gps = Gps::Fixed;
    view.battery = Some(Battery::level(82));
    view.active_secs = case.secs;
    let sample = Sample {
        speed_mm_s: case.speed_mm_s,
        power_watts: case.watts,
        heart_bpm: case.bpm,
        cadence_tenths: case.cadence_tenths,
        gradient_tenths: case.grade_tenths,
        ..Sample::default()
    };
    view.totals.add(1000, case.distance_mm, &sample, None);
    view.sample = sample;
    view.power_3s = case.watts;
    view.compose_main();
    view.prepare();
    view
}

fn written(metric: Metric, value: Option<i32>) -> String {
    let mut text = Text::<12>::new();
    metric.write(value, &mut text);
    String::from(text.as_str())
}

fn color(canvas: &Canvas, text: &str) -> u16 {
    canvas
        .find(text)
        .unwrap_or_else(|| panic!("{text:?} is not on the page"))
        .color
}

/// Each fill's colour, in canvas order.
fn fills(canvas: &Canvas) -> Vec<u16> {
    canvas
        .shapes
        .iter()
        .filter_map(|shape| match shape {
            Shape::Panel { color, .. } => Some(*color),
            _ => None,
        })
        .collect()
}

#[test]
fn the_page_shows_the_approved_grid_with_its_titles() {
    let view = view(RIDE, &crate::screens::C606, Flags::NONE, false);
    let canvas = view.canvas();
    for title in [
        "PWR 3S W",
        "HEART BPM",
        "SPD KM/H",
        "CAD RPM",
        "GRADE",
        "CLIMB M",
        "DIST KM",
        "NP W",
    ] {
        assert!(canvas.shows(title), "{title}");
    }
    for value in [
        "200", "142", "31.3", "92", "2.4", "%", "42.7", "1:23:45", "82%",
    ] {
        assert!(canvas.shows(value), "{value}");
    }
    let field = |text: &str| canvas.find(text).unwrap();
    let (power, heart) = (field("200"), field("142"));
    let (speed, cadence, distance) = (field("31.3"), field("92"), field("42.7"));
    assert_eq!(power.top, heart.top, "power and heart share the top row");
    assert!(power.left < heart.left);
    assert_eq!(power.left, speed.left, "the columns line up");
    assert_eq!(heart.left, cadence.left, "the columns line up");
    assert!(speed.top < field("2.4").top && field("2.4").top < distance.top);
}

#[test]
fn power_and_heart_fill_with_their_zones_and_write_in_black() {
    let view = view(RIDE, &crate::screens::C606, Flags::NONE, false);
    let canvas = view.canvas();
    let profile = &view.profile;
    let power = profile.power_zone(200);
    let heart = profile.heart_zone(142);
    assert_eq!(
        fills(canvas),
        [
            DARK.zone_fill[usize::from(power) - 1],
            DARK.zone_fill[usize::from(heart) - 1]
        ]
    );
    for text in ["PWR 3S W", "200", "HEART BPM", "142"] {
        assert_eq!(color(canvas, text), DARK.on_fill, "{text}");
    }
    assert!(canvas.shows(&std::format!("Z{power}")));
    assert!(canvas.shows(&std::format!("Z{heart}")));
}

#[test]
fn without_a_reading_a_field_has_no_fill_and_shows_a_dash() {
    let empty = Case {
        watts: None,
        bpm: None,
        ..RIDE
    };
    let view = view(empty, &crate::screens::C606, Flags::NONE, false);
    assert!(fills(view.canvas()).is_empty());
    assert_eq!(color(view.canvas(), "PWR 3S W"), DARK.power);
}

#[test]
fn both_looks_draw_one_page_in_their_own_colours() {
    let device = &crate::screens::C606;
    let (night, day) = (
        view(RIDE, device, Flags::RADAR, false),
        view(RIDE, device, Flags::RADAR, true),
    );
    let (night, day) = (night.canvas(), day.canvas());
    let runs = |canvas: &Canvas| -> Vec<(Vec<u8>, usize, usize)> {
        canvas
            .runs
            .iter()
            .map(|run| (run.line.text().to_vec(), run.left, run.top))
            .collect()
    };
    let bounds = |canvas: &Canvas| -> Vec<_> { canvas.shapes.iter().map(Shape::bounds).collect() };
    assert_eq!(runs(night), runs(day));
    assert_eq!(bounds(night), bounds(day));
    let look = |theme: &Theme, canvas: &Canvas| {
        [
            ("SPD KM/H", theme.speed),
            ("92", theme.cadence),
            ("2.4", theme.grade),
            ("42.7", theme.text),
            ("1:23:45", theme.text),
        ]
        .into_iter()
        .for_each(|(text, want)| assert_eq!(color(canvas, text), want, "{text}"));
    };
    look(&DARK, night);
    look(&LIGHT, day);
    assert_ne!(DARK.speed, LIGHT.speed);
}

#[test]
fn the_day_look_paints_white_and_black_untransformed() {
    let device = &crate::screens::C606;
    let mut day = view(RIDE, device, Flags::NONE, true);
    let speed = *day.canvas().find("SPD KM/H").unwrap();
    let blank = (speed.left + 4, speed.bottom() + 2);
    assert_eq!(day.pixel(blank.0, blank.1), LIGHT.bg);
    let inked = (speed.left..speed.left + 40)
        .flat_map(|x| (speed.top..speed.top + 12).map(move |y| (x, y)))
        .any(|(x, y)| day.pixel(x, y) == LIGHT.text);
    assert!(inked, "the title inks pure black");
}

#[test]
fn the_top_line_colours_the_fix_and_the_battery_by_level() {
    let device = &crate::screens::C606;
    let mut view = view(RIDE, device, Flags::NONE, false);
    view.battery = Some(Battery::level(14));
    view.gps = Gps::Searching;
    view.prepare();
    let canvas = view.canvas();
    assert_eq!(color(canvas, "14%"), DARK.bad);
    let battery = canvas.find("14%").unwrap();
    let satellite = canvas.shapes.iter().find_map(|shape| match shape {
        Shape::Icon { color, .. } if shape.bounds().top < battery.bottom() => Some(*color),
        _ => None,
    });
    assert_eq!(satellite, Some(DARK.warn), "a fix still being found");
}

#[test]
fn extreme_readings_never_overlap_or_clip() {
    for device in ALL {
        for flags in [Flags::NONE, Flags::RADAR, Flags::COURSE.with(Flags::RADAR)] {
            for case in cases() {
                for light in [false, true] {
                    let mut view = view(case, device, flags, light);
                    view.prepare().check_layout();
                    let int = |value: Option<u32>| value.map(|value| value as i32);
                    let want = [
                        written(Metric::Speed, int(case.speed_mm_s)),
                        written(Metric::Power, case.watts.map(i32::from)),
                        written(Metric::Heart, case.bpm.map(i32::from)),
                        written(Metric::Elapsed, Some(case.secs as i32)),
                    ];
                    for text in &want {
                        assert!(
                            view.canvas().shows(text),
                            "{text:?} on {:?} {flags}",
                            device.scaled.panel
                        );
                    }
                }
            }
        }
    }
}
