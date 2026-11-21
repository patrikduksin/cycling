use device_api::input::Role;
use device_api::server::Method;
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::devices::Model;
use shell_simulator::models::{C606, SMALL, WIDE};
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Sim, minutes, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::app::Store;
use vana::compose::Flags;
use vana::compose::builtin::{Choices, Laid, ScreenId};
use vana::screens::workout::Page;

const KINDS: [&str; 4] = ["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"];

struct Twin {
    guide: Sim,
    sim: Sim,
}

impl Twin {
    fn paired(model: Model, world: fn(Model) -> Sim, kinds: &[&str]) -> Self {
        let mut guide = world(C606).boot();
        guide.discard_recording();
        let mut twin = Self {
            guide,
            sim: world(model),
        };
        twin.follow(0);
        twin.guide(|sim| sim.pair(kinds));
        twin
    }

    fn guide(&mut self, flow: impl FnOnce(&mut Sim)) {
        let from = self.guide.notes.len();
        flow(&mut self.guide);
        self.follow(from);
    }

    fn follow(&mut self, from: usize) {
        for note in &self.guide.notes[from..] {
            let Some(name) = note.text.strip_prefix("press ") else {
                continue;
            };
            assert_eq!(note.kind, NoteKind::Input);
            let button = [Role::Back, Role::Move, Role::Act]
                .into_iter()
                .find(|role| format!("{role:?}") == name)
                .expect("a role");
            self.sim.advance(note.at_ms - self.sim.now());
            self.sim.report(button);
        }
        self.sim.advance(self.guide.now() - self.sim.now());
    }

    fn both(&mut self, step: impl Fn(&mut Sim)) {
        step(&mut self.guide);
        step(&mut self.sim);
        assert_eq!(self.guide.now(), self.sim.now());
    }

    fn advance(&mut self, ms: u64) {
        self.both(|sim| sim.advance(ms));
    }
}

fn origin() -> Point {
    Point::new(-33.45, -70.66)
}

fn sensors(model: Model) -> Sim {
    let mut sim = Sim::on(model);
    sim.ant_sensor(device_type::HEART_RATE, 4242);
    sim.ant_sensor(device_type::POWER, 1234);
    sim.ant_sensor(device_type::SPEED_CADENCE, 777);
    sim.ant_sensor(device_type::RADAR, 31);
    sim
}

fn world(model: Model) -> Sim {
    let mut sim = sensors(model);
    sim.ride_route(Route::loop_around(origin(), 5_000.0, 20.0));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim
}

fn hilly(model: Model) -> Sim {
    let mut sim = sensors(model);
    sim.ride_route(Route::profile(
        origin(),
        &[
            (0.0, 600.0),
            (1000.0, 600.0),
            (2500.0, 690.0),
            (4000.0, 600.0),
            (6000.0, 600.0),
        ],
    ));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim
}

fn riding(model: Model, world: fn(Model) -> Sim) -> Twin {
    let mut twin = Twin::paired(model, world, &KINDS);
    twin.guide(Sim::start_ride);
    assert_eq!(twin.sim.device.sdk.page(), Page::Ride);
    twin.both(|sim| sim.ride_at(28.0, 230.0, 142.0, 88.0));
    twin.advance(seconds(20));
    twin.both(|sim| {
        for (range_m, side) in [(90.0, Side::Behind), (150.0, Side::Right)] {
            sim.vehicle(Vehicle {
                range_m,
                closing_kmh: 25.0,
                fast: false,
                side,
            });
        }
    });
    twin.advance(seconds(2));
    twin
}

fn page(model: Model, world: fn(Model) -> Sim, indicator: &str) -> Twin {
    let mut twin = riding(model, world);
    twin.guide(|sim| sim.ride_page(indicator));
    twin
}

