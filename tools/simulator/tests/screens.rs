use device_api::input::Role;
use device_api::server::Method;
use shell_simulator::ant::device_type;
use shell_simulator::artifacts::Canvas;
use shell_simulator::bulk::Presence;
use shell_simulator::devices::Model;
use shell_simulator::models::{C606, SMALL, WIDE};
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Sim, minutes, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::app::Store;

const FOLLOWERS: [Model; 2] = [SMALL, WIDE];
const KINDS: [&str; 4] = ["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"];

const UNREACHABLE: &[(&str, &str)] = &[(
    "lose-rides/unsaved",
    "needs a phone sync session holding unconfirmed rides when the host clears",
)];

struct Walk {
    guide: Sim,
    followers: Vec<Sim>,
}

impl Walk {
    fn fresh(world: fn(Model) -> Sim) -> Self {
        let mut walk = Self {
            guide: world(C606),
            followers: FOLLOWERS.map(world).into(),
        };
        for sim in walk.sims() {
            sim.discard_recording();
        }
        walk
    }

    fn paired(world: fn(Model) -> Sim, kinds: &[&str]) -> Self {
        let mut walk = Self::fresh(world);
        walk.guide(|sim| {
            sim.wait_usable();
            sim.skip_setup();
            if !kinds.is_empty() {
                sim.pair(kinds);
            }
        });
        walk
    }

    fn sims(&mut self) -> impl Iterator<Item = &mut Sim> {
        std::iter::once(&mut self.guide).chain(&mut self.followers)
    }

    fn guide(&mut self, flow: impl FnOnce(&mut Sim)) {
        let from = self.guide.notes.len();
        flow(&mut self.guide);
        let (notes, now) = (&self.guide.notes[from..], self.guide.now());
        for sim in &mut self.followers {
            for note in notes {
                let Some(name) = note.text.strip_prefix("press ") else {
                    continue;
                };
                assert_eq!(note.kind, NoteKind::Input);
                let button = [Role::Back, Role::Move, Role::Act]
                    .into_iter()
                    .find(|role| format!("{role:?}") == name)
                    .expect("a role");
                sim.advance(note.at_ms - sim.now());
                sim.report(button);
            }
            sim.advance(now - sim.now());
        }
    }

    fn all(&mut self, step: impl Fn(&mut Sim)) {
        for sim in self.sims() {
            step(sim);
        }
    }

    fn advance(&mut self, ms: u64) {
        self.all(|sim| sim.advance(ms));
    }

    fn ride_to(&mut self, metres: f64) {
        while self.guide.world().distance_m < metres {
            self.advance(seconds(1));
        }
    }

    #[track_caller]
    fn cell(&mut self, screen: &str, variant: &str) {
        let page = self.guide.vana_page();
        for sim in self.sims() {
            assert_eq!(
                sim.vana_page(),
                page,
                "{screen}/{variant} on {}:\n{}",
                sim.device.model.name,
                sim.screen().text()
            );
        }
        self.write(screen, variant);
    }

    fn off_cell(&mut self, screen: &str, variant: &str) {
        for sim in self.sims() {
            assert!(
                sim.is_off(),
                "{screen}/{variant} on {}",
                sim.device.model.name
            );
        }
        self.write(screen, variant);
    }

