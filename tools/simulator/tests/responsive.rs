use device_api::input::Button;
use device_api::server::Method;
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::devices::Model;
use shell_simulator::models::{C606, SMALL, WIDE};
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Sim, seconds};
use shell_simulator::world::{Side, Vehicle};
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
            let button = [
                Button::TopLeft,
                Button::BottomLeft,
                Button::BottomRight,
                Button::Center,
            ]
            .into_iter()
            .find(|button| format!("{button:?}") == name)
            .expect("a button");
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
            sim.press(Button::TopLeft);
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
            sim.press(Button::TopLeft);
            sim.expect_text("END RIDE?");
        });
        shows(&twin.sim, &["END RIDE?", "KM", "YOUR RIDE WILL BE SAVED"]);
        twin.sim.checkpoint(&format!("end ride on {}", model.name));
        twin.guide(|sim| {
            sim.press(Button::BottomRight);
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
            sim.press(Button::BottomLeft);
            sim.expect_text("NP W");
        });
        shows(&twin.sim, &["NP W", "KCAL", "PWR", "HR"]);
        twin.sim.checkpoint(&format!("summary effort on {name}"));
        twin.guide(|sim| {
            sim.press(Button::BottomLeft);
            sim.expect_text("DESCENT M");
        });
        shows(&twin.sim, &["ASCENT M", "DESCENT M", "MIN M", "MAX M"]);
        twin.sim.checkpoint(&format!("summary elevation on {name}"));
        twin.guide(|sim| {
            sim.press(Button::BottomLeft);
            sim.expect_text("BEST POWER W");
        });
        shows(&twin.sim, &["BEST POWER W", "CLIMBS", "LAPS"]);
        twin.sim
            .checkpoint(&format!("summary best power on {name}"));
        twin.guide(|sim| {
            sim.done();
            sim.home("RIDES");
            sim.press(Button::BottomRight);
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
            sim.press(Button::BottomRight);
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
        let extend = model.layout.workout.extend;
        let y = extend.top + extend.height() / 2;
        twin.sim.tap(model.width as u16 / 2, y as u16);
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
