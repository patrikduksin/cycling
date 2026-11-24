use super::*;
use crate::compose::builtin::{Choices, Laid, ScreenId};
use crate::compose::kit::profile;
use crate::compose::registry::{FIELD, ROAD_FIELD, ROAD_PROFILE, RULE, TOP_LINE};
use crate::ride::climb_ahead::{self, Forecast, tests::ahead};
use crate::screens::canvas::Shape;
use crate::screens::theme::{DARK, LIGHT};
use crate::screens::workout::Page;
use std::boxed::Box;
use std::vec::Vec;

impl Frame {
    fn tops(&self) -> Vec<Option<usize>> {
        self.bars.iter().map(|bar| bar.top()).collect()
    }

    fn behind(&self) -> Vec<bool> {
        self.bars.iter().map(|bar| bar.behind()).collect()
    }

    fn bands(&self) -> Vec<Steepness> {
        self.bars.iter().map(|bar| bar.steepness()).collect()
    }

    fn columns(&self) -> usize {
        self.chart.width().min(COLUMNS)
    }
}

struct Ride {
    climbs: Climbs,
    forecast: Forecast,
    pinned: Option<Pinned>,
    odometer_mm: u64,
    altitude_dm: Option<i32>,
}

impl Ride {
    fn new() -> Self {
        Self {
            climbs: Climbs::default(),
            forecast: Forecast::NONE,
            pinned: None,
            odometer_mm: 0,
            altitude_dm: None,
        }
    }

    fn ridden(mut self, metres: u32, altitude_dm: impl Fn(u32) -> i32) -> Self {
        for m in (5..=metres).step_by(5) {
            let z = altitude_dm(m);
            self.climbs.add(1000, 5000, Some(z));
            self.odometer_mm += 5000;
            self.altitude_dm = Some(z);
        }
        self
    }

    fn mapped(mut self, km: usize, road: impl Fn(f64) -> f64) -> Self {
        self.forecast.map = true;
        self.forecast.update(&ahead(1, km, road));
        climb_ahead::follow(&mut self.pinned, Some(&self.forecast), self.odometer_mm);
        self
    }

    fn on(mut self, metres: u64) -> Self {
        self.odometer_mm += metres * 1000;
        climb_ahead::follow(&mut self.pinned, None, self.odometer_mm);
        self
    }

    fn view_on(self, device: &'static crate::screens::Device, light: bool) -> View<'static> {
        let ride: &'static Ride = Box::leak(Box::new(self));
        let mut view = View::new(Page::Ride, 0, None);
        view.device = device;
        view.light = light;
        view.ride_page = crate::screens::ride::RidePage::Climb;
        let sample = view.sample;
        view.totals
            .add(1000, ride.odometer_mm, &sample, Some(95_000));
        view.show_climb(Lent {
            climbs: &ride.climbs,
            climbing: ride.climbs.climbing(),
            pinned: ride.pinned.as_ref(),
        });
        view.compose_main();
        view
    }

    fn view(self) -> View<'static> {
        self.view_on(&crate::screens::C606, false)
    }
}

fn chart(view: &View) -> Frame {
    view.climb_frame().expect("the climb page's chart").clone()
}

fn hill(m: u32) -> i32 {
    match m {
        ..1000 => 0,
        1000..2000 => (m as i32 - 1000) * 7 / 10,
        2000..3000 => 700 + (m as i32 - 2000),
        _ => 1700 - (m as i32 - 3000) / 2,
    }
}

fn mapped_climb(m: f64) -> f64 {
    (m - 300.0).clamp(0.0, 1000.0) * 0.08
}

fn assert_paints_reference(view: &mut View) {
    let panel = view.device.scaled.panel;
    let scene = view.prepare();
    for y in 0..panel.height {
        let mut pixels = [0x5555; crate::screens::chrome::WIDTH];
        firmware_shell::shell::Picture::paint(&scene, &mut Row::new(y, &mut pixels));
        for (x, &pixel) in pixels[..panel.width].iter().enumerate() {
            assert_eq!(pixel, scene.reference_pixel(x, y).0, "{x},{y}");
        }
    }
}

