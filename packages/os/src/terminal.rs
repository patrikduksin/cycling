//! Single bounded USB owner for ordinary commands and JSON logs.
use core::fmt::{self, Write};
use cycling_os::terminal_protocol::{Command, Lines, Request};
use esp_hal::usb_serial_jtag::{UsbSerialJtagRx, UsbSerialJtagTx};
use serde::Serialize;

const BYTES: usize = 1536;
struct Text {
    bytes: [u8; 1024],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self {
            bytes: [0; 1024],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("encoding_failed")
    }
}
impl Write for Text {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.len + s.len() > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}
#[derive(Serialize)]
struct Reply<'a> {
    r#type: &'static str,
    id: u32,
    status: &'static str,
    ms: u64,
    data: &'a str,
}
pub struct Terminal {
    rx: UsbSerialJtagRx<'static, esp_hal::Blocking>,
    tx: UsbSerialJtagTx<'static, esp_hal::Blocking>,
    lines: Lines,
    current: [u8; BYTES],
    length: usize,
    offset: usize,
    reply: [u8; BYTES],
    reply_len: usize,
    pub rejected: u32,
    pub sent: u32,
    tx_max_us: u64,
    reboot: Option<(u64, bool)>,
}
impl Terminal {
    pub fn new(usb: esp_hal::usb_serial_jtag::UsbSerialJtag<'static, esp_hal::Blocking>) -> Self {
        let (rx, tx) = usb.split();
        Self {
            rx,
            tx,
            lines: Lines::default(),
            current: [0; BYTES],
            length: 0,
            offset: 0,
            reply: [0; BYTES],
            reply_len: 0,
            rejected: 0,
            sent: 0,
            tx_max_us: 0,
            reboot: None,
        }
    }
    fn reply(&mut self, id: u32, status: &'static str, data: &str, now: u64) {
        let reply = Reply {
            r#type: "reply",
            id,
            status,
            ms: now,
            data,
        };
        match serde_json_core::to_slice(&reply, &mut self.reply[..BYTES - 1]) {
            Ok(n) => {
                self.reply[n] = b'\n';
                self.reply_len = n + 1;
            }
            Err(_) => {
                self.rejected = self.rejected.saturating_add(1);
                let fallback = Reply {
                    r#type: "reply",
                    id,
                    status: "ENCODING_FAILED",
                    ms: now,
                    data: "",
                };
                let n = serde_json_core::to_slice(&fallback, &mut self.reply[..BYTES - 1]).unwrap();
                self.reply[n] = b'\n';
                self.reply_len = n + 1;
            }
        }
    }
    /// Whole records cannot interleave. Replies win between records; each poll
    /// offers at most 64 bytes and never waits on a disconnected host.
    pub fn pump(&mut self) {
        let started = embassy_time::Instant::now();
        if self.offset == self.length {
            if self.reply_len > 0 {
                self.length = self.reply_len;
                self.current[..self.length].copy_from_slice(&self.reply[..self.length]);
                self.reply_len = 0;
            } else if let Some(record) = crate::logging::take() {
                self.length = record.as_bytes().len();
                self.current[..self.length].copy_from_slice(record.as_bytes());
            } else {
                return;
            }
            self.offset = 0;
        }
        for _ in 0..64 {
            if self.offset == self.length {
                break;
            }
            if self.tx.write_byte_nb(self.current[self.offset]).is_err() {
                break;
            }
            self.offset += 1;
        }
        let _ = self.tx.flush_tx_nb();
        self.tx_max_us = self.tx_max_us.max(started.elapsed().as_micros());
    }
    pub fn finish_reboot(&self, now: u64) {
        if let Some((at, panic)) = self.reboot {
            if now >= at + 1000
                || (now >= at + 100 && self.reply_len == 0 && self.offset == self.length)
            {
                #[cfg(feature = "debug-harness")]
                if panic {
                    crate::device::crash_rtc::controlled_panic();
                }
                #[cfg(not(feature = "debug-harness"))]
                let _ = panic;
                esp_hal::system::software_reset();
            }
        }
    }
    pub fn request(&mut self, now: u64) -> Option<Request> {
        if self.reply_len != 0 {
            return None;
        }
        for _ in 0..64 {
            let Ok(byte) = self.rx.read_byte() else {
                break;
            };
            if let Some(result) = self.lines.push(byte) {
                return match result {
                    Ok(request) => Some(request),
                    Err(error) => {
                        self.rejected = self.rejected.saturating_add(1);
                        self.reply(
                            0,
                            "INVALID",
                            match error {
                                cycling_os::terminal_protocol::Error::Invalid => {
                                    "malformed command"
                                }
                                cycling_os::terminal_protocol::Error::Overlong => {
                                    "line exceeds 128 bytes"
                                }
                            },
                            now,
                        );
                        None
                    }
                };
            }
        }
        None
    }
}