    fn write(&mut self, screen: &str, variant: &str) {
        let Some(dir) = std::env::var_os("SIM_SCREENS") else {
            return;
        };
        let dir = std::path::Path::new(&dir).join(screen);
        std::fs::create_dir_all(&dir).unwrap();
        for sim in self.sims() {
            let shot = sim.screen();
            let mut canvas = Canvas::new(shot.width, shot.height, [0; 3]);
            canvas.blit565(0, 0, shot.width, shot.height, &shot.pixels, 1);
            let name = format!("{variant}@{}x{}.png", shot.width, shot.height);
            std::fs::write(dir.join(name), canvas.png()).unwrap();
        }
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

fn with_files(model: Model) -> Sim {
    let mut sim = world(model);
    sim.fit_bulk(Presence::Ready);
    sim
}

fn hilly(model: Model) -> Sim {
    let mut sim = sensors(model);
    sim.fit_bulk(Presence::Ready);
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

fn vehicles(sim: &mut Sim) {
    for (range_m, side) in [(90.0, Side::Behind), (150.0, Side::Right)] {
        sim.vehicle(Vehicle {
            range_m,
            closing_kmh: 25.0,
            fast: false,
            side,
        });
    }
}

#[test]
fn first_run() {
    let mut walk = Walk::fresh(world);
    while !(walk.guide.vana_page() == "boot" && walk.guide.device.lit()) {
        assert!(walk.guide.now() < seconds(15), "the brand never shows");
        walk.advance(1);
    }
    walk.advance(200);
    walk.cell("brand", "boot");
    walk.guide(|sim| {
        sim.wait_usable();
        sim.expect_text("WELCOME");
    });
    walk.cell("setup", "welcome");
    walk.guide(|sim| {
        sim.press(Role::Act);
        sim.expect_text("SET FTP");
    });
    walk.cell("setup", "power");
    for (next, variant) in [
        ("SET LTHR", "heart"),
        ("PAIR SENSORS", "sensors"),
        ("RIDE", "done"),
    ] {
        walk.guide(|sim| {
            sim.press(Role::Move);
            sim.press(Role::Act);
            sim.expect_text(next);
        });
        walk.cell("setup", variant);
    }
    walk.guide(|sim| {
        sim.press(Role::Move);
        sim.press(Role::Act);
        assert!(sim.on_home(), "{}", sim.screen().text());
    });
    walk.cell("home", "no-sensors");
}

#[test]
fn menus() {
    let mut walk = Walk::paired(sensors, &KINDS[..2]);
    walk.cell("home", "sensors");
    walk.guide(|sim| sim.home("SENSORS"));
    walk.cell("sensors", "list");
    walk.guide(|sim| sim.open_sensor_detail("POWER METER"));
    walk.cell("sensors", "detail");
    walk.guide(|sim| {
        sim.press(Role::Move);
        sim.expect_text("SENSOR?");
    });
    walk.cell("sensors", "forget");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.expect_text("BATTERY");
        sim.press(Role::Back);
        sim.nearby_page();
    });
    walk.cell("sensors", "scanning");
    walk.guide(|sim| sim.wait_for("ID 31", seconds(20)).map(drop).unwrap());
    walk.cell("sensors", "nearby");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.press(Role::Back);
        assert!(sim.on_home(), "{}", sim.screen().text());
        sim.home("RIDE");
        sim.expect_text("READY");
    });
    walk.cell("ready", "free-ride");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.choose_workout("VO2 MAX 5X3");
    });
    walk.cell("ready", "workout");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.home("TRAIN");
        sim.expect_text("WORKOUTS");
    });
    walk.cell("workouts", "top");
    walk.guide(|sim| {
        for _ in 0..9 {
            sim.press(Role::Move);
        }
        sim.advance(seconds(1));
    });
    walk.cell("workouts", "scrolled");
}

#[test]
fn settings() {
    let mut walk = Walk::paired(world, &[]);
    walk.guide(|sim| {
        sim.home("SETTINGS");
        sim.expect_text("PROFILE");
    });
    walk.cell("settings", "profile");
    walk.guide(|sim| {
        sim.press(Role::Act);
        sim.advance(seconds(1));
        sim.expect_field("FTP WATTS", "215");
    });
    walk.cell("settings", "editor");
    let mut moved = 0;
    walk.guide(|sim| {
        sim.press(Role::Back);
        while !sim.screen().contains("WEIGHT KG") {
            assert!(
                moved < 11,
                "no BODY section; screen:\n{}",
                sim.screen().text()
            );
            sim.press(Role::Move);
            moved += 1;
        }
        sim.advance(seconds(1));
    });
    walk.cell("settings", "body");
    walk.guide(|sim| {
        for _ in moved..11 {
            sim.press(Role::Move);
        }
        sim.advance(seconds(1));
        sim.expect_field("MODE", "NORMAL");
    });
    walk.cell("settings", "display-normal");
    walk.guide(|sim| {
        sim.press(Role::Act);
        sim.expect_field("MODE", "SAVER");
        sim.wait_for("SAVED", seconds(5)).unwrap();
    });
    walk.cell("settings", "display-saver");
}