fn course_gpx() -> String {
    let at = |(east, north): (f64, f64)| {
        let o = origin();
        let lat = o.lat + north / 111_195.0;
        let lon = o.lon + east / (111_195.0 * o.lat.to_radians().cos());
        (Point::new(lat, lon), None)
    };
    let legs = [
        (0.0, 0.0),
        (0.0, 1_000.0),
        (180.0, 1_000.0),
        (180.0, 4_000.0),
    ];
    let mut points = vec![at(legs[0])];
    for pair in legs.windows(2) {
        let steps = ((pair[1].0 - pair[0].0).hypot(pair[1].1 - pair[0].1) / 10.0).ceil();
        for step in 1..=steps as usize {
            let t = step as f64 / steps;
            points.push(at((
                pair[0].0 + t * (pair[1].0 - pair[0].0),
                pair[0].1 + t * (pair[1].1 - pair[0].1),
            )));
        }
    }
    shell_simulator::route::to_gpx("course", &points)
}

fn on_course(model: Model) -> Sim {
    let mut sim = sensors(model);
    sim.fit_bulk(Presence::Ready);
    let gpx = course_gpx();
    sim.ride_route(Route::from_gpx(&gpx).expect("a route"));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim
}

fn course_page(model: Model, metres: f64, indicator: &str) -> Twin {
    let mut twin = Twin::paired(model, on_course, &KINDS);
    twin.both(|sim| {
        sim.init_bulk();
        sim.format_files();
        assert_eq!(sim.command("PHONE START").0, "ACCEPTED");
        sim.device.sim_mut().access_point.clients = 1;
        sim.advance(seconds(1));
        let (status, _) = sim.phone_request(Method::Put, "/api/course", course_gpx().as_bytes());
        assert_eq!(status, 200);
        assert_eq!(sim.command("PHONE STOP").0, "ACCEPTED");
        sim.advance(seconds(1));
    });
    twin.guide(Sim::start_ride);
    twin.both(|sim| sim.ride_at(30.0, 210.0, 142.0, 88.0));
    while twin.sim.world().distance_m < metres {
        twin.advance(seconds(1));
    }
    twin.guide(|sim| sim.ride_page(indicator));
    twin
}

fn lit(sim: &Sim) -> usize {
    sim.screen()
        .pixels
        .iter()
        .filter(|&&pixel| pixel != 0)
        .count()
}

#[track_caller]
fn shows(sim: &Sim, texts: &[&str]) {
    let screen = sim.screen();
    for text in texts {
        assert!(screen.contains(text), "{text}: {}", screen.text());
    }
}

#[test]
fn the_main_page_fills_a_taller_panel() {
    let mut twin = riding(WIDE, world);
    let screen = twin.sim.screen();
    assert_eq!((screen.width, screen.height), (240, 400));
    shows(&twin.sim, &["KM/H", "W", "BPM", "RPM", "KM"]);
    let speed = screen.run("KM/H").expect("speed").y;
    let lower = screen.run("RPM").expect("cadence").y;
    assert!(lower > 320, "the readings spread down the panel: {lower}");
    assert!(speed < lower);
    twin.sim.checkpoint("ride main on wide");
}

#[test]
fn the_main_page_lays_out_at_three_quarters() {
    let mut twin = riding(SMALL, world);
    let screen = twin.sim.screen();
    assert_eq!((screen.width, screen.height), (180, 240));
    assert!(lit(&twin.sim) > 2_000, "the page is drawn");
    twin.sim.checkpoint("ride main on small");
}

#[test]
fn every_ride_page_draws_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let mut twin = page(model, world, "2/5");
        shows(&twin.sim, &["DIST KM", "AVG W", "ASCENT M", "PWR", "HR"]);
        twin.sim
            .checkpoint(&format!("ride stats on {}", model.name));
        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.advance(seconds(30));
            sim.ride_page("3/5");
        });
        shows(&twin.sim, &["LAP TIME", "LAP 2", "LAP KM", "W DIFF"]);
        twin.sim.checkpoint(&format!("ride lap on {}", model.name));
        twin.guide(|sim| sim.ride_page("5/5"));
        twin.sim.checkpoint(&format!("ride map on {}", model.name));
        assert!(lit(&twin.sim) > 2_000, "{} map", model.name);
        shows(&twin.sim, &["100 M", "KM"]);
    }
}

