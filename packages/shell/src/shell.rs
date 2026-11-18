//! Portable foreground presentation and preferences. Acquisition lives independently.
use crate::backlight::{Backlight, Curve};
use crate::idle::Config;
use crate::idle::Gate;
use crate::idle::Idle;
use crate::preferences::Settings;
use crate::preferences::Source;
use crate::rendering::row::Row;
use device_api::display::BAND_ROWS;
use device_api::display::Display;
use device_api::display::Rows;
use device_api::input::Fixtures;
use device_api::input::Input;
use device_api::input::InputSource;
use device_api::observation::Availability;
use device_api::observation::Error;
use device_api::observation::Observation;
use device_api::power::Power;
use device_api::sensors::Light;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrightnessChoice {
    Percent,
    Auto,
}

/// Required display/input, with power and persistent settings independently optional.
/// Constant working memory; rendering borrows no frame buffer across submission.
pub const MIN_WIDTH: usize = 16;
pub const MIN_HEIGHT: usize = 16;
pub const CANVAS_MAX_WIDTH: usize = 320;
/// The tallest panel. `tall` raises it for the simulator's test panels;
/// firmware leaves it off, so its band arrays keep their size.
#[cfg(not(feature = "tall"))]
pub const MAX_HEIGHT: usize = 320;
#[cfg(feature = "tall")]
pub const MAX_HEIGHT: usize = 400;
const BANDS: usize = MAX_HEIGHT / BAND_ROWS;
const FNV_OFFSET: u32 = 0x811c_9dc5;
const FNV_PRIME: u32 = 0x0100_0193;
const _: () = assert!(
    cfg!(target_endian = "little"),
    "`digest` reads two pixels as the word `first | second << 16`"
);
/// FNV-1a's step, then a xorshift. A multiply carries a bit only upward, so
/// without the shift two words whose top bits differ cancel, and a second
/// pixel's top bit is a word's.
fn step(hash: u32, word: u32) -> u32 {
    let hash = (hash ^ word).wrapping_mul(FNV_PRIME);
    hash ^ hash >> 16
}
/// A row's pixels hashed two to a word, `first | second << 16`, and an odd
/// last pixel alone. Each step maps the hash one to one, so a row that
/// differs in one word always digests otherwise. A word a step halves the
/// multiplies a frame waits on, and a word-aligned row, as the C606's DMA
/// strips are, reads each word in one load.
fn digest(hash: u32, pixels: &[u16]) -> u32 {
    // SAFETY: every bit pattern is a valid u32, and `align_to` keeps the
    // middle slice inside `pixels` with u32 alignment.
    let (head, words, tail) = unsafe { pixels.align_to::<u32>() };
    if !head.is_empty() {
        return digest_pairs(hash, pixels);
    }
    // Four words a turn, so the loop's own count and branch cost less per pixel.
    let (quads, words) = words.as_chunks::<4>();
    let hash = quads.iter().fold(hash, |hash, &[a, b, c, d]| {
        step(step(step(step(hash, a), b), c), d)
    });
    digest_pairs(
        words.iter().fold(hash, |hash, &word| step(hash, word)),
        tail,
    )
}
fn digest_pairs(hash: u32, pixels: &[u16]) -> u32 {
    let (pairs, last) = pixels.as_chunks::<2>();
    let hash = pairs.iter().fold(hash, |hash, &[first, second]| {
        step(hash, u32::from(first) | u32::from(second) << 16)
    });
    last.iter()
        .fold(hash, |hash, &pixel| step(hash, u32::from(pixel)))
}
/// A frame an application draws through `Shell::draw`, at the panel's size,
/// whose pixels follow from its `Hash` alone, so equal pictures paint alike.
pub trait Picture: core::hash::Hash {
    /// Paint every pixel of `row`'s clip.
    fn paint(&self, row: &mut Row);
    /// Name the picture's parts, so a frame sends only the bands whose
    /// parts changed. One part over every row, the default, sends the
    /// picture whole or not at all.
    fn parts(&self, parts: &mut Parts) {
        parts.add(0..MAX_HEIGHT, self);
    }
}

