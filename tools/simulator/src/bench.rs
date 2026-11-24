//! The host render benchmark (#167): representative VANA screens reached in
//! the simulated world, then their frame painted again and again.
//!
//! The display paints a requested frame `frames` times into a scratch frame,
//! timing each pass in this thread's CPU time, counts the painters' work on
//! one more pass (`firmware_shell::rendering::work`), and hashes the pixels.
//! Host time is only a proxy for the device; the work counts and the hash do
//! not depend on the machine. `sim-bench` prints them and the
//! `render_bench` test holds the hashes, so a faster painter must paint the
//! same pixels.
use firmware_shell::rendering::work::{self, Work};

/// A frame to paint repeatedly, and what painting it took.
#[derive(Clone, Debug, Default)]
pub struct Request {
    pub frames: u32,
    /// CPU time of each pass, in nanoseconds.
    pub passes: Vec<u64>,
    pub work: Work,
    pub hash: u64,
    /// Every pass painted the same pixels.
    pub stable: bool,
}

impl Request {
    pub fn new(frames: u32) -> Self {
        Self {
            frames,
            ..Self::default()
        }
    }

    pub(crate) fn run(
        &mut self,
        (width, height): (usize, usize),
        row: &mut impl FnMut(usize, &mut [u16]),
    ) {
        let mut pixels = vec![0u16; width * height];
        let mut paint = |pixels: &mut [u16]| {
            for (y, line) in pixels.chunks_exact_mut(width).enumerate() {
                row(y, line);
            }
        };
        self.work = work::count(|| paint(&mut pixels));
        self.hash = hash(&pixels);
        self.passes.clear();
        // Timed passes paint over stale pixels, as the device's strip buffers
        // hold the previous strip, so a pixel a painter skips makes the frame
        // unstable.
        let stale = vec![0x5aa5; pixels.len()];
        let first = core::mem::replace(&mut pixels, stale);
        for _ in 0..self.frames {
            let started = cpu_ns();
            paint(&mut pixels);
            self.passes.push(cpu_ns().saturating_sub(started));
        }
        self.stable = self.passes.is_empty() || pixels == first;
        self.frames = 0;
    }

    /// The fastest pass and the median, in microseconds.
    pub fn micros(&self) -> (f64, f64) {
        let mut passes = self.passes.clone();
        passes.sort_unstable();
        let at = |i: usize| passes.get(i).map_or(0.0, |&ns| ns as f64 / 1000.0);
        (at(0), at(passes.len() / 2))
    }
}

/// FNV-1a over the frame's RGB565 pixels.
pub fn hash(pixels: &[u16]) -> u64 {
    pixels.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &pixel| {
        pixel.to_le_bytes().iter().fold(hash, |hash, &byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
        })
    })
}

/// This thread's CPU time, so time the host spends on other work is left out.
#[cfg(target_os = "linux")]
fn cpu_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        seconds: i64,
        nanos: i64,
    }
    unsafe extern "C" {
        fn clock_gettime(clock: i32, time: *mut Timespec) -> i32;
    }
    const CLOCK_THREAD_CPUTIME_ID: i32 = 3;
    let mut time = Timespec {
        seconds: 0,
        nanos: 0,
    };
    // SAFETY: `time` is a valid, writable timespec for the call.
    unsafe { clock_gettime(CLOCK_THREAD_CPUTIME_ID, &mut time) };
    time.seconds as u64 * 1_000_000_000 + time.nanos as u64
}

#[cfg(not(target_os = "linux"))]
fn cpu_ns() -> u64 {
    use std::sync::OnceLock;
    use std::time::Instant;
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_nanos() as u64
}

pub use scenes::{SCENES, Scene, measure};

pub(crate) mod scenes {
    use super::Request;
    use crate::ant::device_type;
    use crate::bulk::Presence;
    use crate::route::{Point, Route};
    use crate::sim::{Sim, seconds};
    use crate::world::{Side, Vehicle};
    use device_api::input::Role;

    /// A screen to measure and how a rider reaches it.
    pub struct Scene {
        pub name: &'static str,
        pub reach: fn() -> Sim,
    }

