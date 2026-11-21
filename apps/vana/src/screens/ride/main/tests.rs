use super::*;
use crate::compose::builtin::main_tree as tree;
use crate::compose::data::{Data, Effort, Metric};
use crate::compose::input::Controls;
use crate::compose::node::Flags;
use crate::compose::node::What;
use crate::compose::parse::Screen;
use crate::compose::registry::{READING, SPACE};
use crate::ride::course::nav::{Distance, Glyph, Sign};
use crate::ride::log::{Sample, Status};
use crate::screens::Layout;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::layout::{assert_shown, text};
use crate::screens::icons::SignForm;
use crate::screens::panels::ALL;
use crate::screens::sign;
use crate::screens::workout::{Page, Text};
use std::string::String;
use std::vec::Vec;

/// Every flag combination the tree mentions.
fn every_flags() -> impl Iterator<Item = Flags> {
    (Flags::COURSE)
        .with(Flags::RADAR)
        .with(Flags::WORKOUT)
        .with(Flags::ARRIVED)
        .subsets()
}

fn every_slots() -> impl Iterator<Item = Slots> {
    Metric::LOWER.into_iter().flat_map(|left| {
        Metric::LOWER
            .into_iter()
            .map(move |right| Slots { left, right })
    })
}

fn bytes(slots: Slots) -> Vec<u8> {
    tree(slots).iter().flat_map(|node| node.bytes()).collect()
}

/// The built-in is accepted, on every panel and for every lower line, as
/// a rider's config would be: so the runtime's solve of it cannot fail.
#[test]
fn the_built_in_is_accepted_on_every_panel_for_every_lower_line() {
    for layout in ALL {
        for slots in every_slots() {
            if let Err(reject) = Screen::parse(
                ScreenId::RideMain.key(),
                &bytes(slots),
                &layout.scaled,
                Controls::FREE,
            ) {
                panic!("{slots:?} on {:?}: {reject}", layout.scaled.panel);
            }
        }
    }
}

/// The tiles of the middle column, grouped into the rows they share,
/// top to bottom: each row's top and bottom. The margins and the radar's
/// room run the column's whole height beside it.
fn rows(laid: &Laid) -> Vec<(u16, u16)> {
    let bar = status_bar(laid).unwrap().bottom as u16;
    let mut rows: Vec<(u16, u16)> = Vec::new();
    let middle = laid
        .leaves()
        .iter()
        .filter(|placed| {
            let what = laid.screen().tree().nodes()[usize::from(placed.node)].what();
            let beside = placed.rect.top == bar;
            !matches!(what, Some(What::Leaf { component, .. }) if component == STATUS || beside)
        })
        .map(|placed| placed.rect);
    for rect in middle {
        match rows
            .iter_mut()
            .find(|(top, bottom)| rect.top < *bottom && *top < rect.bottom)
        {
            Some(row) => *row = (row.0.min(rect.top), row.1.max(rect.bottom)),
            None => rows.push((rect.top, rect.bottom)),
        }
    }
    rows.sort();
    rows
}

/// The balance rule: the room the tiles leave in the middle column is even
/// gaps, above the first row and below the last as well as between them,
/// on every panel, under every flag combination.
#[test]
fn the_main_page_s_gaps_are_even_on_every_panel_and_variant() {
    for layout in ALL {
        let bar = status_bar(&laid(layout, Slots::default(), Flags::NONE)).unwrap();
        for flags in every_flags() {
            let laid = laid(layout, Slots::default(), flags);
            let rows = rows(&laid);
            let mut edges = std::vec![bar.bottom as i32];
            for (top, bottom) in &rows {
                edges.push(i32::from(*top));
                edges.push(i32::from(*bottom));
            }
            edges.push(layout.scaled.panel.height as i32);
            let gaps: Vec<i32> = edges.chunks(2).map(|pair| pair[1] - pair[0]).collect();
            let (least, most) = (gaps.iter().min().unwrap(), gaps.iter().max().unwrap());
            assert!(
                most - least <= 1 && *least > 0,
                "{flags} on {:?}: gaps {gaps:?}",
                layout.scaled.panel
            );
        }
    }
}

