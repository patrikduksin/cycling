//! Deterministic owned devices for the portable shell. No physical timing claims.
use device_api::display::Display;
use device_api::display::Geometry;
use device_api::input::Button;
use device_api::input::Controls;
use device_api::input::Edge;
use device_api::input::Input;
use device_api::input::InputSource;
use device_api::observation::Availability;
use device_api::observation::Error;
use device_api::positioning::Positioning;
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use device_api::storage::Region;
use firmware_services::input::Edges;
use std::cell::RefCell;
use std::rc::Rc;
use std::vec;
use std::vec::Vec;

#[derive(Clone)]
pub struct DisplayDevice(pub Rc<RefCell<Frame>>);
pub struct Frame {
    pub geometry: Geometry,
    pub pixels: Vec<u16>,
    pub submissions: u32,
    pub fail_next: bool,
}
impl DisplayDevice {
    pub fn new(width: usize, height: usize) -> Self {
        Self(Rc::new(RefCell::new(Frame {
            geometry: Geometry { width, height },
            pixels: vec![0; width * height],
            submissions: 0,
            fail_next: false,
        })))
    }
}
impl Display for DisplayDevice {
    fn geometry(&self) -> Geometry {
        self.0.borrow().geometry
    }
    fn submit(&mut self, mut row: impl FnMut(usize, &mut [u16])) -> Result<(), Error> {
        let mut frame = self.0.borrow_mut();
        if core::mem::take(&mut frame.fail_next) {
            return Err(Error::Failed);
        }
        let width = frame.geometry.width;
        for (y, pixels) in frame.pixels.chunks_exact_mut(width).enumerate() {
            row(y, pixels);
        }
        frame.submissions += 1;
        Ok(())
    }
}
#[derive(Clone)]
pub struct InputDevice {
    pub edges: Rc<RefCell<Edges>>,
    pub controls: Controls,
}
impl InputDevice {
    pub fn new(buttons: &'static [Button], touch: Availability) -> Self {
        Self {
            edges: Rc::new(RefCell::new(Edges::new())),
            controls: Controls { touch, buttons },
        }
    }
    pub fn push(&self, now: u64, event: Input) {
        self.edges.borrow_mut().push(now, event);
    }
}
impl InputSource for InputDevice {
    fn controls(&self) -> Controls {
        self.controls
    }
    fn take_edge(&mut self) -> Option<Edge> {
        self.edges.borrow_mut().pop()
    }
}
#[derive(Clone)]
pub struct PowerDevice(pub Rc<RefCell<PowerState>>);
pub struct PowerState {
    pub available: Availability,
    pub battery: Option<(u8, u16, u64)>,
    pub brightness: u8,
    pub writes: u32,
    pub fail: bool,
}
impl PowerDevice {
    pub fn new(available: Availability) -> Self {
        Self(Rc::new(RefCell::new(PowerState {
            available,
            battery: None,
            brightness: 0,
            writes: 0,
            fail: false,
        })))
    }
}
impl Power for PowerDevice {
    fn availability(&self) -> Availability {
        self.0.borrow().available
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        self.0.borrow().battery
    }
    fn brightness(&mut self, percent: u8) -> Result<(), Error> {
        let mut state = self.0.borrow_mut();
        state.writes += 1;
        if state.available == Availability::Unsupported {
            return Err(Error::Unsupported);
        }
        if state.fail {
            return Err(Error::Failed);
        }
        state.brightness = percent;
        Ok(())
    }
}
/// The simulator has no positioning receiver or radios.
pub struct NoPosition;
impl Positioning for NoPosition {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn snapshot(&self, _: u64) -> Option<device_api::positioning::Snapshot> {
        None
    }
}
pub struct Memory {
    pub sectors: [[u8; 4096]; 2],
    pub available: Availability,
    pub fail: bool,
}
impl Default for Memory {
    fn default() -> Self {
        Self {
            sectors: [[0xff; 4096]; 2],
            available: Availability::Ready,
            fail: false,
        }
    }
}
impl Memory {
    fn check(&self) -> Result<(), Error> {
        match self.available {
            Availability::Unsupported => Err(Error::Unsupported),
            Availability::Ready if !self.fail => Ok(()),
            _ => Err(Error::Failed),
        }
    }
}
impl OwnedFlash for Memory {
    type Error = Error;
    fn availability(&self, region: Region) -> Availability {
        if region == Region::Configuration {
            self.available
        } else {
            Availability::Unsupported
        }
    }
    fn geometry(&self, region: Region) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: if region == Region::Configuration {
                8192
            } else {
                0
            },
            program_size: 4,
            erase_size: 4096,
        }
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        if region != Region::Configuration {
            return Err(Error::Unsupported);
        }
        self.check()?;
        device_api::storage::checked_range::<()>(
            self.geometry(region).capacity,
            offset,
            output.len(),
            1,
        )
        .map_err(|_| Error::Invalid)?;
        for (i, byte) in output.iter_mut().enumerate() {
            *byte = self.sectors[(offset + i) / 4096][(offset + i) % 4096];
        }
        Ok(())
    }
    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        if region != Region::Configuration {
            return Err(Error::Unsupported);
        }
        self.check()?;
        device_api::storage::checked_range::<()>(
            self.geometry(region).capacity,
            offset,
            bytes.len(),
            4,
        )
        .map_err(|_| Error::Invalid)?;
        for (i, byte) in bytes.iter().enumerate() {
            self.sectors[(offset + i) / 4096][(offset + i) % 4096] &= *byte;
        }
        Ok(())
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Error> {
        if region != Region::Configuration {
            return Err(Error::Unsupported);
        }
        self.check()?;
        device_api::storage::checked_range::<()>(
            self.geometry(region).capacity,
            offset,
            length,
            4096,
        )
        .map_err(|_| Error::Invalid)?;
        for i in offset..offset + length {
            self.sectors[i / 4096][i % 4096] = 0xff;
        }
        Ok(())
    }
}
/// Buzzer patterns as C606 advertises them.
const PATTERNS: &[device_api::sound::Pattern] = &[
    device_api::sound::Pattern {
        id: 0,
        frequency_hz: 3000,
        nominal_ms: 75,
        guard_ms: 500,
    },
    device_api::sound::Pattern {
        id: 10,
        frequency_hz: 4000,
        nominal_ms: 100,
        guard_ms: 500,
    },
    device_api::sound::Pattern {
        id: 21,
        frequency_hz: 2500,
        nominal_ms: 150,
        guard_ms: 500,
    },
    device_api::sound::Pattern {
        id: 22,
        frequency_hz: 3000,
        nominal_ms: 300,
        guard_ms: 700,
    },
];
/// The production player, with each submitted pattern recorded instead of played.
#[derive(Default)]
pub struct Speaker {
    player: firmware_services::sound::Player,
    pub played: Vec<(u64, u8)>,
}
impl Speaker {
    pub fn tick(&mut self, now: u64) {
        let played = &mut self.played;
        self.player.dispatch(now, |id| {
            if let Some(id) = id {
                played.push((now, id));
            }
            Ok(())
        });
    }
}
impl device_api::sound::Sound for Speaker {
    fn patterns(&self) -> &'static [device_api::sound::Pattern] {
        PATTERNS
    }
    fn snapshot(&self) -> device_api::sound::Snapshot {
        self.player.snapshot()
    }
    fn play(&mut self, id: u8, now: u64) -> Result<(), Error> {
        let pattern = PATTERNS.iter().find(|p| p.id == id).ok_or(Error::Invalid)?;
        self.player.play(*pattern, now)
    }
    fn stop(&mut self, now: u64) -> Result<(), Error> {
        self.player.stop(now)
    }
}
/// Companion battery and button observations as VANA samples them.
#[derive(Clone, Copy, Default)]
pub struct Battery {
    /// Percent and millivolts, or no report.
    pub level: Option<(u8, u16)>,
    pub charging: bool,
}
impl device_api::input::InputObservation for Battery {
    fn snapshot(&self, now: u64) -> Option<device_api::input::InputSnapshot> {
        let battery = match self.level {
            Some(value) => device_api::observation::Observation::Fresh {
                value,
                received_ms: now,
            },
            None => device_api::observation::Observation::Unavailable,
        };
        Some(device_api::input::InputSnapshot {
            battery,
            // Stock treats power status zero as USB charging; 7 is the
            // status observed on battery.
            power: device_api::observation::Observation::Fresh {
                value: if self.charging { 0 } else { 7 },
                received_ms: now,
            },
            button_counts: [0; 3],
            touch_available: false,
            touch_errors: 0,
            companion_valid: 0,
            companion_bad_crc: 0,
            uart_errors: 0,
            input_lost: 0,
        })
    }
}