#[test]
fn bests_and_form() {
    let mut walk = Walk::paired(with_files, &KINDS);
    let bests_row = |sim: &mut Sim| {
        sim.home("SETTINGS");
        sim.press(Role::Move);
        sim.press(Role::Move);
    };
    walk.guide(|sim| {
        bests_row(sim);
        sim.press(Role::Act);
        sim.expect_text("BESTS NOT KEPT");
    });
    walk.cell("bests", "not-kept");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.press(Role::Move);
        sim.press(Role::Act);
        sim.wait_for("NO POWER RIDES", seconds(30)).unwrap();
    });
    walk.cell("form", "no-rides");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.press(Role::Back);
        assert!(sim.on_home(), "{}", sim.screen().text());
    });
    walk.all(|sim| {
        sim.init_bulk();
        sim.init_volumes();
        sim.put_file("notes.txt", b"x");
        sim.ride_at(30.0, 240.0, 140.0, 90.0);
    });
    walk.guide(Sim::start_ride);
    walk.advance(minutes(6));
    walk.guide(|sim| {
        sim.stop_ride();
        sim.expect_text("NEW BESTS");
    });
    walk.cell("summary", "new-bests");
    walk.guide(|sim| {
        for _ in 0..4 {
            if sim.screen().contains("RIDE SAVED") {
                break;
            }
            sim.press(Role::Move);
        }
        sim.done();
        bests_row(sim);
        sim.press(Role::Act);
        sim.expect_text("LONGEST RIDE");
    });
    walk.cell("bests", "kept");
    walk.guide(|sim| {
        sim.press(Role::Act);
        sim.expect_text("CLEAR BESTS?");
    });
    walk.cell("bests", "clear");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.expect_text("LONGEST RIDE");
        sim.press(Role::Back);
        sim.press(Role::Move);
        sim.press(Role::Act);
        sim.wait_for("FATIGUE", seconds(60)).unwrap();
    });
    walk.cell("form", "rides");
}

fn riding(world: fn(Model) -> Sim) -> Walk {
    let mut walk = Walk::paired(world, &KINDS);
    walk.all(|sim| {
        sim.init_bulk();
        sim.format_files();
    });
    walk.guide(Sim::start_ride);
    walk.all(|sim| sim.ride_at(28.0, 230.0, 142.0, 88.0));
    walk.advance(seconds(20));
    walk
}

#[test]
fn a_free_ride() {
    let mut walk = riding(with_files);
    let pages = [
        ("ride-main", "ride/main"),
        ("ride-lap", "ride/lap"),
        ("ride-map", "ride/map"),
    ];
    for (screen, page) in pages {
        walk.guide(|sim| sim.ride_page(page));
        walk.cell(screen, "free-ride");
    }
    walk.all(vehicles);
    walk.advance(seconds(2));
    for (screen, page) in pages {
        walk.guide(|sim| sim.ride_page(page));
        walk.cell(screen, "vehicles");
    }
    walk.advance(seconds(30));
    walk.guide(|sim| {
        sim.ride_page("ride/main");
        sim.pause_ride();
    });
    walk.cell("paused", "free-ride");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.expect_text("END RIDE?");
    });
    walk.cell("end-ride", "free-ride");
    walk.guide(|sim| {
        sim.press(Role::Move);
        sim.expect_text("DISCARD RIDE?");
    });
    walk.cell("discard-ride", "free-ride");
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.expect_text("END RIDE?");
        sim.press(Role::Act);
        sim.wait_for("RIDE SAVED", seconds(5)).unwrap();
    });
    walk.cell("summary", "overview");
}

#[test]
fn a_saver_ride() {
    let mut walk = Walk::paired(world, &KINDS);
    walk.guide(|sim| {
        sim.home("SETTINGS");
        for _ in 0..11 {
            sim.press(Role::Move);
        }
        sim.expect_field("MODE", "NORMAL");
        sim.press(Role::Act);
        sim.expect_field("MODE", "SAVER");
        for _ in 0..3 {
            if sim.on_home() {
                break;
            }
            sim.press(Role::Back);
        }
        sim.start_ride();
    });
    walk.all(|sim| sim.ride_at(28.0, 230.0, 142.0, 88.0));
    walk.advance(seconds(20));
    walk.all(vehicles);
    walk.advance(seconds(2));
    walk.cell("ride-main", "saver");
}

#[test]
fn a_climb_and_its_summary() {
    let mut walk = riding(hilly);
    walk.guide(|sim| sim.ride_page("ride/climb"));
    walk.all(|sim| sim.ride_at(14.0, 260.0, 150.0, 75.0));
    walk.ride_to(1600.0);
    walk.cell("ride-climb", "climbing");
    walk.all(|sim| sim.ride_at(20.0, 260.0, 150.0, 85.0));
    walk.ride_to(1800.0);
    walk.guide(Sim::stop_ride);
    walk.cell("summary", "climb");
    for (variant, text) in [
        ("effort", "NP W"),
        ("elevation", "DESCENT M"),
        ("best-power", "BEST POWER W"),
    ] {
        walk.guide(|sim| {
            sim.press(Role::Move);
            sim.expect_text(text);
        });
        walk.cell("summary", variant);
    }
}

