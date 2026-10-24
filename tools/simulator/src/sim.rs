//! Scenario driver for the simulated device: press buttons, change the world,
//! let time pass and read what the screen shows.
use crate::ant::AntSensor;
use crate::recording::{Outcome, Recording};
use crate::route::Route;
use crate::screen::{Frame, TextRun};
use crate::session::Session;
use crate::world::{Vehicle, World};
use device_api::input::{Button, Input, Point};
use device_api::storage::Region;
use std::fmt::Write as _;

/// Cards on VANA's Home, in MOVE order: RIDE, SENSORS, TRAIN, SETTINGS.
pub const HOME_CARDS: usize = 4;
/// Power-on never takes longer than this to open Home in the simulator.
const BOOT_TIMEOUT_MS: u64 = 15_000;
/// Firmware passes between a press and the next one VANA accepts (debounce is 350 ms).
const PRESS_GAP_MS: u64 = 400;
/// Touch reports arrive this often while a finger moves.
const TOUCH_STEP_MS: u64 = 30;
/// The companion repeats a held button's report this often.
pub const HOLD_REPEAT_MS: u64 = 100;
/// Repeated top-left reports that turn an off device on: about a 1.5 s hold.
/// Counted rather than timed, so a held key powers on at any live speed.
pub const POWER_ON_REPORTS: u32 = 15;

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

    /// The value a label names: the reading just left of a unit on its line,
    /// such as "250" before "W", or the reading under a 5x7 label such as
    /// "AVG WATTS".
    pub fn field(&self, label: &str) -> Option<String> {
        let runs = self.frame().read();
        let label = runs.iter().find(|run| run.text == label)?;
        Self::value_of(&runs, label)
    }

    fn value_of(runs: &[TextRun], label: &TextRun) -> Option<String> {
        let bottom = label.y + 7 * label.scale;
        if label.smooth {
            // A unit names the larger reading just left of it on its line.
            let beside = runs
                .iter()
                .filter(|run| run.smooth && run.scale > label.scale)
                .filter(|run| run.x < label.x && run.x + 200 > label.x)
                .filter(|run| run.y <= bottom && run.y + 9 * run.scale >= bottom)
                .max_by_key(|run| run.x);
            // Otherwise the reading under the label, in its column: larger
            // type, or a state line under a check's name.
            let under = || {
                runs.iter()
                    .filter(|run| run.smooth && run.scale >= label.scale)
                    .filter(|run| run.y > label.y && run.y < bottom + 16)
                    .min_by_key(|run| run.x.abs_diff(label.x))
            };
            return beside.or_else(under).map(|run| run.text.clone());
        }
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

    /// The ride clock: the largest H:MM:SS reading, the top one on a tie.
    /// Its size is not part of it, because layout steps a long clock down.
    pub fn elapsed(&self) -> Option<String> {
        self.frame()
            .read()
            .into_iter()
            .filter(|run| run.smooth)
            .filter(|run| {
                let parts: Vec<_> = run.text.split(':').collect();
                parts.len() == 3
                    && parts
                        .iter()
                        .all(|p| !p.is_empty() && p.chars().all(|c| c.is_ascii_digit()))
            })
            .min_by_key(|run| (std::cmp::Reverse(run.scale), run.y))
            .map(|run| run.text)
    }

    /// The status bar's page dots as "2/3": the lit dot and the count.
    #[cfg(feature = "cycling")]
    pub fn page(&self) -> Option<String> {
        use vana::screens::chrome::{DOT_SIZE, DOTS_Y, TERTIARY, WHITE};
        let row = self
            .pixels
            .get(DOTS_Y * self.width..(DOTS_Y + 1) * self.width)?;
        let mut dots = Vec::new();
        let mut x = 0;
        // Dots sit left of the GPS mark, which starts past two thirds of the
        // row; the clock is white on the right.
        while x < row.len() * 2 / 3 {
            let color = row[x];
            let start = x;
            while x < row.len() && row[x] == color {
                x += 1;
            }
            if (color == WHITE || color == TERTIARY) && (2..=DOT_SIZE).contains(&(x - start)) {
                dots.push(color == WHITE);
            }
        }
        let lit = dots.iter().position(|&lit| lit)?;
        Some(format!("{}/{}", lit + 1, dots.len()))
    }
}

