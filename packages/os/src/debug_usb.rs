//! Device-side test session. No flash writes or arbitrary memory access.
use cycling_os::{
    coin::{HEIGHT, PIXELS, WIDTH},
    companion::{Event, Status},
    debug::{Action, PointerInjection, encode_row},
    idle::{Gate, Idle},
    metrics::Snapshot as Metrics,
    preferences::Settings,
    screenshot::checksum,
    ui::{App, Snapshot},
};
use esp_println::println;

pub struct Debug {
    pub active: bool,
    injection: PointerInjection,
    lease: u64,
    app: Option<Snapshot>,
    counts: [u32; 3],
    last_button: Option<cycling_os::companion::Button>,
    battery: Option<(u8, u16, u8)>,
    pending: Option<(u32, &'static str)>,
    until: u64,
    next: u64,
    interval: u64,
    sequence: u32,
    stream: u32,
    pixels: [u16; PIXELS],
    persistence: Option<(u32, u8)>,
    runtime: Option<(Settings, Idle)>,
    reboot: Option<Reboot>,
    reboot_ready: Option<Reboot>,
    ride: Option<(
        u32,
        Option<(cycling_os::ride::Action, cycling_os::ride_log::Source)>,
    )>,
    export: Option<(u32, Option<u16>)>,
    export_hex: [u8; cycling_os::ride_log::SLOT_SIZE * 2],
}

#[derive(Clone, Copy)]
pub enum Reboot {
    Panic,
    Restart,
}
impl Debug {
    pub fn new() -> Self {
        Self {
            active: false,
            injection: PointerInjection::default(),
            lease: 0,
            app: None,
            counts: [0; 3],
            last_button: None,
            battery: None,
            pending: None,
            until: 0,
            next: 0,
            interval: 0,
            sequence: 0,
            stream: 0,
            pixels: [0; PIXELS],
            persistence: None,
            runtime: None,
            reboot: None,
            reboot_ready: None,
            ride: None,
            export: None,
            export_hex: [0; cycling_os::ride_log::SLOT_SIZE * 2],
        }
    }
    pub fn recording(&self) -> bool {
        self.until != 0
    }
    pub fn touch_injected(&self) -> bool {
        self.injection.active()
    }
    pub fn take_persistence(&mut self) -> Option<(u32, u8)> {
        self.persistence.take()
    }
    pub fn persistence_result(&mut self, id: u32, result: &'static str) {
        self.pending = Some((id, result));
    }
    pub fn take_reboot(&mut self) -> Option<Reboot> {
        self.reboot_ready.take()
    }
    pub fn take_ride(
        &mut self,
    ) -> Option<(
        u32,
        Option<(cycling_os::ride::Action, cycling_os::ride_log::Source)>,
    )> {
        self.ride.take()
    }
    pub fn ride_result(&mut self, id: u32, ok: bool) {
        self.pending = Some((id, if ok { "OK" } else { "RIDE_FAILED" }));
    }
    pub fn take_export(&mut self) -> Option<(u32, Option<u16>)> {
        self.export.take()
    }
    pub fn export_error(&mut self, id: u32, error: &'static str) {
        println!("CYCLING_EXPORT {} ERROR {}", id, error);
    }
    pub fn export_info(&mut self, id: u32, upper: usize, status: &'static str) {
        println!(
            "CYCLING_EXPORT {} INFO {} {} {} {}",
            id,
            cycling_os::ride_log::VERSION,
            cycling_os::ride_log::SLOT_SIZE,
            upper,
            status
        );
    }
    pub fn export_slot(&mut self, id: u32, index: u16, slot: &cycling_os::ride_log::Slot) {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        for (index, byte) in slot.0.iter().copied().enumerate() {
            self.export_hex[index * 2] = HEX[usize::from(byte >> 4)];
            self.export_hex[index * 2 + 1] = HEX[usize::from(byte & 15)];
        }
        let encoded = unsafe { core::str::from_utf8_unchecked(&self.export_hex) };
        esp_println::println!(
            "CYCLING_EXPORT {} SLOT {} {:08x} {}",
            id,
            index,
            cycling_os::ride_log::transport_checksum(&slot.0),
            encoded
        );
    }
    fn stop(&mut self) {
        if self.recording() {
            println!("CYCLING_REC STOP {} {}", self.stream, self.sequence);
        }
        self.until = 0;
    }
    fn end(
        &mut self,
        app: &mut App,
        status: &mut Status,
        settings: &mut Settings,
        idle: &mut Idle,
    ) {
        self.stop();
        if self.active {
            app.cancel();
            if let Some(snapshot) = self.app.take() {
                app.restore(snapshot);
            }
            if let Some((saved_settings, saved_idle)) = self.runtime.take() {
                *settings = saved_settings;
                *idle = saved_idle;
            }
            status.button_counts = self.counts;
            status.last_button = self.last_button;
        }
        self.active = false;
        self.injection.finish();
        self.battery = None;
        crate::wifi::set_fault(0);
    }
    pub fn tick(
        &mut self,
        now: u64,
        app: &mut App,
        status: &mut Status,
        settings: &mut Settings,
        idle: &mut Idle,
    ) {
        if self.active && now >= self.lease {
            self.end(app, status, settings, idle);
            println!("CYCLING_DEBUG expired");
        }
        if self.recording() && now >= self.until {
            self.stop();
        }
    }
    pub fn status(&self, real: &Status) -> Status {
        let mut result = *real;
        if let Some((percent, millivolts, power)) = self.battery {
            result.update(Event::Battery {
                percent,
                millivolts,
            });
            result.update(Event::Power { status: power });
        }
        result
    }
    pub fn command(
        &mut self,
        id: u32,
        action: Action,
        now: u64,
        app: &mut App,
        status: &mut Status,
        settings: &mut Settings,
        idle: &mut Idle,
        screenshot_busy: bool,
    ) {
        let mut result = "OK";
        if !self.active
            && !matches!(
                action,
                Action::Begin
                    | Action::State
                    | Action::End
                    | Action::Persist(_)
                    | Action::Ride(..)
                    | Action::RideInit
                    | Action::Panic
                    | Action::Restart
                    | Action::ExportInfo
                    | Action::ExportSlot(_)
            )
        {
            self.pending = Some((id, "NO_SESSION"));
            return;
        }
        if self.active {
            self.lease = now + 3000;
        }
        match action {
            Action::Begin => {
                if !self.active {
                    self.app = Some(app.snapshot());
                    self.runtime = Some((*settings, *idle));
                    self.counts = status.button_counts;
                    self.last_button = status.last_button;
                    app.cancel();
                    self.active = true;
                    self.lease = now + 3000;
                }
            }
            Action::End => self.end(app, status, settings, idle),
            Action::Ping | Action::State => {}
            Action::Touch(point) => {
                if self.injection.press() {
                    app.cancel();
                }
                if idle.contact(now) == Gate::Forward {
                    app.pointer(point);
                } else {
                    app.cancel();
                }
            }
            Action::Release => {
                if self.injection.finish() {
                    if idle.release(now) == Gate::Forward {
                        app.release_at(now);
                    } else {
                        app.cancel();
                    }
                }
            }
            Action::Cancel => {
                if self.injection.finish() {
                    idle.release(now);
                    app.cancel();
                }
            }
            Action::Button(button, code) => {
                if code != 1 || idle.button(now) == Gate::Forward {
                    app.button_at(button, code, now);
                }
                status.update(Event::Button { button, code });
            }
            Action::Battery(p, mv, power) => self.battery = Some((p, mv, power)),
            Action::Live => self.battery = None,
            Action::Wifi => {
                if crate::wifi::state() == 0 {
                    result = "UNCONFIGURED";
                } else {
                    crate::wifi::reconnect();
                }
            }
            Action::WifiFault(fault) => crate::wifi::set_fault(fault),
            Action::Stop => self.stop(),
            Action::Persist(brightness) => {
                self.end(app, status, settings, idle);
                self.persistence = Some((id, brightness));
                return;
            }
            Action::Idle(seconds, dim) => {
                *settings = settings.with_idle_preferences(seconds, dim).unwrap();
                app.dim_timeout_secs = seconds;
                app.dim_brightness = dim;
            }
            Action::Panic | Action::Restart => {
                let reboot = if matches!(action, Action::Panic) {
                    Reboot::Panic
                } else {
                    Reboot::Restart
                };
                self.end(app, status, settings, idle);
                self.reboot = Some(reboot);
                self.pending = Some((id, "ARMED"));
                return;
            }
            Action::Ride(action, source) => {
                self.end(app, status, settings, idle);
                self.ride = Some((id, Some((action, source))));
                return;
            }
            Action::RideInit => {
                self.end(app, status, settings, idle);
                self.ride = Some((id, None));
                return;
            }
            Action::ExportInfo => {
                self.export = Some((id, None));
                return;
            }
            Action::ExportSlot(index) => {
                self.export = Some((id, Some(index)));
                return;
            }
            Action::Record(_, _) | Action::Capture => {
                if screenshot_busy || self.recording() {
                    result = "BUSY";
                } else {
                    let (duration, fps) = match action {
                        Action::Record(ms, fps) => (u64::from(ms), u64::from(fps)),
                        _ => (1, 1),
                    };
                    self.until = now + duration;
                    self.next = 0;
                    self.interval = 1000 / fps;
                    self.sequence = 0;
                    self.stream = id;
                }
            }
        }
        self.pending = Some((id, result));
    }
    /// Called after the LCD update decision, with the canvas matching panel state.
    pub fn drawn(
        &mut self,
        frame: u32,
        now: u64,
        canvas: &[u16; PIXELS],
        app: &App,
        status: &Status,
        touch_ok: bool,
        metrics: &Metrics,
    ) {
        if let Some((id, result)) = self.pending.take() {
            let (wifi_associations, wifi_successes, wifi_failures, wifi_fault) =
                crate::wifi::stats();
            let clock = cycling_os::network_time::snapshot(
                now,
                app.timezone_minutes,
                crate::wifi::online(),
            );
            let ride = app.ride.metrics(now);
            let (ride_speed, ride_distance, ride_elapsed) =
                if metrics.ride_source == Some(cycling_os::ride_log::Source::Live) {
                    (-1, -1, metrics.recording_active_ms as i64)
                } else {
                    (
                        i64::from(ride.speed_mm_s),
                        ride.distance_mm.min(i64::MAX as u64) as i64,
                        ride.active_ms.min(i64::MAX as u64) as i64,
                    )
                };
            let (percent, mv) = status
                .battery
                .map(|(p, m)| (i32::from(p), i32::from(m)))
                .unwrap_or((-1, -1));
            let (x, y) = app
                .point()
                .map(|p| (i32::from(p.x), i32::from(p.y)))
                .unwrap_or((-1, -1));
            println!(
                "CYCLING_DEBUG {} {} {{\"protocol\":1,\"screen\":\"{}\",\"focus\":{},\"pressed\":{},\"input_blocked\":{},\"frame\":{},\"ms\":{},\"active\":{},\"brightness\":{},\"effective_brightness\":{},\"dimmed\":{},\"idle_ms\":{},\"dim_timeout\":{},\"dim_brightness\":{},\"timezone\":{},\"time_status\":\"{}\",\"utc\":{},\"time_ms\":{},\"time_age_ms\":{},\"ride_phase\":\"{}\",\"ride_speed_mm_s\":{},\"ride_distance_mm\":{},\"ride_elapsed_ms\":{},\"ride_page\":{},\"ride_layout\":{},\"ride_recording\":\"{}\",\"ride_source\":\"{}\",\"recording_active_ms\":{},\"recorded_samples\":{},\"recording_dropped\":{},\"recorded_rides\":{},\"recording_slot\":{},\"recording_write_ms\":{},\"recording_erase_ms\":{},\"x\":{},\"y\":{},\"buttons\":[{},{},{}],\"battery\":{},\"millivolts\":{},\"power\":{},\"fake_battery\":{},\"wifi\":{},\"wifi_associations\":{},\"wifi_successes\":{},\"wifi_failures\":{},\"wifi_fault\":{},\"touch_ok\":{},\"heap_free\":{},\"heap_min_sampled\":{},\"psram_capacity\":{},\"psram_free\":{},\"frame_ms\":{},\"max_frame_ms\":{},\"display_draws\":{},\"display_skips\":{},\"valid\":{},\"bad_crc\":{},\"uart_errors\":{},\"touch_errors\":{},\"reset_reason\":\"{}\",\"crash_marker\":\"{}\",\"crash_firmware\":\"{}\",\"gps_state\":\"{}\",\"gps_lat_e7\":{},\"gps_lon_e7\":{},\"gps_satellites\":{},\"gps_age_ms\":{},\"gps_bytes\":{},\"gps_valid\":{},\"gps_checksum_errors\":{},\"gps_parse_errors\":{},\"gps_overflows\":{},\"gps_line_overflows\":{},\"gps_uart_errors\":{},\"recording\":{}}}",
                id,
                result,
                app.screen.name(),
                app.focus,
                app.pressed.map(i32::from).unwrap_or(-1),
                app.pointer_suppressed(),
                frame,
                now,
                self.active,
                app.controls.brightness,
                metrics.effective_brightness,
                metrics.dimmed,
                metrics.idle_ms,
                metrics.dim_timeout_secs,
                metrics.dim_brightness,
                app.timezone_minutes,
                clock.status.name(),
                clock.unix_seconds.unwrap_or(0),
                clock.millis,
                clock.age_ms.unwrap_or(0),
                app.ride.phase().name(),
                ride_speed,
                ride_distance,
                ride_elapsed,
                app.ride_page,
                app.ride_layout,
                metrics.ride_recording.name(),
                metrics
                    .ride_source
                    .map(|source| source.name())
                    .unwrap_or("none"),
                metrics.recording_active_ms,
                metrics.recorded_samples,
                metrics.recording_dropped,
                metrics.recorded_rides,
                metrics.recording_slot,
                metrics.recording_write_ms,
                metrics.recording_erase_ms,
                x,
                y,
                status.button_counts[0],
                status.button_counts[1],
                status.button_counts[2],
                percent,
                mv,
                status.power.map(i32::from).unwrap_or(-1),
                self.battery.is_some(),
                crate::wifi::state(),
                wifi_associations,
                wifi_successes,
                wifi_failures,
                wifi_fault,
                touch_ok,
                metrics.heap_free,
                metrics.heap_min_sampled,
                metrics.psram_capacity,
                metrics.psram_free,
                metrics.frame_ms,
                metrics.max_frame_ms,
                metrics.display_draws,
                metrics.display_skips,
                metrics.companion_valid,
                metrics.companion_bad_crc,
                metrics.uart_errors,
                metrics.touch_errors,
                metrics.reset.name(),
                metrics.crash.name(),
                metrics.crash.firmware(),
                metrics.gps.state.name(),
                metrics.gps.latitude_e7.unwrap_or(i32::MIN),
                metrics.gps.longitude_e7.unwrap_or(i32::MIN),
                metrics.gps.satellites.map(i32::from).unwrap_or(-1),
                metrics.gps.age_ms.unwrap_or(0),
                metrics.gps.bytes,
                metrics.gps.valid_sentences,
                metrics.gps.checksum_errors,
                metrics.gps.parse_errors,
                metrics.gps.overflows,
                metrics.gps.line_overflows,
                metrics.gps.uart_errors,
                self.recording()
            );
            if self.reboot.is_some() {
                self.reboot_ready = self.reboot.take();
            }
        }
        if self.recording() && now >= self.next {
            let key = self.sequence == 0;
            println!(
                "CYCLING_REC BEGIN {} {} {} {} {} {:08x}",
                self.stream,
                self.sequence,
                frame,
                now,
                u8::from(key),
                checksum(canvas)
            );
            let mut encoded = [0; 480];
            for row in 0..HEIGHT {
                let range = row * WIDTH..(row + 1) * WIDTH;
                if key || canvas[range.clone()] != self.pixels[range.clone()] {
                    let n = encode_row(&canvas[range.clone()], &mut encoded);
                    println!(
                        "CYCLING_REC ROW {} {} {} {}",
                        self.stream,
                        self.sequence,
                        row,
                        core::str::from_utf8(&encoded[..n]).unwrap()
                    );
                    self.pixels[range.clone()].copy_from_slice(&canvas[range]);
                }
            }
            println!("CYCLING_REC END {} {}", self.stream, self.sequence);
            self.sequence += 1;
            self.next = now + self.interval;
            if self.interval == 1000 && self.until <= now {
                self.stop();
            }
        }
    }
}
