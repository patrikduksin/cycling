//! Scenario driver for the simulated device: press buttons, change the world,
//! let time pass and read what the screen shows.
use crate::ant::AntSensor;
use crate::recording::{Outcome, Recording};
use crate::route::Route;
use crate::screen::{Frame, TextRun};
use crate::session::Session;
use crate::world::{Vehicle, World};
use device_api::input::{Button, Input};
use device_api::storage::Region;
use std::fmt::Write as _;

/// The boot splash lasts 6.5 s; Home is drawn on the next frame.
const BOOT_MS: u64 = 7_000;
/// Firmware passes between a press and the next one VANA accepts (debounce is 350 ms).
const PRESS_GAP_MS: u64 = 400;

pub fn seconds(s: u64) -> u64 {
    s * 1000
}

pub fn minutes(m: u64) -> u64 {
    m * 60_000
}

/// What happened during a scenario, in world time, for logs and reports.
#[derive(Clone, Debug)]
pub struct Note {
    pub at_ms: u64,
    pub kind: NoteKind,
    pub text: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoteKind {
    Input,
    World,
    Device,
    Check,
}

/// An owned copy of the panel.
#[derive(Clone)]
pub struct Screen {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u16>,
}

impl Screen {
    pub fn frame(&self) -> Frame<'_> {
        Frame {
            width: self.width,
            height: self.height,
            pixels: &self.pixels,
        }
    }

    pub fn text(&self) -> String {
        self.frame().text()
    }

    pub fn contains(&self, needle: &str) -> bool {
        self.frame().contains(needle)
    }

    pub fn find(&self, needle: &str) -> Vec<TextRun> {
        self.frame().find(needle)
    }

    /// The value shown under a field label, such as the reading under "WATTS".
    pub fn field(&self, label: &str) -> Option<String> {
        let runs = self.frame().read();
        let label = runs.iter().find(|run| run.text == label)?;
        let bottom = label.y + 7 * label.scale;
        runs.iter()
            .filter(|run| run.y > bottom && run.y < bottom + 120)
            .filter(|run| {
                run.x + 30 >= label.x && run.x < label.x + 6 * label.scale * label.text.len()
            })
            .min_by_key(|run| run.y)
            .map(|run| run.text.clone())
    }

    /// A text run that reads exactly `text`, such as a button label.
    pub fn run(&self, text: &str) -> Option<TextRun> {
        self.frame().read().into_iter().find(|run| run.text == text)
    }
}

pub struct Sim {
    pub device: Session,
    pub notes: Vec<Note>,
    recording: Option<Recording>,
}

impl Sim {
    /// A powered-off C606 with VANA, an ANT radio and empty memory media.
    pub fn new() -> Self {
        let device = Session::device().expect("memory media always opens");
        let geometry = device.display.0.borrow().geometry;
        Self {
            recording: Recording::from_env(geometry.width, geometry.height),
            device,
            notes: Vec::new(),
        }
    }

    /// Power on and wait for the splash to finish.
    pub fn boot(mut self) -> Self {
        self.advance(BOOT_MS);
        self
    }

    pub fn now(&self) -> u64 {
        self.device.world.time_ms
    }

    pub fn world(&mut self) -> &mut World {
        &mut self.device.world
    }

    pub fn note(&mut self, kind: NoteKind, text: impl Into<String>) {
        self.notes.push(Note {
            at_ms: self.now(),
            kind,
            text: text.into(),
        });
    }

    pub fn advance(&mut self, ms: u64) {
        let Some(recording) = &mut self.recording else {
            self.device.step(ms);
            return;
        };
        let end = self.device.world.time_ms + ms;
        while self.device.world.time_ms < end {
            self.device.step((end - self.device.world.time_ms).min(10));
            let frame = self.device.display.0.borrow();
            recording.observe(
                self.device.world.time_ms,
                frame.submissions,
                &frame.pixels,
                &self.device.world,
            );
        }
    }

    /// Name this moment; recordings keep a screenshot of it.
    pub fn checkpoint(&mut self, name: &str) {
        self.note(NoteKind::Check, format!("checkpoint {name}"));
        if let Some(recording) = &mut self.recording {
            recording.checkpoint(
                name,
                self.device.world.time_ms,
                &self.device.display.0.borrow().pixels,
            );
        }
    }