#[test]
fn the_climb_page_draws_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let mut twin = page(model, hilly, "4/5");
        twin.both(|sim| sim.ride_at(14.0, 260.0, 150.0, 75.0));
        while twin.sim.world().distance_m < 1600.0 {
            twin.advance(seconds(1));
        }
        twin.sim
            .checkpoint(&format!("ride climb on {}", model.name));
        assert!(lit(&twin.sim) > 2_000, "{} climb", model.name);
        shows(
            &twin.sim,
            &["CLIMBING", "POWER 3S", "HEART", "SO FAR KM", "GAIN M"],
        );
    }
}

#[test]
fn the_paused_end_and_summary_screens_draw_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let mut twin = riding(model, world);
        twin.advance(seconds(60));
        twin.guide(Sim::pause_ride);
        shows(&twin.sim, &["PAUSED", "FOR", "RIDE TIME", "KM"]);
        twin.sim.checkpoint(&format!("paused on {}", model.name));
        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.expect_text("END RIDE?");
        });
        shows(&twin.sim, &["END RIDE?", "KM", "YOUR RIDE WILL BE SAVED"]);
        twin.sim.checkpoint(&format!("end ride on {}", model.name));
        twin.guide(|sim| {
            sim.press(Role::Act);
            sim.wait_for("RIDE SAVED", seconds(5)).unwrap();
        });
        twin.sim.checkpoint(&format!("summary on {}", model.name));
        assert!(lit(&twin.sim) > 2_000, "{} summary", model.name);
    }
}

#[test]
fn the_summary_pages_and_a_saved_ride_draw_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = riding(model, hilly);
        twin.both(|sim| sim.ride_at(20.0, 260.0, 150.0, 85.0));
        while twin.sim.world().distance_m < 1800.0 {
            twin.advance(seconds(5));
        }
        twin.guide(Sim::stop_ride);
        shows(&twin.sim, &["RIDE SAVED", "TIME", "DIST KM", "AVG BPM"]);
        twin.sim.checkpoint(&format!("summary overview on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Move);
            sim.expect_text("NP W");
        });
        shows(&twin.sim, &["NP W", "KCAL", "PWR", "HR"]);
        twin.sim.checkpoint(&format!("summary effort on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Move);
            sim.expect_text("DESCENT M");
        });
        shows(&twin.sim, &["ASCENT M", "DESCENT M", "MIN M", "MAX M"]);
        twin.sim.checkpoint(&format!("summary elevation on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Move);
            sim.expect_text("BEST POWER W");
        });
        shows(&twin.sim, &["BEST POWER W", "CLIMBS", "LAPS"]);
        twin.sim
            .checkpoint(&format!("summary best power on {name}"));
        twin.guide(|sim| {
            sim.done();
            sim.home("RIDES");
            sim.press(Role::Act);
            sim.wait_gone("READING", seconds(5)).unwrap();
            sim.expect_text("SAVED RIDE");
        });
        assert_eq!(twin.sim.device.sdk.page(), Page::PastRide);
        shows(&twin.sim, &["SAVED RIDE", "TIME", "AVG W"]);
        twin.sim.checkpoint(&format!("saved ride on {name}"));
    }
}

#[test]
fn the_workout_page_draws_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let mut twin = Twin::paired(model, world, &KINDS);
        twin.guide(|sim| {
            sim.choose_workout("VO2 MAX 5X3");
            sim.press(Role::Act);
            sim.wait_for("RECORDING", seconds(2)).unwrap();
        });
        twin.both(|sim| sim.ride_at(30.0, 120.0, 142.0, 90.0));
        twin.advance(seconds(20));
        assert_eq!(twin.guide.screen().page().as_deref(), Some("1/6"));
        shows(&twin.sim, &["VO2 MAX", "WARM UP", "W", "LEFT", "NEXT"]);
        if model.name == WIDE.name {
            shows(&twin.sim, &["1/12", "108-151 W"]);
        }
        twin.sim
            .checkpoint(&format!("ride workout on {}", model.name));
        assert!(lit(&twin.sim) > 2_000, "{} workout", model.name);

        let left: u64 = twin.sim.workout()["left_s"].parse().unwrap();
        let words = twin.sim.screen().run("LEFT").expect("the time left");
        twin.sim.tap(model.width as u16 / 2, words.y as u16);
        twin.sim.advance(seconds(1));
        let extended: u64 = twin.sim.workout()["left_s"].parse().unwrap();
        assert!(
            extended >= left + 58,
            "{}: {left} s then {extended} s",
            model.name
        );
    }
}