#[test]
fn on_the_flat_the_page_shows_the_altitude_ascent_and_the_last_2_km() {
    let mut view = Ride::new().ridden(3000, |m| (m as i32 / 100) % 3).view();
    view.prepare();
    let canvas = view.canvas();
    for text in ["ALTITUDE M", "ASCENT M", "LAST", "2 km", "AVG", "MAX"] {
        assert!(canvas.shows(text), "{text}");
    }
    assert!(!canvas.shows("CLIMB DIST KM"));
    let frame = chart(&view);
    let columns = frame.columns();
    assert!(frame.tops()[..columns].iter().all(Option::is_some));
    assert!(
        frame.behind()[..columns].iter().all(|&behind| behind),
        "all behind"
    );
    assert_eq!(usize::from(frame.rider), columns - 1);
    assert_eq!(frame.sums.map(|sums| sums.length_mm), Some(FLAT_WINDOW_MM));
    assert_paints_reference(&mut view);
}

#[test]
fn climbing_without_a_map_shows_the_climb_so_far_from_its_foot() {
    let mut view = Ride::new().ridden(2600, hill).view();
    view.prepare();
    let canvas = view.canvas();
    for text in ["CLIMB DIST KM", "CLIMB ELEV M", "CLIMB"] {
        assert!(canvas.shows(text), "{text}");
    }
    assert!(!canvas.shows("ALTITUDE M"));
    let climb = view.climb_lent().climbing.expect("climbing");
    let frame = chart(&view);
    let sums = frame.sums.unwrap();
    assert_eq!(sums.length_mm, climb.length_mm);
    assert_eq!(sums.average_tenths, climb.grade_tenths());
    assert!(sums.steepest_tenths >= Some(95), "the 10% kilometre");
    let columns = frame.columns();
    let tops = frame.tops();
    assert!(tops[..columns].iter().all(Option::is_some));
    assert!(
        tops[0] > tops[columns - 1],
        "the foot is lower than the rider"
    );
    assert_eq!(frame.bands()[columns - 1], Steepness::Hard);
    let km = (climb.length_mm / 1000) as i32;
    assert_eq!(view.metric(Metric::ClimbDistance), Some(km));
    assert_paints_reference(&mut view);
}

#[test]
fn on_a_course_the_whole_climb_shows_and_the_road_behind_is_dimmed() {
    let mut view = Ride::new().mapped(5, mapped_climb).on(700).view();
    view.prepare();
    let canvas = view.canvas();
    for text in ["DIST TO TOP KM", "ELEV TO TOP M", "CLIMB", "1 km", "8.0%"] {
        assert!(canvas.shows(text), "{text}");
    }
    let frame = chart(&view);
    let columns = frame.columns();
    let rider = usize::from(frame.rider);
    assert!(
        (columns * 35 / 100..columns * 45 / 100).contains(&rider),
        "{rider}"
    );
    assert!(frame.behind()[..rider].iter().all(|&behind| behind));
    assert!(
        frame.behind()[rider + 1..columns]
            .iter()
            .all(|&behind| !behind)
    );
    assert_eq!(view.metric(Metric::ToTop), Some(600));
    assert_eq!(view.metric(Metric::UpToGo), Some(48));
    assert_paints_reference(&mut view);
}

#[test]
fn ahead_of_a_mapped_climb_the_chart_runs_from_the_rider_to_its_top() {
    let mut view = Ride::new().mapped(5, mapped_climb).view();
    view.prepare();
    assert!(view.canvas().shows("DIST TO TOP KM"));
    let frame = chart(&view);
    assert_eq!(frame.rider, 0);
    assert!(
        frame.behind()[..frame.columns()]
            .iter()
            .all(|&behind| !behind)
    );
    assert_eq!(view.metric(Metric::ToTop), Some(1300));
    assert_eq!(view.metric(Metric::UpToGo), Some(80));
}

