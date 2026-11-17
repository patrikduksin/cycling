//! Scenario driver for the simulated device: press buttons, change the world,
//! let time pass and read what the screen shows.
use crate::ant::AntSensor;
use crate::compare::Tap;
use crate::devices::Model;
use crate::recording::{Outcome, Recording};
use crate::route::Route;
use crate::screen::{Frame, TextRun};
use crate::session::Session;
use crate::world::{Vehicle, World};
use device_api::input::{Button, Input, Point};
use device_api::storage::Region;
use firmware_shell::rendering::row::Look;
use std::fmt::Write as _;
use vana::app::Store;

/// Cards on VANA's Home, in MOVE order: RIDE, SENSORS, TRAIN, SETTINGS.
pub const HOME_CARDS: usize = 4;
/// Power-on never takes longer than this to open Home in the simulator.
const BOOT_TIMEOUT_MS: u64 = 15_000;
/// Firmware passes between a press and the next one VANA accepts (debounce is 350 ms).
const PRESS_GAP_MS: u64 = 400;
/// Touch reports arrive this often while a finger moves.
const TOUCH_STEP_MS: u64 = 30;
const FOREIGN_WRITE_ORDER: [Store; Store::ALL.len()] = [
    Store::Records,
    Store::Files,
    Store::Rides,
    Store::Traces,
    Store::Internal,
    Store::Settings,
    Store::Table,
];
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
    pub look: Look,
}