#[test]
fn a_course_ride_draws_its_sign_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let mut twin = course_page(model, 700.0, "1/5");
        twin.sim
            .checkpoint(&format!("course main on {}", model.name));
        let distance = twin.guide.screen().field("M").expect("the C606's sign");
        shows(&twin.sim, &[&distance, "M", "BPM", "RPM"]);
        twin.guide(|sim| sim.ride_page("2/5"));
        twin.sim
            .checkpoint(&format!("course banner on {}", model.name));
        shows(&twin.sim, &[&distance, "M", "AVG W"]);
        twin.guide(|sim| sim.ride_page("5/5"));
        twin.sim
            .checkpoint(&format!("course map on {}", model.name));
        assert!(lit(&twin.sim) > 2_000, "{} course map", model.name);
    }
}

fn with_files(model: Model) -> Sim {
    let mut sim = world(model);
    sim.fit_bulk(Presence::Ready);
    sim
}

fn bests_row(sim: &mut Sim) {
    sim.home("SETTINGS");
    sim.press(Role::Move);
    sim.press(Role::Move);
}

#[test]
fn bests_and_form_read_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, with_files, &KINDS);
        twin.guide(|sim| {
            bests_row(sim);
            sim.expect_field("BESTS", "NOT KEPT");
            sim.press(Role::Act);
            sim.expect_text("BESTS NOT KEPT");
        });
        shows(&twin.sim, &["BESTS", "KEPT", "NO PLACE TO KEEP THEM"]);
        twin.sim.checkpoint(&format!("bests not kept on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.press(Role::Move);
            sim.press(Role::Act);
            sim.wait_for("NO POWER RIDES", seconds(30)).unwrap();
        });
        shows(&twin.sim, &["NO POWER RIDES", "NEEDS A POWER METER"]);
        twin.sim
            .checkpoint(&format!("form without rides on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.press(Role::Back);
            assert!(sim.on_home(), "{}", sim.screen().text());
        });

        twin.both(|sim| {
            sim.init_bulk();
            sim.init_volumes();
            sim.put_file("notes.txt", b"x");
            sim.ride_at(30.0, 240.0, 140.0, 90.0);
        });
        twin.guide(Sim::start_ride);
        twin.advance(minutes(6));
        twin.guide(|sim| {
            sim.stop_ride();
            sim.expect_text("NEW BESTS");
            for _ in 0..4 {
                if sim.screen().contains("RIDE SAVED") {
                    break;
                }
                sim.press(Role::Move);
            }
            sim.done();
            bests_row(sim);
        });
        twin.guide(|sim| {
            sim.press(Role::Act);
            sim.expect_text("LONGEST RIDE");
        });
        shows(
            &twin.sim,
            &[
                "BESTS",
                "5 S",
                "MIN",
                "240 W",
                "LONGEST RIDE",
                "MOST ASCENT",
            ],
        );
        twin.sim.checkpoint(&format!("bests on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Act);
            sim.expect_text("CLEAR BESTS?");
        });
        shows(&twin.sim, &["CLEAR BESTS?", "THEY CANNOT COME BACK"]);
        twin.sim.checkpoint(&format!("clear bests on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.expect_text("LONGEST RIDE");
            sim.press(Role::Back);
            sim.press(Role::Move);
            sim.press(Role::Act);
            sim.wait_for("FATIGUE", seconds(60)).unwrap();
        });
        shows(
            &twin.sim,
            &["FORM", "FITNESS", "FATIGUE", "WEEKS AGO", "NOW"],
        );
        twin.sim.checkpoint(&format!("form on {name}"));
    }
}

