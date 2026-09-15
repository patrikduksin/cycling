//! Headless backend: production parser, shell handlers, journals and SDK runtime.
use crate::devices::DisplayDevice;
use crate::devices::InputDevice;
use crate::devices::PowerDevice;
use crate::storage::{Fault, FileFlash};
use device_api::ant::Ant;
use device_api::ant::AntOperation;
use device_api::ble_transport::Ble;
use device_api::display::Display;
use device_api::input::Button;
use device_api::input::InputObservation;
use device_api::input::InputSnapshot;
use device_api::observation::Availability;
use device_api::observation::Error;
use device_api::positioning::Positioning;
use firmware_console::protocol::Command;
use firmware_console::protocol::Request;
use std::fmt::Write as _;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::string::String;
use std::vec::Vec;
#[derive(Default)]
pub struct PositionFixture {
    pub acquisition: firmware_services::positioning::Acquisition,
    pub missing: bool,
    dma_losses: u32,
    uart_errors: u32,
}
impl Positioning for PositionFixture {
    fn availability(&self) -> Availability {
        if self.missing {
            Availability::Unsupported
        } else {
            Availability::Ready
        }
    }
    fn snapshot(&self, now: u64) -> Option<device_api::positioning::Snapshot> {
        (!self.missing).then(|| self.acquisition.snapshot(now))
    }
}
#[derive(Default)]
pub struct BleFixture {
    pub snapshot: device_api::ble_transport::Snapshot,
    pub packets: std::collections::VecDeque<device_api::ble_transport::Packet>,
}
impl Ble for BleFixture {
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn snapshot(&self) -> device_api::ble_transport::Snapshot {
        self.snapshot
    }
    fn take_packet(&mut self) -> Option<device_api::ble_transport::Packet> {
        self.packets.pop_front()
    }
    fn reconnect(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
pub struct NoAnt;
impl Ant for NoAnt {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn scanning(&self) -> bool {
        false
    }
    fn discoveries(&self) -> [Option<device_api::ant::Discovery>; 8] {
        [None; 8]
    }
    fn request(&mut self, _: AntOperation, _: u64) -> &'static str {
        "UNSUPPORTED"
    }
    fn channels(
        &self,
        _: u64,
    ) -> [Option<device_api::ant::Snapshot>; device_api::ant::CHANNEL_CAPACITY] {
        [None; device_api::ant::CHANNEL_CAPACITY]
    }
    fn take_packet(&mut self) -> Option<device_api::ant::Packet> {
        None
    }
}
pub struct NoInputObservation;
impl InputObservation for NoInputObservation {
    fn snapshot(&self, _: u64) -> Option<InputSnapshot> {
        None
    }
}
pub type VirtualShell =
    firmware_shell::shell::Shell<DisplayDevice, InputDevice, PowerDevice, FileFlash>;
pub struct Session {
    pub shell: VirtualShell,
    pub now: u64,
    pub boot: u32,
    pub position: PositionFixture,
    pub ble: BleFixture,
    wifi_control: device_api::connectivity::ControlStatus,
    ble_control: device_api::connectivity::ControlStatus,
    media: FileFlash,
    display: DisplayDevice,
    #[cfg(feature = "cycling")]
    pub sdk: vana::app::Runtime,
}
impl Session {
    pub fn new(root: &Path, width: usize, height: usize) -> io::Result<Self> {
        if !(16..=320).contains(&width) || !(16..=320).contains(&height) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "geometry must be 16..320",
            ));
        }
        Self::with_media(FileFlash::open(root)?, width, height)
    }
    fn with_media(media: FileFlash, width: usize, height: usize) -> io::Result<Self> {
        let boot = media.begin_boot()?;
        let display = DisplayDevice::new(width, height);
        let input = InputDevice::new(
            &[Button::TopLeft, Button::BottomLeft, Button::BottomRight],
            Availability::Ready,
        );
        let mut shell = firmware_shell::shell::Shell::new(
            display.clone(),
            input,
            PowerDevice::new(Availability::Ready),
            media.clone(),
            0,
        )
        .map_err(|_| io::Error::other("shell unavailable"))?;
        #[cfg(feature = "debug-harness")]
        {
            shell.harness = firmware_shell::harness::State::new(boot);
            shell.harness.buffer = Some(firmware_shell::harness::CaptureBuffer::Owned(
                vec![0u8; firmware_shell::harness::FRAMES * width * height * 2].into_boxed_slice(),
            ));
        }
        shell.boot_id = boot;
        shell.present();
        #[cfg(feature = "cycling")]
        let profile = if shell.settings.ble.is_some() {
            vana::sensors::ble_profile::Profile::from_u8(shell.settings.ble_profile)
        } else {
            vana::sensors::ble_profile::Profile::HeartRate
        };
        Ok(Self {
            shell,
            now: 0,
            boot,
            position: PositionFixture::default(),
            ble: BleFixture::default(),
            wifi_control: device_api::connectivity::ControlStatus::new(),
            ble_control: device_api::connectivity::ControlStatus::new(),
            media,
            display,
            #[cfg(feature = "cycling")]
            sdk: vana::app::Runtime::new(profile),
        })
    }
    pub fn tick(&mut self) {
        self.shell.observe_position(&self.position, self.now);
        #[cfg(feature = "cycling")]
        self.shell.set_app_active(self.sdk.input_active());
        self.shell.tick(self.now);
        #[cfg(feature = "cycling")]
        let mut ant = NoAnt;
        #[cfg(feature = "cycling")]
        for _ in 0..16 {
            let Some(edge) = self.shell.take_app_input() else {
                break;
            };
            self.sdk.input(edge.input, self.now, &mut ant);
        }
        self.shell.present();
        #[cfg(feature = "cycling")]
        self.sdk.tick(
            &mut self.shell.data_storage(),
            self.now,
            &mut ant,
            &mut self.ble,
            &self.position,
            false,
            &NoInputObservation,
            device_api::sensors::Snapshot {
                pressure: device_api::observation::Observation::Unavailable,
                motion: [device_api::observation::Observation::Unavailable; 2],
                identity: device_api::observation::Observation::Unavailable,
                reports: 0,
                invalid_reports: 0,
                losses: 0,
            },
        );
    }
    pub fn advance(&mut self, ms: u64) -> Result<(), Error> {
        if ms > 120000 {
            return Err(Error::Invalid);
        }
        let end = self.now.checked_add(ms).ok_or(Error::Invalid)?;
        while self.now < end {
            self.now = (self.now + 10).min(end);
            self.tick();
        }
        Ok(())
    }
    pub fn execute(&mut self, request: Request, out: &mut String) -> &'static str {
        self.tick();
        if let Some(status) = firmware_console::commands::shell::execute(
            request.command,
            &mut self.shell,
            self.now,
            out,
        ) {
            #[cfg(feature = "cycling")]
            if matches!(request.command, Command::Display(_)) {
                self.sdk.suspend_display();
            }
            if matches!(
                request.command,
                Command::Harness(firmware_shell::harness::Command {
                    operation: firmware_shell::harness::Operation::Caps,
                    ..
                })
            ) {
                out.push_str(" ordinary=HELP,INFO,STATUS,POSITION,SETTINGS,BRIGHTNESS,TIMEZONE,IDLE,SAVE,ACTIVITY,STORAGE,DISPLAY,WIFI,BLE,RESTART");
                #[cfg(feature = "cycling")]
                out.push_str(",RIDE,EXPORT,RADAR");
            }
            return status;
        }
        match request.command {
            Command::Info => {
                let _ = write!(
                    out,
                    "board=virtual commit={} harness={} cycling={} protocol=1 boot={} physical_evidence=unsupported",
                    option_env!("CYCLING_BUILD_COMMIT").unwrap_or("unknown"),
                    cfg!(feature = "debug-harness"),
                    cfg!(feature = "cycling"),
                    self.boot
                );
            }
            Command::Help => {
                out.push_str("CMD id INFO|STATUS|SETTINGS|BRIGHTNESS n|TIMEZONE minutes|IDLE seconds level|SAVE|ACTIVITY|DISPLAY rgb565hex|POSITION|HARNESS CAPS|RESTART; physical-only operations UNSUPPORTED; fixtures use separate FIXTURE lines");
            }
            Command::Status => {
                firmware_console::commands::shell::status(&self.shell, self.now, out)
            }
            Command::Storage => {
                let _ = write!(out, "{:?}", self.shell.data_storage().geometry());
            }
            Command::Wifi => {
                let _ = write!(
                    out,
                    "state=0 online=false saved={} operation={} sequence={} error={} radio=unsupported fixture=true",
                    self.shell.settings.wifi.is_some(),
                    self.wifi_control.operation.name(),
                    self.wifi_control.sequence,
                    self.wifi_control.error
                );
            }
            Command::WifiConfigure(_) | Command::WifiForget => {
                let previous = self.shell.settings;
                self.shell.settings.wifi = if let Command::WifiConfigure(config) = request.command {
                    Some(config)
                } else {
                    None
                };
                if !self.shell.save_connectivity() {
                    self.shell.settings = previous;
                    return "FAILED";
                }
                self.wifi_control.sequence = self.wifi_control.sequence.wrapping_add(1);
                self.wifi_control.operation = device_api::connectivity::Operation::Completed;
                return "ACCEPTED";
            }
            Command::BleForget => {
                let previous = self.shell.settings;
                self.shell.settings.ble = None;
                self.shell.settings.ble_profile = 0;
                if !self.shell.save_connectivity() {
                    self.shell.settings = previous;
                    return "FAILED";
                }
                self.ble.packets.clear();
                self.ble.snapshot.link = device_api::ble_transport::Link::Off;
                self.ble_control.sequence = self.ble_control.sequence.wrapping_add(1);
                self.ble_control.operation = device_api::connectivity::Operation::Completed;
                #[cfg(feature = "cycling")]
                self.sdk.select_profile(
                    vana::sensors::ble_profile::Profile::Echo,
                    self.ble.snapshot.connections,
                );
                return "ACCEPTED";
            }
            #[cfg(feature = "cycling")]
            Command::BleSelect(peer, profile) => {
                let profile = vana::sensors::ble_profile::Profile::from_u8(profile);
                let previous = self.shell.settings;
                self.shell.settings.ble =
                    vana::sensors::ble::selection(profile, peer.name.bytes(), peer.address);
                self.shell.settings.ble_profile = profile as u8;
                if !self.shell.save_connectivity() {
                    self.shell.settings = previous;
                    return "FAILED";
                }
                self.ble.packets.clear();
                self.ble.snapshot.link = device_api::ble_transport::Link::Off;
                self.sdk
                    .select_profile(profile, self.ble.snapshot.connections);
                self.ble_control.sequence = self.ble_control.sequence.wrapping_add(1);
                self.ble_control.operation = device_api::connectivity::Operation::Completed;
                return "ACCEPTED";
            }
            Command::Ble => {
                let s = self.ble.snapshot();
                let _ = write!(
                    out,
                    "link={} connections={} disconnections={} notifications={} dropped={} scan_reports={} saved={} profile={} operation={} sequence={} error={} radio=unsupported fixture=true",
                    s.link.name(),
                    s.connections,
                    s.disconnections,
                    s.notifications,
                    s.dropped,
                    s.scan_reports,
                    self.shell.settings.ble.is_some(),
                    self.shell.settings.ble_profile,
                    self.ble_control.operation.name(),
                    self.ble_control.sequence,
                    self.ble_control.error
                );
            }
            #[cfg(feature = "cycling")]
            Command::Domain { bytes, length } => {
                return self.sdk.command(
                    core::str::from_utf8(&bytes[..length]).unwrap_or(""),
                    &mut self.shell.data_storage(),
                    self.now,
                    out,
                );
            }
            Command::Restart => return "ACCEPTED",
            _ => return "UNSUPPORTED",
        }
        "OK"
    }
    pub fn fixture(&mut self, line: &str, out: &mut String) -> &'static str {
        // Validate the complete fixture shape before changing any fixture state.
        let parts: Vec<_> = line.split_ascii_whitespace().collect();
        let expected = match parts.get(1).copied() {
            Some("DISPLAY_FAIL") => 2,
            Some("STORAGE_FAIL") => 4,
            Some("POSITION") if parts.get(2) == Some(&"NMEA") => 4,
            Some("BLE") if parts.get(2) == Some(&"HEART") => 4,
            _ => 3,
        };
        if parts.len() != expected {
            return "INVALID";
        }
        let mut w = line.split_ascii_whitespace();
        if w.next() != Some("FIXTURE") {
            return "INVALID";
        }
        match w.next() {
            Some("ADVANCE") => {
                let Some(ms) = w.next().and_then(|s| s.parse().ok()) else {
                    return "INVALID";
                };
                if w.next().is_some() || self.advance(ms).is_err() {
                    return "INVALID";
                }
            }
            Some("DISPLAY_FAIL") => {
                if w.next().is_some() {
                    return "INVALID";
                }
                self.display.0.borrow_mut().fail_next = true;
            }
            Some("STORAGE_FAIL") => {
                let Some(after) = w.next().and_then(|s| s.parse::<u32>().ok()) else {
                    return "INVALID";
                };
                let Some(partial) = w.next().and_then(|s| s.parse::<usize>().ok()) else {
                    return "INVALID";
                };
                if after > 1024 || partial > 4096 || w.next().is_some() {
                    return "INVALID";
                }
                *self.media.fault.borrow_mut() = Fault {
                    after: Some(after),
                    partial,
                };
            }
            Some("POSITION") => {
                match w.next() {
                    Some("NONE") => self.position = PositionFixture::default(),
                    Some("UNSUPPORTED") => self.position.missing = true,
                    Some("NMEA") => {
                        let Some(sentence) = w.next() else {
                            return "INVALID";
                        };
                        if sentence.len() > 100 {
                            return "INVALID";
                        }
                        let mut bytes = sentence.as_bytes().to_vec();
                        bytes.extend_from_slice(b"\r\n");
                        self.position.missing = false;
                        self.position.acquisition.ingest(
                            self.now,
                            &bytes,
                            self.position.dma_losses,
                            self.position.uart_errors,
                        );
                    }
                    Some("LOSS") => {
                        self.position.dma_losses = self.position.dma_losses.saturating_add(1);
                        self.position.uart_errors = self.position.uart_errors.saturating_add(1);
                        self.position.acquisition.ingest(
                            self.now,
                            &[],
                            self.position.dma_losses,
                            self.position.uart_errors,
                        );
                    }
                    _ => return "INVALID",
                };
                if w.next().is_some() {
                    return "INVALID";
                }
            }
            Some("BLE") => {
                match w.next() {
                    Some("CONNECT") => {
                        self.ble.snapshot.link = device_api::ble_transport::Link::Connected;
                        self.ble.snapshot.connections += 1;
                        self.ble.packets.clear();
                    }
                    Some("DISCONNECT") => {
                        self.ble.snapshot.link = device_api::ble_transport::Link::Off;
                        self.ble.snapshot.disconnections += 1;
                        self.ble.packets.clear();
                    }
                    Some("LOSS") => {
                        self.ble.snapshot.dropped += 1;
                    }
                    Some("HEART") => {
                        let Some(bpm) = w.next().and_then(|s| s.parse::<u8>().ok()) else {
                            return "INVALID";
                        };
                        if self.ble.snapshot.link != device_api::ble_transport::Link::Connected {
                            return "UNAVAILABLE";
                        }
                        self.ble.snapshot.notifications += 1;
                        if self.ble.packets.len() == 4 {
                            self.ble.snapshot.dropped += 1;
                        } else {
                            let mut bytes = [0; 60];
                            bytes[1] = bpm;
                            self.ble
                                .packets
                                .push_back(device_api::ble_transport::Packet {
                                    connection: self.ble.snapshot.connections,
                                    sequence: self.ble.snapshot.notifications,
                                    dropped: self.ble.snapshot.dropped,
                                    received_ms: self.now,
                                    bytes,
                                    length: 2,
                                });
                        }
                    }
                    _ => return "INVALID",
                };
                if w.next().is_some() {
                    return "INVALID";
                }
            }
            _ => return "UNSUPPORTED",
        }
        self.tick();
        let _ = write!(out, "now_ms={} boot={} fixture=true", self.now, self.boot);
        "OK"
    }
    pub fn restart(&mut self) -> io::Result<()> {
        let g = self.display.geometry();
        *self = Self::with_media(self.media.clone(), g.width, g.height)?;
        Ok(())
    }
}
#[derive(serde::Serialize)]
struct Reply<'a> {
    r#type: &'static str,
    id: u32,
    status: &'static str,
    ms: u64,
    boot: u32,
    data: &'a str,
}
pub fn terminal(root: &Path, width: usize, height: usize) -> io::Result<()> {
    let mut session = Session::new(root, width, height)?;
    let mut line = Vec::new();
    let mut overflow = false;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    for byte in stdin.lock().bytes() {
        let byte = byte?;
        if byte != b'\n' {
            if line.len() < 256 {
                line.push(byte);
            } else {
                overflow = true;
            }
            continue;
        }
        let mut out = String::new();
        let mut id = 0;
        let mut restart = false;
        let fixture = line.starts_with(b"FIXTURE ");
        let status = if overflow {
            "INVALID"
        } else if fixture {
            session.fixture(core::str::from_utf8(&line).unwrap_or(""), &mut out)
        } else {
            match if line.len() > firmware_console::protocol::MAX_LINE {
                Err(firmware_console::protocol::Error::Overlong)
            } else {
                firmware_console::protocol::parse(&line)
            } {
                Ok(request) => {
                    id = request.id;
                    restart = request.command == Command::Restart;
                    session.execute(request, &mut out)
                }
                Err(_) => "INVALID",
            }
        };
        let reply = Reply {
            r#type: if fixture { "fixture" } else { "reply" },
            id,
            status,
            ms: session.now,
            boot: session.boot,
            data: &out,
        };
        let mut bytes = [0; 1536];
        let n = serde_json_core::to_slice(&reply, &mut bytes)
            .map_err(|_| io::Error::other("reply exceeds bound"))?;
        stdout.write_all(&bytes[..n])?;
        stdout.write_all(b"\n")?;
        stdout.flush()?;
        line.clear();
        overflow = false;
        if restart {
            session.restart()?;
        }
    }
    Ok(())
}
