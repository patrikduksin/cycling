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
                Action::Begin | Action::State | Action::End | Action::Persist(_)
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
            let (percent, mv) = status
                .battery
                .map(|(p, m)| (i32::from(p), i32::from(m)))
                .unwrap_or((-1, -1));
            let (x, y) = app
                .point()
                .map(|p| (i32::from(p.x), i32::from(p.y)))
                .unwrap_or((-1, -1));
            println!(
                "CYCLING_DEBUG {} {} {{\"protocol\":1,\"screen\":\"{}\",\"focus\":{},\"pressed\":{},\"input_blocked\":{},\"frame\":{},\"ms\":{},\"active\":{},\"brightness\":{},\"effective_brightness\":{},\"dimmed\":{},\"idle_ms\":{},\"dim_timeout\":{},\"dim_brightness\":{},\"timezone\":{},\"time_status\":\"{}\",\"utc\":{},\"time_ms\":{},\"time_age_ms\":{},\"ride_phase\":\"{}\",\"ride_speed_mm_s\":{},\"ride_distance_mm\":{},\"ride_elapsed_ms\":{},\"ride_page\":{},\"ride_layout\":{},\"x\":{},\"y\":{},\"buttons\":[{},{},{}],\"battery\":{},\"millivolts\":{},\"power\":{},\"fake_battery\":{},\"wifi\":{},\"wifi_associations\":{},\"wifi_successes\":{},\"wifi_failures\":{},\"wifi_fault\":{},\"touch_ok\":{},\"heap_free\":{},\"heap_min_sampled\":{},\"psram_capacity\":{},\"psram_free\":{},\"frame_ms\":{},\"max_frame_ms\":{},\"display_draws\":{},\"display_skips\":{},\"valid\":{},\"bad_crc\":{},\"uart_errors\":{},\"touch_errors\":{},\"gps_state\":\"{}\",\"gps_lat_e7\":{},\"gps_lon_e7\":{},\"gps_satellites\":{},\"gps_age_ms\":{},\"gps_bytes\":{},\"gps_valid\":{},\"gps_checksum_errors\":{},\"gps_parse_errors\":{},\"gps_overflows\":{},\"gps_line_overflows\":{},\"gps_uart_errors\":{},\"recording\":{}}}",
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
                ride.speed_mm_s,
                ride.distance_mm,
                ride.active_ms,
                app.ride_page,
                app.ride_layout,
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