pub fn execute(
    terminal: &mut Terminal,
    request: Request,
    system: &mut crate::core_system::System,
    now: u64,
    #[cfg(feature = "cycling")] sdk: &mut crate::sdk_runtime::Runtime,
) {
    let mut output = Text::new();
    let mut status = "OK";
    match request.command {
        #[cfg(feature = "cycling")]
        Command::Domain { bytes, length } => {
            status = sdk.command(
                core::str::from_utf8(&bytes[..length]).unwrap_or(""),
                &mut system.store,
                now,
                &mut output,
            );
        }
        Command::Help => {
            let _ = write!(
                output,
                "CMD id HELP|INFO|STATUS|POSITION|INPUT|BATTERY|TIME|SETTINGS|BRIGHTNESS n|TIMEZONE minutes|IDLE seconds level|SAVE|ACTIVITY|WIFI [RECONNECT]|BLE [RECONNECT]|STORAGE|DISPLAY rgb565hex|RESTART|TEST n"
            );
        }
        Command::Info => {
            crate::logging::metadata(false);
            let _ = write!(
                output,
                "board=c606 commit={} harness={} cycling={} logging=INFO recording=false protocol=1 max_line=128 log_slots=8 log_bytes=384",
                option_env!("CYCLING_BUILD_COMMIT").unwrap_or("unknown"),
                cfg!(feature = "debug-harness"),
                cfg!(feature = "cycling")
            );
        }
        Command::Status => {
            let logs = crate::logging::stats();
            let _ = write!(
                output,
                "uptime_ms={} reset={} crash={} heap_free={} heap_min_sampled={} psram_free={} input_events={} display_submissions={} display_max_ms={} storage_ops={} storage_max_ms={} terminal_rejected={} log_attempts={} log_total_us={} log_max_us={} log_lost={} log_depth={} log_buffer_bytes={} usb_poll_max_us={}",
                now,
                system.reset.name(),
                system.crash.name(),
                esp_alloc::HEAP.free(),
                system.heap_min_sampled,
                crate::device::psram::external_free(),
                system.input_events,
                system.display_submissions,
                system.display_max_ms,
                system.operations,
                system.storage_max_ms,
                terminal.rejected,
                logs.0,
                logs.1,
                logs.2,
                logs.3,
                logs.4,
                logs.5,
                terminal.tx_max_us
            );
        }
        Command::Position => {
            if let Some(p) = crate::services::positioning::snapshot(now) {
                let g = p.gps;
                let _ = write!(
                    output,
                    "transport={} fix={} sequence={} bytes={} valid={} checksum_errors={} parse_errors={} dma_losses={} line_overflows={} uart_errors={} fix_age_ms={:?} satellites={:?}",
                    p.transport.name(),
                    g.state.name(),
                    p.sequence,
                    g.bytes,
                    g.valid_sentences,
                    g.checksum_errors,
                    g.parse_errors,
                    g.overflows,
                    g.line_overflows,
                    g.uart_errors,
                    g.age_ms,
                    g.satellites
                );
            } else {
                status = "UNAVAILABLE";
            }
        }
        Command::Input | Command::Battery => {
            if let Some(s) = crate::services::io::snapshot(now) {
                let _ = write!(
                    output,
                    "touch_available={} touch_errors={} companion_valid={} bad_crc={} uart_errors={} input_lost={} buttons={:?} battery={:?} power={:?}",
                    s.touch_available,
                    s.touch_errors,
                    s.companion_valid,
                    s.companion_bad_crc,
                    s.uart_errors,
                    s.input_lost,
                    s.button_counts,
                    s.battery,
                    s.power
                );
            } else {
                status = "UNAVAILABLE";
            }
        }
        Command::Time => {
            let t = cycling_os::network_time::snapshot(
                now,
                system.settings.timezone_minutes,
                crate::wifi::online(),
            );
            let _ = write!(output, "{:?}", t);
        }
        Command::Settings => {
            let s = system.settings;
            let _ = write!(
                output,
                "brightness={} dim_timeout={} dim_brightness={} timezone={} effective={} dimmed={} source={} persistence_error={}",
                s.brightness,
                s.dim_timeout_secs,
                s.dim_brightness,
                s.timezone_minutes,
                system.effective(),
                system.dimmed(),
                system.settings_source,
                system.settings_error
            );
        }
        Command::SetBrightness(v) => {
            if let Some(s) = system.settings.with_brightness(v) {
                system.settings = s;
                system.activity(now);
            } else {
                status = "INVALID";
            }
        }
        Command::SetTimezone(v) => {
            if let Some(s) = system.settings.with_timezone(v) {
                system.settings = s;
            } else {
                status = "INVALID";
            }
        }
        Command::SetIdle(t, v) => {
            if let Some(s) = system.settings.with_idle_preferences(t, v) {
                system.settings = s;
                system.activity(now);
            } else {
                status = "INVALID";
            }
        }
        Command::Save => {
            if !system.save() {
                status = "FAILED";
            }
        }
        Command::Activity => system.activity(now),
        Command::Wifi => {
            let _ = write!(
                output,
                "state={} online={} stats={:?}",
                crate::wifi::state(),
                crate::wifi::online(),
                crate::wifi::stats()
            );
        }
        Command::WifiReconnect => {
            crate::wifi::reconnect();
            status = "ACCEPTED";
        }
        Command::Ble => {
            let s = crate::bluetooth::snapshot();
            let _ = write!(
                output,
                "link={} connections={} disconnections={} notifications={} dropped={} scan_reports={}",
                s.link.name(),
                s.connections,
                s.disconnections,
                s.notifications,
                s.dropped,
                s.scan_reports
            );
        }
        Command::BleReconnect => {
            status = if crate::bluetooth::request_reconnect() {
                "ACCEPTED"
            } else {
                "UNSUPPORTED"
            };
        }
        Command::Storage => {
            let _ = write!(output, "{:?}", system.store.geometry());
        }
        Command::Display(color) => system.fill(color),
        Command::Restart => {
            terminal.reboot = Some((now, false));
            status = "ACCEPTED";
        }
        Command::Test(n) => {
            #[cfg(feature = "debug-harness")]
            {
                match n {
                    0..=4 => {
                        crate::wifi::set_fault(n);
                        status = "ACCEPTED";
                    }
                    20 => {
                        log::warn!(target:"diagnostic","controlled executor stall ms=6000");
                        esp_hal::delay::Delay::new().delay_millis(6000);
                        log::info!(target:"diagnostic","executor stall ended");
                        status = "ACCEPTED";
                    }
                    11 => {
                        terminal.reboot = Some((now, true));
                        status = "ACCEPTED";
                    }
                    10 => {
                        for _ in 0..64 {
                            log::warn!(target:"diagnostic","controlled log saturation");
                        }
                        status = "ACCEPTED";
                    }
                    _ => status = "UNSUPPORTED",
                }
            }
            #[cfg(not(feature = "debug-harness"))]
            {
                let _ = n;
                status = "UNSUPPORTED";
            }
        }
    }
    terminal.reply(request.id, status, output.as_str(), now);
    terminal.sent = terminal.sent.saturating_add(1);
}