#[test]
fn the_sensor_pages_draw_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, sensors, &KINDS[..2]);
        twin.guide(|sim| sim.home("SENSORS"));
        shows(
            &twin.sim,
            &["SENSORS", "HEART RATE", "POWER METER", "LIVE", "ADD SENSOR"],
        );
        twin.sim.checkpoint(&format!("sensors on {name}"));
        twin.guide(|sim| sim.open_sensor_detail("POWER METER"));
        shows(&twin.sim, &["POWER METER", "LIVE", "BATTERY", "ID", "1234"]);
        twin.sim.checkpoint(&format!("sensor detail on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.nearby_page();
        });
        shows(&twin.sim, &["NEARBY"]);
        twin.sim.checkpoint(&format!("scanning on {name}"));
        twin.guide(|sim| sim.wait_for("ID 31", seconds(20)).map(drop).unwrap());
        shows(&twin.sim, &["NEARBY", "RADAR", "ID 31", "ID 777"]);
        twin.sim.checkpoint(&format!("nearby on {name}"));

        twin.sim.press(Role::Back);
        twin.sim.expect_text("ADD SENSOR");
        let list = composed_list(model, ScreenId::Sensors);
        twin.sim.tap(model.width as u16 / 2, card_middle(&list, 1));
        twin.sim.advance(seconds(1));
        shows(&twin.sim, &["POWER METER", "BATTERY", "1234"]);
    }
}

#[test]
fn home_lays_out_and_takes_taps_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, world, &KINDS);
        assert_eq!(twin.sim.device.sdk.page(), Page::Home);
        shows(
            &twin.sim,
            &["RIDE", "SENSORS", "LIVE", "TRAIN", "SETTINGS", "RIDES"],
        );
        twin.sim.checkpoint(&format!("home on {name}"));
        let laid = Laid::new(
            ScreenId::Home,
            model.layout,
            Default::default(),
            Flags::NONE,
            None,
        )
        .expect("Home has a tree");
        let solved = laid.solved();
        let cards = solved.focus_order().map(|placed| placed.rect);
        let pages = [
            Page::Ready,
            Page::Sensors,
            Page::Workouts,
            Page::Settings,
            Page::History,
        ];
        for (card, page) in cards.zip(pages) {
            let (x, y) = (card.left + card.width() / 2, card.top + card.height() / 2);
            twin.sim.tap(x, y);
            twin.sim.advance(seconds(1));
            assert_eq!(twin.sim.device.sdk.page(), page, "{name}: a tap at {x},{y}");
            twin.sim.press(Role::Back);
            twin.sim.advance(seconds(1));
            assert_eq!(
                twin.sim.device.sdk.page(),
                Page::Home,
                "{name}: back from {page:?}"
            );
        }
    }
}

fn with_rides(model: Model) -> Twin {
    let mut twin = Twin::paired(model, world, &["HEART RATE"]);
    twin.both(|sim| {
        sim.ride_at(20.0, 150.0, 120.0, 80.0);
        for _ in 0..8 {
            for (command, then) in [("START", seconds(60)), ("PAUSE", 500), ("FINISH", 500)] {
                let (_, ride) = sim.command("RIDE STATUS");
                if command != "PAUSE" || !ride.contains("state=paused") {
                    let (status, out) = sim.command(&format!("RIDE INTERNAL {command}"));
                    assert_eq!(status, "ACCEPTED", "{command}: {out} {ride}");
                }
                sim.advance(then);
            }
        }
        sim.advance(seconds(3));
    });
    twin
}

/// The number editor's plus pad on `model`'s panel.
fn raise_pad(model: Model) -> vana::screens::chrome::Rect {
    let choices = Choices {
        editing: true,
        ..Choices::default()
    };
    let laid =
        Laid::new(ScreenId::Settings, model.layout, choices, Flags::NONE, None).expect("Settings");
    let nodes = laid.screen().tree().nodes();
    let pads = laid.leaves().iter().filter(|placed| {
        let what = nodes[usize::from(placed.node)].what();
        what == Some(vana::compose::node::What::Leaf {
            component: vana::compose::registry::PAD,
            arg: 1,
        })
    });
    pads.map(|placed| placed.rect.rect())
        .next()
        .expect("the plus pad")
}