/// The parts of a picture, as `Shell::draw` keys its bands: a band's key
/// folds the key of every part that touches it, in the order they were
/// added.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Parts {
    keys: [u32; BANDS],
}

impl Parts {
    /// The parts `picture` names, keyed band by band.
    pub fn of<F: Picture>(picture: &F) -> Self {
        let mut parts = Self {
            keys: [FNV_OFFSET; BANDS],
        };
        picture.parts(&mut parts);
        parts
    }

    /// The bands whose keys differ from `other`'s: what a frame sends to
    /// bring a panel that shows one picture to the other.
    pub fn changed(&self, other: &Self) -> Rows {
        (0..BANDS)
            .filter(|&band| self.keys[band] != other.keys[band])
            .fold(Rows::NONE, |rows, band| rows | Rows::band(band))
    }

    /// `key` must cover everything paint draws in `rows` for this part.
    pub fn add(&mut self, rows: core::ops::Range<usize>, key: &(impl core::hash::Hash + ?Sized)) {
        let mut fnv = Fnv1a(FNV_OFFSET);
        key.hash(&mut fnv);
        let key = fnv.0;
        let end = rows.end.min(MAX_HEIGHT).div_ceil(BAND_ROWS);
        for band in self
            .keys
            .get_mut(rows.start / BAND_ROWS..end)
            .unwrap_or_default()
        {
            *band = step(*band, key);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Drawn {
    /// The panel already showed it, and nothing waited for a frame.
    Unchanged,
    Sent,
    Failed,
}

struct Fnv1a(u32);

impl core::hash::Hasher for Fnv1a {
    fn finish(&self) -> u64 {
        u64::from(self.0)
    }

    fn write(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.0 = (self.0 ^ u32::from(byte)).wrapping_mul(FNV_PRIME);
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Status,
    Blank,
}
/// A pass's first synthetic inputs, at most 4: the shell sits in the console
/// task's future, so the rest of a crowded pass go unrecorded rather than
/// take its RAM.
pub type Synthetic = heapless::Vec<Input, 4>;

pub struct Shell<D, I, P, B> {
    pub harness: crate::harness::State,
    synthetic: Synthetic,
    pub boot_id: u32,
    pub synthetic_events: u32,
    flash: B,
    store: &'static mut dyn crate::storage::Settings,
    pub settings: Settings,
    pub settings_source: Source,
    pub settings_error: bool,
    pub reset: device_api::crash::Reset,
    pub crash: device_api::crash::Marker,
    pub heap_min_sampled: usize,
    pub operations: u32,
    pub storage_max_ms: u64,
    pub display_submissions: u32,
    sent: [u32; BANDS],
    panel: Option<u32>,
    shown: Option<Parts>,
    /// The digest `take_frame` last handed out.
    taken: Option<u32>,
    /// The latest submission, rendering included; measured when `clock` is set.
    pub display_ms: u64,
    pub display_max_ms: u64,
    pub input_events: u32,
    /// Press-to-frame latency: from a routed button press to the end of the
    /// next frame submission, measured when `clock` is set.
    pub press_frame_ms: u64,
    pub press_frame_max_ms: u64,
    pub press_frames: u32,
    pressed_at: Option<u64>,
    /// Milliseconds on the same clock as input timestamps.
    pub clock: Option<fn() -> u64>,
    idle: Idle,
    effective: u8,
    curve: Option<&'static Curve>,
    backlight: Backlight,
    light: Observation<Light>,
    brightness_choice: Option<BrightnessChoice>,
    power: P,
    display: D,
    input: I,
    app_input: firmware_services::input::Edges,
    app_active: bool,
    pub foreground: Screen,
    pub routed_events: u32,
    pub display_error: bool,
    pub power_error: bool,
    pub power_availability: Availability,
    pub battery: device_api::observation::Observation<(u8, u16)>,
    pub positioning_availability: Availability,
    pub position: Option<device_api::positioning::Snapshot>,
    buttons: Fixtures,
    next_button: Option<u8>,
    dirty: bool,
    fill_color: Option<u16>,
    dev_frame: u8,
}
impl<D: Display, I: InputSource, P: Power, B: device_api::storage::OwnedFlash> Shell<D, I, P, B> {
    pub fn new(
        display: D,
        input: I,
        power: P,
        flash: B,
        store: &'static mut dyn crate::storage::Settings,
        now: u64,
    ) -> Result<Self, Error> {
        match display.availability() {
            Availability::Ready => {}
            Availability::Unsupported => return Err(Error::Unsupported),
            Availability::Failed => return Err(Error::Failed),
            _ => return Err(Error::Unavailable),
        }
        let geometry = display.geometry();
        if geometry.width < MIN_WIDTH
            || geometry.height < MIN_HEIGHT
            || geometry.height > MAX_HEIGHT
        {
            return Err(Error::Invalid);
        }
        let controls = input.controls();
        if controls.buttons.as_slice().is_empty()
            && controls.touch != device_api::observation::Availability::Ready
        {
            return Err(Error::Unavailable);
        }
        let next_button = controls
            .buttons
            .as_slice()
            .first()
            .map(|fixture| fixture.id);
        let storage_available = flash.availability(device_api::storage::Region::Configuration);
        let loaded = if storage_available == Availability::Ready {
            Some(store.load())
        } else {
            None
        };
        let (settings, source, error) = match loaded {
            Some(Ok(value)) => (value.settings, value.source, false),
            Some(Err(_)) => (Settings::default(), Source::Failed, true),
            None if storage_available == Availability::Unsupported => {
                (Settings::default(), Source::Unavailable, false)
            }
            None => (Settings::default(), Source::Failed, true),
        };
        log::info!(target:"settings", "loaded source={} failed={}", source.name(), error);
        Ok(Self {
            harness: crate::harness::State::default(),
            synthetic: Synthetic::new(),
            boot_id: 0,
            synthetic_events: 0,
            sent: [FNV_OFFSET; BANDS],
            panel: None,
            shown: None,
            taken: None,
            flash,
            store,
            settings,
            settings_source: source,
            settings_error: error,
            reset: device_api::crash::Reset::Unknown,
            crash: device_api::crash::Marker::None,
            heap_min_sampled: 0,
            operations: 0,
            storage_max_ms: 0,
            display_submissions: 0,
            display_ms: 0,
            display_max_ms: 0,
            input_events: 0,
            press_frame_ms: 0,
            press_frame_max_ms: 0,
            press_frames: 0,
            pressed_at: None,
            clock: None,
            idle: Idle::new(now),
            effective: 0,
            curve: None,
            backlight: Backlight::default(),
            light: Observation::Unavailable,
            brightness_choice: None,
            power,
            display,
            input,
            app_input: firmware_services::input::Edges::new(),
            app_active: false,
            foreground: Screen::Status,
            routed_events: 0,
            display_error: false,
            power_error: false,
            power_availability: Availability::Initializing,
            battery: device_api::observation::Observation::Unavailable,
            positioning_availability: Availability::Unsupported,
            position: None,
            buttons: controls.buttons,
            next_button,
            dirty: true,
            fill_color: None,
            dev_frame: 0,
        })
    }
    /// Borrow application-owned bytes without exposing the preferences journal.
    pub fn data_storage(&mut self) -> firmware_services::storage::AppStorage<'_, B> {
        firmware_services::storage::AppStorage::new(&mut self.flash)
    }
    /// The owned flash, for a composition's own diagnostics.
    pub fn flash(&mut self) -> &mut B {
        &mut self.flash
    }

    pub fn activity(&mut self, now: u64) {
        self.idle.button(now);
    }
    pub fn set_app_active(&mut self, active: bool) {
        if self.app_active != active {
            self.app_input = firmware_services::input::Edges::new();
            self.app_active = active;
        }
    }
    /// The next input for the application. Presses of buttons without a
    /// role never reach it.
    pub fn take_app_input(&mut self) -> Option<crate::input::Edge> {
        use crate::input::Input as App;
        use device_api::input::Input;
        while let Some(edge) = self.app_input.pop() {
            let input = match edge.input {
                Input::Touch(point) => App::Touch(point),
                Input::Release => App::Release,
                Input::Cancel => App::Cancel,
                Input::Button { id, code } => match self.buttons.role(id) {
                    Some(role) => App::Press { role, code },
                    None => continue,
                },
            };
            return Some(crate::input::Edge {
                received_ms: edge.received_ms,
                input,
            });
        }
        None
    }
    /// The harness's synthetic inputs handed on like physical ones since the
    /// last call, in order.
    pub fn take_synthetic(&mut self) -> Synthetic {
        core::mem::take(&mut self.synthetic)
    }
    /// A digest of the pixels on the panel, when they changed since the
    /// last call.
    pub fn take_frame(&mut self) -> Option<u32> {
        let changed = self.panel != self.taken;
        self.taken = self.panel;
        self.panel.filter(|_| changed)
    }
    /// Forget the panel, so the next frame drawn counts as a change. Boards
    /// boot through other frames; their traces start alike after this.
    pub fn forget_frame(&mut self) {
        (self.panel, self.taken) = (None, None);
    }
    pub fn display_geometry(&self) -> device_api::display::Geometry {
        self.display.geometry()
    }
    /// The physical input source, for composition that records what `tick` took.
    pub fn input_source(&mut self) -> &mut I {
        &mut self.input
    }
    pub fn effective(&self) -> u8 {
        self.effective
    }
    /// Follow the ambient light along `curve` from the next tick, or show the
    /// stored brightness with none.
    pub fn set_curve(&mut self, curve: Option<&'static Curve>) {
        self.curve = curve;
    }
    pub fn auto(&self) -> bool {
        self.curve.is_some()
    }
    pub fn offer_brightness_choice(&mut self, choice: BrightnessChoice) {
        self.brightness_choice = Some(choice);
    }
    /// The choice offered since the last call, once.
    pub fn take_brightness_choice(&mut self) -> Option<BrightnessChoice> {
        self.brightness_choice.take()
    }
    pub fn observe_light(&mut self, light: Observation<Light>) {
        self.light = light;
    }
    pub fn dimmed(&self) -> bool {
        self.idle.dimmed()
    }
    /// Something waits for the next frame even if it shows nothing new: a
    /// routed press waits for its answer, or a harness capture is due.
    pub fn frame_wanted(&self, now: u64) -> bool {
        if self.harness.capture_due(now) {
            return true;
        }
        self.pressed_at.is_some()
    }
    /// When the shell next acts without an input: the harness's next
    /// scheduled input or capture frame.
    pub fn next_due_ms(&self) -> Option<u64> {
        self.harness.next_due()
    }
    /// Time since the last input or reported activity. The shell owns this
    /// inactivity clock; applications combine it with their own work state.
    pub fn inactive_ms(&self, now: u64) -> u64 {
        self.idle.age_ms(now)
    }
    pub fn observe_position(
        &mut self,
        positioning: &impl device_api::positioning::Positioning,
        now: u64,
    ) {
        self.positioning_availability = positioning.availability();
        self.position = if self.positioning_availability == Availability::Ready {
            positioning.snapshot(now)
        } else {
            None
        };
    }
    pub fn tick(&mut self, now: u64) {
        self.power_availability = self.power.availability();
        self.battery = if self.power_availability == Availability::Ready {
            device_api::observation::observation(
                self.power
                    .battery()
                    .map(|(percent, mv, at)| ((percent, mv), at)),
                now,
                5_000,
            )
        } else {
            device_api::observation::Observation::Unavailable
        };
        for _ in 0..16 {
            let Some(edge) = self.input.take_edge() else {
                break;
            };
            self.input_events = self.input_events.saturating_add(1);
            if edge.input == Input::Cancel {
                self.harness
                    .stop_input(crate::harness::InputState::PhysicalLoss);
            }
            self.consume(edge.input, now, edge.received_ms);
        }
        // Physical events already queued win ties; synthetic events use the same
        // consumer and wake gate without incrementing physical observations.
        {
            self.harness.expire(now);
            for _ in 0..crate::harness::EVENTS + 1 {
                let Some(event) = self.harness.next_input(now) else {
                    break;
                };
                self.synthetic_events = self.synthetic_events.saturating_add(1);
                let _ = self.synthetic.push(event);
                self.consume(event, now, self.harness.due);
            }
            if self.harness.capture_due(now) {
                self.dirty = true;
            }
        }
        let config = Config {
            timeout_ms: (self.settings.dim_timeout_secs != 0)
                .then_some(u64::from(self.settings.dim_timeout_secs) * 1000),
            dim_level: self.settings.dim_brightness,
        };
        let screen_off = !self.app_active && self.foreground == Screen::Blank;
        let effective = if screen_off {
            // Manual screen-off persists until the next screen toggle. Do not
            // let idle dimming consume that first press when returning.
            0
        } else {
            self.idle.tick(now, config);
            let selected = match self.curve {
                Some(curve) => {
                    self.backlight
                        .step(self.light, curve, self.settings.brightness, now)
                }
                None => {
                    self.backlight = Backlight::default();
                    self.settings.brightness
                }
            };
            self.idle.effective(selected, config)
        };
        self.power_error = self.power.availability() == Availability::Failed;
        if self.power.availability() == Availability::Ready && self.effective != effective {
            match self.power.brightness(effective) {
                Ok(()) => {
                    self.effective = effective;
                    self.power_error = false;
                }
                Err(_) => {
                    self.power_error = true;
                }
            }
        }
        let frame = if self.idle.dimmed() {
            0
        } else {
            crate::rendering::dev::frame(now)
        };
        if self.dev_frame != frame {
            self.dev_frame = frame;
            if !self.app_active && self.foreground == Screen::Status && self.fill_color.is_none() {
                self.dirty = true;
            }
        }
        // Foreground owns display submission. Background acquisition never waits
        // on this owner or on terminal attachment.
    }
    /// `at` is when the input happened, which can precede `now` while a frame renders.
    fn consume(&mut self, input: Input, now: u64, at: u64) {
        let gate = match input {
            Input::Touch(_) => self.idle.contact(now),
            Input::Release | Input::Cancel => self.idle.release(now),
            Input::Button { .. } => self.idle.button(now),
        };
        if gate == Gate::Forward {
            self.routed_events = self.routed_events.saturating_add(1);
            if matches!(input, Input::Button { code: 1, .. }) {
                self.pressed_at.get_or_insert(at);
            }
            if self.app_active {
                let _ = self.app_input.push(at, input);
                return;
            }
            if matches!(input, Input::Button { id, code: 1 } if Some(id) == self.next_button) {
                self.fill_color = None;
                self.foreground = match self.foreground {
                    Screen::Status => Screen::Blank,
                    Screen::Blank => Screen::Status,
                };
                self.dirty = true;
            }
        }
    }
    pub fn harness_command(
        &mut self,
        command: crate::harness::Command,
        now: u64,
        out: &mut impl core::fmt::Write,
    ) -> &'static str {
        {
            let status = self.harness.command(
                command,
                self.input.controls(),
                self.display.geometry(),
                now,
                out,
            );
            if self.harness.take_cancel() {
                self.synthetic_events = self.synthetic_events.saturating_add(1);
                let _ = self.synthetic.push(Input::Cancel);
                self.consume(Input::Cancel, now, now);
            }
            status
        }
    }

    pub fn present(&mut self) {
        if self.app_active || !self.dirty {
            return;
        }
        let screen = self.foreground;
        let fill = self.fill_color;
        let frame = self.dev_frame;
        // Keep layout independent of native panel geometry.
        self.draw_canvas(240, 320, |row| match (fill, screen) {
            (Some(color), _) => row.fill(color),
            (None, Screen::Blank) => row.fill(0),
            (None, Screen::Status) => crate::rendering::dev::paint(row, frame),
        });
        self.dirty = self.display_error;
    }
    /// Blocks saves until the restart, so FORMAT's erased journal stays blank.
    pub fn erase_settings(&mut self) -> bool {
        self.settings_source = Source::Foreign;
        self.store.erase().is_ok()
    }

    pub fn save_connectivity(&mut self) -> bool {
        self.store_settings(|store, settings| store.save_connectivity(settings))
    }

    pub fn save_wifi(
        &mut self,
        radio: device_api::connectivity::Operation,
        wifi: Option<device_api::connectivity::WifiConfig>,
    ) -> Result<(), &'static str> {
        if radio == device_api::connectivity::Operation::Pending {
            return Err("UNAVAILABLE");
        }
        let previous = self.settings;
        self.settings.wifi = wifi;
        if !self.save_connectivity() {
            self.settings = previous;
            return Err("FAILED");
        }
        Ok(())
    }
    /// Persist the application's preferences without saving temporary shell
    /// changes; on success they also become the current settings.
    pub fn save_app_preferences(&mut self, app: crate::preferences::AppPreferences) -> bool {
        let saved = self.store_settings(|store, _| store.save_app(app));
        if saved {
            self.settings.app = app;
        }
        saved
    }
    pub fn save(&mut self) -> bool {
        self.store_settings(|store, settings| store.save(settings))
    }

    fn store_settings(
        &mut self,
        save: impl FnOnce(
            &mut dyn crate::storage::Settings,
            Settings,
        ) -> Result<firmware_services::storage::Record, crate::storage::Error>,
    ) -> bool {
        let saved =
            self.settings_source != Source::Foreign && save(self.store, self.settings).is_ok();
        self.operations = self.operations.saturating_add(1);
        self.settings_error = !saved;
        if saved {
            self.settings_source = Source::Current;
        }
        saved
    }
    /// Bring the panel to `picture`: send the bands whose parts changed
    /// since the panel showed them. A panel that shows something else, or a
    /// frame something waits for (`frame_wanted`), takes the whole picture.
    pub fn draw<F: Picture>(&mut self, picture: &F, now: u64) -> Drawn {
        let parts = Parts::of(picture);
        let height = self.display.geometry().height;
        let changed = self.shown.map(|shown| shown.changed(&parts));
        let rows = match changed {
            _ if self.frame_wanted(now) => Rows::all(height),
            Some(changed) => changed,
            None => Rows::all(height),
        };
        let drawn = if rows.is_empty() {
            Drawn::Unchanged
        } else {
            self.send(rows, |y, pixels| picture.paint(&mut Row::new(y, pixels)));
            if self.display_error {
                self.shown = None;
                return Drawn::Failed;
            }
            self.shown = Some(parts);
            Drawn::Sent
        };
        self.display
            .check(|y, pixels| picture.paint(&mut Row::new(y, pixels)));
        drawn
    }
    /// The next `draw` sends its picture whole, whatever the panel showed.
    pub fn forget_panel(&mut self) {
        self.shown = None;
    }
    /// Submit a frame row by row; `row(y, pixels)` must fill every physical pixel.
    pub fn draw_rows(&mut self, row: impl FnMut(usize, &mut [u16])) {
        self.shown = None;
        self.send(Rows::all(self.display.geometry().height), row);
    }
    fn send(&mut self, rows: Rows, mut row: impl FnMut(usize, &mut [u16])) {
        let started = self.clock.map(|clock| clock());
        let sent = &mut self.sent;
        let row = |y: usize, pixels: &mut [u16]| {
            row(y, pixels);
            let band = &mut sent[y / BAND_ROWS];
            let from = if y.is_multiple_of(BAND_ROWS) {
                FNV_OFFSET
            } else {
                *band
            };
            *band = digest(from, pixels);
        };
        {
            let geometry = self.display.geometry();
            if self.harness.begin_frame(geometry) {
                let every_row = Rows::all(geometry.height);
                let mut row = row;
                let harness = &mut self.harness;
                self.display_error = self
                    .display
                    .submit(every_row, |y, pixels| {
                        row(y, pixels);
                        harness.row(y, pixels);
                    })
                    .is_err();
                self.harness.finish_frame(!self.display_error);
            } else {
                self.display_error = self.display.submit(rows, row).is_err();
            }
        }
        if !self.display_error {
            let bands = self.display.geometry().height.div_ceil(BAND_ROWS);
            self.panel = Some(
                self.sent[..bands]
                    .iter()
                    .fold(FNV_OFFSET, |hash, &band| step(hash, band)),
            );
        }
        self.display_submissions = self.display_submissions.saturating_add(1);
        if let (Some(clock), Some(started)) = (self.clock, started) {
            self.display_ms = clock().saturating_sub(started);
            self.display_max_ms = self.display_max_ms.max(self.display_ms);
        }
        if !self.display_error
            && let Some(at) = self.pressed_at.take()
            && let Some(clock) = self.clock
        {
            self.press_frame_ms = clock().saturating_sub(at);
            self.press_frame_max_ms = self.press_frame_max_ms.max(self.press_frame_ms);
            self.press_frames = self.press_frames.saturating_add(1);
        }
    }
    /// Per-pixel convenience over `draw_rows` for small or irregular frames.
    pub fn draw_pixels(&mut self, pixel: impl Fn(usize, usize) -> u16) {
        self.draw_rows(|y, pixels| {
            for (x, value) in pixels.iter_mut().enumerate() {
                *value = pixel(x, y);
            }
        });
    }
    /// Paint an application's logical canvas row by row, fitted to the advertised
    /// display geometry: physical pixel (x, y) shows logical pixel
    /// (x * width / geometry.width, y * height / geometry.height).
    /// A scaled canvas is at most `CANVAS_MAX_WIDTH` logical pixels wide.
    pub fn draw_canvas(&mut self, width: usize, height: usize, mut paint: impl FnMut(&mut Row)) {
        let geometry = self.display.geometry();
        if geometry.width == width && geometry.height == height {
            self.draw_rows(|y, pixels| paint(&mut Row::new(y, pixels)));
            return;
        }
        if width == 0 || width > CANVAS_MAX_WIDTH {
            self.shown = None;
            self.display_error = true;
            return;
        }
        let mut logical = [0u16; CANVAS_MAX_WIDTH];
        let mut painted = None;
        self.draw_rows(|y, pixels| {
            let source = y * height / geometry.height;
            if painted != Some(source) {
                paint(&mut Row::new(source, &mut logical[..width]));
                painted = Some(source);
            }
            for (x, value) in pixels.iter_mut().enumerate() {
                *value = logical[x * width / geometry.width];
            }
        });
    }
    pub fn fill(&mut self, color: u16) {
        self.fill_color = Some(color);
        self.draw_rows(|_, pixels| pixels.fill(color));
        self.dirty = false;
    }
}

#[cfg(test)]
mod tests {
    use super::{FNV_OFFSET, FNV_PRIME, digest};

    #[test]
    fn a_row_digests_two_pixels_a_word_wherever_it_sits_in_memory() {
        let defined = |pixels: &[u16]| {
            pixels.chunks(2).fold(FNV_OFFSET, |hash, pair| {
                let word = u32::from(pair[0]) | pair.get(1).map_or(0, |&p| u32::from(p) << 16);
                let hash = (hash ^ word).wrapping_mul(FNV_PRIME);
                hash ^ hash >> 16
            })
        };
        let mut words = [0u32; 130];
        // SAFETY: a u16 view of u32s, which every bit pattern fits.
        let (_, memory, _) = unsafe { words.align_to_mut::<u16>() };
        for (x, pixel) in memory.iter_mut().enumerate() {
            *pixel = (x as u16).wrapping_mul(0x9e37);
        }
        for start in [0, 1, 2] {
            for len in 0..=245 {
                let row = &memory[start..start + len];
                assert_eq!(
                    digest(FNV_OFFSET, row),
                    defined(row),
                    "{len} pixels from {start}"
                );
            }
        }
    }
}
