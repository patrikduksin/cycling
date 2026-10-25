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

#[cfg(feature = "cycling")]
pub use scenes::{SCENES, Scene, measure};

#[cfg(feature = "cycling")]
mod scenes {
    use super::Request;
    use crate::ant::device_type;
    use crate::bulk::Presence;
    use crate::route::{Point, Route};
    use crate::sim::{Sim, minutes, seconds};
    use crate::world::{Side, Vehicle};
    use device_api::input::Button;

    /// A screen to measure and how a rider reaches it.
    pub struct Scene {
        pub name: &'static str,
        pub reach: fn() -> Sim,
    }

    pub static SCENES: [Scene; 32] = [
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
            name: "summary",
            reach: summary,
        },
        Scene {
            name: "sensors list",
            reach: sensors,
        },
        Scene {
            name: "workouts",
            reach: workouts,
        },
        Scene {
            name: "ride stats",
            reach: stats,
        },
        Scene {
            name: "lap",
            reach: lap_page,
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
            name: "back to start",
            reach: back_page,
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
            name: "ready",
            reach: ready,
        },
        Scene {
            name: "settings",
            reach: settings,
        },
        Scene {
            name: "setup",
            reach: setup,
        },
        Scene {
            name: "phone",
            reach: phone,
        },
        Scene {
            name: "bests",
            reach: bests,
        },
        Scene {
            name: "form",
            reach: form,
        },
        Scene {
            name: "history",
            reach: history,
        },
        Scene {
            name: "past ride",
            reach: past_ride,
        },
        Scene {
            name: "sensor detail",
            reach: sensor_detail,
        },
        Scene {
            name: "summary page 2",
            reach: summary_second,
        },
        Scene {
            name: "boot",
            reach: boot,
        },
        Scene {
            name: "setup power step",
            reach: setup_power_step,
        },
        Scene {
            name: "settings editing",
            reach: settings_editing,
        },
        Scene {
            name: "bests confirm",
            reach: bests_confirm,
        },
        Scene {
            name: "form no power",
            reach: form_no_power,
        },
        Scene {
            name: "phone from saved ride",
            reach: phone_from_ride,
        },
        Scene {
            name: "main with workout band",
            reach: main_with_band,
        },
        Scene {
            name: "settings light",
            reach: light,
        },
        Scene {
            name: "home light",
            reach: home_light,
        },
    ];

    /// Paint the next frame `frames` times, plus once counted.
    #[track_caller]
    pub fn measure(sim: &mut Sim, frames: u32) -> Request {
        let display = sim.device.display.clone();
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

    fn home() -> Sim {
        let mut sim = rider(false);
        sim.advance(seconds(2));
        assert!(sim.on_home(), "{}", sim.screen().text());
        sim
    }

    fn ride_main() -> Sim {
        let mut sim = rider(true);
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

    fn climb() -> Sim {
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
        sim.ride_page("4/6");
        while sim.world().distance_m < 1000.0 {
            sim.advance(seconds(1));
        }
        sim.ride_at(14.0, 260.0, 0.0, 75.0);
        while sim.world().distance_m < 1600.0 {
            sim.advance(seconds(1));
        }
        sim.expect_text("CLIMBING");
        sim
    }

    fn summary() -> Sim {
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
        sim.wait_for("LIVE DATA", seconds(20)).unwrap();
        sim.expect_text("MY SENSORS");
        sim
    }

    fn workouts() -> Sim {
        let mut sim = rider(false);
        sim.home("TRAIN");
        sim.expect_text("WORKOUTS");
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

    fn stats() -> Sim {
        let mut sim = riding(120);
        sim.ride_page("2/6");
        sim.expect_text("AVG W");
        sim
    }

    fn lap_page() -> Sim {
        let mut sim = riding(60);
        sim.lap(1);
        sim.advance(seconds(30));
        sim.ride_page("3/6");
        sim.expect_text("LAP TIME");
        sim
    }

    fn workout_page() -> Sim {
        let mut sim = rider(false);
        sim.choose_workout("VO2 MAX 5X3");
        sim.press(Button::BottomRight);
        sim.wait_for("RECORDING", seconds(2)).unwrap();
        sim.ride_at(30.0, 120.0, 142.0, 90.0);
        sim.advance(seconds(20));
        assert_eq!(sim.screen().page().as_deref(), Some("1/7"));
        sim.expect_text("1/12");
        sim
    }

    fn map_page() -> Sim {
        let mut sim = riding(90);
        sim.ride_page("5/6");
        sim.expect_text("MAP");
        sim
    }

    fn back_page() -> Sim {
        let mut sim = riding(90);
        sim.ride_page("6/6");
        sim.expect_text("BACK TO START");
        sim
    }

    fn paused() -> Sim {
        let mut sim = riding(30);
        sim.pause_ride();
        sim.expect_text("PAUSED");
        sim
    }

    fn end_ride() -> Sim {
        let mut sim = paused();
        sim.press(Button::TopLeft);
        sim.expect_text("END RIDE?");
        sim
    }

    fn ready() -> Sim {
        let mut sim = rider(false);
        sim.home("RIDE");
        sim.expect_text("READY");
        sim
    }

    fn settings() -> Sim {
        let mut sim = rider(false);
        sim.home("SETTINGS");
        sim.expect_text("PROFILE");
        sim
    }

    fn setup() -> Sim {
        let mut sim = Sim::new();
        sim.wait_usable();
        sim.expect_text("WELCOME");
        sim
    }

    fn phone() -> Sim {
        let mut sim = rider(false);
        sim.connect_phone();
        sim.expect_text("RIDES");
        sim
    }

    /// A card with its volumes made and one ride that sets every best, on Home.
    fn after_a_long_ride() -> Sim {
        let mut sim = Sim::new();
        sim.fit_bulk(Presence::Ready);
        let mut sim = sim.boot();
        sim.init_bulk();
        sim.init_volumes();
        sim.put_file("notes.txt", b"x");
        sim.ant_sensor(device_type::HEART_RATE, 4242);
        sim.ant_sensor(device_type::POWER, 1234);
        sim.ant_sensor(device_type::SPEED_CADENCE, 777);
        sim.ride_route(Route::loop_around(
            Point::new(-33.45, -70.66),
            5_000.0,
            20.0,
        ));
        sim.ride_at(0.0, 0.0, 72.0, 0.0);
        sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
        sim.start_ride();
        sim.ride_at(30.0, 200.0, 140.0, 90.0);
        sim.advance(minutes(22));
        sim.stop_ride();
        sim.done();
        sim
    }

    fn bests() -> Sim {
        let mut sim = after_a_long_ride();
        sim.home("SETTINGS");
        sim.press(Button::BottomLeft);
        sim.press(Button::BottomLeft);
        sim.press(Button::BottomRight);
        sim.expect_text("LONGEST RIDE");
        sim
    }

    fn form() -> Sim {
        let mut sim = after_a_long_ride();
        sim.home("SETTINGS");
        for _ in 0..3 {
            sim.press(Button::BottomLeft);
        }
        sim.press(Button::BottomRight);
        sim.wait_for("FATIGUE", seconds(60)).unwrap();
        sim
    }

    fn saved_rides() -> Sim {
        let mut sim = rider(false);
        for length in [3, 5] {
            sim.start_ride();
            sim.ride_at(28.0, 230.0, 142.0, 88.0);
            sim.advance(minutes(length));
            sim.stop_ride();
            sim.done();
        }
        sim
    }

    fn history() -> Sim {
        let mut sim = saved_rides();
        sim.home("RIDES");
        sim.expect_text("1/2");
        sim
    }

    fn past_ride() -> Sim {
        let mut sim = history();
        sim.press(Button::BottomRight);
        sim.wait_gone("READING", seconds(5)).unwrap();
        sim.expect_text("SAVED RIDE");
        sim
    }

    fn sensor_detail() -> Sim {
        let mut sim = rider(false);
        sim.home("SENSORS");
        sim.wait_for("LIVE DATA", seconds(20)).unwrap();
        sim.open_sensor_detail("POWER METER");
        sim
    }

    fn summary_second() -> Sim {
        let mut sim = summary();
        sim.swipe_next();
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

    fn setup_power_step() -> Sim {
        let mut sim = setup();
        sim.press(Button::BottomRight);
        sim.expect_text("SET FTP");
        assert_eq!(sim.vana_page(), "setup");
        sim
    }

    fn settings_editing() -> Sim {
        let mut sim = settings();
        sim.press(Button::BottomRight);
        sim
    }

    fn bests_confirm() -> Sim {
        let mut sim = bests();
        sim.press(Button::BottomRight);
        sim.expect_text("CLEAR BESTS?");
        sim
    }

    fn form_no_power() -> Sim {
        let mut sim = rider(false);
        sim.home("SETTINGS");
        for _ in 0..3 {
            sim.press(Button::BottomLeft);
        }
        sim.press(Button::BottomRight);
        sim.wait_for("NO POWER RIDES", seconds(60)).unwrap();
        sim
    }

    fn phone_from_ride() -> Sim {
        let mut sim = past_ride();
        sim.press(Button::BottomRight);
        sim.advance(seconds(1));
        sim.expect_text("JOIN VANA WI-FI");
        sim
    }

    fn main_with_band() -> Sim {
        let mut sim = workout_page();
        sim.ride_page("2/7");
        sim
    }

    fn light() -> Sim {
        let mut sim = rider(false);
        sim.home("SETTINGS");
        sim.swipe_next();
        sim.swipe_next();
        for _ in 0..3 {
            sim.press(Button::BottomLeft);
        }
        sim.expect_field("THEME", "DARK");
        sim.press(Button::BottomRight);
        sim
    }

    fn home_light() -> Sim {
        let mut sim = light();
        sim.press(Button::TopLeft);
        sim.expect_text("READY TO RIDE");
        sim
    }
}