/// The page of cards built-in `id` scrolls on `model`'s panel.
fn composed_list(model: Model, id: ScreenId) -> vana::screens::scroll::Layout {
    let laid = Laid::new(id, model.layout, Choices::default(), Flags::NONE, None);
    *laid.expect("a built-in").page().expect("a list screen")
}

fn card_middle(list: &vana::screens::scroll::Layout, index: usize) -> u16 {
    let (top, bottom) = list.span(index);
    (list.top as i32 + (top + bottom) / 2) as u16
}

#[test]
fn the_rides_list_draws_and_scrolls_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, world, &KINDS);
        twin.guide(|sim| {
            sim.home("RIDES");
            sim.expect_text("NO RIDES YET");
        });
        shows(
            &twin.sim,
            &["THIS WEEK", "NO RIDES YET", "SAVED RIDES SHOW HERE"],
        );
        twin.sim.checkpoint(&format!("no rides on {name}"));

        let mut twin = with_rides(model);
        twin.guide(|sim| {
            sim.home("RIDES");
            sim.wait_for("1/8", seconds(5)).unwrap();
            sim.advance(seconds(1));
        });
        assert_eq!(twin.sim.device.sdk.page(), Page::History);
        shows(&twin.sim, &["THIS WEEK", "RIDES", "KM", "LAST WEEK"]);
        twin.sim.checkpoint(&format!("rides on {name}"));
        let list = &composed_list(model, ScreenId::History);
        let header = twin.sim.screen().run("THIS WEEK").expect("the header").y;

        twin.guide(|sim| {
            for _ in 0..7 {
                sim.press(Role::Move);
            }
            sim.wait_for("8/8", seconds(2)).unwrap();
            sim.advance(seconds(2));
        });
        // The week stays over the rides as they scroll.
        shows(&twin.sim, &["THIS WEEK"]);
        twin.sim.checkpoint(&format!("rides scrolled on {name}"));

        twin.guide(|sim| {
            sim.press(Role::Move);
            sim.wait_for("1/8", seconds(2)).unwrap();
            sim.advance(seconds(2));
        });
        let x = model.width as u16 / 2;
        let low = card_middle(list, 2);
        twin.sim.drag((x, low), (x, low - 60), 300, 200);
        twin.sim.advance(seconds(2));
        let moved = twin.sim.screen().run("THIS WEEK").map(|run| run.y);
        assert_eq!(moved, Some(header), "{name}: the week stays put");

        twin.sim.drag((x, low - 60), (x, low), 300, 200);
        twin.sim.advance(seconds(2));
        twin.sim.tap(x, card_middle(list, 1));
        twin.sim.wait_gone("READING", seconds(5)).unwrap();
        twin.sim.expect_text("SAVED RIDE");
        assert_eq!(twin.sim.device.sdk.page(), Page::PastRide, "{name}");
    }
}

#[test]
fn settings_draw_scroll_and_edit_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, world, &KINDS);
        twin.guide(|sim| {
            sim.home("SETTINGS");
            sim.expect_text("PROFILE");
        });
        shows(
            &twin.sim,
            &["PROFILE", "FTP WATTS", "LTHR BPM", "BESTS", "FORM"],
        );
        twin.sim.checkpoint(&format!("settings on {name}"));

        twin.guide(|sim| {
            sim.press(Role::Act);
            sim.advance(seconds(1));
        });
        shows(&twin.sim, &["PROFILE", "FTP WATTS", "215"]);
        twin.sim.checkpoint(&format!("number editor on {name}"));
        let raise = raise_pad(model);
        let middle = (raise.top + raise.height() / 2) as u16;
        twin.sim.tap(raise.center_x() as u16, middle);
        twin.sim.advance(seconds(1));
        shows(&twin.sim, &["FTP WATTS", "220"]);
        catch_up(&mut twin);

        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.expect_text("LTHR BPM");
            for _ in 0..7 {
                sim.press(Role::Move);
            }
            sim.expect_text("DISPLAY");
            for _ in 0..4 {
                sim.press(Role::Move);
            }
            sim.advance(seconds(1));
        });
        shows(&twin.sim, &["DISPLAY", "THEME", "MODE"]);
        twin.sim.checkpoint(&format!("settings scrolled on {name}"));
    }
}