impl Screen {
    pub fn frame(&self) -> Frame<'_> {
        Frame {
            width: self.width,
            height: self.height,
            pixels: &self.pixels,
            look: self.look,
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

    /// Whether `text` is painted white, as VANA's focused titles are; the
    /// light look shows them black.
    pub fn white(&self, text: &str) -> bool {
        let white = self.look.ink(0xffff).0;
        self.run(text).is_some_and(|run| run.color == white)
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

pub struct Media {
    flash: crate::storage::FlashImage,
    bulk: crate::bulk::Image,
}

impl Sim {
    /// A powered-off C606 with VANA, an ANT radio and empty memory media.
    pub fn new() -> Self {
        Self::on(crate::models::C606)
    }

    pub fn on(model: Model) -> Self {
        Self::with(model, Recording::from_env, requested_tap())
    }

    /// Like `new`, in comparison mode whatever the environment says.
    pub fn comparing() -> Self {
        Self::with(
            crate::models::C606,
            Recording::from_env,
            Some(Tap::default()),
        )
    }

    /// Like `new`, recorded under `name` rather than the test thread's name.
    /// Keep this session's artifacts from being written.
    pub fn discard_recording(&mut self) {
        self.recording = None;
    }

    pub fn named(name: &str) -> Self {
        Self::with(
            crate::models::C606,
            |model| Recording::named(name, model),
            requested_tap(),
        )
    }

    fn with(
        model: Model,
        recording: impl FnOnce(&Model) -> Option<Recording>,
        tap: Option<Tap>,
    ) -> Self {
        let device = match tap {
            Some(tap) => Session::comparing(model, tap),
            None => Session::device(model),
        };
        let device = device.expect("memory media always opens");
        Self {
            recording: recording(&model),
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
        if self.device.sdk.page() == vana::screens::workout::Page::Setup {
            self.press(device_api::input::Button::BottomLeft);
            self.expect_text("SKIP SETUP");
            self.press(device_api::input::Button::BottomRight);
            assert!(self.device.sdk.page() == vana::screens::workout::Page::Home);
        }
    }

    /// Advance until the panel is lit and Home is open. Returns
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
        let home = matches!(
            self.device.sdk.page(),
            vana::screens::workout::Page::Home | vana::screens::workout::Page::Setup
        );
        self.device.lit() && home
    }

    /// Whether the device turned itself off: no power, or VANA off in
    /// charging standby.
    pub fn is_off(&self) -> bool {
        self.device.off || self.device.charging_standby()
    }

    /// Hold the top-left button of a device that is off, as a rider turns
    /// the C606 on, and wait until it can be used.
    pub fn turn_on(&mut self) -> u64 {
        assert!(self.is_off(), "turn_on needs a device that is off");
        self.note(NoteKind::Input, "hold TopLeft to turn on");
        self.device.power_button().expect("memory media reopens");
        self.wait_usable()
    }

    /// World time; a replay has no world, so its device uptime.
    pub fn now(&self) -> u64 {
        if self.device.is_replay() {
            return self.device.now;
        }
        self.device.sim().world.time_ms
    }

    pub fn world(&mut self) -> &mut World {
        &mut self.device.sim_mut().world
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
        let end = self.device.sim().world.time_ms + ms;
        while self.device.sim().world.time_ms < end {
            self.device
                .step((end - self.device.sim().world.time_ms).min(10));
            let frame = self.device.display.0.borrow();
            recording.observe(
                self.device.sim().world.time_ms,
                frame.submissions,
                &frame.pixels,
                Some(&self.device.sim().world),
            );
        }
    }

    /// Name this moment; recordings keep a screenshot of it.
    pub fn checkpoint(&mut self, name: &str) {
        self.note(NoteKind::Check, format!("checkpoint {name}"));
        let now = self.now();
        if let Some(recording) = &mut self.recording {
            recording.checkpoint(
                name,
                now,
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
            .push(self.device.clock_ms(), Input::Button { button, code: 1 });
    }

    /// A finger on the screen at (x, y), lifted at once.
    pub fn tap(&mut self, x: u16, y: u16) {
        self.note(NoteKind::Input, format!("tap {x},{y}"));
        self.touch(&[(x, y), (x, y)]);
    }

    /// A finger held still at (`x`, `y`) for `ms`, then lifted.
    pub fn long_press(&mut self, x: u16, y: u16, ms: u64) {
        self.note(NoteKind::Input, format!("long press {x},{y} {ms} ms"));
        let steps = (ms / TOUCH_STEP_MS + 1) as usize;
        self.touch(&std::vec![(x, y); steps]);
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

    /// A finger dragged from `from` to `to` over `ms`, reported every
    /// 10 ms as the C606's touch poll does. Lifted straight after the last
    /// move, the list coasts on at the finger's speed; with `hold_ms` the
    /// finger rests there first, so it throws nothing.
    pub fn drag(&mut self, from: (u16, u16), to: (u16, u16), ms: u64, hold_ms: u64) {
        self.note(
            NoteKind::Input,
            format!(
                "drag {},{} to {},{} in {ms} ms, held {hold_ms} ms",
                from.0, from.1, to.0, to.1
            ),
        );
        let steps = (ms / 10).max(1) as i32;
        let lerp = |a: u16, b: u16, i: i32| {
            (i32::from(a) + (i32::from(b) - i32::from(a)) * i / steps) as u16
        };
        for i in 0..=steps {
            self.finger(Point {
                x: lerp(from.0, to.0, i),
                y: lerp(from.1, to.1, i),
            });
            self.advance(10);
        }
        self.advance(hold_ms);
        self.device
            .input
            .push(self.device.clock_ms(), Input::Release);
        self.advance(PRESS_GAP_MS);
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
            self.finger(Point { x, y });
            self.advance(TOUCH_STEP_MS);
        }
        self.device
            .input
            .push(self.device.clock_ms(), Input::Release);
        self.advance(PRESS_GAP_MS);
    }

    #[track_caller]
    fn finger(&mut self, point: Point) {
        let model = self.device.model;
        assert!(model.touch, "{} has no touch panel", model.name);
        self.device
            .input
            .push(self.device.clock_ms(), Input::Touch(point));
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
        self.hold_while(button, ms, |_, _| {});
    }

    /// `hold`, calling `during` with how long the button has been held
    /// before each of its reports.
    pub fn hold_while(&mut self, button: Button, ms: u64, mut during: impl FnMut(&mut Self, u64)) {
        self.note(NoteKind::Input, format!("hold {button:?} {ms} ms"));
        let mut held = 0;
        while held < ms {
            during(self, held);
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
            look: self.look(),
        }
    }

    fn look(&self) -> Look {
        vana::screens::look::look(self.device.sdk.light())
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
        let mut out = String::new();
        match self
            .device
            .command(format!("CMD 1 {text}").as_bytes(), &mut out)
        {
            Ok((_, status)) => (status, out),
            Err(_) => ("INVALID", String::new()),
        }
    }

    /// Keep a hash of every frame from now on, to compare two runs.
    pub fn hash_frames(&mut self) {
        self.device.display.0.borrow_mut().hashes = Some(Vec::new());
    }

    /// The hash of every frame shown since `hash_frames`.
    pub fn frame_hashes(&self) -> Vec<u64> {
        let frame = self.device.display.0.borrow();
        frame.hashes.clone().unwrap_or_default()
    }

    /// The trace of this power-on session so far.
    pub fn trace(&self) -> Vec<u8> {
        self.device.trace()
    }

    /// What a replay of this power-on session needs.
    pub fn capture(&self) -> crate::replay::Capture {
        self.device.capture().unwrap_or_else(|why| panic!("{why}"))
    }

    /// Run a captured session again on its media, pass by pass at the
    /// recorded uptimes, with the trace standing in for the world. Frames
    /// are hashed throughout.
    pub fn replay(capture: &crate::replay::Capture) -> Self {
        Self::replay_as(capture, crate::replay::Mode::Strict, |_| {})
    }

    /// As `replay`, in `mode`, with the replay's devices as `setup` leaves
    /// them before the first pass.
    pub fn replay_as(
        capture: &crate::replay::Capture,
        mode: crate::replay::Mode,
        setup: impl FnOnce(&mut crate::replay::Sources),
    ) -> Self {
        let mut sim = Self::replaying(capture, mode, None).expect("a replayable capture");
        sim.hash_frames();
        setup(sim.device.replay_sources().expect("a replay"));
        sim.replay_until(u64::MAX);
        sim
    }

    /// A replay of `capture` in `mode` that has run nothing yet, recorded
    /// into `recording` when given. Drive it with `replay_until`.
    pub fn replaying(
        capture: &crate::replay::Capture,
        mode: crate::replay::Mode,
        recording: Option<Recording>,
    ) -> std::io::Result<Self> {
        Ok(Self {
            device: Session::replaying(capture, mode)?,
            notes: Vec::new(),
            recording,
            power_hold: 0,
        })
    }

    pub fn attach_recording(&mut self, recording: Recording) {
        self.recording = Some(recording);
    }

    /// Run the trace's passes and commands until the uptime reaches
    /// `until_ms`; false once the trace is spent.
    pub fn replay_until(&mut self, until_ms: u64) -> bool {
        while self.device.now < until_ms {
            if let crate::session::Step::End = self.device.replay_step() {
                return false;
            }
            if let Some(recording) = &mut self.recording {
                let frame = self.device.display.0.borrow();
                recording.observe(self.device.now, frame.submissions, &frame.pixels, None);
            }
        }
        true
    }

    /// What the rider saw at this moment, by this session's own trace.
    pub fn seen(&self) -> crate::trace::timeline::SeenAt {
        crate::trace::timeline::read(&self.trace())
            .expect("the session's own trace decodes")
            .seen_at(self.device.now)
    }

    /// An ANT+ sensor near the rider; created in range when new.
    pub fn ant_sensor(&mut self, device_type: u8, device_number: u16) -> &mut AntSensor {
        self.note(
            NoteKind::World,
            format!("ANT sensor {device_type}/{device_number} nearby"),
        );
        self.device.sim_mut().ant.sensor(device_type, device_number)
    }

    pub fn set_present(&mut self, device_type: u8, device_number: u16, present: bool) {
        self.note(
            NoteKind::World,
            format!(
                "ANT sensor {device_type}/{device_number} {}",
                if present { "in range" } else { "out of range" }
            ),
        );
        self.device
            .sim_mut()
            .ant
            .sensor(device_type, device_number)
            .present = present;
    }

    /// Follow a route with a GPS receiver that has sky view.
    pub fn ride_route(&mut self, route: Route) {
        self.note(
            NoteKind::World,
            format!("route of {:.0} m with GPS", route.length_m()),
        );
        self.device.sim_mut().world.route = Some(route);
        self.device.sim_mut().world.distance_m = 0.0;
        self.device
            .sim_mut()
            .gps
            .get_or_insert_with(Default::default);
    }

    /// Store a BLE heart-rate strap as the selected peer, as the companion
    /// app does; the client then scans for it and connects.
    pub fn select_ble_heart_strap(&mut self) {
        self.note(NoteKind::Input, "select BLE heart strap");
        // "STRAP" in hex, matched by name.
        let (status, out) = self.command("BLE SELECT HRS 5354524150 -");
        assert_eq!(status, "ACCEPTED", "BLE SELECT: {out}");
    }

    /// The BLE radio is busy with an operation, as the C606's owner task is
    /// between accepting a request and completing it, until
    /// `finish_ble_operation`.
    pub fn hold_ble_operation(&mut self) {
        self.note(NoteKind::World, "BLE operation pending");
        self.device.sim_mut().ble.control.operation = device_api::connectivity::Operation::Pending;
    }

    pub fn finish_ble_operation(&mut self) {
        self.note(NoteKind::World, "BLE operation completed");
        self.device.sim_mut().ble.control.operation =
            device_api::connectivity::Operation::Completed;
    }

    pub fn hold_wifi_operation(&mut self) {
        self.note(NoteKind::World, "Wi-Fi operation pending");
        self.device.sim_mut().wifi.control.operation = device_api::connectivity::Operation::Pending;
    }

    pub fn finish_wifi_operation(&mut self) {
        self.note(NoteKind::World, "Wi-Fi operation completed");
        self.device.sim_mut().wifi.control.operation =
            device_api::connectivity::Operation::Completed;
    }

    /// Wi-Fi time synchronization succeeds now with this UTC, in ms since
    /// 1970, over a station that is online; the device keeps it until it
    /// reboots.
    pub fn sync_network_time(&mut self, unix_ms: u64) {
        self.note(
            NoteKind::World,
            format!("network time synced to {unix_ms} ms"),
        );
        let now = self.device.now;
        let sim = self.device.sim_mut();
        sim.wifi.online = true;
        sim.network_time = Some(firmware_services::network_time::Anchor {
            unix_ms,
            uptime_ms: now,
        });
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
            .sim_mut()
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
            .sim_mut()
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
        self.device.sim_mut().world.barometer_lost = !present;
    }

    pub fn set_ambient_light(&mut self, light: Option<u16>) {
        self.note(NoteKind::World, format!("ambient light {light:?}"));
        self.device.sim_mut().world.ambient_light = light;
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
        if let Some(gps) = &mut self.device.sim_mut().gps {
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
        if let Some(gps) = &mut self.device.sim_mut().gps {
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
        let now_ms = self.device.sim().world.time_ms;
        if let Some(gps) = &mut self.device.sim_mut().gps {
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
        let world = &mut self.device.sim_mut().world;
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
        self.device.sim_mut().world.vehicles.push(vehicle);
    }

    pub fn set_battery(&mut self, percent: u8, charging: bool) {
        self.note(
            NoteKind::World,
            format!(
                "battery {percent}%{}",
                if charging { " charging" } else { "" }
            ),
        );
        self.device.sim_mut().battery.level = Some((percent, 3300 + u16::from(percent) * 9));
        self.device.sim_mut().battery.charging = charging;
    }

    /// Cut power without warning and boot again; media keeps what was written.
    pub fn power_loss(&mut self) {
        self.cut_power();
        self.wait_usable();
    }

    /// Everything the device's media hold, to put back with `restore_media`.
    pub fn media(&self) -> Media {
        Media {
            flash: self.device.media.image().expect("memory media").1,
            bulk: self.device.bulk.image(),
        }
    }

    /// Put `media` back, then cut the power, so the device boots from them
    /// as if they had never changed since.
    pub fn restore_media(&mut self, media: &Media) {
        self.device.media.restore(&media.flash);
        self.device.bulk.restore(&media.bulk);
        self.cut_power();
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
        let mut fault = self.device.media.fault.borrow_mut();
        fault.after = Some(after);
        fault.partial = partial;
    }

    /// Fail every read of internal flash `region` until the next boot.
    pub fn fail_flash_reads(&mut self, region: device_api::storage::Region) {
        self.note(NoteKind::Device, format!("{region:?} reads fail"));
        self.device.media.fault.borrow_mut().unreadable = Some(region);
    }

    /// Lose power at the internal flash mutation after `after` succeed, cut
    /// with `partial` of its length done: a program lands that far and an
    /// erase sets each bit with odds `partial / length`. From it on, neither
    /// internal flash nor the card takes or gives anything until `cut_power`
    /// boots again.
    pub fn lose_power_at_flash_write(&mut self, after: u32, partial: usize) {
        self.fail_storage(after, partial);
        self.device.media.fault.borrow_mut().power_cut = true;
    }

    /// As `lose_power_at_flash_write`, at the card write after `after`.
    pub fn lose_power_at_card_write(&mut self, after: u32, partial: usize) {
        self.note(
            NoteKind::Device,
            format!("power lost at card write {after}"),
        );
        self.device.bulk.fail(crate::bulk::BulkFault {
            after,
            partial,
            power_cut: true,
        });
    }

    /// Power was lost at an armed write this boot.
    pub fn power_lost(&self) -> bool {
        self.device.media.power_lost().get()
    }

    /// Disarm a storage fault `fail_storage` armed that has not fired.
    pub fn clear_storage_fault(&mut self) {
        self.device.media.fault.borrow_mut().after = None;
    }

    /// A flash fault is armed, or a card fault is armed or failed this boot.
    pub fn storage_faulted(&self) -> bool {
        self.device.media.fault.borrow().after.is_some() || self.device.bulk.faulted()
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
        self.device.bulk.fail(crate::bulk::BulkFault {
            after,
            partial,
            power_cut: false,
        });
    }

    /// Explicitly format the bulk ride log, as a host would, and wait for it.
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

    /// Write the volume table over a blank or lost region, as `VOLUME INIT`
    /// would.
    pub fn init_volumes(&mut self) -> firmware_services::volume::Table {
        let mut sector = [0u8; 512];
        firmware_services::volume::create(&mut self.device.host_bulk(), &mut sector)
            .expect("volume table")
    }

    /// Install `bytes` as the ready TERRAIN pack, as the host's terrain write
    /// would (volume table created if needed).
    pub fn install_terrain(&mut self, tag: &[u8; 16], bytes: &[u8]) {
        use firmware_services::{pack, volume};
        use sha2::Digest;
        self.init_volumes();
        let mut sector = [0u8; 512];
        let mut media = self.device.host_bulk();
        let mut terrain = volume::open(&mut media, &volume::TERRAIN, &mut sector).expect("TERRAIN");
        let spec = pack::Spec {
            length: bytes.len() as u64,
            crc: firmware_services::checksum::crc32(bytes),
            tag: *tag,
            digest: sha2::Sha256::digest(bytes).into(),
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

    /// Format the FILES store if it has none, as the host's `files format`
    /// would; a formatted one is left as it is.
    pub fn format_files(&mut self) {
        use firmware_services::{files, volume};
        self.init_volumes();
        let mut sector = [0u8; 512];
        let mut media = self.device.host_bulk();
        let mut store = volume::open(&mut media, &volume::FILES, &mut sector).expect("FILES");
        if files::mount(&mut store, &mut sector) == Err(files::Error::Uninitialized) {
            files::format(&mut store, 1, &mut sector).expect("format");
        }
    }

    /// Put a file into FILES, formatting the store first if it has none, as
    /// the host's file put would.
    pub fn put_file(&mut self, name: &str, bytes: &[u8]) {
        use firmware_services::{files, volume};
        self.format_files();
        let mut sector = [0u8; 512];
        let mut media = self.device.host_bulk();
        let mut store = volume::open(&mut media, &volume::FILES, &mut sector).expect("FILES");
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
        let mut media = self.device.bulk.host();
        let mut store = volume::open(&mut media, &volume::FILES, &mut sector).ok()?;
        let dir = files::mount(&mut store, &mut sector).ok()?;
        let entry = dir
            .find(&mut store, &files::Name::new(name).ok()?, &mut sector)
            .ok()??;
        let mut bytes = vec![0u8; entry.length as usize];
        dir.read(&mut store, &entry, 0, &mut bytes, &mut sector)
            .ok()?;
        Some(bytes)
    }

    /// The names in the FILES volume, as a host lists them.
    pub fn file_names(&mut self) -> Option<Vec<String>> {
        use firmware_services::{files, volume};
        let mut sector = [0u8; 512];
        let mut media = self.device.bulk.host();
        let mut store = volume::open(&mut media, &volume::FILES, &mut sector).ok()?;
        let dir = files::mount(&mut store, &mut sector).ok()?;
        let mut names = Vec::new();
        dir.list(&mut store, &mut sector, |entry| {
            names.push(entry.name.as_str().to_string())
        })
        .ok()?;
        Some(names)
    }

    /// Format an empty store in the bulk region's STORE volume, as the
    /// owner's `STORE FORMAT CONFIRM` would: the volume table is created
    /// first if there is none.
    pub fn format_store(&mut self) {
        use firmware_console::commands::volumes;
        self.init_volumes();
        let mut out = String::new();
        let mut media = self.device.host_bulk();
        let status = volumes::execute(volumes::Command::StoreFormat, &mut media, &mut out);
        assert_eq!(status, "OK", "STORE FORMAT CONFIRM: {out}");
    }

    /// A file's bytes from STORE, as the host's `store get` would read
    /// them, if it exists and the medium lost none of its records.
    pub fn store_file(&mut self, name: &str) -> Option<Vec<u8>> {
        use firmware_services::{store, volume};
        let mut sector = [0u8; 512];
        let mut media = self.device.bulk.host();
        let mut vol = volume::open(&mut media, &volume::STORE, &mut sector).ok()?;
        let mounted = store::mount(&mut vol, &mut sector).ok()?;
        let name = store::Name::new(name).ok()?;
        let mut reader = mounted.reader(&mut vol, &name, &mut sector).ok()?;
        let (mut bytes, mut record) = (Vec::new(), [0u8; store::PAYLOAD]);
        loop {
            match reader
                .next(&mounted, &mut vol, &mut record, &mut sector)
                .ok()?
            {
                store::Next::Record(n) => bytes.extend_from_slice(&record[..n]),
                store::Next::Hole { .. } => return None,
                store::Next::End => return Some(bytes),
            }
        }
    }

    /// Rides on the bulk log, read through `EXPORT BULK` as a host would.
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

    /// Leave `stores` as another version of the firmware would have written
    /// them: the current format with another version byte, resealed. Cut the
    /// power afterwards, so the device reads them.
    pub fn make_foreign(&mut self, stores: &[Store]) {
        for store in FOREIGN_WRITE_ORDER
            .into_iter()
            .filter(|store| stores.contains(store))
        {
            self.make_store_foreign(store);
        }
    }

    /// Leave the terrain pack as another version would have written it.
    pub fn make_terrain_foreign(&mut self) {
        self.note(NoteKind::Device, "terrain of another version");
        self.install_terrain(b"CHILE-TERRAIN-V1", &[7; 600]);
        self.reseal_volume_foreign(firmware_services::volume::TERRAIN, [0, 1], 16);
    }

    fn make_store_foreign(&mut self, store: Store) {
        use device_api::storage::{OwnedFlash, Region};
        use firmware_services::volume;
        self.note(
            NoteKind::Device,
            format!("{} of another version", store.name()),
        );
        match store {
            Store::Settings => {
                let mut journal = firmware_services::storage::Journal::new(
                    self.device.media.untapped(),
                    Region::Configuration,
                );
                journal.save(b"another version").expect("the journal saves");
            }
            Store::Internal => {
                self.device
                    .media
                    .program(Region::Data, 0, b"RIDE\x01\0\0\0")
                    .expect("the journal programs");
            }
            Store::Records => {
                let mut bests = vana::kept::bests::encode_file(&Default::default());
                bests[4] = 1;
                let body = bests.len() - 4;
                let crc = firmware_services::checksum::crc32(&bests[..body]);
                bests[body..].copy_from_slice(&crc.to_le_bytes());
                self.put_file("bests.dat", &bests);
            }
            Store::Table => {
                self.init_volumes();
                let mut bulk = self.device.host_bulk();
                for copy in 0..2 {
                    reseal_foreign(&mut bulk, copy, volume::VERSION_AT);
                }
            }
            Store::Rides => {
                self.format_rides();
                self.reseal_volume_foreign(volume::RIDES, [0, 1], 5);
            }
            Store::Files => {
                self.format_files();
                self.reseal_volume_foreign(volume::FILES, [0, 32], 16);
            }
            Store::Traces => {
                self.format_store();
                self.reseal_volume_foreign(volume::STORE, [0, 32], 16);
            }
        }
    }

    fn reseal_volume_foreign(
        &mut self,
        name: firmware_services::volume::Name,
        copies: [u64; 2],
        byte: usize,
    ) {
        let table = self.init_volumes();
        let extent = table.find(&name).expect("the volume");
        let mut bulk = self.device.host_bulk();
        let mut media = firmware_services::volume::Volume::new(&mut bulk, extent);
        for copy in copies {
            reseal_foreign(&mut media, copy, byte);
        }
    }

    /// Start an empty ride log in RIDES, as `RIDE BULK INIT` would.
    pub fn format_rides(&mut self) {
        use firmware_services::volume::{self, Volume};
        let table = self.init_volumes();
        let mut sector = [0u8; 512];
        let mut bulk = self.device.host_bulk();
        let mut rides = Volume::new(&mut bulk, table.find(&volume::RIDES).expect("RIDES"));
        vana::ride::bulk::start(&mut rides, 1, &mut sector).expect("the log formats");
    }

    /// Rides committed to media, decoded by VANA's journal format.
    pub fn rides(&mut self) -> Vec<crate::rides::Ride> {
        crate::rides::read(&self.ride_media())
    }

    /// The scenario's comparison hashes as they stand: the media now, and
    /// every session so far with the current one as if it ended now.
    pub fn hashes(&mut self) -> Option<crate::compare::Hashes> {
        let tap = self.device.tap.clone()?;
        Some(tap.hashes(crate::compare::media(
            &mut self.device.media,
            &self.device.bulk,
        )))
    }

    /// A readable summary of what the device and world look like right now.
    pub fn describe(&self) -> String {
        let mut out = String::new();
        let world = &self.device.sim().world;
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
        let name = recording.name.clone();
        let hashes = if std::thread::panicking() {
            Ok(None)
        } else {
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| self.hashes()))
        };
        let (hashes, unhashed) = match hashes {
            Ok(hashes) => (hashes, None),
            Err(panic) => (None, Some(panic)),
        };
        let compared = hashes.is_some();
        // A second panic while a failed scenario unwinds would abort the whole
        // test binary, so artifact work never unwinds out of here.
        let written = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let rides = self.rides().iter().map(crate::rides::summary).collect();
            recording.write(Outcome {
                device: self.device.model.name,
                notes: &self.notes,
                end_ms: self.now(),
                rides,
                hashes,
            })
        }));
        let written = match written {
            Ok(Ok(())) => true,
            Ok(Err(error)) => {
                eprintln!("scenario artifacts for {name}: {error}");
                false
            }
            Err(_) => {
                eprintln!("scenario artifacts for {name} panicked");
                false
            }
        };
        if let Some(panic) = unhashed {
            std::panic::resume_unwind(panic);
        }
        assert!(
            written || !compared,
            "{name} passed, but its comparison hashes were not written"
        );
    }
}

fn requested_tap() -> Option<Tap> {
    crate::compare::requested().then(Tap::default)
}

impl Default for Sim {
    fn default() -> Self {
        Self::new()
    }
}

fn reseal_foreign(media: &mut impl device_api::bulk::ReadWrite, at: u64, byte: usize) {
    let mut sector = [0u8; 512];
    media.owned_read(at, &mut sector).expect("read");
    sector[byte] = 1;
    let crc = firmware_services::checksum::crc32(&sector[..508]);
    sector[508..].copy_from_slice(&crc.to_le_bytes());
    media.owned_write(at, &sector).expect("written");
}
