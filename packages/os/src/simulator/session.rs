//! Headless backend: production parser, shell handlers, journals and SDK runtime.
use super::{DisplayDevice, InputDevice, PowerDevice};
use crate::{
    capabilities::*,
    storage::{OwnedFlash, Region},
    terminal_protocol::{Command, Request},
};
use std::{
    cell::RefCell,
    fmt::Write as _,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    rc::Rc,
    string::{String, ToString},
    vec,
    vec::Vec,
};
const DATA_BYTES: usize = 1024 * 1024;
#[derive(Default)]
pub struct Fault {
    pub after: Option<u32>,
    pub partial: usize,
}
#[derive(Clone)]
pub struct FileFlash {
    root: PathBuf,
    _lock: Rc<fs::File>,
    pub fault: Rc<RefCell<Fault>>,
}
impl FileFlash {
    pub fn open(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("session.lock"))?;
        lock.try_lock()
            .map_err(|e| io::Error::other(std::format!("virtual media already owned: {e}")))?;
        for (name, size) in [("configuration.bin", 8192), ("data.bin", DATA_BYTES)] {
            let path = root.join(name);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    f.write_all(&vec![0xff; size])?;
                    f.sync_all()?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if fs::metadata(&path)?.len() != size as u64 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "virtual media size differs; preserving file",
                        ));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Self {
            root: root.to_path_buf(),
            _lock: Rc::new(lock),
            fault: Rc::default(),
        })
    }
    fn path(&self, r: Region) -> PathBuf {
        self.root.join(if r == Region::Configuration {
            "configuration.bin"
        } else {
            "data.bin"
        })
    }
    fn check(
        &self,
        r: Region,
        offset: usize,
        length: usize,
        alignment: usize,
    ) -> Result<(), Error> {
        crate::storage::checked_range::<()>(self.geometry(r).capacity, offset, length, alignment)
            .map_err(|_| Error::Invalid)
    }
    fn mutation(
        &mut self,
        r: Region,
        offset: usize,
        bytes: &[u8],
        erase: bool,
    ) -> Result<(), Error> {
        self.check(r, offset, bytes.len(), if erase { 4096 } else { 4 })?;
        let mut fault = self.fault.borrow_mut();
        let failed = fault.after == Some(0);
        if let Some(n) = fault.after.as_mut() {
            *n = n.saturating_sub(1);
        }
        let count = if failed {
            fault.partial.min(bytes.len())
        } else {
            bytes.len()
        };
        if failed {
            fault.after = None;
        }
        drop(fault);
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.path(r))
            .map_err(|_| Error::Failed)?;
        let mut result = vec![0; count];
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| Error::Failed)?;
        file.read_exact(&mut result).map_err(|_| Error::Failed)?;
        for (dst, src) in result.iter_mut().zip(bytes) {
            *dst = if erase { *src } else { *dst & *src };
        }
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| Error::Failed)?;
        file.write_all(&result)
            .and_then(|_| file.sync_data())
            .map_err(|_| Error::Failed)?;
        if failed { Err(Error::Failed) } else { Ok(()) }
    }
}
impl OwnedFlash for FileFlash {
    type Error = Error;
    fn availability(&self, _: Region) -> Availability {
        Availability::Ready
    }
    fn geometry(&self, r: Region) -> crate::storage::Geometry {
        crate::storage::Geometry {
            capacity: if r == Region::Configuration {
                8192
            } else {
                DATA_BYTES
            },
            program_size: 4,
            erase_size: 4096,
        }
    }
    fn read(&mut self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        self.check(r, offset, out.len(), 1)?;
        let mut f = fs::File::open(self.path(r)).map_err(|_| Error::Failed)?;
        f.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| f.read_exact(out))
            .map_err(|_| Error::Failed)
    }
    fn program(&mut self, r: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        self.mutation(r, offset, bytes, false)
    }
    fn erase(&mut self, r: Region, offset: usize, len: usize) -> Result<(), Error> {
        self.check(r, offset, len, 4096)?;
        self.mutation(r, offset, &vec![0xff; len], true)
    }
}
#[derive(Default)]
pub struct PositionFixture {
    pub acquisition: crate::positioning::Acquisition,
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
    fn snapshot(&self, now: u64) -> Option<crate::positioning::Snapshot> {
        (!self.missing).then(|| self.acquisition.snapshot(now))
    }
}
#[derive(Default)]
pub struct BleFixture {
    pub snapshot: crate::ble_transport::Snapshot,
    pub packets: std::collections::VecDeque<crate::ble_transport::Packet>,
}
impl Ble for BleFixture {
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn snapshot(&self) -> crate::ble_transport::Snapshot {
        self.snapshot
    }
    fn take_packet(&mut self) -> Option<crate::ble_transport::Packet> {
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
    fn discoveries(&self) -> [Option<crate::ant::Discovery>; 8] {
        [None; 8]
    }
    fn request(&mut self, _: AntOperation, _: u64) -> &'static str {
        "UNSUPPORTED"
    }
    fn channels(&self, _: u64) -> [Option<crate::ant::Snapshot>; crate::ant::CHANNEL_CAPACITY] {
        [None; crate::ant::CHANNEL_CAPACITY]
    }
    fn take_packet(&mut self) -> Option<crate::ant::Packet> {
        None
    }
}
pub struct NoInputObservation;
impl InputObservation for NoInputObservation {
    fn snapshot(&self, _: u64) -> Option<InputSnapshot> {
        None
    }
}
pub type VirtualShell = crate::shell::Shell<DisplayDevice, InputDevice, PowerDevice, FileFlash>;
pub struct Session {
    pub shell: VirtualShell,
    pub now: u64,
    pub boot: u32,
    pub position: PositionFixture,
    pub ble: BleFixture,
    wifi_control: crate::connectivity::ControlStatus,
    ble_control: crate::connectivity::ControlStatus,
    media: FileFlash,
    display: DisplayDevice,
    #[cfg(feature = "cycling")]
    pub sdk: crate::sdk_runtime::Runtime,
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
        *media.fault.borrow_mut() = Fault::default();
        let boot_path = media.root.join("boot");
        let previous = match fs::read_to_string(&boot_path) {
            Ok(s) => s.trim().parse::<u32>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid virtual boot counter")
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        let boot = previous
            .checked_add(1)
            .ok_or_else(|| io::Error::other("boot counter exhausted"))?;
        fs::write(boot_path, boot.to_string())?;
        let display = DisplayDevice::new(width, height);
        let input = InputDevice::new(
            &[Button::TopLeft, Button::BottomLeft, Button::BottomRight],
            Availability::Ready,
        );
        let mut shell = crate::shell::Shell::new(
            display.clone(),
            input,
            PowerDevice::new(Availability::Ready),
            media.clone(),
            0,
        )
        .map_err(|_| io::Error::other("shell unavailable"))?;
        #[cfg(feature = "debug-harness")]
        {
            shell.harness = crate::harness::State::new(boot);
            shell.harness.buffer = Some(crate::harness::CaptureBuffer::Owned(
                vec![0u8; crate::harness::FRAMES * width * height * 2].into_boxed_slice(),
            ));
        }
        shell.boot_id = boot;
        shell.present();
        #[cfg(feature = "cycling")]
        let profile = if shell.settings.ble.is_some() {
            crate::sdk::ble_sensor::Profile::from_u8(shell.settings.ble_profile)
        } else {
            crate::sdk::ble_sensor::Profile::HeartRate
        };
        Ok(Self {
            shell,
            now: 0,
            boot,
            position: PositionFixture::default(),
            ble: BleFixture::default(),
            wifi_control: crate::connectivity::ControlStatus::new(),
            ble_control: crate::connectivity::ControlStatus::new(),
            media,
            display,
            #[cfg(feature = "cycling")]
            sdk: crate::sdk_runtime::Runtime::new(profile),
        })
    }
    pub fn tick(&mut self) {
        self.shell.observe_position(&self.position, self.now);
        self.shell.tick(self.now);
        self.shell.present();
        #[cfg(feature = "cycling")]
        self.sdk.tick(
            &mut self.shell.store.data(),
            self.now,
            &mut NoAnt,
            &mut self.ble,
            &self.position,
            false,
            &NoInputObservation,
            crate::companion_sensors::State::default().snapshot(self.now, 5_000),
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
        if let Some(status) =
            crate::shell::commands::execute(request.command, &mut self.shell, self.now, out)
        {
            if matches!(
                request.command,
                Command::Harness(crate::harness::Command {
                    operation: crate::harness::Operation::Caps,
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
            Command::Status => crate::shell::commands::status(&self.shell, self.now, out),
            Command::Storage => {
                let _ = write!(out, "{:?}", self.shell.store.data().geometry());
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
                self.wifi_control.operation = crate::connectivity::Operation::Completed;
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
                self.ble.snapshot.link = crate::ble_transport::Link::Off;
                self.ble_control.sequence = self.ble_control.sequence.wrapping_add(1);
                self.ble_control.operation = crate::connectivity::Operation::Completed;
                #[cfg(feature = "cycling")]
                self.sdk.select_profile(
                    crate::sdk::ble_sensor::Profile::Echo,
                    self.ble.snapshot.connections,
                );
                return "ACCEPTED";
            }
            #[cfg(feature = "cycling")]
            Command::BleSelect(peer, profile) => {
                let profile = crate::sdk::ble_sensor::Profile::from_u8(profile);
                let previous = self.shell.settings;
                self.shell.settings.ble =
                    crate::sdk::ble::selection(profile, peer.name.bytes(), peer.address);
                self.shell.settings.ble_profile = profile as u8;
                if !self.shell.save_connectivity() {
                    self.shell.settings = previous;
                    return "FAILED";
                }
                self.ble.packets.clear();
                self.ble.snapshot.link = crate::ble_transport::Link::Off;
                self.sdk
                    .select_profile(profile, self.ble.snapshot.connections);
                self.ble_control.sequence = self.ble_control.sequence.wrapping_add(1);
                self.ble_control.operation = crate::connectivity::Operation::Completed;
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
                    &mut self.shell.store.data(),
                    self.now,
                    out,
                    &NoAnt,
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
                        self.ble.snapshot.link = crate::ble_transport::Link::Connected;
                        self.ble.snapshot.connections += 1;
                        self.ble.packets.clear();
                    }
                    Some("DISCONNECT") => {
                        self.ble.snapshot.link = crate::ble_transport::Link::Off;
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
                        if self.ble.snapshot.link != crate::ble_transport::Link::Connected {
                            return "UNAVAILABLE";
                        }
                        self.ble.snapshot.notifications += 1;
                        if self.ble.packets.len() == 4 {
                            self.ble.snapshot.dropped += 1;
                        } else {
                            let mut bytes = [0; 60];
                            bytes[1] = bpm;
                            self.ble.packets.push_back(crate::ble_transport::Packet {
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
            match if line.len() > crate::terminal_protocol::MAX_LINE {
                Err(crate::terminal_protocol::Error::Overlong)
            } else {
                crate::terminal_protocol::parse(&line)
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