/// On a course a turn's banner covers the time, the grade and the chip,
/// and never a reading under them.
#[test]
fn the_banner_covers_the_time_and_chip_and_no_reading() {
    for layout in ALL {
        let banner = sign::place(turn(), SignForm::Banner, false, layout)
            .unwrap()
            .rect();
        for flags in every_flags().filter(|flags| flags.contains(Flags::COURSE)) {
            let laid = laid(layout, Slots::default(), flags);
            let nodes = laid.screen().tree().nodes();
            for placed in laid.leaves() {
                let Some(What::Leaf { component, arg }) = nodes[usize::from(placed.node)].what()
                else {
                    continue;
                };
                let under = placed.rect.top < banner.bottom as u16;
                let top_line = component == CHIP
                    || (component == READING
                        && [Metric::Elapsed, Metric::Grade]
                            .map(|metric| metric.id())
                            .contains(&(arg & 0x0f)));
                if component == STATUS || component == SPACE {
                    continue;
                }
                assert_eq!(
                    under, top_line,
                    "{flags} on {:?}: node {} at {:?}, banner to {}",
                    layout.scaled.panel, placed.node, placed.rect, banner.bottom
                );
            }
        }
    }
}

fn turn() -> Sign {
    Sign::Turn {
        glyph: Glyph::Left,
        distance: Distance::from_mm(400_000),
    }
}

/// One extreme reading of each kind the main page shows.
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
    let mut cases = std::vec![WIDEST];
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
    for v in [None, Some(0), Some(900)] {
        vary(&|c| c.cadence_tenths = v);
    }
    for v in [0, 15_000_000] {
        vary(&|c| c.distance_mm = v);
    }
    for v in [0, 3_599] {
        vary(&|c| c.secs = v);
    }
    for v in [None, Some(0), Some(199)] {
        vary(&|c| c.grade_tenths = v);
    }
    cases
}