#[test]
fn grade_fills_with_its_steepness_and_power_and_heart_with_their_zones() {
    for light in [false, true] {
        let theme = if light { &LIGHT } else { &DARK };
        let mut view = Ride::new().view_on(&crate::screens::C606, light);
        view.sample.gradient_tenths = Some(84);
        view.power_3s = Some(285);
        view.sample.heart_bpm = Some(161);
        view.sample.cadence_tenths = Some(710);
        view.prepare();
        let canvas = view.canvas();
        let fills: Vec<u16> = canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Panel { color, .. } => Some(*color),
                _ => None,
            })
            .collect();
        let power = view.profile.power_zone(285);
        let heart = view.profile.heart_zone(161);
        let effort = crate::compose::data::Effort::Power;
        let pulse = crate::compose::data::Effort::Heart;
        assert_eq!(
            fills,
            [
                theme.steep(84).0,
                theme.fill(effort, power).unwrap().0,
                theme.fill(pulse, heart).unwrap().0,
            ]
        );
        assert_eq!(canvas.find("8.4").map(|run| run.color), Some(theme.on_fill));
        assert_eq!(canvas.find("71").map(|run| run.color), Some(theme.cadence));
        assert_eq!(
            canvas.find("CAD RPM").map(|run| run.color),
            Some(theme.cadence)
        );
        assert!(canvas.shows(&std::format!("Z{power}")));
    }
}

#[test]
fn each_look_colours_the_chart_from_its_own_zones() {
    for (light, theme) in [(false, &DARK), (true, &LIGHT)] {
        let mut view = Ride::new()
            .mapped(5, mapped_climb)
            .on(700)
            .view_on(&crate::screens::C606, light);
        view.prepare();
        let frame = chart(&view);
        let ahead = frame.columns() - 2;
        let dimmed = 2;
        let colour = |column: usize| {
            let top = frame.bars[column].top().unwrap();
            frame.color(column, top + 2 * usize::from(frame.outline_rows) + 4)
        };
        let band = |column: usize| theme.zone[zone(frame.bands()[column])];
        assert_eq!(colour(ahead), Some(band(ahead)));
        assert_eq!(
            colour(dimmed),
            Some(mix(band(dimmed), theme.bg, BEHIND_COVERAGE))
        );
        let top = frame.bars[ahead].top().unwrap();
        assert_eq!(frame.color(ahead, top), Some(theme.text), "the outline");
        assert_eq!(frame.color(ahead, top.saturating_sub(1)), None);
        assert_paints_reference(&mut view);
    }
}

fn laid(device: &'static crate::screens::Device, radar: bool) -> Laid {
    let flags = crate::screens::ride::main_flags(false, radar, false, false);
    Laid::new(ScreenId::RideClimb, device, Choices::default(), flags, None).unwrap()
}

#[test]
fn the_fields_keep_their_full_look_and_the_profile_takes_the_rest() {
    for device in crate::screens::panels::ALL {
        for radar in [false, true] {
            let laid = laid(device, radar);
            let what = std::format!("radar {radar} on {:?}", device.scaled.panel);
            let at = &device.scaled;
            let leaves: Vec<(u8, Rect)> = laid
                .leaves()
                .iter()
                .filter_map(|placed| {
                    match laid.screen().tree().nodes()[usize::from(placed.node)].what()? {
                        crate::compose::node::What::Leaf { component, .. } => {
                            Some((component, placed.rect.rect()))
                        }
                        _ => None,
                    }
                })
                .collect();
            let fields: Vec<Rect> = leaves
                .iter()
                .filter(|(kind, _)| [FIELD, ROAD_FIELD].contains(kind))
                .map(|(_, rect)| *rect)
                .collect();
            assert_eq!(fields.len(), 6, "{what}");
            let tall = fields.iter().map(Rect::height).collect::<Vec<_>>();
            let full = at.face(LAYOUT.face.digits_mid).fit_height();
            assert!(
                tall.iter().all(|&rows| rows > full && rows == tall[0]),
                "{what}: {tall:?}"
            );
            let widths = fields.iter().map(Rect::width).collect::<Vec<_>>();
            assert!(
                widths.iter().max().unwrap() - widths.iter().min().unwrap() <= 1,
                "{what}"
            );
            let profile = laid.box_of(ROAD_PROFILE).unwrap();
            let grid_bottom = fields.iter().map(|rect| rect.bottom).max().unwrap();
            assert_eq!(profile.bottom, at.panel.height, "{what}");
            assert_eq!(profile.top, grid_bottom + px_rule(at), "{what}");
            let chart = profile::chart(profile, at);
            assert!(
                chart.height() >= at.unit(usize::from(LAYOUT.space.chart)),
                "{what}"
            );
            assert!(leaves.iter().any(|(kind, _)| *kind == TOP_LINE));
            assert_eq!(
                leaves.iter().filter(|(kind, _)| *kind == RULE).count(),
                1 + 2 + 3 + 1,
                "{what}"
            );
        }
    }
}