    pub static SCENES: [Scene; 20] = [
        Scene {
            name: "home",
            reach: home,
        },
        Scene {
            name: "ride main + radar",
            reach: ride_main,
        },
        Scene {
            name: "climb",
            reach: climb,
        },
        Scene {
            name: "ride done",
            reach: ride_done,
        },
        Scene {
            name: "sensors list",
            reach: sensors,
        },
        Scene {
            name: "ride stats",
            reach: stats,
        },
        Scene {
            name: "workout",
            reach: workout_page,
        },
        Scene {
            name: "map",
            reach: map_page,
        },
        Scene {
            name: "map course",
            reach: map_course,
        },
        Scene {
            name: "map off course",
            reach: map_off_course,
        },
        Scene {
            name: "map santiago 15 km/h",
            reach: || densest_santiago((1_200.0, 0.0), 45.0, 60.0, 15.0, "50 M"),
        },
        Scene {
            name: "map santiago 25 km/h",
            reach: || densest_santiago((1_200.0, 0.0), 45.0, 250.0, 25.0, "100 M"),
        },
        Scene {
            name: "map santiago 50 km/h",
            reach: || densest_santiago((-800.0, 1_200.0), 135.0, 500.0, 50.0, "250 M"),
        },
        Scene {
            name: "map santiago 90 km/h",
            reach: || densest_santiago((-1_600.0, -1_200.0), 45.0, 1_500.0, 90.0, "250 M"),
        },
        Scene {
            name: "paused",
            reach: paused,
        },
        Scene {
            name: "end ride",
            reach: end_ride,
        },
        Scene {
            name: "sensor detail",
            reach: sensor_detail,
        },
        Scene {
            name: "boot",
            reach: boot,
        },
        Scene {
            name: "main with workout band",
            reach: main_with_band,
        },
        Scene {
            name: "home light",
            reach: home_light,
        },
    ];

    /// Paint the next frame `frames` times, plus once counted.
    #[track_caller]
    pub fn measure(sim: &mut Sim, frames: u32) -> Request {
        vana::screens::check::enable(false);
        sim.device.shell.forget_panel();
        let display = sim.device.display.clone();
        display.0.borrow_mut().check_skipped_rows = false;
        display.0.borrow_mut().bench = Some(Request::new(frames));
        let submitted = display.0.borrow().submissions;
        for _ in 0..500 {
            sim.advance(10);
            if display.0.borrow().submissions != submitted {
                break;
            }
        }
        let request = display.0.borrow_mut().bench.take().unwrap_or_default();
        assert!(
            display.0.borrow().submissions != submitted,
            "no frame within 5 s"
        );
        request
    }

    fn rider(radar: bool) -> Sim {
        let mut sim = Sim::new().boot();
        sim.discard_recording();
        sim.ant_sensor(device_type::HEART_RATE, 4242);
        sim.ant_sensor(device_type::POWER, 1234);
        sim.ant_sensor(device_type::SPEED_CADENCE, 777);
        let mut kinds = vec!["HEART RATE", "POWER METER", "SPEED/CAD"];
        if radar {
            sim.ant_sensor(device_type::RADAR, 31);
            kinds.push("RADAR");
        }
        sim.ride_route(Route::loop_around(
            Point::new(-33.45, -70.66),
            5_000.0,
            20.0,
        ));
        sim.ride_at(0.0, 0.0, 72.0, 0.0);
        sim.pair(&kinds);
        sim
    }

    pub(crate) fn home() -> Sim {
        let mut sim = rider(false);
        sim.advance(seconds(2));
        assert!(sim.on_home(), "{}", sim.screen().text());
        sim
    }

    pub(crate) fn ride_main_radar_saver() -> Sim {
        let mut sim = rider(true);
        sim.option("DISPLAY", "SAVER");
        ride_main_from(sim)
    }

    pub(crate) fn ride_main() -> Sim {
        ride_main_from(rider(true))
    }

    fn ride_main_from(mut sim: Sim) -> Sim {
        sim.start_ride();
        sim.ride_at(28.0, 230.0, 142.0, 88.0);
        sim.advance(seconds(20));
        for (range_m, side) in [(90.0, Side::Behind), (150.0, Side::Right)] {
            sim.vehicle(Vehicle {
                range_m,
                closing_kmh: 25.0,
                fast: false,
                side,
            });
        }
        sim.advance(seconds(2));
        sim
    }

