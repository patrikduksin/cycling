//! Headless backend: production parser, shell handlers, journals and SDK runtime.
use crate::ant::SimulatedAnt;
use crate::devices::Battery;
use crate::devices::DisplayDevice;
use crate::devices::InputDevice;
use crate::devices::PowerDevice;
use crate::devices::Speaker;
use crate::gps::Receiver;
use crate::storage::{Fault, FileFlash};
use crate::world::World;
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
use device_api::sensors::Startup;
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
/// A strap in range connects and notifies Heart Rate Measurements at 1 Hz;
/// out of range, the link drops.
#[derive(Default)]
pub struct BleStrap {
    pub present: bool,
    next_ms: u64,
}
impl BleStrap {
    pub fn new() -> Self {
        Self {
            present: true,
            next_ms: 0,
        }
    }
    fn tick(&mut self, world: &crate::world::World, now: u64, ble: &mut BleFixture) {
        use device_api::ble_transport::Link;
        let link = ble.snapshot.link;
        if self.present && link != Link::Connected {
            ble.snapshot.link = Link::Connected;
            ble.snapshot.connections += 1;
            ble.packets.clear();
        } else if !self.present && link == Link::Connected {
            ble.snapshot.link = Link::Off;
            ble.snapshot.disconnections += 1;
            ble.packets.clear();
        }
        if !self.present || world.time_ms < self.next_ms {
            return;
        }
        self.next_ms = world.time_ms - world.time_ms % 1000 + 1000;
        ble.snapshot.notifications += 1;
        // The transport keeps two packets; older notifications are dropped.
        if ble.packets.len() >= 2 {
            ble.packets.pop_front();
            ble.snapshot.dropped += 1;
        }
        let mut bytes = [0; 60];
        bytes[1] = world.heart_bpm.round().clamp(0.0, 255.0) as u8;
        ble.packets.push_back(device_api::ble_transport::Packet {
            connection: ble.snapshot.connections,
            sequence: ble.snapshot.notifications,
            dropped: ble.snapshot.dropped,
            received_ms: now,
            bytes,
            length: 2,
        });
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
    fn request(
        &mut self,
        _: AntOperation,
        _: u64,
    ) -> Result<device_api::ant::Admission, device_api::ant::Error> {
        Err(device_api::ant::Error::Unavailable)
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
/// Uptime before a powered-on companion reports ready.
const COMPANION_BOOT_MS: u64 = 1_500;
pub type VirtualShell =
    firmware_shell::shell::Shell<DisplayDevice, InputDevice, PowerDevice, FileFlash>;
/// The simulated device and the world it observes, composed like C606 firmware.
pub struct Session {
    pub shell: VirtualShell,
    /// Device uptime; it restarts from zero with the device.
    pub now: u64,
    pub boot: u32,
    pub position: PositionFixture,
    pub ble: BleFixture,
    /// A BLE heart-rate strap that notifies the world's heart rate once a second.
    pub ble_strap: Option<BleStrap>,
    pub ant: SimulatedAnt,
    pub world: World,
    pub gps: Option<Receiver>,
    pub speaker: Speaker,
    pub battery: Battery,
    pub startup: Startup,
    /// Uptime at which a starting companion runs, and the ANT availability then.
    companion_boot_ms: Option<(u64, Availability)>,
    pub input: InputDevice,
    pub power: PowerDevice,
    pub display: DisplayDevice,
    wifi_control: device_api::connectivity::ControlStatus,
    ble_control: device_api::connectivity::ControlStatus,
    pub media: FileFlash,
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
    /// A C606-sized device on memory media whose companion and ANT radio
    /// start shortly after boot.
    pub fn device() -> io::Result<Self> {
        let mut session = Self::with_media(FileFlash::memory(), 240, 320)?;
        session.companion_starting(Availability::Ready);
        Ok(session)
    }
    /// The companion is probing after power-on; ANT admits nothing until it runs.
    fn companion_starting(&mut self, ant: Availability) {
        self.startup = Startup::Probing;
        self.ant.availability = Availability::Initializing;
        self.companion_boot_ms = Some((COMPANION_BOOT_MS, ant));
    }
    fn ant_ready_availability(&self) -> Availability {
        match self.companion_boot_ms {
            Some((_, ant)) => ant,
            None => self.ant.availability,
        }
    }
    fn with_media(media: FileFlash, width: usize, height: usize) -> io::Result<Self> {
        let boot = media.begin_boot()?;
        let display = DisplayDevice::new(width, height);
        let input = InputDevice::new(
            &[Button::TopLeft, Button::BottomLeft, Button::BottomRight],
            Availability::Ready,
        );
        let power = PowerDevice::new(Availability::Ready);
        let mut shell = firmware_shell::shell::Shell::new(
            display.clone(),
            input.clone(),
            power.clone(),
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
        #[cfg(feature = "cycling")]
        {
            let view =
                vana::screens::workout::View::new(vana::screens::workout::Page::Boot, false, None);
            shell.draw_scaled(240, 320, |x, y| view.pixel(x, y));
        }
        Ok(Self {
            shell,
            now: 0,
            boot,
            position: PositionFixture::default(),
            ble: BleFixture::default(),
            ble_strap: None,
            ant: SimulatedAnt::new(Availability::Unsupported),
            world: World::default(),
            gps: None,
            speaker: Speaker::default(),
            battery: Battery::default(),
            startup: Startup::Unsupported,
            companion_boot_ms: None,
            input,
            power,
            display,
            wifi_control: device_api::connectivity::ControlStatus::new(),
            ble_control: device_api::connectivity::ControlStatus::new(),
            media,
            #[cfg(feature = "cycling")]
            sdk: vana::app::Runtime::new(profile),
        })
    }
    /// One pass of the firmware loop, in the order C606 composition runs it.
    pub fn tick(&mut self) {
        if let Some((at, ant)) = self.companion_boot_ms
            && self.now >= at
        {
            self.companion_boot_ms = None;
            self.startup = Startup::Ready;
            self.ant.availability = ant;
        }
        self.ant.tick(&self.world, self.now);
        if let Some(strap) = &mut self.ble_strap {
            strap.tick(&self.world, self.now, &mut self.ble);
        }
        if let Some(gps) = &mut self.gps {
            gps.tick(&self.world, self.now, &mut self.position.acquisition);
        }
        self.power.0.borrow_mut().battery = self
            .battery
            .level
            .map(|(percent, mv)| (percent, mv, self.now));
        self.speaker.tick(self.now);
        #[cfg(feature = "cycling")]
        {
            self.sdk.set_startup_status(self.startup);
            self.shell.set_app_active(self.sdk.input_active());
        }
        self.shell.tick(self.now);
        #[cfg(feature = "cycling")]
        for _ in 0..16 {
            let Some(edge) = self.shell.take_app_input() else {
                break;
            };
            self.sdk.input(edge.input, self.now, &mut self.ant);
        }
        self.shell.observe_position(&self.position, self.now);
        self.shell.present();
        #[cfg(feature = "cycling")]
        {
            let pressure = match self.world.pressure_centi_pa() {
                Some(pressure_centi_pa) => device_api::observation::Observation::Fresh {
                    value: device_api::sensors::Pressure {
                        pressure_centi_pa,
                        temperature_centi_c: 2000,
                    },
                    received_ms: self.now,
                },
                None => device_api::observation::Observation::Unavailable,
            };
            self.sdk.tick(
                &mut self.shell.data_storage(),
                self.now,
                &mut self.ant,
                &mut self.ble,
                vana::app::Observations {
                    position: &self.position,
                    network_online: false,
                    input: &self.battery,
                    physical: device_api::sensors::Snapshot {
                        pressure,
                        motion: [device_api::observation::Observation::Unavailable; 2],
                        identity: device_api::observation::Observation::Unavailable,
                        reports: 0,
                        invalid_reports: 0,
                        losses: 0,
                    },
                },
            );
            self.sdk
                .present(&mut self.shell, self.now, &self.ant, &self.position);
            self.sdk.alert(&mut self.speaker, self.now);
        }
    }
    /// Advance device and world time together in 10 ms firmware passes.
    pub fn step(&mut self, ms: u64) {
        let end = self.now + ms;
        while self.now < end {
            let step = (end - self.now).min(10);
            self.now += step;
            self.world.advance(step);
            self.tick();
        }
    }
    pub fn advance(&mut self, ms: u64) -> Result<(), Error> {
        if ms > 120000 {
            return Err(Error::Invalid);
        }
        self.now.checked_add(ms).ok_or(Error::Invalid)?;
        self.step(ms);
        Ok(())
    }
    pub fn execute(&mut self, request: Request, out: &mut String) -> &'static str {
        if let Some(status) =
            firmware_console::commands::ant::execute(request.command, &mut self.ant, self.now, out)
        {
            return status;
        }
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
                    self.wifi_control.error_name()
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
                    self.ble_control.error_name()
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
    /// A main-CPU restart, as the terminal `RESTART` requests. The companion
    /// keeps running with its channels; see `power_cycle` for a full loss.
    pub fn restart(&mut self) -> io::Result<()> {
        self.reboot(false)
    }
    /// Power lost and restored: the companion boots again too.
    pub fn power_cycle(&mut self) -> io::Result<()> {
        self.reboot(true)
    }
    /// Reboot on the same media. The world, sensors and receiver presence
    /// persist; everything held in device memory starts over.
    fn reboot(&mut self, power: bool) -> io::Result<()> {
        let g = self.display.geometry();
        let mut next = Self::with_media(self.media.clone(), g.width, g.height)?;
        next.world = core::mem::take(&mut self.world);
        // Read before the swap: afterwards `self.ant` is the fresh radio.
        let ready = self.ant_ready_availability();
        core::mem::swap(&mut next.ant, &mut self.ant);
        if power {
            next.ant.power_cycled();
            if self.companion_boot_ms.is_some() || self.startup == Startup::Ready {
                next.companion_starting(ready);
            }
        } else {
            next.ant.host_restarted();
            next.startup = if self.startup == Startup::Ready {
                Startup::AlreadyRunning
            } else {
                self.startup
            };
            // A CPU reset does not stop a companion that is still booting;
            // the new uptime starts at zero, so carry what is left.
            next.companion_boot_ms = self
                .companion_boot_ms
                .map(|(at, ant)| (at.saturating_sub(self.now), ant));
        }
        next.gps = self.gps.take().map(|mut gps| {
            gps.reset();
            gps
        });
        next.position.missing = self.position.missing;
        next.battery = self.battery;
        next.ble_strap = self.ble_strap.take();
        next.speaker.played = core::mem::take(&mut self.speaker.played);
        *self = next;
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