/// Preparation the C606 runs between accepting shutdown and going dark: it
/// quiesces Wi-Fi, BLE, ANT, GNSS and sound, then darkens the backlight.
/// Modelled, not measured; see the power-flow PR for device timings.
pub const SHUTDOWN_PREPARE_MS: u64 = 500;

/// Coordinated power control as C606 composition sees it
/// (`devices/c606/src/capabilities/power.rs`): work is not ready while the
/// companion starts, and an accepted shutdown closes work until power is gone.
pub struct PowerSwitch {
    pub now: u64,
    pub status: device_api::power::Status,
    /// The companion finished starting, as `Startup::{Ready, AlreadyRunning}` report.
    pub startup_ready: bool,
    /// Degraded companion startup keeps work ready but refuses shutdown.
    pub startup_running: bool,
}
impl Default for PowerSwitch {
    fn default() -> Self {
        Self {
            now: 0,
            status: device_api::power::Status::IDLE,
            startup_ready: true,
            startup_running: true,
        }
    }
}
impl PowerSwitch {
    /// When an accepted shutdown has finished preparing and the device is off.
    pub fn off_due(&self) -> bool {
        self.status.state == device_api::power::State::Requested
            && self
                .status
                .prepare_at_ms
                .is_some_and(|at| self.now >= at + SHUTDOWN_PREPARE_MS)
    }
}
impl device_api::power::PowerControl for PowerSwitch {
    fn capabilities(&self) -> device_api::power::Capabilities {
        use device_api::power::Support;
        device_api::power::Capabilities {
            shutdown: Support::Supported,
            ..device_api::power::Capabilities::UNSUPPORTED
        }
    }
    fn status(&self) -> device_api::power::Status {
        let mut status = self.status;
        if status.state == device_api::power::State::Idle && !self.startup_running {
            status.state = device_api::power::State::Initializing;
            status.ready = false;
        }
        status
    }
    fn request(&mut self, operation: device_api::power::Operation) -> Result<(), Error> {
        if operation != device_api::power::Operation::Shutdown {
            return Err(Error::Unsupported);
        }
        if !self.startup_ready || !self.status().ready {
            return Err(Error::Unavailable);
        }
        self.status = device_api::power::Status {
            sequence: self.status.sequence.wrapping_add(1),
            operation: Some(operation),
            state: device_api::power::State::Requested,
            at_ms: self.now,
            prepare_at_ms: Some(self.now),
            failure: None,
            ready: false,
        };
        Ok(())
    }
}