fn catch_up(twin: &mut Twin) {
    let behind = twin.sim.now() - twin.guide.now();
    twin.guide.advance(behind);
}

#[test]
fn the_workout_picker_draws_and_scrolls_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, world, &KINDS);
        twin.guide(|sim| {
            sim.home("TRAIN");
            sim.expect_text("WORKOUTS");
        });
        shows(&twin.sim, &["WORKOUTS", "FREE RIDE", "NO TARGETS"]);
        twin.sim.checkpoint(&format!("workouts on {name}"));
        twin.guide(|sim| {
            for _ in 0..9 {
                sim.press(Role::Move);
            }
            sim.advance(seconds(1));
        });
        assert!(!twin.sim.screen().contains("FREE RIDE"), "{name} scrolled");
        shows(&twin.sim, &["WORKOUTS", "MIN"]);
        twin.sim.checkpoint(&format!("workouts scrolled on {name}"));
    }
}

#[test]
fn the_ftp_sheet_draws_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let mut twin = Twin::paired(model, world, &KINDS);
        twin.guide(|sim| {
            sim.choose_workout("FTP TEST 20 MIN");
            sim.press(Role::Act);
            sim.wait_for("RECORDING", seconds(2)).unwrap();
        });
        twin.both(|sim| sim.ride_at(30.0, 150.0, 142.0, 90.0));
        twin.advance(seconds(30));
        twin.guide(|sim| {
            for _ in 0..10 {
                sim.press(Role::Back);
                sim.advance(seconds(3));
            }
            sim.expect_text("ALL OUT");
        });
        twin.both(|sim| sim.ride_at(30.0, 260.0, 142.0, 90.0));
        twin.guide(|sim| {
            sim.wait_for("NEW FTP", seconds(25 * 60)).unwrap();
        });
        shows(&twin.sim, &["NEW FTP", "247", "SAVE?"]);
        twin.sim.checkpoint(&format!("ftp sheet on {}", model.name));
    }
}

fn fresh(model: Model, world: fn(Model) -> Sim) -> Twin {
    let mut guide = world(C606);
    guide.discard_recording();
    Twin {
        guide,
        sim: world(model),
    }
}

#[test]
fn the_boot_ready_and_first_run_setup_draw_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = fresh(model, world);
        while !(twin.sim.vana_page() == "boot" && twin.sim.device.lit()) {
            assert!(twin.sim.now() < seconds(15), "{name} never shows the brand");
            twin.advance(1);
        }
        twin.advance(200);
        shows(&twin.sim, &["VANA"]);
        twin.sim.checkpoint(&format!("boot on {name}"));
        twin.guide(|sim| {
            sim.wait_usable();
            sim.expect_text("WELCOME");
        });
        shows(&twin.sim, &["WELCOME", "SET UP", "SKIP SETUP"]);
        twin.sim.checkpoint(&format!("setup welcome on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Act);
            sim.expect_text("SET FTP");
        });
        shows(&twin.sim, &["POWER", "SET FTP", "I DON'T KNOW"]);
        twin.sim.checkpoint(&format!("setup power on {name}"));
        for (next, words) in [
            ("SET LTHR", ["HEART", "SET LTHR", "I DON'T KNOW"]),
            ("PAIR SENSORS", ["SENSORS", "PAIR SENSORS", "LATER"]),
            ("RIDE", ["READY", "RIDE", "HOME"]),
        ] {
            twin.guide(|sim| {
                sim.press(Role::Move);
                sim.press(Role::Act);
                sim.expect_text(next);
            });
            shows(&twin.sim, &words);
        }
        twin.sim.checkpoint(&format!("setup done on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Move);
            sim.press(Role::Act);
            assert!(sim.on_home(), "{}", sim.screen().text());
            sim.home("RIDE");
            sim.expect_text("READY");
        });
        assert_eq!(twin.sim.device.sdk.page(), Page::Ready);
        shows(&twin.sim, &["READY", "FREE RIDE", "GPS", "BATTERY"]);
        twin.sim.checkpoint(&format!("ready on {name}"));
    }
}