#[test]
fn a_workout() {
    let mut walk = Walk::paired(world, &KINDS);
    walk.guide(|sim| {
        sim.choose_workout("FTP TEST 20 MIN");
        sim.press(Role::Act);
        sim.wait_for("RECORDING", seconds(2)).unwrap();
    });
    walk.all(|sim| sim.ride_at(30.0, 150.0, 142.0, 90.0));
    walk.advance(seconds(30));
    walk.cell("ride-workout", "warm-up");
    walk.guide(|sim| sim.ride_page("ride/main"));
    walk.cell("ride-main", "workout");
    walk.guide(|sim| {
        sim.ride_page("ride/workout");
        for _ in 0..10 {
            sim.press(Role::Back);
            sim.advance(seconds(3));
        }
        sim.expect_text("ALL OUT");
    });
    walk.cell("ride-workout", "all-out");
    walk.all(|sim| sim.ride_at(30.0, 260.0, 142.0, 90.0));
    walk.guide(|sim| {
        sim.wait_for("NEW FTP", seconds(25 * 60)).unwrap();
    });
    walk.cell("sheet", "new-ftp");
}

#[derive(Clone, Copy)]
struct Waypoint {
    east_m: f64,
    north_m: f64,
}

const fn waypoint(east_m: f64, north_m: f64) -> Waypoint {
    Waypoint { east_m, north_m }
}

fn gpx(name: &str, waypoints: &[Waypoint]) -> String {
    let at = |point: Waypoint| {
        let o = origin();
        let lat = o.lat + point.north_m / 111_195.0;
        let lon = o.lon + point.east_m / (111_195.0 * o.lat.to_radians().cos());
        (Point::new(lat, lon), None)
    };
    let mut points = vec![at(waypoints[0])];
    for pair in waypoints.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        let (east, north) = (to.east_m - from.east_m, to.north_m - from.north_m);
        let steps = (east.hypot(north) / 10.0).ceil();
        for step in 1..=steps as usize {
            let t = step as f64 / steps;
            points.push(at(waypoint(
                from.east_m + t * east,
                from.north_m + t * north,
            )));
        }
    }
    shell_simulator::route::to_gpx(name, &points)
}

const COURSE: [Waypoint; 4] = [
    waypoint(0.0, 0.0),
    waypoint(0.0, 1_000.0),
    waypoint(180.0, 1_000.0),
    waypoint(180.0, 1_800.0),
];

fn on_course(route: fn(Model) -> Sim) -> Walk {
    let mut walk = Walk::paired(route, &KINDS[..3]);
    walk.all(|sim| {
        sim.init_bulk();
        sim.format_files();
        assert_eq!(sim.command("PHONE START").0, "ACCEPTED");
        sim.device.sim_mut().access_point.clients = 1;
        sim.advance(seconds(1));
        let (status, _) = sim.phone_request(
            Method::Put,
            "/api/course",
            gpx("course", &COURSE).as_bytes(),
        );
        assert_eq!(status, 200);
        assert_eq!(sim.command("PHONE STOP").0, "ACCEPTED");
        sim.advance(seconds(1));
    });
    walk.guide(|sim| {
        sim.home("RIDE");
        sim.expect_text("READY");
    });
    walk
}

fn course_world(model: Model, waypoints: &[Waypoint]) -> Sim {
    let mut sim = sensors(model);
    sim.fit_bulk(Presence::Ready);
    sim.ride_route(Route::from_gpx(&gpx("route", waypoints)).expect("a route"));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim
}

#[test]
fn a_course() {
    let mut walk = on_course(|model| course_world(model, &COURSE));
    walk.cell("ready", "course");
    walk.guide(|sim| {
        sim.press(Role::Act);
        sim.wait_for("RECORDING", seconds(2)).unwrap();
    });
    walk.all(|sim| sim.ride_at(30.0, 210.0, 142.0, 88.0));
    walk.ride_to(300.0);
    walk.cell("ride-main", "course-ahead");
    walk.guide(|sim| sim.ride_page("ride/map"));
    walk.cell("ride-map", "course");
    walk.ride_to(900.0);
    walk.guide(|sim| sim.ride_page("ride/main"));
    walk.cell("ride-main", "turn");
    while !walk.guide.screen().contains("ARRIVED") {
        assert!(walk.guide.world().distance_m < 2_200.0, "never arrived");
        walk.advance(seconds(1));
    }
    walk.advance(seconds(1));
    walk.cell("ride-main", "arrived");
}

