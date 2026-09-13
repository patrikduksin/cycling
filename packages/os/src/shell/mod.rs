//! Portable foreground presentation and preferences. Acquisition lives independently.
pub mod idle;
pub mod preferences;
pub mod storage;
use crate::capabilities::{Availability, Button, Display, Error, InputSource, Power};
use crate::{
    capabilities::Input,
    shell::idle::{Config, Gate, Idle},
    shell::preferences::Settings,
    shell::storage::Store,
};

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
    pub store: Store<B>,
    pub settings: Settings,
    pub settings_source: &'static str,
    pub settings_error: bool,
    pub reset: crate::crash::Reset,
    pub crash: crate::crash::Marker,
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
    pub foreground: Screen,
    pub routed_events: u32,
    pub display_error: bool,
    pub power_error: bool,
    pub power_availability: Availability,
    pub battery: crate::capabilities::Observation<(u8, u16)>,
    pub positioning_availability: Availability,
    pub position: Option<crate::positioning::Snapshot>,
    next_button: Option<Button>,
    dirty: bool,
}
impl<D: Display, I: InputSource, P: Power, B: crate::storage::OwnedFlash> Shell<D, I, P, B> {
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
        if controls.buttons.is_empty() && controls.touch != crate::capabilities::Availability::Ready
        {
            return Err(Error::Unavailable);
        }
        let next_button = controls.buttons.first().copied();
        let storage_available = backend.availability(crate::storage::Region::Configuration);
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
                    crate::shell::preferences::Source::Current => "current",
                    crate::shell::preferences::Source::Missing => "missing",
                    crate::shell::preferences::Source::Migrated => "migrated",
                    crate::shell::preferences::Source::LegacyDefaults => "legacy",
                    crate::shell::preferences::Source::Malformed => "malformed",
                    crate::shell::preferences::Source::Unsupported => "unsupported",
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
            store,
            settings,
            settings_source: source,
            settings_error: error,
            reset: crate::crash::Reset::Unknown,
            crash: crate::crash::Marker::None,
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
            foreground: Screen::Status,
            routed_events: 0,
            display_error: false,
            power_error: false,
            power_availability: Availability::Initializing,
            battery: crate::capabilities::Observation::Unavailable,
            positioning_availability: Availability::Unsupported,
            position: None,
            next_button,
            dirty: true,
        })
    }
    pub fn activity(&mut self, now: u64) {
        self.idle.button(now);
    }
    pub fn effective(&self) -> u8 {
        self.effective
    }
    pub fn dimmed(&self) -> bool {
        self.idle.dimmed()
    }
    pub fn observe_position(
        &mut self,
        positioning: &impl crate::capabilities::Positioning,
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
            crate::capabilities::observation(
                self.power
                    .battery()
                    .map(|(percent, mv, at)| ((percent, mv), at)),
                now,
                5_000,
            )
        } else {
            crate::capabilities::Observation::Unavailable
        };
        for _ in 0..16 {
            let Some(edge) = self.input.take_edge() else {
                break;
            };
            self.input_events = self.input_events.saturating_add(1);
            let gate = match edge.input {
                Input::Touch(_) => self.idle.contact(now),
                Input::Release | Input::Cancel => self.idle.release(now),
                Input::Button { .. } => self.idle.button(now),
            };
            if gate == Gate::Forward {
                self.routed_events = self.routed_events.saturating_add(1);
                if matches!(edge.input, Input::Button { button, code: 1 } if Some(button) == self.next_button)
                {
                    self.foreground = match self.foreground {
                        Screen::Status => Screen::Blank,
                        Screen::Blank => Screen::Status,
                    };
                    self.dirty = true;
                }
            }
        }
        let config = Config {
            timeout_ms: (self.settings.dim_timeout_secs != 0)
                .then_some(u64::from(self.settings.dim_timeout_secs) * 1000),
            dim_level: self.settings.dim_brightness,
        };
        self.idle.tick(now, config);
        let effective = self.idle.effective(self.settings.brightness, config);
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
    pub fn present(&mut self) {
        if !self.dirty {
            return;
        }
        let geometry = self.display.geometry();
        let screen = self.foreground;
        self.draw_pixels(|x, y| {
            if screen == Screen::Blank {
                0
            } else if x < geometry.width / 8 || y < geometry.height / 8 {
                0x07e0
            } else {
                0
            }
        });
        self.dirty = self.display_error;
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
        self.display_error = self.display.submit(pixel).is_err();
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
        self.draw_pixels(|_, _| color);
        self.dirty = false;
    }
}