pub struct Sim {
    pub device: Session,
    pub notes: Vec<Note>,
    recording: Option<Recording>,
    /// Top-left reports seen in a row while the device is off.
    /// Top-left reports in the current hold while the device is off.
    power_hold: u32,
}

impl Sim {
    /// A powered-off C606 with VANA, an ANT radio and empty memory media.
    pub fn new() -> Self {
        Self::with(Recording::from_env)
    }

    /// Like `new`, recorded under `name` rather than the test thread's name.
    /// Keep this session's artifacts from being written.
    pub fn discard_recording(&mut self) {
        self.recording = None;
    }

    pub fn named(name: &str) -> Self {
        Self::with(|width, height| Recording::named(name, width, height))
    }

    fn with(recording: impl FnOnce(usize, usize) -> Option<Recording>) -> Self {
        let device = Session::device().expect("memory media always opens");
        let geometry = device.display.0.borrow().geometry;
        Self {
            recording: recording(geometry.width, geometry.height),
            device,
            notes: Vec::new(),
            power_hold: 0,
        }
    }

    /// Power on and wait until the device can be used.
    pub fn boot(mut self) -> Self {
        self.wait_usable();
        self.skip_setup();
        self
    }

    /// A fresh device starts with first-run setup: skip it, as a rider who
    /// knows VANA would. Does nothing on Home.
    pub fn skip_setup(&mut self) {
        #[cfg(feature = "cycling")]
        if self.device.sdk.page() == vana::screens::workout::Page::Setup {
            self.press(device_api::input::Button::BottomLeft);
            self.expect_text("SKIP SETUP");
            self.press(device_api::input::Button::BottomRight);
            assert!(self.device.sdk.page() == vana::screens::workout::Page::Home);
        }
    }

    /// Advance until the panel is lit and, with VANA, Home is open. Returns
    /// the time waited.
    #[track_caller]
    pub fn wait_usable(&mut self) -> u64 {
        let start = self.now();
        while !self.usable() {
            assert!(
                self.now() - start < BOOT_TIMEOUT_MS,
                "not usable {BOOT_TIMEOUT_MS} ms after power-on; screen:\n{}",
                self.screen().text()
            );
            self.advance(10);
        }
        self.now() - start
    }

    fn usable(&self) -> bool {
        #[cfg(feature = "cycling")]
        let home = matches!(
            self.device.sdk.page(),
            vana::screens::workout::Page::Home | vana::screens::workout::Page::Setup
        );
        #[cfg(not(feature = "cycling"))]
        let home = true;
        self.device.lit() && home
    }

    /// Whether the device turned itself off.
    pub fn is_off(&self) -> bool {
        self.device.off
    }