fn px_rule(at: &Scaled) -> usize {
    at.unit(usize::from(LAYOUT.space.rule))
}

#[test]
fn the_chart_paints_as_its_reference_in_the_largest_box_on_every_panel() {
    type Case = (&'static str, fn() -> Ride);
    let cases: [Case; 6] = [
        ("nothing ridden", Ride::new),
        ("two samples", || Ride::new().ridden(10, |m| m as i32 * 2)),
        ("climbing 25%", || {
            Ride::new().ridden(4000, |m| m as i32 * 25 / 10)
        }),
        ("climb under way", || Ride::new().ridden(1300, hill)),
        ("far HC", || {
            Ride::new().mapped(10, |m| (m - 450.0).max(0.0) * 0.25)
        }),
        ("on HC", || {
            Ride::new()
                .mapped(10, |m| (m - 100.0).max(0.0) * 0.25)
                .on(5000)
        }),
    ];
    for (what, ride) in cases {
        for device in crate::screens::panels::ALL {
            let rect = crate::screens::ride::largest_box(device, ROAD_PROFILE);
            let view = ride().view_on(device, false);
            let chart = profile::chart(rect, &device.scaled);
            let frame = frame(&view, chart);
            let what = std::format!("{what} on {:?}", device.scaled.panel);
            assert!(
                frame.tops()[..frame.columns()]
                    .iter()
                    .flatten()
                    .all(|&top| top < chart.height()),
                "{what}"
            );
            if frame.rider != NO_RIDER {
                assert!(usize::from(frame.rider) < frame.columns(), "{what}");
            }
            for y in rect.top..rect.bottom {
                let mut pixels = [DARK.bg; crate::screens::chrome::WIDTH];
                paint_chart(&frame, &mut Row::new(y, &mut pixels));
                for (x, &pixel) in pixels.iter().enumerate() {
                    let want = chart_pixel(&frame, x, y).unwrap_or(DARK.bg);
                    assert_eq!(pixel, want, "{what}: {x},{y}");
                }
            }
        }
    }
}

#[test]
fn the_summary_writes_lengths_and_grades_to_fit_its_columns() {
    let length = |mm| {
        let mut text = Text::<12>::new();
        Sums {
            length_mm: mm,
            ..Sums::default()
        }
        .write_length(&mut text);
        text
    };
    assert_eq!(length(750_000).as_str(), "0.75 km");
    assert_eq!(length(2_000_000).as_str(), "2 km");
    assert_eq!(length(4_120_000).as_str(), "4.1 km");
    assert_eq!(length(123_400_000).as_str(), "123 km");
    let grade = |tenths| {
        let mut text = Text::<12>::new();
        Sums::write_grade(tenths, &mut text);
        text
    };
    assert_eq!(grade(Some(58)).as_str(), "5.8%");
    assert_eq!(grade(Some(124)).as_str(), "12%");
    assert_eq!(grade(Some(-31)).as_str(), "-3.1%");
    assert_eq!(grade(None).as_str(), "--%");
}

#[test]
fn the_page_reads_grade_power_heart_rate_and_cadence() {
    let readings = crate::screens::ride::RidePage::Climb.readings().unwrap();
    assert_eq!(readings.to_vec(), ["time", "power", "hr", "cad", "grade"]);
}

#[test]
fn a_road_slot_asks_for_one_box_on_every_stretch() {
    use crate::compose::kit::road_field::RoadSlot;
    use crate::compose::registry::tile;
    let field = tile(FIELD);
    for device in crate::screens::panels::ALL {
        let at = &device.scaled;
        for slot in RoadSlot::ALL {
            let slot = slot.spec();
            let boxes = [slot.flat, slot.climb, slot.course]
                .map(|kind| field.forms(kind as u8, at)[0])
                .map(|form| (form.width, form.height));
            assert!(boxes.iter().all(|&size| size == boxes[0]), "{boxes:?}");
        }
    }
}
