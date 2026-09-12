//! General settings, owned storage and display/power mechanisms.
use cycling_os::{
    capabilities::Input,
    idle::{Config, Idle},
    preferences::Settings,
};
use esp_hal::ledc::{
    LowSpeed,
    channel::{Channel, ChannelIFace},
};

pub struct System {
    pub store: crate::persistent::Store,
    pub settings: Settings,
    pub settings_source: &'static str,
    pub settings_error: bool,
    pub reset: cycling_os::crash::Reset,
    pub crash: cycling_os::crash::Marker,
    pub heap_min_sampled: usize,
    pub operations: u32,
    pub storage_max_ms: u64,
    pub display_submissions: u32,
    pub display_max_ms: u64,
    pub input_events: u32,
    idle: Idle,
    effective: u8,
    backlight: Channel<'static, LowSpeed>,
    screen: crate::device::display::Display<'static>,
}
impl System {
    pub fn new(
        flash: esp_hal::peripherals::FLASH<'static>,
        backlight: Channel<'static, LowSpeed>,
        screen: crate::device::display::Display<'static>,
        reset: cycling_os::crash::Reset,
        crash: cycling_os::crash::Marker,
    ) -> Self {
        let (store, loaded) = crate::persistent::Store::open(flash);
        let (settings, source, error) = match loaded {
            Ok(value) => (
                value.settings,
                match value.source {
                    cycling_os::preferences::Source::Current => "current",
                    cycling_os::preferences::Source::Missing => "missing",
                    cycling_os::preferences::Source::Migrated => "migrated",
                    cycling_os::preferences::Source::LegacyDefaults => "legacy",
                    cycling_os::preferences::Source::Malformed => "malformed",
                    cycling_os::preferences::Source::Unsupported => "unsupported",
                },
                false,
            ),
            Err(_) => (Settings::default(), "failed", true),
        };
        log::info!(target:"settings", "loaded source={} failed={}", source,error);
        Self {
            store,
            settings,
            settings_source: source,
            settings_error: error,
            reset,
            crash,
            heap_min_sampled: esp_alloc::HEAP.free(),
            operations: 0,
            storage_max_ms: 0,
            display_submissions: 0,
            display_max_ms: 0,
            input_events: 0,
            idle: Idle::new(embassy_time::Instant::now().as_millis()),
            effective: 0,
            backlight,
            screen,
        }
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
    pub fn tick(&mut self, now: u64) {
        self.heap_min_sampled = self.heap_min_sampled.min(esp_alloc::HEAP.free());
        for _ in 0..16 {
            let Some(edge) = crate::services::io::take_edge() else {
                break;
            };
            self.input_events = self.input_events.saturating_add(1);
            match edge.input {
                Input::Touch(_) => {
                    self.idle.contact(now);
                }
                Input::Release | Input::Cancel => {
                    self.idle.release(now);
                }
                Input::Button { .. } => {
                    self.idle.button(now);
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
        if self.effective != effective {
            match self.backlight.set_duty(effective) {
                Ok(()) => self.effective = effective,
                Err(_) => log::error!(target:"power","backlight failed"),
            }
        }
    }
    /// Explicit persistence only. A failed write is not automatically retried.
    pub fn save(&mut self) -> bool {
        let start = embassy_time::Instant::now();
        let result = self.store.save(self.settings);
        let elapsed = start.elapsed().as_millis();
        self.operations = self.operations.saturating_add(1);
        self.storage_max_ms = self.storage_max_ms.max(elapsed);
        self.settings_error = result.is_err();
        if result.is_ok() {
            self.settings_source = "current";
            log::info!(target:"storage","settings committed ms={}",elapsed);
        } else {
            log::error!(target:"storage","settings commit failed ms={}",elapsed);
        }
        result.is_ok()
    }
    /// One synchronous owned submission. Completion returns only after the DMA buffer is back with the driver.
    pub fn fill(&mut self, color: u16) {
        let start = embassy_time::Instant::now();
        self.screen.fill(color);
        let elapsed = start.elapsed().as_millis();
        self.display_max_ms = self.display_max_ms.max(elapsed);
        self.display_submissions = self.display_submissions.saturating_add(1);
        log::info!(target:"display","submitted ms={} count={}",elapsed,self.display_submissions);
    }
}