    /// Hold the top-left button of a device that is off, as a rider turns
    /// the C606 on, and wait until it can be used.
    pub fn turn_on(&mut self) -> u64 {
        assert!(self.is_off(), "turn_on needs a device that is off");
        self.note(NoteKind::Input, "hold TopLeft to turn on");
        self.device.power_button().expect("memory media reopens");
        self.wait_usable()
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
                self.notes.len(),
            );
        }
    }

    /// One physical press, then enough time for the next press to count.
    pub fn press(&mut self, button: Button) {
        self.note(NoteKind::Input, format!("press {button:?}"));
        self.report(button);
        self.advance(PRESS_GAP_MS);
    }

    /// One companion report of `button` going down, now, without waiting.
    pub fn report(&mut self, button: Button) {
        self.deliver(button, false);
    }

    /// A repeated report of a button still held since the previous report.
    /// The caller knows the hold is continuous: the live window keeps time in
    /// real milliseconds, while uptime runs faster at 10x and 60x.
    pub fn repeat(&mut self, button: Button) {
        self.deliver(button, true);
    }

    /// While the device is off, only a held top-left button does anything:
    /// it turns the device on, as the companion does.
    fn deliver(&mut self, button: Button, continuing: bool) {
        if self.is_off() {
            if button != Button::TopLeft {
                self.power_hold = 0;
                return;
            }
            self.power_hold = if continuing && self.power_hold > 0 {
                self.power_hold + 1
            } else {
                1
            };
            if self.power_hold >= POWER_ON_REPORTS {
                self.power_hold = 0;
                self.note(NoteKind::Device, "top-left held: turning on");
                self.device.power_button().expect("memory media reopens");
            }
            return;
        }
        self.power_hold = 0;
        self.device
            .input
            .push(self.device.now, Input::Button { button, code: 1 });
    }

    /// A finger on the screen at (x, y), lifted at once.
    pub fn tap(&mut self, x: u16, y: u16) {
        self.note(NoteKind::Input, format!("tap {x},{y}"));
        self.touch(&[(x, y), (x, y)]);
    }

    /// A finger dragged from `from` to `to` in a quarter second.
    pub fn swipe(&mut self, from: (u16, u16), to: (u16, u16)) {
        self.note(
            NoteKind::Input,
            format!("swipe {},{} to {},{}", from.0, from.1, to.0, to.1),
        );
        let lerp =
            |a: u16, b: u16, i: i32| (i32::from(a) + (i32::from(b) - i32::from(a)) * i / 8) as u16;
        let points: Vec<_> = (0..=8)
            .map(|i| (lerp(from.0, to.0, i), lerp(from.1, to.1, i)))
            .collect();
        self.touch(&points);
    }

    /// Swipe right to left across the middle: the next page.
    pub fn swipe_next(&mut self) {
        self.swipe((200, 160), (40, 160));
    }

    /// Swipe left to right across the middle: the previous page.
    pub fn swipe_previous(&mut self) {
        self.swipe((40, 160), (200, 160));
    }

    /// Report a contact through `points`, then the release.
    fn touch(&mut self, points: &[(u16, u16)]) {
        for &(x, y) in points {
            self.device
                .input
                .push(self.device.now, Input::Touch(Point { x, y }));
            self.advance(TOUCH_STEP_MS);
        }
        self.device.input.push(self.device.now, Input::Release);
        self.advance(PRESS_GAP_MS);
    }

    /// Buttons pressed in the same companion report window.
    pub fn press_together(&mut self, buttons: &[Button]) {
        self.note(NoteKind::Input, format!("press together {buttons:?}"));
        for &button in buttons {
            self.report(button);
        }
        self.advance(PRESS_GAP_MS);
    }

    /// The companion repeats code-1 reports while a button stays down; the
    /// firmware cannot observe a release (see issue #86).
    pub fn hold(&mut self, button: Button, ms: u64) {
        self.note(NoteKind::Input, format!("hold {button:?} {ms} ms"));
        let mut held = 0;
        while held < ms {
            self.deliver(button, held > 0);
            self.advance(HOLD_REPEAT_MS);
            held += HOLD_REPEAT_MS;
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

    /// Advance until the field under `label` reads `value`; panics on timeout.
    #[track_caller]
    pub fn wait_for_field(&mut self, label: &str, value: &str, timeout_ms: u64) {
        if let Err(text) = self.wait_until(timeout_ms, |screen| {
            screen.field(label).as_deref() == Some(value)
        }) {
            panic!("{label} did not show {value} within {timeout_ms} ms; screen:\n{text}");
        }
        self.note(NoteKind::Check, format!("{label} shows {value}"));
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
        // `done` reads only pixels, and reading text is most of a long
        // scenario's time: a resubmitted frame identical to the last one
        // read cannot change the answer.
        let mut read: Option<Vec<u16>> = None;
        loop {
            let current = self.device.display.0.borrow().submissions;
            if current != submissions {
                submissions = current;
                let unchanged = read
                    .as_deref()
                    .is_some_and(|pixels| *pixels == *self.device.display.0.borrow().pixels);
                if !unchanged {
                    let screen = self.screen();
                    if done(&screen) {
                        return Ok(self.now() - start);
                    }
                    read = Some(screen.pixels);
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

    /// Store a BLE heart-rate strap as the selected peer, as the companion
    /// app does; the client then scans for it and connects.
    #[cfg(feature = "cycling")]
    pub fn select_ble_heart_strap(&mut self) {
        self.note(NoteKind::Input, "select BLE heart strap");
        // "STRAP" in hex, matched by name.
        let (status, out) = self.command("BLE SELECT HRS 5354524150 -");
        assert_eq!(status, "ACCEPTED", "BLE SELECT: {out}");
    }

    /// The raw Battery Level (0x2A19) the BLE heart strap reports on its next
    /// connection and each minute after, or none for a strap without the
    /// Battery Service.
    pub fn ble_heart_strap_battery(&mut self, raw: Option<u8>) {
        self.note(
            NoteKind::World,
            format!("BLE heart strap battery level {raw:?}"),
        );
        self.device
            .ble_strap
            .get_or_insert_with(crate::session::BleStrap::new)
            .battery = raw;
    }

    /// A BLE heart-rate strap worn by the rider, in range or not. It only
    /// connects once selected.
    pub fn ble_heart_strap(&mut self, present: bool) {
        self.note(
            NoteKind::World,
            if present {
                "BLE heart strap in range"
            } else {
                "BLE heart strap out of range"
            },
        );
        self.device
            .ble_strap
            .get_or_insert_with(crate::session::BleStrap::new)
            .present = present;
    }

    /// Barometric pressure readings stop (or resume); elevation goes on.
    pub fn set_barometer(&mut self, present: bool) {
        self.note(
            NoteKind::World,
            if present {
                "barometer reporting"
            } else {
                "barometer silent"
            },
        );
        self.device.world.barometer_lost = !present;
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

    /// Change what the GPS receiver reports: whether RMC carries Doppler
    /// speed, the HDOP, and how far each fix strays (m per axis).
    pub fn set_gps_quality(&mut self, doppler: bool, hdop: f64, jitter_m: f64) {
        self.note(
            NoteKind::World,
            format!(
                "GPS {} Doppler, HDOP {hdop:.1}, jitter {jitter_m:.1} m",
                if doppler { "with" } else { "without" }
            ),
        );
        if let Some(gps) = &mut self.device.gps {
            gps.nmea.doppler = doppler;
            gps.nmea.hdop = hdop;
            gps.jitter_m = jitter_m;
        }
    }

    /// Set the receiver's clock so it reads `utc_s` seconds after UTC
    /// midnight now, on `date` (day, month, two-digit year).
    pub fn set_gps_clock(&mut self, utc_s: u32, date: (u8, u8, u8)) {
        self.note(
            NoteKind::World,
            format!(
                "GPS clock {:02}:{:02} UTC on {:02}/{:02}/{:02}",
                utc_s / 3600,
                utc_s / 60 % 60,
                date.0,
                date.1,
                date.2
            ),
        );
        let now_ms = self.device.world.time_ms;
        if let Some(gps) = &mut self.device.gps {
            let mut nmea = crate::route::Nmea::reading(utc_s, date, now_ms);
            nmea.doppler = gps.nmea.doppler;
            nmea.hdop = gps.nmea.hdop;
            gps.nmea = nmea;
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
        self.cut_power();
        self.wait_usable();
    }

    /// Cut power without warning; the device starts booting at once.
    pub fn cut_power(&mut self) {
        self.note(NoteKind::Device, "power lost; rebooting");
        self.device.power_cycle().expect("memory media reopens");
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

    /// Fit the C606 owned MMC region, as it is before VANA formats it.
    pub fn fit_bulk(&mut self, presence: crate::bulk::Presence) {
        self.note(NoteKind::Device, format!("bulk media {presence:?}"));
        self.device.bulk.set_presence(presence);
    }

    /// Fail the bulk write after `after` succeed, tearing it after `partial` bytes.
    pub fn fail_bulk(&mut self, after: u32, partial: usize) {
        self.note(
            NoteKind::Device,
            format!("bulk media fails after {after} writes"),
        );
        self.device
            .bulk
            .fail(crate::bulk::BulkFault { after, partial });
    }

    /// Explicitly format the bulk ride log, as a host would, and wait for it.
    #[cfg(feature = "cycling")]
    #[track_caller]
    pub fn init_bulk(&mut self) {
        let (status, out) = self.command("RIDE BULK INIT");
        assert_eq!(status, "ACCEPTED", "RIDE BULK INIT: {out}");
        self.advance(100);
        let (_, out) = self.command("RIDE BULK STATUS");
        assert!(
            out.contains("state=ready") && out.contains("result=OK"),
            "{out}"
        );
    }

    /// Write the volume table over a blank or legacy region, as
    /// `VOLUME INIT` would.
    pub fn init_volumes(&mut self) -> firmware_services::volume::Table {
        let mut sector = [0u8; 512];
        firmware_services::volume::create(&mut self.device.bulk, &mut sector).expect("volume table")
    }

    /// Install `bytes` as the ready TERRAIN pack, as the host's terrain write
    /// would (volume table created if needed).
    pub fn install_terrain(&mut self, tag: &[u8; 16], bytes: &[u8]) {
        use firmware_services::{pack, volume};
        self.init_volumes();
        let mut sector = [0u8; 512];
        let media = &mut self.device.bulk;
        let mut terrain = volume::open(media, &volume::TERRAIN, &mut sector).expect("TERRAIN");
        let spec = pack::Spec {
            length: bytes.len() as u64,
            crc: firmware_services::checksum::crc32(bytes),
            tag: *tag,
            digest: [0; 32],
            stamp: 0,
        };
        let mut writer = pack::begin(&mut terrain, spec, &mut sector).expect("begin");
        for (index, chunk) in bytes.chunks(512).enumerate() {
            let mut data = [0u8; 512];
            data[..chunk.len()].copy_from_slice(chunk);
            writer
                .write(&mut terrain, index as u64, &data)
                .expect("terrain write");
        }
        writer.commit(&mut terrain, &mut sector).expect("commit");
        self.note(
            NoteKind::Device,
            format!("host installed a {} byte terrain pack", bytes.len()),
        );
    }

    /// Put a file into FILES, formatting the store first if it has none, as
    /// the host's file put would.
    pub fn put_file(&mut self, name: &str, bytes: &[u8]) {
        use firmware_services::{files, volume};
        self.init_volumes();
        let mut sector = [0u8; 512];
        let media = &mut self.device.bulk;
        let mut store = volume::open(media, &volume::FILES, &mut sector).expect("FILES");
        if files::mount(&mut store, &mut sector) == Err(files::Error::Uninitialized) {
            files::format(&mut store, 1, &mut sector).expect("format");
        }
        let name = files::Name::new(name).expect("file name");
        files::write(&mut store, &name, bytes, &mut sector).expect("file write");
        self.note(
            NoteKind::Device,
            format!("host put {} ({} bytes)", name.as_str(), bytes.len()),
        );
    }

    /// A file's bytes from FILES, if it exists.
    pub fn file(&mut self, name: &str) -> Option<Vec<u8>> {
        use firmware_services::{files, volume};
        let mut sector = [0u8; 512];
        let media = &mut self.device.bulk;
        let mut store = volume::open(media, &volume::FILES, &mut sector).ok()?;
        let dir = files::mount(&mut store, &mut sector).ok()?;
        let entry = dir
            .find(&mut store, &files::Name::new(name).ok()?, &mut sector)
            .ok()??;
        let mut bytes = vec![0u8; entry.length as usize];
        dir.read(&mut store, &entry, 0, &mut bytes, &mut sector)
            .ok()?;
        Some(bytes)
    }

    /// Rides on the bulk log, read through `EXPORT BULK` as a host would.
    #[cfg(feature = "cycling")]
    pub fn bulk_rides(&mut self) -> Vec<crate::rides::Ride> {
        let (status, info) = self.command("EXPORT BULK INFO");
        assert_eq!(status, "OK", "EXPORT BULK INFO: {info}");
        let upper: usize = info.split_whitespace().nth(3).unwrap().parse().unwrap();
        let mut bytes = Vec::new();
        for index in 0..upper {
            let (status, out) = self.command(&format!("EXPORT BULK SLOT {index}"));
            assert_eq!(status, "OK", "{out}");
            let hex = out.split_whitespace().nth(3).unwrap();
            for pair in hex.as_bytes().chunks(2) {
                let text = std::str::from_utf8(pair).unwrap();
                bytes.push(u8::from_str_radix(text, 16).unwrap());
            }
        }
        crate::rides::read(&bytes)
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
