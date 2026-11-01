//! Portable foreground presentation and preferences. Acquisition lives independently.
use crate::idle::Config;
use crate::idle::Gate;
use crate::idle::Idle;
use crate::preferences::Settings;
use crate::rendering::row::Row;
use crate::storage::Store;
use device_api::display::Display;
use device_api::input::Button;
use device_api::input::Input;
use device_api::input::InputSource;
use device_api::observation::Availability;
use device_api::observation::Error;
use device_api::power::Power;

/// Required display/input, with power and persistent settings independently optional.
/// Constant working memory; rendering borrows no frame buffer across submission.
pub const MIN_WIDTH: usize = 16;
pub const MIN_HEIGHT: usize = 16;
pub const CANVAS_MAX_WIDTH: usize = 320;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Status,
    Blank,
}
/// A pass's first synthetic inputs, at most 4: the shell sits in the console
/// task's future, so the rest of a crowded pass go unrecorded rather than
/// take its RAM.
#[cfg(feature = "debug-harness")]
pub type Synthetic = heapless::Vec<Input, 4>;

pub struct Shell<D, I, P, B> {
    #[cfg(feature = "debug-harness")]
    pub harness: crate::harness::State,
    #[cfg(feature = "debug-harness")]
    synthetic: Synthetic,
    pub boot_id: u32,
    pub synthetic_events: u32,
    store: Store<B>,
    pub settings: Settings,
    pub settings_source: &'static str,
    pub settings_error: bool,
    pub reset: device_api::crash::Reset,
    pub crash: device_api::crash::Marker,
    pub heap_min_sampled: usize,
    pub operations: u32,
    pub storage_max_ms: u64,
    pub display_submissions: u32,
    /// A digest of the pixels on the panel.
    panel: Option<u32>,
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
    /// The most backlight the application allows now, such as at night.
    /// It is never stored.
    ceiling: u8,
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
    next_button: Option<Button>,
    dirty: bool,
    fill_color: Option<u16>,
    dev_frame: u8,
}
impl<D: Display, I: InputSource, P: Power, B: device_api::storage::OwnedFlash> Shell<D, I, P, B> {
    pub fn new(display: D, input: I, power: P, backend: B, now: u64) -> Result<Self, Error> {
        match display.availability() {
            Availability::Ready => {}
            Availability::Unsupported => return Err(Error::Unsupported),
            Availability::Failed => return Err(Error::Failed),
            _ => return Err(Error::Unavailable),
        }
        let geometry = display.geometry();
        if geometry.width < MIN_WIDTH || geometry.height < MIN_HEIGHT {
            return Err(Error::Invalid);
        }
        let controls = input.controls();
        if controls.buttons.is_empty()
            && controls.touch != device_api::observation::Availability::Ready
        {
            return Err(Error::Unavailable);
        }
        let next_button = controls.buttons.first().copied();
        let storage_available = backend.availability(device_api::storage::Region::Configuration);
        let mut store = Store::new(backend);
        let loaded = if storage_available == Availability::Ready {
            Some(store.load())
        } else {
            None
        };
        let (settings, source, error) = match loaded {
            Some(Ok(value)) => (
                value.settings,
                match value.source {
                    crate::preferences::Source::Current => "current",
                    crate::preferences::Source::Missing => "missing",
                    crate::preferences::Source::Migrated => "migrated",
                    crate::preferences::Source::LegacyDefaults => "legacy",
                    crate::preferences::Source::Malformed => "malformed",
                    crate::preferences::Source::Unsupported => "unsupported",
                },
                false,
            ),
            Some(Err(_)) => (Settings::default(), "failed", true),
            None if storage_available == Availability::Unsupported => {
                (Settings::default(), "unavailable", false)
            }
            None => (Settings::default(), "failed", true),
        };
        log::info!(target:"settings", "loaded source={} failed={}", source,error);
        Ok(Self {
            #[cfg(feature = "debug-harness")]
            harness: crate::harness::State::default(),
            #[cfg(feature = "debug-harness")]
            synthetic: Synthetic::new(),
            boot_id: 0,
            synthetic_events: 0,
            panel: None,
            taken: None,
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
            ceiling: 100,
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
            next_button,
            dirty: true,
            fill_color: None,
            dev_frame: 0,
        })
    }
    /// Borrow application-owned bytes without exposing the preferences journal.
    pub fn data_storage(&mut self) -> firmware_services::storage::RegionAccess<'_, B> {
        self.store.data()
    }
    /// Borrow the device's panic journal for `CRASH`.
    pub fn crash_storage(&mut self) -> firmware_services::storage::RegionAccess<'_, B> {
        self.store.crash()
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
    pub fn take_app_input(&mut self) -> Option<device_api::input::Edge> {
        self.app_input.pop()
    }
    /// The harness's synthetic inputs handed on like physical ones since the
    /// last call, in order.
    #[cfg(feature = "debug-harness")]
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
    /// The physical input source, for composition that records what `tick` took.
    pub fn input_source(&mut self) -> &mut I {
        &mut self.input
    }
    pub fn effective(&self) -> u8 {
        self.effective
    }
    /// Cap the backlight at `percent` from the next tick, without changing
    /// the stored brightness.
    pub fn set_ceiling(&mut self, percent: u8) {
        self.ceiling = percent.min(100);
    }
    pub fn ceiling(&self) -> u8 {
        self.ceiling
    }
    pub fn dimmed(&self) -> bool {
        self.idle.dimmed()
    }
    /// Something waits for the next frame even if it shows nothing new: a
    /// routed press waits for its answer, or a harness capture is due.
    pub fn frame_wanted(&self, now: u64) -> bool {
        #[cfg(feature = "debug-harness")]
        if self.harness.capture_due(now) {
            return true;
        }
        #[cfg(not(feature = "debug-harness"))]
        let _ = now;
        self.pressed_at.is_some()
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
            #[cfg(feature = "debug-harness")]
            if edge.input == Input::Cancel {
                self.harness
                    .stop_input(crate::harness::InputState::PhysicalLoss);
            }
            self.consume(edge.input, now, edge.received_ms);
        }
        // Physical events already queued win ties; synthetic events use the same
        // consumer and wake gate without incrementing physical observations.
        #[cfg(feature = "debug-harness")]
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
            self.idle
                .effective(self.settings.brightness, config)
                .min(self.ceiling)
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
            if matches!(input, Input::Button { button, code: 1 } if Some(button) == self.next_button)
            {
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
        #[cfg(feature = "debug-harness")]
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
        #[cfg(not(feature = "debug-harness"))]
        {
            let _ = now;
            let _ = write!(
                out,
                "version=1 input=unsupported capture=unsupported harness=false boot={} session=0",
                self.boot_id
            );
            if command.operation == crate::harness::Operation::Caps {
                "OK"
            } else {
                "UNSUPPORTED"
            }
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
    pub fn save_connectivity(&mut self) -> bool {
        let result = self.store.save_connectivity(self.settings);
        self.operations = self.operations.saturating_add(1);
        self.settings_error = result.is_err();
        if result.is_ok() {
            self.settings_source = "current";
        }
        result.is_ok()
    }
    /// Persist the application's preferences without saving temporary shell
    /// changes; on success they also become the current settings.
    pub fn save_app_preferences(&mut self, app: crate::preferences::AppPreferences) -> bool {
        let result = self.store.save_app(app);
        self.operations = self.operations.saturating_add(1);
        self.settings_error = result.is_err();
        if result.is_ok() {
            self.settings.app = app;
            self.settings_source = "current";
        }
        result.is_ok()
    }
    pub fn save(&mut self) -> bool {
        let result = self.store.save(self.settings);
        self.operations = self.operations.saturating_add(1);
        self.settings_error = result.is_err();
        if result.is_ok() {
            self.settings_source = "current";
        }
        result.is_ok()
    }
    /// Submit a frame row by row; `row(y, pixels)` must fill every physical pixel.
    pub fn draw_rows(&mut self, mut row: impl FnMut(usize, &mut [u16])) {
        let started = self.clock.map(|clock| clock());
        let mut frame = FNV_OFFSET;
        let row = |y: usize, pixels: &mut [u16]| {
            row(y, pixels);
            frame = digest(frame, pixels);
        };
        #[cfg(feature = "debug-harness")]
        {
            if self.harness.begin_frame(self.display.geometry()) {
                let mut row = row;
                let harness = &mut self.harness;
                self.display_error = self
                    .display
                    .submit(|y, pixels| {
                        row(y, pixels);
                        harness.row(y, pixels);
                    })
                    .is_err();
                self.harness.finish_frame(!self.display_error);
            } else {
                self.display_error = self.display.submit(row).is_err();
            }
        }
        #[cfg(not(feature = "debug-harness"))]
        {
            self.display_error = self.display.submit(row).is_err();
        }
        if !self.display_error {
            self.panel = Some(frame);
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
