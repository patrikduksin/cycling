//! Portable foreground presentation and preferences. Acquisition lives independently.
use crate::idle::Config;
use crate::idle::Gate;
use crate::idle::Idle;
use crate::preferences::Settings;
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
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Screen {
    Status,
    Blank,
}
pub struct Shell<D, I, P, B> {
    #[cfg(feature = "debug-harness")]
    pub harness: crate::harness::State,
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
    pub display_max_ms: u64,
    pub input_events: u32,
    idle: Idle,
    effective: u8,
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
            boot_id: 0,
            synthetic_events: 0,
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
            display_max_ms: 0,
            input_events: 0,
            idle: Idle::new(now),
            effective: 0,
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
        })
    }
    /// Borrow application-owned bytes without exposing the preferences journal.
    pub fn data_storage(&mut self) -> firmware_services::storage::RegionAccess<'_, B> {
        self.store.data()
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
    pub fn effective(&self) -> u8 {
        self.effective
    }
    pub fn dimmed(&self) -> bool {
        self.idle.dimmed()
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
                self.harness.stop_input("physical_loss");
            }
            self.consume(edge.input, now);
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
                self.consume(event, now);
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
            self.idle.effective(self.settings.brightness, config)
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
        // Foreground owns display submission. Background acquisition never waits
        // on this owner or on terminal attachment.
    }
    fn consume(&mut self, input: Input, now: u64) {
        let gate = match input {
            Input::Touch(_) => self.idle.contact(now),
            Input::Release | Input::Cancel => self.idle.release(now),
            Input::Button { .. } => self.idle.button(now),
        };
        if gate == Gate::Forward {
            self.routed_events = self.routed_events.saturating_add(1);
            if self.app_active {
                self.app_input.push(now, input);
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
                self.consume(Input::Cancel, now);
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
        let geometry = self.display.geometry();
        let screen = self.foreground;
        let fill = self.fill_color;
        self.draw_pixels(|x, y| {
            if let Some(color) = fill {
                color
            } else if screen == Screen::Blank {
                0
            } else {
                // Keep layout independent of native panel geometry.
                let x = x * 240 / geometry.width;
                let y = y * 320 / geometry.height;
                let text = crate::rendering::text::text;
                if text(x, y, 36, 42, 4, b"CYCLING")
                    || text(x, y, 60, 104, 2, b"BASE READY")
                    || text(x, y, 48, 186, 2, b"USB COMMANDS")
                    || text(x, y, 30, 268, 2, b"TOP LEFT SCREEN")
                {
                    0xffff
                } else {
                    0
                }
            }
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
    pub fn save(&mut self) -> bool {
        let result = self.store.save(self.settings);
        self.operations = self.operations.saturating_add(1);
        self.settings_error = result.is_err();
        if result.is_ok() {
            self.settings_source = "current";
        }
        result.is_ok()
    }
    pub fn draw_pixels(&mut self, pixel: impl Fn(usize, usize) -> u16) {
        #[cfg(feature = "debug-harness")]
        {
            if self.harness.begin_frame(self.display.geometry()) {
                let capture = core::cell::RefCell::new(&mut self.harness);
                self.display_error = self
                    .display
                    .submit(|x, y| {
                        let value = pixel(x, y);
                        capture.borrow_mut().pixel(x, y, value);
                        value
                    })
                    .is_err();
                self.harness.finish_frame(!self.display_error);
            } else {
                self.display_error = self.display.submit(pixel).is_err();
            }
        }
        #[cfg(not(feature = "debug-harness"))]
        {
            self.display_error = self.display.submit(pixel).is_err();
        }
        self.display_submissions = self.display_submissions.saturating_add(1);
    }
    /// Fit an application's logical canvas to the advertised display geometry.
    pub fn draw_scaled(
        &mut self,
        width: usize,
        height: usize,
        pixel: impl Fn(usize, usize) -> u16,
    ) {
        let geometry = self.display.geometry();
        self.draw_pixels(|x, y| pixel(x * width / geometry.width, y * height / geometry.height));
    }
    pub fn fill(&mut self, color: u16) {
        self.fill_color = Some(color);
        self.draw_pixels(|_, _| color);
        self.dirty = false;
    }
}