    pub(crate) fn climb() -> Sim {
        let mut sim = Sim::new().boot();
        sim.discard_recording();
        sim.ant_sensor(device_type::SPEED_CADENCE, 777);
        sim.ant_sensor(device_type::POWER, 1234);
        sim.ride_route(Route::profile(
            Point::new(-33.45, -70.66),
            &[
                (0.0, 600.0),
                (1000.0, 600.0),
                (2500.0, 690.0),
                (4000.0, 600.0),
                (6000.0, 600.0),
            ],
        ));
        sim.ride_at(0.0, 0.0, 0.0, 0.0);
        sim.pair(&["SPEED/CAD", "POWER METER"]);
        sim.start_ride();
        sim.ride_at(30.0, 180.0, 0.0, 85.0);
        sim.ride_page("ride/climb");
        while sim.world().distance_m < 1000.0 {
            sim.advance(seconds(1));
        }
        sim.ride_at(14.0, 260.0, 0.0, 75.0);
        while sim.world().distance_m < 1600.0 {
            sim.advance(seconds(1));
        }
        sim.expect_text("CLIMB DIST KM");
        sim
    }

    fn ride_done() -> Sim {
        let mut sim = rider(false);
        sim.start_ride();
        sim.ride_at(28.0, 230.0, 142.0, 88.0);
        sim.advance(seconds(90));
        sim.stop_ride();
        sim
    }

    fn sensors() -> Sim {
        let mut sim = rider(false);
        sim.home("SENSORS");
        sim.wait_for("LIVE", seconds(20)).unwrap();
        sim.expect_text("SENSORS");
        sim
    }

    /// Riding at a steady effort for `secs`, as `rider(false)` is paired.
    fn riding(secs: u64) -> Sim {
        let mut sim = rider(false);
        sim.start_ride();
        sim.ride_at(28.0, 230.0, 142.0, 88.0);
        sim.advance(seconds(secs));
        sim
    }

    pub(crate) fn stats() -> Sim {
        let mut sim = riding(120);
        sim.ride_page("ride/main");
        sim.expect_text("NP W");
        sim
    }

    pub(crate) fn workout_page() -> Sim {
        let mut sim = rider(false);
        sim.start_workout("VO2 MAX 5X3");
        sim.ride_at(30.0, 120.0, 142.0, 90.0);
        sim.advance(seconds(20));
        assert_eq!(sim.ride_page_name().as_deref(), Some("ride/workout"));
        sim.expect_text("LEFT");
        sim
    }

    pub(crate) fn map_page() -> Sim {
        let mut sim = riding(90);
        sim.ride_page("ride/map");
        sim.expect_text("100 M");
        sim
    }

    fn gpx_every_ten_metres(origin: Point, legs: &[(f64, f64)]) -> String {
        let at = |(east, north): (f64, f64)| {
            let lat = origin.lat + north / 111_195.0;
            let lon = origin.lon + east / (111_195.0 * origin.lat.to_radians().cos());
            (Point::new(lat, lon), None)
        };
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
        crate::route::to_gpx("bench", &points)
    }

    fn following_on_the_map(metres: f64) -> Sim {
        let origin = Point::new(-33.45, -70.66);
        let mut sim = Sim::new();
        sim.fit_bulk(Presence::Ready);
        let mut sim = sim.boot();
        sim.discard_recording();
        sim.init_bulk();
        sim.format_files();
        sim.connect_phone();
        let course = gpx_every_ten_metres(
            origin,
            &[
                (0.0, 0.0),
                (0.0, 1_000.0),
                (60.0, 1_010.0),
                (150.0, 1_060.0),
                (183.0, 1_066.0),
                (183.0, 4_000.0),
            ],
        );
        let (status, _) = sim.phone_request(
            device_api::server::Method::Put,
            "/api/course",
            course.as_bytes(),
        );
        assert_eq!(status, 200);
        sim.disconnect_phone();
        sim.advance(seconds(1));
        let route = gpx_every_ten_metres(origin, &[(0.0, 0.0), (0.0, 2_000.0)]);
        sim.ride_route(Route::from_gpx(&route).unwrap());
        sim.ant_sensor(device_type::POWER, 1234);
        sim.ant_sensor(device_type::SPEED_CADENCE, 777);
        sim.pair(&["POWER METER", "SPEED/CAD"]);
        sim.ride_at(36.0, 210.0, 0.0, 88.0);
        sim.start_ride();
        while sim.world().distance_m < metres {
            sim.advance(seconds(1));
        }
        sim.ride_page("ride/map");
        sim
    }