#[test]
fn off_course() {
    let mut walk =
        on_course(|model| course_world(model, &[waypoint(0.0, 0.0), waypoint(0.0, 1_400.0)]));
    walk.guide(|sim| {
        sim.press(Role::Act);
        sim.wait_for("RECORDING", seconds(2)).unwrap();
    });
    walk.all(|sim| sim.ride_at(30.0, 210.0, 142.0, 88.0));
    while !walk.guide.screen().contains("TURN BACK") {
        assert!(walk.guide.world().distance_m < 1_350.0, "never off course");
        walk.advance(seconds(1));
    }
    walk.cell("ride-main", "off-course");
}

#[test]
fn history() {
    let mut walk = Walk::paired(world, &["HEART RATE"]);
    walk.guide(|sim| {
        sim.home("RIDES");
        sim.expect_text("NO RIDES YET");
    });
    walk.cell("history", "empty");
    let mut walk = Walk::paired(world, &["HEART RATE"]);
    walk.all(|sim| {
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
    walk.guide(|sim| {
        sim.home("RIDES");
        sim.wait_for("1/8", seconds(5)).unwrap();
        sim.advance(seconds(1));
    });
    walk.cell("history", "rides");
    walk.guide(|sim| {
        for _ in 0..7 {
            sim.press(Role::Move);
        }
        sim.wait_for("8/8", seconds(2)).unwrap();
        sim.advance(seconds(2));
    });
    walk.cell("history", "scrolled");
}

#[test]
fn a_saved_ride_and_the_phone() {
    let mut walk = Walk::paired(with_files, &KINDS);
    walk.all(|sim| {
        sim.init_bulk();
        sim.format_files();
    });
    walk.guide(Sim::start_ride);
    walk.all(|sim| sim.ride_at(28.0, 230.0, 142.0, 88.0));
    walk.advance(seconds(60));
    walk.guide(|sim| {
        sim.stop_ride();
        sim.done();
        sim.home("RIDES");
        sim.press(Role::Act);
        sim.wait_gone("READING", seconds(5)).unwrap();
        sim.expect_text("SAVED RIDE");
    });
    walk.cell("past-ride", "overview");
    for (variant, text) in [
        ("effort", "NP W"),
        ("elevation", "DESCENT M"),
        ("best-power", "BEST POWER W"),
        ("delete", "IT GOES FROM YOUR RIDES"),
    ] {
        walk.guide(|sim| {
            sim.press(Role::Move);
            sim.expect_text(text);
        });
        walk.cell("past-ride", variant);
    }
    walk.guide(|sim| {
        sim.press(Role::Move);
        sim.expect_text("SAVED RIDE");
        sim.press(Role::Act);
        sim.advance(seconds(1));
        sim.expect_text("JOIN VANA WI-FI");
    });
    walk.cell("phone", "join");
    walk.all(|sim| {
        sim.device.sim_mut().access_point.clients = 1;
        sim.advance(seconds(1));
    });
    walk.guide(|sim| sim.expect_text("OPEN YOUR RIDES"));
    walk.cell("phone", "open");
}

#[test]
fn turning_off() {
    let mut walk = Walk::paired(world, &[]);
    walk.all(|sim| sim.set_battery(64, true));
    walk.advance(seconds(2));
    walk.guide(|sim| {
        sim.press(Role::Back);
        sim.expect_text("TURN OFF");
    });
    walk.cell("power", "turn-off");
    walk.all(|sim| {
        sim.report(Role::Act);
        sim.advance(80);
    });
    walk.cell("power", "turning-off");
    walk.advance(seconds(2));
    for sim in walk.sims() {
        assert!(sim.device.standby_lit());
    }
    walk.off_cell("power", "charging");
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
fn the_format_offer() {
    let mut walk = Walk::fresh(foreign);
    walk.guide(|sim| {
        sim.wait_for("FORMAT STORAGE?", seconds(20)).unwrap();
    });
    walk.cell("format", "foreign-settings");
}

#[test]
fn unreachable_cells_are_listed() {
    let Some(dir) = std::env::var_os("SIM_SCREENS") else {
        return;
    };
    let lines: String = UNREACHABLE
        .iter()
        .map(|(cell, why)| format!("{cell}\t{why}\n"))
        .collect();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(std::path::Path::new(&dir).join("unreachable.tsv"), lines).unwrap();
}