    /// One physical press, then enough time for the next press to count.
    pub fn press(&mut self, button: Button) {
        self.note(NoteKind::Input, format!("press {button:?}"));
        self.device
            .input
            .push(self.device.now, Input::Button { button, code: 1 });
        self.advance(PRESS_GAP_MS);
    }

    /// Buttons pressed in the same companion report window.
    pub fn press_together(&mut self, buttons: &[Button]) {
        self.note(NoteKind::Input, format!("press together {buttons:?}"));
        for &button in buttons {
            self.device
                .input
                .push(self.device.now, Input::Button { button, code: 1 });
        }
        self.advance(PRESS_GAP_MS);
    }

    /// The companion repeats code-1 reports while a button stays down; the
    /// firmware cannot observe a release (see issue #86).
    pub fn hold(&mut self, button: Button, ms: u64) {
        self.note(NoteKind::Input, format!("hold {button:?} {ms} ms"));
        let mut held = 0;
        while held < ms {
            self.device
                .input
                .push(self.device.now, Input::Button { button, code: 1 });
            self.advance(100);
            held += 100;
        }
        self.advance(PRESS_GAP_MS);
    }

    pub fn screen(&self) -> Screen {
        let frame = self.device.display.0.borrow();
        Screen {
            width: frame.geometry.width,
            height: frame.geometry.height,
            pixels: frame.pixels.clone(),
        }
    }

    /// Advance until `needle` is on screen. Returns the waiting time, or the
    /// screen text when it never appeared.
    pub fn wait_for(&mut self, needle: &str, timeout_ms: u64) -> Result<u64, String> {
        self.wait_until(timeout_ms, |screen| screen.contains(needle))
            .map_err(|text| format!("{needle:?} not shown within {timeout_ms} ms; screen:\n{text}"))
    }

    /// Advance until `needle` is gone from the screen.
    pub fn wait_gone(&mut self, needle: &str, timeout_ms: u64) -> Result<u64, String> {
        self.wait_until(timeout_ms, |screen| !screen.contains(needle))
            .map_err(|text| {
                format!("{needle:?} still shown after {timeout_ms} ms; screen:\n{text}")
            })
    }

    fn wait_until(
        &mut self,
        timeout_ms: u64,
        done: impl Fn(&Screen) -> bool,
    ) -> Result<u64, String> {
        let start = self.now();
        let mut submissions = u32::MAX;
        loop {
            let current = self.device.display.0.borrow().submissions;
            if current != submissions {
                submissions = current;
                let screen = self.screen();
                if done(&screen) {
                    return Ok(self.now() - start);
                }
            }
            if self.now() - start >= timeout_ms {
                return Err(self.screen().text());
            }
            self.advance(50);
        }
    }

    /// Assert the screen shows `needle` now.
    #[track_caller]
    pub fn expect_text(&mut self, needle: &str) {
        let screen = self.screen();
        assert!(
            screen.contains(needle),
            "expected {needle:?} on screen at {} ms; screen:\n{}",
            self.now(),
            screen.text()
        );
        self.note(NoteKind::Check, format!("saw {needle}"));
    }

    /// Assert the value under a field label.
    #[track_caller]
    pub fn expect_field(&mut self, label: &str, value: &str) {
        let screen = self.screen();
        assert_eq!(
            screen.field(label).as_deref(),
            Some(value),
            "{label} at {} ms; screen:\n{}",
            self.now(),
            screen.text()
        );
        self.note(NoteKind::Check, format!("{label} shows {value}"));
    }

    /// Assert the screen does not show `needle` now.
    #[track_caller]
    pub fn expect_no_text(&mut self, needle: &str) {
        let screen = self.screen();
        assert!(
            !screen.contains(needle),
            "unexpected {needle:?} on screen at {} ms; screen:\n{}",
            self.now(),
            screen.text()
        );
        self.note(NoteKind::Check, format!("did not see {needle}"));
    }

    /// A terminal command as the USB console would send it.
    pub fn command(&mut self, text: &str) -> (&'static str, String) {
        let request = match firmware_console::protocol::parse(format!("CMD 1 {text}").as_bytes()) {
            Ok(request) => request,
            Err(_) => return ("INVALID", String::new()),
        };
        let mut out = String::new();
        let status = self.device.execute(request, &mut out);
        (status, out)
    }

    /// An ANT+ sensor near the rider; created in range when new.
    pub fn ant_sensor(&mut self, device_type: u8, device_number: u16) -> &mut AntSensor {
        self.note(
            NoteKind::World,
            format!("ANT sensor {device_type}/{device_number} nearby"),
        );
        self.device.ant.sensor(device_type, device_number)
    }