    const SANTIAGO: &[u8] =
        include_bytes!("../../../packages/terrain/tests/data/santiago-centro.terrain");
    const PLAZA: (f64, f64) = (-33.4378, -70.6505);

    fn densest_santiago(at: (f64, f64), bearing: f64, turn_m: f64, kmh: f64, scale: &str) -> Sim {
        let (sin, cos) = bearing.to_radians().sin_cos();
        let to = |right: f64, ahead: f64| {
            (
                at.0 + right * cos + ahead * sin,
                at.1 - right * sin + ahead * cos,
            )
        };
        let origin = Point::new(PLAZA.0, PLAZA.1);
        let mut sim = Sim::new();
        sim.fit_bulk(Presence::Ready);
        let mut sim = sim.boot();
        sim.discard_recording();
        sim.init_bulk();
        sim.format_files();
        sim.install_terrain(b"CHILE-TERRAIN-V1", SANTIAGO);
        // The map's zoom in these scenes depends on when the ride starts.
        sim.advance(seconds(11));
        sim.connect_phone();
        let course = gpx_every_ten_metres(
            origin,
            &[to(0.0, -600.0), to(0.0, turn_m), to(1_000.0, turn_m)],
        );
        let (status, _) = sim.phone_request(
            device_api::server::Method::Put,
            "/api/course",
            course.as_bytes(),
        );
        assert_eq!(status, 200);
        sim.disconnect_phone();
        sim.advance(seconds(1));
        let route = gpx_every_ten_metres(origin, &[to(0.0, -600.0), to(0.0, turn_m)]);
        sim.ride_route(Route::from_gpx(&route).unwrap());
        sim.ant_sensor(device_type::POWER, 1234);
        sim.ant_sensor(device_type::SPEED_CADENCE, 777);
        sim.pair(&["POWER METER", "SPEED/CAD"]);
        sim.ride_at(kmh, 210.0, 0.0, 88.0);
        sim.start_ride();
        while sim.world().distance_m < 570.0 {
            sim.advance(seconds(1));
        }
        sim.ride_page("ride/map");
        sim.advance(seconds(2));
        sim.expect_text(scale);
        sim
    }

    fn map_course() -> Sim {
        let mut sim = following_on_the_map(750.0);
        sim.expect_text("100 M");
        sim
    }

    fn map_off_course() -> Sim {
        let mut sim = following_on_the_map(1_140.0);
        sim.expect_text("TURN BACK");
        sim
    }

    fn paused() -> Sim {
        let mut sim = riding(30);
        sim.pause_ride();
        sim.expect_text("Paused");
        sim
    }

    fn end_ride() -> Sim {
        let mut sim = paused();
        sim.press(Role::Back);
        sim.expect_text("End ride?");
        sim
    }

    fn sensor_detail() -> Sim {
        let mut sim = rider(false);
        sim.home("SENSORS");
        sim.wait_for("LIVE", seconds(20)).unwrap();
        sim.open_sensor_detail("POWER METER");
        sim
    }

    fn boot() -> Sim {
        let mut sim = Sim::new();
        sim.discard_recording();
        for _ in 0..5_000 {
            if sim.vana_page() == "boot" {
                break;
            }
            sim.advance(1);
        }
        assert_eq!(sim.vana_page(), "boot");
        sim.advance(200);
        sim
    }

    fn main_with_band() -> Sim {
        let mut sim = workout_page();
        sim.ride_page("ride/main");
        sim
    }

    fn home_light() -> Sim {
        let mut sim = rider(false);
        sim.option("THEME", "LIGHT");
        sim.advance(seconds(1));
        assert!(sim.on_home(), "{}", sim.screen().text());
        sim
    }
}