#[test]
fn a_saved_ride_asks_to_delete_it_and_connects_a_phone_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, with_files, &KINDS);
        twin.both(|sim| {
            sim.init_bulk();
            sim.format_files();
        });
        twin.guide(Sim::start_ride);
        twin.both(|sim| sim.ride_at(28.0, 230.0, 142.0, 88.0));
        twin.advance(seconds(60));
        twin.guide(|sim| {
            sim.stop_ride();
            sim.done();
            sim.home("RIDES");
            sim.press(Role::Act);
            sim.wait_gone("READING", seconds(5)).unwrap();
            for _ in 0..6 {
                if sim.screen().contains("DELETE RIDE?") {
                    break;
                }
                sim.press(Role::Move);
            }
            sim.expect_text("IT GOES FROM YOUR RIDES");
        });
        shows(
            &twin.sim,
            &["DELETE RIDE?", "KM", "IT GOES FROM YOUR RIDES"],
        );
        twin.sim.checkpoint(&format!("delete ride on {name}"));
        twin.guide(|sim| {
            sim.press(Role::Move);
            sim.expect_text("SAVED RIDE");
            sim.press(Role::Act);
            sim.advance(seconds(1));
            sim.expect_text("JOIN VANA WI-FI");
        });
        assert_eq!(twin.sim.device.sdk.page(), Page::Phone);
        shows(&twin.sim, &["STEP 1 OF 2", "JOIN VANA WI-FI", "VANA"]);
        twin.sim.checkpoint(&format!("phone join on {name}"));
        twin.both(|sim| {
            sim.device.sim_mut().access_point.clients = 1;
            sim.advance(seconds(1));
        });
        twin.guide(|sim| sim.expect_text("OPEN YOUR RIDES"));
        shows(&twin.sim, &["STEP 2 OF 2", "OPEN YOUR RIDES"]);
        twin.sim.checkpoint(&format!("phone open on {name}"));
    }
}

#[test]
fn turning_off_and_the_charge_draw_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let name = model.name;
        let mut twin = Twin::paired(model, world, &[]);
        twin.both(|sim| sim.set_battery(64, true));
        twin.advance(seconds(2));
        twin.guide(|sim| {
            sim.press(Role::Back);
            sim.expect_text("TURN OFF");
        });
        shows(&twin.sim, &["TURN OFF", "RIDES ARE SAVED"]);
        twin.sim.checkpoint(&format!("turn off on {name}"));
        twin.both(|sim| {
            sim.report(Role::Act);
            sim.advance(80);
        });
        shows(&twin.sim, &["TURNING OFF", "VANA"]);
        twin.sim.checkpoint(&format!("turning off on {name}"));
        twin.advance(seconds(2));
        assert!(twin.sim.is_off(), "{name} still on");
        assert!(twin.sim.device.standby_lit(), "{name}: the charge shows");
        shows(&twin.sim, &["CHARGING", "64"]);
        twin.sim.checkpoint(&format!("charging on {name}"));
    }
}

fn foreign(model: Model) -> Sim {
    let mut sim = Sim::on(model);
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    sim.make_foreign(&[Store::Settings]);
    sim.cut_power();
    sim
}

#[test]
fn the_format_offer_draws_on_both_test_panels() {
    for model in [WIDE, SMALL] {
        let mut twin = fresh(model, foreign);
        twin.guide(|sim| {
            sim.wait_for("FORMAT STORAGE?", seconds(20)).unwrap();
        });
        assert_eq!(twin.sim.device.sdk.page(), Page::Format);
        shows(
            &twin.sim,
            &["FORMAT STORAGE?", "MADE BY ANOTHER VERSION", "SETTINGS GO"],
        );
        twin.sim
            .checkpoint(&format!("format offer on {}", model.name));
    }
}