    pub fn set_present(&mut self, device_type: u8, device_number: u16, present: bool) {
        self.note(
            NoteKind::World,
            format!(
                "ANT sensor {device_type}/{device_number} {}",
                if present { "in range" } else { "out of range" }
            ),
        );
        self.device.ant.sensor(device_type, device_number).present = present;
    }

    /// Follow a route with a GPS receiver that has sky view.
    pub fn ride_route(&mut self, route: Route) {
        self.note(
            NoteKind::World,
            format!("route of {:.0} m with GPS", route.length_m()),
        );
        self.device.world.route = Some(route);
        self.device.world.distance_m = 0.0;
        self.device.gps.get_or_insert_with(Default::default);
    }

    pub fn set_gps_fix(&mut self, fix: bool) {
        self.note(
            NoteKind::World,
            if fix {
                "GPS sky view restored"
            } else {
                "GPS sky view lost"
            },
        );
        if let Some(gps) = &mut self.device.gps {
            gps.fix = fix;
        }
    }

    pub fn ride_at(&mut self, speed_kmh: f64, power_w: f64, heart_bpm: f64, cadence_rpm: f64) {
        self.note(
            NoteKind::World,
            format!("riding {speed_kmh} km/h {power_w} W {heart_bpm} bpm {cadence_rpm} rpm"),
        );
        let world = &mut self.device.world;
        world.speed_kmh = speed_kmh;
        world.power_w = power_w;
        world.heart_bpm = heart_bpm;
        world.cadence_rpm = cadence_rpm;
    }

    pub fn vehicle(&mut self, vehicle: Vehicle) {
        self.note(
            NoteKind::World,
            format!(
                "vehicle {:?} at {:.0} m closing {:.0} km/h",
                vehicle.side, vehicle.range_m, vehicle.closing_kmh
            ),
        );
        self.device.world.vehicles.push(vehicle);
    }

    pub fn set_battery(&mut self, percent: u8, charging: bool) {
        self.note(
            NoteKind::World,
            format!(
                "battery {percent}%{}",
                if charging { " charging" } else { "" }
            ),
        );
        self.device.battery.level = Some((percent, 3300 + u16::from(percent) * 9));
        self.device.battery.charging = charging;
    }

    /// Cut power without warning and boot again; media keeps what was written.
    pub fn power_loss(&mut self) {
        self.note(NoteKind::Device, "power lost; rebooting");
        self.device.power_cycle().expect("memory media reopens");
        self.advance(BOOT_MS);
    }

    /// Fail the next storage mutations after `after` succeed, tearing the failed one.
    pub fn fail_storage(&mut self, after: u32, partial: usize) {
        self.note(
            NoteKind::Device,
            format!("storage fails after {after} writes"),
        );
        *self.device.media.fault.borrow_mut() = crate::storage::Fault {
            after: Some(after),
            partial,
        };
    }

    /// Recorded ride journal bytes, as `EXPORT` would read them.
    pub fn ride_media(&mut self) -> Vec<u8> {
        self.device.media.contents(Region::Data)
    }

    /// Rides committed to media, decoded by VANA's journal format.
    #[cfg(feature = "cycling")]
    pub fn rides(&mut self) -> Vec<crate::rides::Ride> {
        crate::rides::read(&self.ride_media())
    }

    /// A readable summary of what the device and world look like right now.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        let world = &self.device.world;
        let _ = writeln!(
            out,
            "t={} ms uptime={} ms boot={} distance={:.0} m speed={} km/h",
            world.time_ms, self.device.now, self.device.boot, world.distance_m, world.speed_kmh
        );
        let _ = writeln!(out, "screen:\n{}", self.screen().text());
        out
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        let Some(recording) = self.recording.take() else {
            return;
        };
        // A second panic while a failed scenario unwinds would abort the whole
        // test binary, so artifact work never unwinds out of here.
        let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            #[cfg(feature = "cycling")]
            let rides = self.rides().iter().map(crate::rides::summary).collect();
            #[cfg(not(feature = "cycling"))]
            let rides = Vec::new();
            recording.write(Outcome {
                notes: &self.notes,
                end_ms: self.device.world.time_ms,
                rides,
            })
        }));
        match written {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("scenario artifacts for {}: {error}", recording.name),
            Err(_) => eprintln!("scenario artifacts for {} panicked", recording.name),
        }
    }
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}
