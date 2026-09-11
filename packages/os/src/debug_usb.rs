//! Device-side test session. No flash writes or arbitrary memory access.
use cycling_os::{
    coin::{HEIGHT, PIXELS, WIDTH},
    companion::{Event, Status},
    controls::Controls,
    debug::{Action, encode_row},
    screenshot::checksum,
};
use esp_println::println;

pub struct Debug {
    pub active: bool,
    pub touch_injected: bool,
    lease: u64,
    brightness: u8,
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
    pub heap_min: usize,
    pub frame_ms: u64,
    pub max_frame_ms: u64,
}
impl Debug {
    pub fn new() -> Self {
        Self {
            active: false,
            touch_injected: false,
            lease: 0,
            brightness: 50,
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
            heap_min: usize::MAX,
            frame_ms: 0,
            max_frame_ms: 0,
        }
    }
    pub fn recording(&self) -> bool {
        self.until != 0
    }
    fn stop(&mut self) {
        if self.recording() {
            println!("CYCLING_REC STOP {} {}", self.stream, self.sequence);
        }
        self.until = 0;
    }
    fn end(&mut self, ui: &mut Controls, status: &mut Status) {
        self.stop();
        if self.active {
            ui.update(None);
            ui.brightness = self.brightness;
            status.button_counts = self.counts;
            status.last_button = self.last_button;
        }
        self.active = false;
        self.touch_injected = false;
        self.battery = None;
    }
    pub fn tick(&mut self, now: u64, ui: &mut Controls, status: &mut Status) {
        self.heap_min = self.heap_min.min(esp_alloc::HEAP.free());
        if self.active && now >= self.lease {
            self.end(ui, status);
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
        ui: &mut Controls,
        status: &mut Status,
        screenshot_busy: bool,
    ) {
        let mut result = "OK";
        if !self.active && !matches!(action, Action::Begin | Action::State | Action::End) {
            self.pending = Some((id, "NO_SESSION"));
            return;
        }
        if self.active {
            self.lease = now + 3000;
        }
        match action {
            Action::Begin => {
                if !self.active {
                    self.brightness = ui.brightness;
                    self.counts = status.button_counts;
                    self.last_button = status.last_button;
                    ui.update(None);
                    self.active = true;
                    self.lease = now + 3000;
                }
            }
            Action::End => self.end(ui, status),
            Action::Ping | Action::State => {}
            Action::Touch(point) => {
                self.touch_injected = true;
                ui.update(Some(point));
            }
            Action::Release => {
                self.touch_injected = false;
                ui.update(None);
            }
            Action::Button(button, code) => {
                ui.button(button, code);
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
            Action::Stop => self.stop(),
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
    /// Called after LCD draw, so ACK frame numbers identify a displayed result.
    pub fn drawn(
        &mut self,
        frame: u32,
        now: u64,
        canvas: &[u16; PIXELS],
        ui: &Controls,
        status: &Status,
        touch_ok: bool,
        valid: u32,
        bad_crc: u32,
        uart_errors: u32,
    ) {
        if let Some((id, result)) = self.pending.take() {
            let (percent, mv) = status
                .battery
                .map(|(p, m)| (i32::from(p), i32::from(m)))
                .unwrap_or((-1, -1));
            let (x, y) = ui
                .point
                .map(|p| (i32::from(p.x), i32::from(p.y)))
                .unwrap_or((-1, -1));
            println!(
                "CYCLING_DEBUG {} {} {{\"protocol\":1,\"screen\":\"controls\",\"frame\":{},\"ms\":{},\"active\":{},\"brightness\":{},\"x\":{},\"y\":{},\"buttons\":[{},{},{}],\"battery\":{},\"millivolts\":{},\"power\":{},\"fake_battery\":{},\"wifi\":{},\"touch_ok\":{},\"heap_free\":{},\"heap_min_sampled\":{},\"frame_ms\":{},\"max_frame_ms\":{},\"valid\":{},\"bad_crc\":{},\"uart_errors\":{},\"recording\":{}}}",
                id,
                result,
                frame,
                now,
                self.active,
                ui.brightness,
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
                touch_ok,
                esp_alloc::HEAP.free(),
                self.heap_min,
                self.frame_ms,
                self.max_frame_ms,
                valid,
                bad_crc,
                uart_errors,
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