fn view(case: Case, layout: &'static Layout, flags: Flags) -> View<'static> {
    let mut view = View::new(Page::Ride, 0, Some(crate::clock::Local(86_399)));
    view.layout = layout;
    view.course = flags.contains(Flags::COURSE);
    view.radar_paired = flags.contains(Flags::RADAR);
    if flags.contains(Flags::ARRIVED) {
        view.nav = Sign::Arrived;
    }
    view.state = Status::Recording;
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
    view.graphics.power_3s = case.watts;
    view.graphics.heart_bpm = case.bpm;
    view.compose_main();
    view.prepare();
    view
}

fn written(metric: Metric, value: Option<i32>) -> String {
    let mut text = Text::<12>::new();
    metric.write(value, &mut text);
    String::from(text.as_str())
}

/// Every extreme reading, on every panel, beside a radar or not and on a
/// course or not, shows whole and apart from every other.
#[test]
fn extreme_readings_never_overlap_or_clip() {
    for layout in ALL {
        let at = &layout.scaled;
        for flags in [Flags::NONE, Flags::RADAR, Flags::COURSE.with(Flags::RADAR)] {
            for case in cases() {
                let view = view(case, layout, flags);
                let what = std::format!(
                    "{flags} on {:?}: {:?} km/h, {:?} W, {:?} bpm, {:?} rpm/10, {} mm, {} s, {:?}",
                    at.panel,
                    case.speed_mm_s,
                    case.watts,
                    case.bpm,
                    case.cadence_tenths,
                    case.distance_mm,
                    case.secs,
                    case.grade_tenths
                );
                let int = |value: Option<u32>| value.map(|value| value as i32);
                let want = [
                    written(Metric::Speed, int(case.speed_mm_s)),
                    written(Metric::Power, case.watts.map(i32::from)),
                    written(Metric::Heart, case.bpm.map(i32::from)),
                    written(
                        Metric::Cadence,
                        case.cadence_tenths.map(|t| i32::from((t + 5) / 10)),
                    ),
                    written(Metric::Distance, Some((case.distance_mm / 1000) as i32)),
                    written(Metric::Elapsed, Some(case.secs as i32)),
                    written(Metric::Grade, case.grade_tenths.map(i32::from)),
                ];
                let mut want: Vec<&str> = want.iter().map(String::as_str).collect();
                want.extend(["KM/H", "W", "BPM", "RPM", "KM"]);
                view.canvas()
                    .assert_laid_out_at(at, at.screen(), &want, &what);
            }
        }
    }
}

/// Every reading in either slot, against every reading in the other, at
/// its widest, keeps its value whole and clear of its neighbour.
#[test]
fn every_reading_fits_either_slot_at_its_widest() {
    struct Widest;
    impl Data for Widest {
        fn metric(&self, metric: Metric) -> Option<i32> {
            Some(match metric {
                Metric::Grade => i32::MIN,
                Metric::Kcal => 9_999,
                _ => i32::MAX,
            })
        }
        fn zone(&self, _: Effort) -> Option<u8> {
            None
        }
        fn list(&self, _: crate::compose::data::ListKind) -> u16 {
            0
        }
    }
    for layout in ALL {
        let at = &layout.scaled;
        for flags in [Flags::NONE, Flags::RADAR] {
            for slots in every_slots() {
                let laid = laid(layout, slots, flags);
                let mut canvas = Canvas::default();
                laid.prepare(&Widest, &mut canvas);
                let what = std::format!("{slots:?} {flags} on {:?}", at.panel);
                let units = [slots.left.unit(), slots.right.unit()];
                canvas.assert_laid_out_at(at, at.screen(), &units, &what);
            }
        }
    }
}

/// A unit follows the ink of its value: "--" keeps it as close as a
/// number does.
#[test]
fn units_follow_the_value_they_belong_to() {
    let gap = |case: Case, unit: &str| {
        let view = view(case, &crate::screens::BASE, Flags::NONE);
        let items: Vec<(String, Rect)> = view.canvas().runs.iter().filter_map(text).collect();
        let unit = items.iter().find(|(name, _)| name == unit).unwrap().1;
        let value = items
            .iter()
            .map(|(_, rect)| *rect)
            .filter(|rect| rect.right <= unit.left && rect.top < unit.bottom + 40)
            .filter(|rect| unit.top < rect.bottom + 40)
            .max_by_key(|rect| rect.right)
            .unwrap();
        unit.left - value.right
    };
    let empty = Case {
        speed_mm_s: None,
        watts: None,
        bpm: None,
        cadence_tenths: None,
        ..WIDEST
    };
    let full = Case {
        speed_mm_s: Some(8334),
        watts: Some(250),
        bpm: Some(140),
        cadence_tenths: Some(900),
        ..WIDEST
    };
    for unit in ["KM/H", "W", "BPM", "RPM"] {
        let (empty, full) = (gap(empty, unit), gap(full, unit));
        assert!(empty.abs_diff(full) <= 2, "{unit}: {empty} vs {full}");
    }
}

#[test]
fn the_default_slots_show_cadence_then_distance_and_chosen_ones_replace_them() {
    let names = |view: &View| -> Vec<String> {
        view.canvas()
            .runs
            .iter()
            .filter_map(text)
            .map(|(name, _)| name)
            .collect()
    };
    let mut view = view(WIDEST, &crate::screens::BASE, Flags::NONE);
    let shown = names(&view);
    assert!(shown.contains(&"RPM".into()) && shown.contains(&"KM".into()));
    view.main_slots = Slots {
        left: Metric::NormalizedPower,
        right: Metric::Ascent,
    };
    view.compose_main();
    view.prepare();
    let shown = names(&view);
    assert!(
        shown.contains(&"NP".into()) && shown.contains(&"M".into()),
        "{shown:?}"
    );
    assert!(!shown.contains(&"RPM".into()), "{shown:?}");
}

#[test]
fn the_banner_hides_the_time_and_grade_and_the_chip_does_not() {
    let chip = Sign::Chip {
        glyph: Glyph::Right,
        distance: Distance::from_mm(6_400_000),
    };
    for (nav, shown) in [(Sign::None, true), (chip, true), (turn(), false)] {
        let mut view = view(WIDEST, &crate::screens::BASE, Flags::COURSE);
        view.nav = nav;
        view.prepare();
        assert_eq!(view.canvas().shows("99:59:59"), shown, "{nav:?}");
        assert!(view.canvas().shows("199"), "{nav:?} keeps the readings");
    }
}

#[test]
fn arrived_shows_the_ride_totals_without_gauges() {
    let view = view(
        WIDEST,
        &crate::screens::BASE,
        Flags::COURSE.with(Flags::ARRIVED),
    );
    let items: Vec<(String, Rect)> = view.canvas().runs.iter().filter_map(text).collect();
    let want = [
        "DISTANCE",
        "RIDE TIME",
        "AVG KM/H",
        "AVG W",
        "AVG BPM",
        "KM",
    ];
    assert_shown(&items, &want, "arrived");
    assert!(view.canvas().shapes.is_empty(), "no gauges");
    let top = |name: &str| items.iter().find(|(n, _)| n == name).unwrap().1.top;
    assert!(top("DISTANCE") < top("RIDE TIME") && top("RIDE TIME") < top("AVG W"));
}

/// A ride past 2147.5 km shows its real distance on the lower line and in
/// the arrival totals, up to the reading's 9999.9 km end.
#[test]
fn a_long_ride_shows_its_real_distance_up_to_the_end() {
    for (distance_mm, shown) in [
        (2_200_000_000, "2200.0"),
        (9_999_900_000, "9999.9"),
        (20_000_000_000, "9999.9"),
    ] {
        for flags in [Flags::NONE, Flags::COURSE.with(Flags::ARRIVED)] {
            let case = Case {
                distance_mm,
                ..WIDEST
            };
            let view = view(case, &crate::screens::BASE, flags);
            assert!(
                view.canvas().shows(shown),
                "{distance_mm} mm under {flags} shows {:?}, not {shown}",
                view.canvas()
                    .runs
                    .iter()
                    .filter_map(text)
                    .map(|(name, _)| name)
                    .collect::<Vec<_>>()
            );
        }
    }
}

/// The gauges glide: where their lit runs land follows the gliding power
/// and heart rate, and nothing glides without them.
#[test]
fn glides_land_where_the_gauges_light() {
    use crate::screens::data::Gliding;
    use crate::screens::workout::Graphics;
    let profile = crate::ride::zones::TrainingProfile::default();
    let laid = laid(&crate::screens::BASE, Slots::default(), Flags::NONE);
    let at = |watts| {
        let graphics = Graphics {
            power_3s: watts,
            ..Graphics::default()
        };
        laid.glides(&Gliding {
            graphics: &graphics,
            profile: &profile,
        })
    };
    assert_eq!(at(None), [None; GLIDES]);
    let (low, high) = (at(Some(100))[0].unwrap(), at(Some(300))[0].unwrap());
    assert!(low.right < high.right && low.left == high.left);
    assert_eq!(at(Some(100))[1], None, "no heart rate");
}

/// A workout's target sits under the gauge of its metric, inside the
/// gauge's columns, clear of every reading.
#[test]
fn a_workout_target_spans_its_watts_under_the_power_gauge() {
    use crate::screens::canvas::Shape;
    use crate::screens::chrome::WHITE;
    use crate::screens::ride::Training;
    use crate::workout::library::{ENDURANCE, VO2_MAX};
    use crate::workout::player::Player;
    for layout in ALL {
        let at = &layout.scaled;
        for workout in [VO2_MAX, ENDURANCE] {
            let mut player = Player::new(workout);
            player.skip();
            let mut view = view(WIDEST, layout, Flags::NONE);
            view.training = Some(Training::of(&player, &view.profile));
            view.compose_main();
            view.prepare();
            let canvas = view.canvas();
            let watts = canvas.find("1999").unwrap();
            let target = canvas
                .shapes
                .iter()
                .find_map(|shape| match *shape {
                    Shape::Bar { rect, color, .. } if color == WHITE => Some(rect),
                    _ => None,
                })
                .expect("a power target");
            let gauge = canvas
                .shapes
                .iter()
                .find_map(|shape| match *shape {
                    Shape::Split { rect, .. } => Some(rect),
                    _ => None,
                })
                .unwrap();
            assert!(target.top >= gauge.bottom && target.top > watts.bottom());
            assert!(target.left >= gauge.left && target.right > target.left + 2);
            let what = std::format!("{:?} on {:?}", workout.name, at.panel);
            canvas.assert_laid_out_at(at, at.screen(), &["1999"], &what);
        }
    }
}

/// A rider's tree may put the status bar under a reading: the bar shows
/// in its own rows as it does at the top, and the reading above it keeps
/// every pixel.
#[test]
fn a_status_bar_under_a_reading_paints_only_its_own_rows() {
    use crate::compose::builtin::Laids;
    use crate::compose::node::{Node, Size};
    use crate::compose::tiles::reading::{Inline, Shown};
    use crate::screens::chrome::{BLACK, Row};
    use firmware_shell::shell::Picture;
    let layout = &crate::screens::BASE;
    let at = &layout.scaled;
    let (above, bar) = (48, at.unit(crate::compose::tiles::status::BAR_UNITS));
    let speed = Inline(Shown::left(Metric::Speed, 1)).byte();
    let frame = |view: &mut View| -> Vec<Vec<u16>> {
        let scene = view.prepare();
        (0..at.panel.height)
            .map(|y| {
                let mut pixels = std::vec![0; at.panel.width];
                scene.paint(&mut Row::new(y, &mut pixels));
                pixels
            })
            .collect()
    };
    let shown = |middle: u8| {
        let nodes = [
            Node::col(3, Size::grow(1)),
            Node::leaf(READING, speed, Size::units(above as u8)),
            Node::leaf(middle, 0, Size::units(bar as u8)),
            Node::leaf(SPACE, 0, Size::grow(1)),
        ];
        let bytes: Vec<u8> = nodes.iter().flat_map(|node| node.bytes()).collect();
        let screen = Screen::parse(ScreenId::RideMain.key(), &bytes, at, Controls::FREE).unwrap();
        let mut view = view(WIDEST, layout, Flags::NONE);
        view.laids = Some(std::boxed::Box::leak(std::boxed::Box::new(
            Laids::with_main(screen, Flags::NONE),
        )));
        frame(&mut view)
    };
    let (moved, without) = (shown(STATUS), shown(SPACE));
    let at_top = frame(&mut view(WIDEST, layout, Flags::NONE));
    assert!(
        without[..above]
            .iter()
            .flatten()
            .any(|&pixel| pixel != BLACK)
    );
    let lost = (0..above)
        .flat_map(|y| (0..at.panel.width).map(move |x| (x, y)))
        .filter(|&(x, y)| moved[y][x] != without[y][x])
        .count();
    assert_eq!(
        lost, 0,
        "the bar changed {lost} pixels of the reading above it"
    );
    assert!(
        moved[above..above + bar] == at_top[..bar],
        "the bar under the reading shows as it does at the top"
    );
}

/// On the 240-wide panels the speed takes the hero face off a course and
/// without a radar, its look measured on "99.9" since it reads whole from
/// 100 km/h.
#[test]
fn speed_takes_the_hero_face_on_the_240_wide_panels() {
    use firmware_shell::rendering::font::FaceRole;
    for layout in [&crate::screens::BASE, &crate::screens::panels::WIDE] {
        for speed in [27_777, 40_000] {
            let case = Case {
                speed_mm_s: Some(speed),
                ..WIDEST
            };
            let view = view(case, layout, Flags::NONE);
            let text = written(Metric::Speed, Some(speed as i32));
            let run = view
                .canvas()
                .runs
                .iter()
                .find(|run| run.line.text() == text.as_bytes())
                .unwrap();
            assert!(
                core::ptr::eq(run.line.face(), layout.scaled.face(FaceRole::Hero)),
                "{text} on {:?}",
                layout.scaled.panel
            );
        }
    }
}
