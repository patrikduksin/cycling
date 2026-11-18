//! Headless backend: production parser, shell handlers, journals and SDK runtime.
use crate::ant::SimulatedAnt;
use crate::compare::Tap;
use crate::devices::Battery;
use crate::devices::DisplayDevice;
use crate::devices::InputDevice;
use crate::devices::Model;
use crate::devices::PowerDevice;
use crate::devices::PowerSwitch;
use crate::devices::Scripted;
use crate::devices::Speaker;
use crate::gps::Receiver;
use crate::replay::{Next, Options, Report};
use crate::storage::{FileFlash, Volatile};
use crate::twin::flash::{Flash, ScriptedFlash};
use crate::twin::world::{Left, Recorded};
use crate::world::World;
use device_api::Absent;
use device_api::ant::Ant;
use device_api::ble_transport::Ble;
use device_api::bulk::ReadWrite;
use device_api::display::DisplayObserved;
use device_api::input::InputObservation;
use device_api::input::InputSnapshot;
use device_api::input::InputSourceObserved;
use device_api::journal::Latest;
use device_api::observation::Availability;
use device_api::observation::Error;
use device_api::observation::Observation;
use device_api::positioning::Positioning;
use device_api::power::PowerObserved;
use device_api::sensors::Startup;
use device_api::server::AccessPoint;
use device_api::storage::OwnedFlashObserved;
use device_api::{power::PowerControl, server::Server, sound::Sound};
use firmware_console::protocol::Command;
use firmware_console::protocol::Request;
use firmware_services::trace::log::Log;
use firmware_services::trace::media::Volumes;
use firmware_services::trace::origin::{self, Memory};
use firmware_services::trace::recorder::{Recorder, Sink};
use firmware_services::trace::redact;
use firmware_services::trace::spool::{self, Spool, Staging};
use firmware_services::trace::tail;
use firmware_services::trace::{Activity, Consumers, Event, Gnss, Health, Vitals};
use firmware_shell::storage::{Error as StoreError, Loaded, Settings, Store};
use std::cell::RefCell;
use std::cell::RefMut;
use std::fmt::Write as _;
use std::io;
use std::io::Read;
use std::io::Write;
use std::path::Path;
use std::rc::Rc;
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
use device_api::ble_transport::Link;
/// A heart-rate strap seen through the C606 BLE client
/// (`devices/c606/src/drivers/bluetooth.rs`). It only connects with a stored
/// selection: a 10 s scan, then connection and discovery. In range it notifies
/// Heart Rate Measurements at 1 Hz. Out of range, notifications stop and the
/// link drops after a supervision timeout; reconnects wait the production
/// backoff between scans. Connection and supervision times are assumptions.
#[derive(Default)]
pub struct BleStrap {
    pub present: bool,
    /// The raw Battery Level the strap reports, or none when it has no
    /// Battery Service. The raw byte so hostile values can be sent.
    pub battery: Option<u8>,
    phase: Phase,
    retry: firmware_services::connectivity::Reconnect,
    next_ms: u64,
    battery_due_ms: u64,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Phase {
    /// No selection, or the device just started.
    #[default]
    Idle,
    Scanning(u64),
    Connecting(u64),
    Connected,
    /// Out of range on a live link, until supervision gives up.
    Silent(u64),
    /// Backing off before the next scan.
    Waiting(u64),
}
const SCAN_MS: u64 = 10_000;
const CONNECT_MS: u64 = 1_500;
const SUPERVISION_MS: u64 = 4_000;
/// The C606 BLE client re-reads Battery Level this often.
const BATTERY_REREAD_MS: u64 = 60_000;
/// The C606 BLE client queues this many packets and drops newer ones.
const PACKET_QUEUE: usize = 4;
impl BleStrap {
    pub fn new() -> Self {
        Self {
            present: true,
            ..Self::default()
        }
    }
    /// The device restarted; its BLE client starts over.
    fn reset(&mut self) {
        self.phase = Phase::Idle;
        self.retry = Default::default();
    }
    fn back_off(&mut self, now: u64, ble: &mut BleFixture) {
        ble.snapshot.link = Link::Failed;
        self.phase = Phase::Waiting(now + self.retry.delay() * 1000);
    }
    fn tick(
        &mut self,
        world: &crate::world::World,
        now: u64,
        selected: bool,
        ble: &mut BleFixture,
    ) {
        if !selected {
            if self.phase != Phase::Idle {
                self.reset();
                ble.packets.clear();
            }
            return;
        }
        // Selecting a peer resets the link; the client starts over.
        if matches!(self.phase, Phase::Connected | Phase::Silent(_))
            && ble.snapshot.link != Link::Connected
        {
            self.reset();
        }
        match self.phase {
            Phase::Idle => {
                self.retry.connect();
                self.phase = Phase::Scanning(now + SCAN_MS);
                ble.snapshot.link = Link::Scanning;
            }
            Phase::Waiting(until) if now >= until => {
                self.phase = Phase::Scanning(now + SCAN_MS);
                ble.snapshot.link = Link::Scanning;
            }
            Phase::Scanning(until) if now >= until => {
                if self.present {
                    self.phase = Phase::Connecting(now + CONNECT_MS);
                    ble.snapshot.link = Link::Connecting;
                } else {
                    self.back_off(now, ble);
                }
            }
            Phase::Connecting(until) if now >= until => {
                if self.present {
                    self.phase = Phase::Connected;
                    self.retry.recovered();
                    ble.snapshot.link = Link::Connected;
                    ble.snapshot.connections += 1;
                    ble.packets.clear();
                    ble.battery = None;
                    self.battery_due_ms = now;
                } else {
                    self.back_off(now, ble);
                }
            }
            Phase::Connected if !self.present => self.phase = Phase::Silent(now + SUPERVISION_MS),
            Phase::Silent(_) if self.present => self.phase = Phase::Connected,
            Phase::Silent(until) if now >= until => {
                ble.snapshot.disconnections += 1;
                ble.packets.clear();
                self.phase = Phase::Waiting(now + self.retry.delay() * 1000);
                ble.snapshot.link = Link::Retrying;
            }
            _ => {}
        }
        if self.phase != Phase::Connected || world.time_ms < self.next_ms {
            return;
        }
        if now >= self.battery_due_ms {
            self.battery_due_ms = now + BATTERY_REREAD_MS;
            ble.battery = self
                .battery
                .map(|raw| device_api::ble_transport::BatteryLevel {
                    connection: ble.snapshot.connections,
                    raw,
                    received_ms: now,
                });
        }
        self.next_ms = world.time_ms - world.time_ms % 1000 + 1000;
        ble.snapshot.notifications += 1;
        if ble.packets.len() >= PACKET_QUEUE {
            ble.snapshot.dropped += 1;
            return;
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
    pub battery: Option<device_api::ble_transport::BatteryLevel>,
    pub control: device_api::connectivity::ControlStatus,
}
impl Ble for BleFixture {
    fn request(&mut self, operation: device_api::ble_transport::Operation) -> Result<(), Error> {
        let device_api::ble_transport::Operation::Select(_) = operation else {
            return Err(Error::Unsupported);
        };
        self.packets.clear();
        self.snapshot.link = Link::Off;
        self.control.sequence = self.control.sequence.wrapping_add(1);
        self.control.operation = device_api::connectivity::Operation::Completed;
        Ok(())
    }
    fn control(&self) -> device_api::connectivity::ControlStatus {
        self.control
    }
    fn discoveries(&self) -> [Option<device_api::ble_transport::Discovery>; 8] {
        [None; 8]
    }
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn snapshot(&self) -> device_api::ble_transport::Snapshot {
        self.snapshot
    }
    fn take_packet(&mut self) -> Option<device_api::ble_transport::Packet> {
        self.packets.pop_front()
    }
    fn take_battery(&mut self) -> Option<device_api::ble_transport::BatteryLevel> {
        self.battery.take()
    }
}
#[derive(Default)]
pub struct WifiFixture {
    pub control: device_api::connectivity::ControlStatus,
    pub online: bool,
}
impl device_api::connectivity::Wifi for WifiFixture {
    fn request(&mut self, operation: device_api::connectivity::WifiOperation) -> Result<(), Error> {
        let device_api::connectivity::WifiOperation::Configure(_) = operation else {
            return Err(Error::Unsupported);
        };
        self.control.sequence = self.control.sequence.wrapping_add(1);
        self.control.operation = device_api::connectivity::Operation::Completed;
        Ok(())
    }
    fn control(&self) -> device_api::connectivity::ControlStatus {
        self.control
    }
    fn discoveries(&self) -> [Option<device_api::connectivity::NetworkDiscovery>; 8] {
        [None; 8]
    }
    fn state(&self) -> device_api::connectivity::WifiState {
        device_api::connectivity::WifiState::Unconfigured
    }
    fn online(&self) -> bool {
        self.online
    }
    fn stats(&self) -> device_api::connectivity::WifiStats {
        device_api::connectivity::WifiStats::default()
    }
}
struct UnrecordedConsolePower(device_api::power::Status);
impl PowerControl for UnrecordedConsolePower {
    fn capabilities(&self) -> device_api::power::Capabilities {
        device_api::power::Capabilities::UNSUPPORTED
    }
    fn status(&self) -> device_api::power::Status {
        self.0
    }
    fn request(&mut self, _: device_api::power::Operation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn request_after(&mut self, _: device_api::power::Operation, _: u32) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}
const ASSUMED_BAROMETER_MS: u64 = 100;
/// The companion sends ambient light about three times a second.
const ASSUMED_LIGHT_MS: u64 = 333;
/// Uptime before a powered-on companion reports ready. Modelled on C606
/// startup: three seconds of passive observation before the startup
/// acknowledgment, then an operating reason and two advancing sensor samples.
pub const COMPANION_BOOT_MS: u64 = 3_300;
/// How long C606's startup check watches a running companion after a
/// main-CPU restart before it admits work: one passive probe window.
pub const RESTART_PROBE_MS: u64 = 3_000;
pub type VirtualShell = firmware_shell::shell::Shell<
    device_api::display::DisplayRecorder<DisplayDevice, Journal, Kept<DisplayObserved>>,
    device_api::input::InputSourceRecorder<InputDevice, Journal, Kept<InputSourceObserved>>,
    device_api::power::PowerRecorder<PowerDevice, Journal, Kept<PowerObserved>>,
    device_api::storage::OwnedFlashRecorder<Flash, Journal, Kept<OwnedFlashObserved>>,
>;

/// The session's world log: every recorder of the composition writes to it.
pub type TraceRecorder = Log<Box<[u8]>, Staged, Box<firmware_services::trace::media::Memory>>;
/// The log as the shell's own recorders hold it.
pub type Journal = Rc<RefCell<TraceRecorder>>;
/// What a recorder the shell owns keeps between passes.
pub type Kept<T> = Rc<RefCell<T>>;

#[derive(Default)]
struct Observed {
    ant: RefCell<device_api::ant::AntObserved>,
    ble: RefCell<device_api::ble_transport::BleObserved>,
    bulk: RefCell<device_api::bulk::ReadWriteObserved>,
    access_point: RefCell<device_api::server::AccessPointObserved>,
    server: RefCell<device_api::server::ServerObserved>,
    sound: RefCell<device_api::sound::SoundObserved>,
    control: RefCell<device_api::power::PowerControlObserved>,
    position: RefCell<device_api::positioning::PositioningObserved>,
    companion: RefCell<device_api::input::InputObservationObserved>,
    sensors: RefCell<device_api::sensors::SensorsObserved>,
    wifi: RefCell<device_api::connectivity::WifiObserved>,
}

/// The sensors a pass reads, as the recorder samples them: the simulator
/// has no sensor device, only what its world gives each pass.
struct Sensed {
    startup: Startup,
    physical: Option<device_api::sensors::Snapshot>,
    light: Observation<device_api::sensors::Light>,
    fitted: device_api::sensors::SensorSet,
}

impl device_api::sensors::Sensors for Sensed {
    fn startup_status(&self) -> Startup {
        self.startup
    }
    fn startup_reason(&self) -> Option<u8> {
        None
    }
    fn snapshot(&self, _: u64) -> device_api::sensors::Snapshot {
        self.physical.unwrap_or(device_api::sensors::Snapshot {
            pressure: Observation::Unavailable,
            motion: [Observation::Unavailable; 2],
            identity: Observation::Unavailable,
            reports: 0,
            invalid_reports: 0,
            losses: 0,
        })
    }
    fn light(&self, _: u64) -> Observation<device_api::sensors::Light> {
        self.light
    }
    fn identity_status(&self) -> device_api::sensors::IdentityQuery {
        device_api::sensors::IdentityQuery::Idle
    }
    fn query_identity(&mut self, _: u64) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn fitted(&self) -> device_api::sensors::SensorSet {
        self.fitted
    }
}

/// The trace's sealed blocks: kept for a capture, and staged for its file
/// while the card's STORE volume records it, as the C606 stages them. A
/// block the spool has no room for is lost from both, except in a replay.
#[derive(Default)]
pub struct Staged {
    kept: Vec<u8>,
    spool: Option<Spool<Vec<u8>>>,
    capture: Capture,
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Capture {
    #[default]
    SpoolAdmits,
    KeepsEveryBlock,
}

impl Sink for Staged {
    fn block(&mut self, bytes: &[u8]) -> bool {
        let spooled = self.spool.as_mut().is_none_or(|spool| spool.block(bytes));
        let taken = spooled || self.capture == Capture::KeepsEveryBlock;
        if taken {
            self.kept.extend_from_slice(bytes);
        }
        taken
    }
}

impl Staging for Staged {
    type Ring = Vec<u8>;
    fn spool(&mut self) -> &mut Spool<Vec<u8>> {
        self.spool.get_or_insert_with(|| Spool::new(Vec::new()))
    }
}

fn record_synthetic(shell: &mut VirtualShell, recorder: &RefCell<TraceRecorder>) {
    for input in shell.take_synthetic() {
        recorder.borrow_mut().record(&Event::Synthetic(input));
    }
}

/// What a pass read of the companion, kept while the pass holds the
/// devices.
#[derive(Clone, Copy)]
struct CompanionView(Option<InputSnapshot>);

impl InputObservation for CompanionView {
    fn snapshot(&self, _: u64) -> Option<InputSnapshot> {
        self.0
    }
}

impl CompanionView {
    fn battery(&self) -> Option<(u8, u16, u64)> {
        match self.0?.battery {
            Observation::Fresh {
                value: (percent, mv),
                received_ms,
            }
            | Observation::Stale {
                value: (percent, mv),
                received_ms,
            } => Some((percent, mv, received_ms)),
            Observation::Unavailable => None,
        }
    }
}

/// What a pass read of the position, kept while the pass holds the devices.
#[derive(Clone, Copy)]
struct PositionView {
    availability: Availability,
    snapshot: Option<device_api::positioning::Snapshot>,
}

impl Positioning for PositionView {
    fn availability(&self) -> Availability {
        self.availability
    }
    fn snapshot(&self, _: u64) -> Option<device_api::positioning::Snapshot> {
        self.snapshot
    }
}

/// The buzzer, noting each pattern it accepted: the trace names the cue a
/// pattern played for.
struct Started<'a, S> {
    sound: S,
    started: &'a mut Vec<u8>,
}

impl<S: Sound> Sound for Started<'_, S> {
    fn patterns(&self) -> &'static [device_api::sound::Pattern] {
        self.sound.patterns()
    }
    fn snapshot(&self) -> device_api::sound::Snapshot {
        self.sound.snapshot()
    }
    fn play(&mut self, id: u8, now: u64) -> Result<(), Error> {
        let played = self.sound.play(id, now);
        if played.is_ok() {
            self.started.push(id);
        }
        played
    }
    fn stop(&mut self, now: u64) -> Result<(), Error> {
        self.sound.stop(now)
    }
}

struct Trace {
    recorder: Journal,
    observed: Rc<Observed>,
    network_time: Latest<firmware_services::network_time::Snapshot>,
    startup: Startup,
    status: device_api::power::Status,
    started: Vec<u8>,
    start: Start,
    file: spool::File,
    spooling: bool,
    vitals: Vitals,
    window: Window,
    /// What a panic or a watchdog reset left of the session before: its
    /// spool's ring and its open block, until the file stages.
    remains: Option<(Vec<u8>, Vec<u8>)>,
}

/// What the current activity window has counted so far.
#[derive(Clone, Copy, Default)]
struct Window {
    start_ms: u64,
    passes: u32,
    /// The display's frame count as the window began.
    frames: u32,
}

struct Start {
    device: &'static str,
    boot: u32,
    redacted: bool,
    terrain: origin::Found<origin::Terrain>,
}

impl Start {
    fn read_terrain(&mut self, bulk: &crate::bulk::SimBulk) {
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        self.terrain = origin::terrain(&mut bulk.host(), &mut sector);
    }

    fn header(&self) -> Vec<u8> {
        let stamp = origin::Stamp {
            build: crate::compare::build_hash(),
            device: self.device,
            boot: self.boot,
        };
        let mut out = vec![0u8; 256];
        let len = origin::header(stamp, self.terrain, &mut out).expect("the header fits a block");
        out.truncate(len);
        out
    }
}

impl Trace {
    fn log(replay: bool) -> Journal {
        let buffer = vec![0u8; tail::BLOCK].into_boxed_slice();
        let staged = Staged {
            capture: match replay {
                true => Capture::KeepsEveryBlock,
                false => Capture::SpoolAdmits,
            },
            ..Staged::default()
        };
        let recorder = Recorder::new(buffer, staged);
        Rc::new(RefCell::new(Log::new(Some(recorder), Box::default())))
    }

    fn new(start: Start, recorder: Journal) -> Self {
        Self {
            start,
            recorder,
            observed: Rc::default(),
            file: spool::File::Waiting,
            spooling: false,
            remains: None,
            vitals: Vitals::default(),
            window: Window::default(),
            network_time: Latest::default(),
            startup: Startup::Unsupported,
            status: device_api::power::Status::IDLE,
            started: Vec::new(),
        }
    }

    fn record(&self, event: &Event<'_>) {
        self.recorder.borrow_mut().record(event);
    }

    fn blocks(&self) -> RefMut<'_, Recorder<Box<[u8]>, Staged>> {
        RefMut::map(self.recorder.borrow_mut(), |log| {
            log.recorder.as_mut().expect("a session records")
        })
    }

    /// From the first pass, as the C606 spools from power-on, when the
    /// card holds a STORE its trace will stage for: the card does not
    /// change while it is closed.
    fn spool(&mut self, bulk: &crate::bulk::SimBulk) {
        self.spooling = true;
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let file = spool::File::stage(&mut bulk.host(), &mut sector);
        if let spool::File::Staging(..) = file {
            self.blocks().sink_mut().spool = Some(Spool::new(vec![0u8; spool::RING_BYTES]));
        }
    }

    fn stage(&mut self, bulk: &crate::bulk::SimBulk) {
        self.start.read_terrain(bulk);
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        self.file = spool::File::stage(&mut bulk.host(), &mut sector);
        if !matches!(self.file, spool::File::Staging(..)) {
            self.blocks().sink_mut().spool = None;
        }
    }

    /// Match the C606's salvage, drain and horizon ordering so simulated crash recovery observes
    /// the same durable tail.
    fn drain(&mut self, bulk: &crate::bulk::SimBulk, card: spool::Card, now: u64) {
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let mut window = vec![0u8; tail::WINDOW];
        if card != spool::Card::Closed
            && matches!(self.file, spool::File::Staging(..))
            && let Some((ring, open)) = self.remains.take()
        {
            let remains = tail::Remains {
                ring: &ring,
                open: &open,
            };
            if let Ok(blocks) =
                self.file
                    .salvage(&mut bulk.host(), remains, &mut window, &mut sector)
            {
                self.record(&Event::Salvaged { blocks });
            }
        }
        let opened = self.file.is_open();
        let start = &self.start;
        let header = || Some((start.header(), start.boot));
        let mut log = self.recorder.borrow_mut();
        let recorder = log.recorder.as_mut().expect("a session records");
        let passed = self
            .file
            .pass(recorder, &mut bulk.host(), card, header, now, &mut sector);
        if passed.is_err() {
            recorder.sink_mut().spool = None;
        }
        drop(log);
        if opened
            && card != spool::Card::Closed
            && let Some(horizon) = self
                .file
                .horizon(&mut bulk.host(), &mut window, &mut sector)
        {
            self.record(&horizon.event());
        }
    }

    /// What a crash would leave in memory: the spool's ring and the
    /// recorder's open block.
    fn remains(&self) -> Option<(Vec<u8>, Vec<u8>)> {
        let log = self.recorder.borrow();
        let recorder = log.recorder.as_ref()?;
        let ring = recorder.sink().spool.as_ref()?.memory().to_vec();
        Some((ring, recorder.open_memory().to_vec()))
    }

    fn sync(&mut self, bulk: &crate::bulk::SimBulk, now: u64) -> bool {
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let mut log = self.recorder.borrow_mut();
        let recorder = log.recorder.as_mut().expect("a session records");
        let open = self.file.is_open();
        let synced = self.file.sync(recorder, &mut bulk.host(), now, &mut sector);
        if synced.is_err() {
            recorder.sink_mut().spool = None;
        }
        open && synced.is_ok()
    }

    fn after_host(&mut self, bulk: &crate::bulk::SimBulk, now: u64) {
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let mut log = self.recorder.borrow_mut();
        let after = self
            .file
            .after_host(&mut log, &mut bulk.host(), false, now, &mut sector);
        if after.is_err() {
            let recorder = log.recorder.as_mut().expect("a session records");
            recorder.sink_mut().spool = None;
        }
    }

    /// Tell the log which card sectors are TERRAIN and STORE, from the
    /// volume table as `media` reads it: the read is recorded, so a replay
    /// finds the same volumes.
    fn read_volumes(&self, media: &mut dyn ReadWrite) {
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let volumes = Volumes::read(&mut &mut *media, &mut sector);
        self.recorder.borrow_mut().volumes(volumes);
    }

    fn bytes(&self) -> Vec<u8> {
        let mut recorder = self.blocks();
        let mut bytes = self.start.header();
        bytes.extend_from_slice(&recorder.sink().kept);
        bytes.extend_from_slice(recorder.open_block());
        bytes
    }
}

struct Twin<'a> {
    media: &'a mut dyn ReadWrite,
    trace: &'a mut Trace,
    trace_card: crate::bulk::SimBulk,
    startup: Startup,
    network_time: Option<firmware_services::network_time::Anchor>,
}

struct TwinCard<'a> {
    media: &'a mut dyn ReadWrite,
    trace: &'a mut Trace,
    trace_card: &'a crate::bulk::SimBulk,
}

impl firmware_console::commands::Card for TwinCard<'_> {
    fn media(&mut self) -> impl ReadWrite + '_ {
        &mut *self.media
    }
    fn store(&mut self) -> impl ReadWrite + '_ {
        self.trace_card.host()
    }
    fn trace_in_store(&self) -> bool {
        self.trace.file.is_open()
    }
    fn trace_status(&self, out: &mut dyn std::fmt::Write) {
        let recorder = self.trace.blocks();
        self.trace.file.status(recorder.sink().spool.as_ref(), out);
    }
    fn after_volume(&mut self, now: u64) {
        self.trace.after_host(self.trace_card, now);
        self.trace.read_volumes(&mut *self.media);
    }
    fn sync_trace(&mut self, now: u64) {
        self.trace.sync(self.trace_card, now);
    }
}

impl firmware_console::commands::Board<VirtualShell> for Twin<'_> {
    const HARDWARE: &'static str = "";
    fn startup(&self) -> Startup {
        self.startup
    }
    fn card(&mut self) -> impl firmware_console::commands::Card + '_ {
        TwinCard {
            media: &mut *self.media,
            trace: &mut *self.trace,
            trace_card: &self.trace_card,
        }
    }
    fn transfer(&mut self) -> &'static str {
        "UNSUPPORTED"
    }
    fn restart(&mut self, _: u64) {}
    fn identity(&mut self, _: bool) -> firmware_console::commands::Identity {
        firmware_console::commands::Identity {
            board: VIRTUAL,
            slot: "unknown",
            commit: crate::compare::build_hash(),
        }
    }
    fn status(&self, _: &mut dyn std::fmt::Write) {}
    fn clock(&self) -> firmware_services::network_time::Clock {
        use firmware_services::network_time::Clock;
        self.network_time
            .map_or(Clock::Unsynced { syncing: false }, Clock::Anchored)
    }
    fn hardware(
        &mut self,
        _: &mut VirtualShell,
        command: Command,
        _: u64,
        out: &mut dyn std::fmt::Write,
    ) -> &'static str {
        match command {
            Command::Crash => {
                let _ = out.write_str("none");
                "OK"
            }
            Command::CrashClear => "OK",
            _ => "UNSUPPORTED",
        }
    }
}

fn observe_startup(switch: &mut PowerSwitch, now: u64, startup: Startup) {
    switch.now = now;
    switch.recover();
    switch.startup_ready = matches!(
        startup,
        Startup::Ready | Startup::AlreadyRunning | Startup::Unsupported
    );
    switch.startup_running = switch.startup_ready || startup == Startup::RunningDegraded;
}

/// The shell's settings journal. The shell holds it `'static`, as on the
/// device, so each boot lends it a leaked handle, and the session takes the
/// journal back when it ends: a leaked journal would keep the media and its
/// lock.
#[derive(Clone)]
struct Lent(Rc<RefCell<Option<Store<Volatile<FileFlash>>>>>);

impl Lent {
    fn new(flash: Volatile<FileFlash>) -> Self {
        Self(Rc::new(RefCell::new(Some(Store::new(flash)))))
    }
    fn store(&self) -> RefMut<'_, Store<Volatile<FileFlash>>> {
        RefMut::map(self.0.borrow_mut(), |store| {
            store.as_mut().expect("lent while its session lives")
        })
    }
}

impl Settings for Lent {
    fn load(&mut self) -> Result<Loaded, StoreError> {
        self.store().load()
    }
    fn save(
        &mut self,
        settings: firmware_shell::preferences::Settings,
    ) -> Result<firmware_services::storage::Record, StoreError> {
        self.store().save(settings)
    }
    fn save_app(
        &mut self,
        app: firmware_shell::preferences::AppPreferences,
    ) -> Result<firmware_services::storage::Record, StoreError> {
        self.store().save_app(app)
    }
    fn save_connectivity(
        &mut self,
        current: firmware_shell::preferences::Settings,
    ) -> Result<firmware_services::storage::Record, StoreError> {
        self.store().save_connectivity(current)
    }
    fn erase(&mut self) -> Result<(), StoreError> {
        self.store().erase()
    }
}

/// Look-ahead blocks of ended sessions. VANA holds its block as `&'static mut`,
/// as on the device, so each block is leaked once and then handed from one
/// session or boot to the next instead of leaking one per boot.
static TERRAIN_BLOCKS: std::sync::Mutex<Vec<&'static mut vana::app::Terrain>> =
    std::sync::Mutex::new(Vec::new());

/// A look-ahead block as new: a reused one, reset, or a fresh one.
fn terrain_block() -> &'static mut vana::app::Terrain {
    let reused = TERRAIN_BLOCKS
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .pop();
    match reused {
        Some(terrain) => {
            terrain.reset();
            terrain
        }
        None => {
            let arena = Vec::leak(vec![0u8; vana::app::TERRAIN_ARENA_BYTES]);
            Box::leak(Box::new(vana::app::Terrain::new(arena)))
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.settings.0.borrow_mut().take();
        if let Some(terrain) = self.sdk.detach_terrain() {
            TERRAIN_BLOCKS
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(terrain);
        }
    }
}

/// The world and the simulated devices that observe it. A replay has none:
/// its trace answers for them.
pub struct Simulated {
    pub world: World,
    pub position: PositionFixture,
    pub ble: BleFixture,
    /// A BLE heart-rate strap that notifies the world's heart rate once a second.
    pub ble_strap: Option<BleStrap>,
    pub ant: SimulatedAnt,
    pub gps: Option<Receiver>,
    pub speaker: Speaker,
    pub battery: Battery,
    pub startup: Startup,
    /// Wi-Fi time synchronization, as the device's clock owner holds it;
    /// device memory, so a reboot loses it.
    pub network_time: Option<firmware_services::network_time::Anchor>,
    /// Uptime at which a starting companion runs, and the ANT availability then.
    companion_boot_ms: Option<(u64, Availability)>,
    /// How long a main-CPU restart probes a running companion before work
    /// is admitted; zero admits work at once. Kept across reboots.
    pub restart_probe_ms: u64,
    /// Uptime at which this boot's restart probe ends, and the phase then.
    restart_probe: Option<(u64, Startup)>,
    /// Coordinated shutdown and work readiness, as composition reads them.
    pub switch: PowerSwitch,
    barometer: Option<(u64, u32)>,
    pub wifi: WifiFixture,
    pub phone_server: crate::phone::SimServer,
    pub access_point: crate::phone::SimAccessPoint,
    /// The parts the model has.
    fitted: crate::devices::Fitted,
}

fn absent() -> &'static mut Absent {
    // `Absent` has no size, so leaking one allocates nothing.
    Box::leak(Box::new(Absent))
}

impl Simulated {
    fn carry_into(&mut self, next: &mut Simulated, power: bool, uptime: u64) {
        next.world = core::mem::take(&mut self.world);
        let ready = self.ant_ready_availability();
        core::mem::swap(&mut next.ant, &mut self.ant);
        if power {
            next.ant.power_cycled();
            if self.companion_boot_ms.is_some()
                || matches!(self.startup, Startup::Ready | Startup::AlreadyRunning)
            {
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
                .map(|(at, ant)| (at.saturating_sub(uptime), ant));
            let companion_running = next.companion_boot_ms.is_none();
            if companion_running && self.restart_probe_ms > 0 {
                next.restart_probe = Some((self.restart_probe_ms, next.startup));
                next.startup = Startup::Probing;
            }
        }
        next.restart_probe_ms = self.restart_probe_ms;
        next.gps = self.gps.take().map(|mut gps| {
            gps.reset();
            gps
        });
        next.position.missing = self.position.missing;
        next.battery = self.battery;
        next.ble_strap = self.ble_strap.take().map(|mut strap| {
            strap.reset();
            strap
        });
        next.speaker.played = core::mem::take(&mut self.speaker.played);
    }
    fn new(fitted: crate::devices::Fitted) -> Self {
        Self {
            world: World::default(),
            position: PositionFixture::default(),
            ble: BleFixture::default(),
            ble_strap: None,
            ant: SimulatedAnt::new(Availability::Unsupported),
            gps: None,
            speaker: Speaker::default(),
            battery: Battery::default(),
            startup: Startup::Unsupported,
            network_time: None,
            companion_boot_ms: None,
            restart_probe_ms: 0,
            restart_probe: None,
            switch: PowerSwitch {
                shutdown: fitted.shutdown,
                ..PowerSwitch::default()
            },
            barometer: None,
            wifi: WifiFixture::default(),
            phone_server: Default::default(),
            access_point: Default::default(),
            fitted,
        }
    }

    fn companion_starting(&mut self, ant: Availability) {
        self.startup = Startup::Probing;
        self.ant.availability = Availability::Initializing;
        self.companion_boot_ms = Some((COMPANION_BOOT_MS, ant));
    }

    fn companion_charging(&mut self) {
        self.startup = Startup::Charging;
        self.ant.availability = self.ant_ready_availability();
        self.companion_boot_ms = None;
        self.switch = PowerSwitch::charging(self.fitted.shutdown);
    }

    fn wake(&mut self, now: u64) {
        self.switch.now = now;
        self.switch.wake();
        let ant = self.ant.availability;
        self.ant.availability = Availability::Initializing;
        self.companion_boot_ms = Some((now + crate::devices::WAKE_MS, ant));
    }

    fn ant_ready_availability(&self) -> Availability {
        match self.companion_boot_ms {
            Some((_, ant)) => ant,
            None => self.ant.availability,
        }
    }

    fn environment(&mut self, now: u64, ble_selected: bool) {
        if let Some((at, ant)) = self.companion_boot_ms
            && now >= at
        {
            self.companion_boot_ms = None;
            self.startup = Startup::Ready;
            self.ant.availability = ant;
        }
        if let Some((at, phase)) = self.restart_probe
            && now >= at
        {
            self.restart_probe = None;
            self.startup = phase;
        }
        self.ant.tick(&self.world, now);
        if let Some(strap) = &mut self.ble_strap {
            strap.tick(&self.world, now, ble_selected, &mut self.ble);
        }
        if let Some(gps) = &mut self.gps {
            gps.tick(&self.world, now, &mut self.position.acquisition);
        }
    }

    fn light(&self, now: u64) -> Observation<device_api::sensors::Light> {
        match self.world.ambient_light {
            Some(raw) => Observation::Fresh {
                value: device_api::sensors::Light(raw),
                received_ms: now - now % ASSUMED_LIGHT_MS,
            },
            None => Observation::Unavailable,
        }
    }

    fn physical(&mut self, now: u64) -> device_api::sensors::Snapshot {
        let slot = now - now % ASSUMED_BAROMETER_MS;
        self.barometer = match (self.world.pressure_centi_pa(), self.barometer) {
            (None, _) => None,
            (Some(_), Some((at, pressure))) if at == slot => Some((at, pressure)),
            (Some(pressure), _) => Some((slot, pressure)),
        };
        let pressure = match self.barometer {
            Some((received_ms, pressure_centi_pa)) => Observation::Fresh {
                value: device_api::sensors::Pressure {
                    pressure_centi_pa,
                    temperature_centi_c: 2000,
                },
                received_ms,
            },
            None => Observation::Unavailable,
        };
        device_api::sensors::Snapshot {
            pressure,
            motion: [Observation::Unavailable; 2],
            identity: Observation::Unavailable,
            reports: 0,
            invalid_reports: 0,
            losses: 0,
        }
    }
}

enum Devices {
    Simulated(Box<Simulated>),
    Twin(Box<Servers>),
}

struct Servers {
    world: Scripted,
    ant: device_api::ant::AntReplay<Scripted>,
    ble: device_api::ble_transport::BleReplay<Scripted>,
    bulk: device_api::bulk::ReadWriteReplay<Scripted>,
    access_point: device_api::server::AccessPointReplay<Scripted>,
    server: device_api::server::ServerReplay<Scripted>,
    sound: device_api::sound::SoundReplay<Scripted>,
    control: device_api::power::PowerControlReplay<Scripted>,
    position: device_api::positioning::PositioningReplay<Scripted>,
    companion: device_api::input::InputObservationReplay<Scripted>,
    sensors: device_api::sensors::SensorsReplay<Scripted>,
    wifi: device_api::connectivity::WifiReplay<Scripted>,
}

impl Servers {
    fn new(world: Scripted) -> Self {
        Self {
            ant: device_api::ant::AntReplay::new(world.clone()),
            ble: device_api::ble_transport::BleReplay::new(world.clone()),
            bulk: device_api::bulk::ReadWriteReplay::new(world.clone()),
            access_point: device_api::server::AccessPointReplay::new(world.clone()),
            server: device_api::server::ServerReplay::new(world.clone()),
            sound: device_api::sound::SoundReplay::new(world.clone()),
            control: device_api::power::PowerControlReplay::new(world.clone()),
            position: device_api::positioning::PositioningReplay::new(world.clone()),
            companion: device_api::input::InputObservationReplay::new(world.clone()),
            sensors: device_api::sensors::SensorsReplay::new(world.clone()),
            wifi: device_api::connectivity::WifiReplay::new(world.clone()),
            world,
        }
    }
}

struct StandIns {
    ant: Absent,
    power: UnrecordedConsolePower,
}

/// The unrecorded input a terminal command reads, besides the radios.
fn terminal_read(command: Command) -> Option<&'static str> {
    if command.is_ant() {
        Some("the ANT radio the terminal reads")
    } else if matches!(command, Command::Time | Command::Input | Command::Battery) {
        Some("the clock, link or companion the terminal reads")
    } else if matches!(
        command,
        Command::Power(..) | Command::PowerShutdownAfter(..)
    ) {
        Some("the power control the terminal reads")
    } else {
        None
    }
}

impl Devices {
    #[allow(clippy::type_complexity)]
    fn raw<'a>(
        &'a mut self,
        bulk: &'a mut crate::bulk::SimBulk,
    ) -> (
        &'a mut dyn Ant,
        &'a mut dyn Ble,
        &'a mut dyn ReadWrite,
        &'a mut dyn AccessPoint,
        &'a mut dyn Server,
        &'a mut dyn Sound,
        &'a mut dyn PowerControl,
    ) {
        match self {
            Devices::Simulated(sim) => {
                let fitted = sim.fitted;
                (
                    if fitted.ant { &mut sim.ant } else { absent() },
                    if fitted.ble { &mut sim.ble } else { absent() },
                    if fitted.card { bulk } else { absent() },
                    if fitted.wifi {
                        &mut sim.access_point
                    } else {
                        absent()
                    },
                    if fitted.wifi {
                        &mut sim.phone_server
                    } else {
                        absent()
                    },
                    if fitted.sound {
                        &mut sim.speaker
                    } else {
                        absent()
                    },
                    &mut sim.switch,
                )
            }
            Devices::Twin(servers) => (
                &mut servers.ant,
                &mut servers.ble,
                &mut servers.bulk,
                &mut servers.access_point,
                &mut servers.server,
                &mut servers.sound,
                &mut servers.control,
            ),
        }
    }

    fn bulk<'a>(&'a mut self, bulk: &'a mut crate::bulk::SimBulk) -> &'a mut dyn ReadWrite {
        match self {
            Devices::Simulated(_) => bulk,
            Devices::Twin(servers) => &mut servers.bulk,
        }
    }

    fn unrecorded_console<'a>(
        &'a mut self,
        bulk: &'a mut crate::bulk::SimBulk,
        stand_ins: &'a mut StandIns,
        command: Command,
        frozen: device_api::power::Status,
    ) -> (
        firmware_console::commands::Capabilities<'a>,
        &'a mut dyn ReadWrite,
    ) {
        match self {
            Devices::Simulated(sim) => {
                let fitted = sim.fitted;
                (
                    firmware_console::commands::Capabilities {
                        ant: if fitted.ant { &mut sim.ant } else { absent() },
                        ble: if fitted.ble { &mut sim.ble } else { absent() },
                        wifi: if fitted.wifi { &mut sim.wifi } else { absent() },
                        power: &mut sim.switch,
                        input: &Absent,
                    },
                    if fitted.card { bulk } else { absent() },
                )
            }
            Devices::Twin(servers) => {
                if firmware_console::protocol::admitted(command, frozen)
                    && let Some(input) = terminal_read(command)
                {
                    servers.world.borrow_mut().unrecorded(input);
                }
                (
                    firmware_console::commands::Capabilities {
                        ant: &mut stand_ins.ant,
                        ble: &mut servers.ble,
                        wifi: &mut servers.wifi,
                        power: &mut stand_ins.power,
                        input: &Absent,
                    },
                    &mut servers.bulk,
                )
            }
        }
    }

    fn companion(
        &self,
        observed: &RefCell<device_api::input::InputObservationObserved>,
        now: u64,
        record: &RefCell<TraceRecorder>,
    ) -> CompanionView {
        let source: &dyn InputObservation = match self {
            Devices::Simulated(sim) => &sim.battery,
            Devices::Twin(servers) => &servers.companion,
        };
        let recorded = device_api::input::InputObservationRecorder::new(source, record, observed);
        CompanionView(recorded.snapshot(now))
    }

    fn position(
        &self,
        observed: &RefCell<device_api::positioning::PositioningObserved>,
        now: u64,
        record: &RefCell<TraceRecorder>,
    ) -> PositionView {
        let source: &dyn Positioning = match self {
            Devices::Simulated(sim) if !sim.fitted.gps => &Absent,
            Devices::Simulated(sim) => &sim.position,
            Devices::Twin(servers) => &servers.position,
        };
        let recorded = device_api::positioning::PositioningRecorder::new(source, record, observed);
        PositionView {
            availability: recorded.availability(),
            snapshot: recorded.snapshot(now),
        }
    }

    /// The power status this pass reads, or `None` once the power is gone.
    fn power(
        &mut self,
        observed: &RefCell<device_api::power::PowerControlObserved>,
        now: u64,
        startup: Startup,
        record: &RefCell<TraceRecorder>,
    ) -> Option<device_api::power::Status> {
        let control: &mut dyn PowerControl = match self {
            Devices::Simulated(sim) => {
                sim.speaker.tick(now);
                observe_startup(&mut sim.switch, now, startup);
                if sim.switch.off_due() {
                    return None;
                }
                &mut sim.switch
            }
            Devices::Twin(servers) => {
                if servers.world.borrow().powered_off() {
                    return None;
                }
                &mut servers.control
            }
        };
        Some(device_api::power::PowerControlRecorder::new(control, record, observed).status())
    }

    /// The power status a terminal command's gate reads.
    fn command_power(
        &mut self,
        observed: &RefCell<device_api::power::PowerControlObserved>,
        now: u64,
        record: &RefCell<TraceRecorder>,
    ) -> device_api::power::Status {
        let control: &mut dyn PowerControl = match self {
            Devices::Simulated(sim) => {
                observe_startup(&mut sim.switch, now, sim.startup);
                &mut sim.switch
            }
            Devices::Twin(servers) => &mut servers.control,
        };
        device_api::power::PowerControlRecorder::new(control, record, observed).status()
    }
}

#[derive(Default)]
struct Boot {
    passes_since_start: u64,
    /// Secrets are replaced from before the shell loads its settings.
    redacted: bool,
    twin: Option<Scripted>,
}

#[derive(Debug)]
pub enum Step {
    Pass,
    Command {
        line: String,
        status: &'static str,
        reply: String,
    },
    End,
}

enum Stopped {
    LeftTheRecording,
    Panicked,
}

/// A sized session has no manifest a replay could rebuild its geometry from.
pub const VIRTUAL: &str = "virtual";

pub struct Session {
    pub model: Model,
    pub shell: VirtualShell,
    /// Device uptime; it restarts from zero with the device.
    pub now: u64,
    pub boot: u32,
    pub input: InputDevice,
    pub power: PowerDevice,
    /// The device turned itself off; only a power-button start brings it back.
    pub off: bool,
    pub display: DisplayDevice,
    pub media: FileFlash,
    settings: Lent,
    /// The owned MMC region; absent unless a scenario fits one.
    pub bulk: crate::bulk::SimBulk,
    pub sdk: vana::app::Runtime,
    trace: Trace,
    trace_is_the_record: bool,
    unrecorded: Option<(&'static str, u64)>,
    unseen_ms: u64,
    rest_ms: u64,
    /// `Edges::pushed` when the last pass began: an edge since ends a rest.
    edges_seen: u32,
    /// A terminal request ran since the last pass, as the C606's terminal
    /// reports traffic.
    host: bool,
    devices: Devices,
    pub(crate) tap: Option<Tap>,
    passes_since_start: u64,
}
impl Session {
    pub fn new(root: &Path, width: usize, height: usize) -> io::Result<Self> {
        if !(16..=320).contains(&width) || !(16..=512).contains(&height) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "geometry must be 16..320 wide and 16..512 tall",
            ));
        }
        let layout = vana::screens::scaled::Scaled::fitting(width, height).map_or(
            &vana::screens::BASE,
            |scaled| {
                Box::leak(Box::new(vana::screens::Layout::of(
                    scaled,
                    &crate::models::C606.buttons,
                )))
            },
        );
        let model = Model {
            name: VIRTUAL,
            width,
            height,
            layout,
            ..crate::models::C606
        };
        Self::with_media(
            model,
            FileFlash::open(root)?,
            crate::bulk::SimBulk::default(),
            Memory::ALL,
            None,
            Boot::default(),
        )
    }
    /// A `model` on memory media whose companion and ANT radio start
    /// shortly after boot.
    pub fn device(model: Model) -> io::Result<Self> {
        Self::simulated(model, None)
    }
    /// Like `device`, folding into `tap` everything comparison mode hashes.
    pub fn comparing(model: Model, tap: Tap) -> io::Result<Self> {
        Self::simulated(model, Some(tap))
    }
    fn simulated(model: Model, tap: Option<Tap>) -> io::Result<Self> {
        let mut session = Self::with_media(
            model,
            FileFlash::memory(),
            crate::bulk::SimBulk::default(),
            Memory::ALL,
            tap,
            Boot::default(),
        )?;
        let sim = session.sim_mut();
        sim.companion_starting(Availability::Ready);
        sim.restart_probe_ms = RESTART_PROBE_MS;
        Ok(session)
    }
    /// The world and the simulated devices. A replay has none.
    pub fn sim(&self) -> &Simulated {
        match &self.devices {
            Devices::Simulated(sim) => sim,
            Devices::Twin(_) => panic!("a replay has no simulated world"),
        }
    }
    pub fn sim_mut(&mut self) -> &mut Simulated {
        match &mut self.devices {
            Devices::Simulated(sim) => sim,
            Devices::Twin(_) => panic!("a replay has no simulated world"),
        }
    }
    pub fn is_replay(&self) -> bool {
        !matches!(self.devices, Devices::Simulated(_))
    }
    fn with_media(
        model: Model,
        media: FileFlash,
        bulk: crate::bulk::SimBulk,
        memory: Memory,
        tap: Option<Tap>,
        boot: Boot,
    ) -> io::Result<Self> {
        let Boot {
            passes_since_start,
            redacted,
            twin,
        } = boot;
        let (width, height) = (model.width, model.height);
        crate::logs::install();
        if tap.is_some() {
            crate::logs::enable_info();
        }
        crate::logs::enter_device(tap.as_ref(), 0);
        let media = media.tapped(tap.clone());
        let bulk = bulk.tapped(tap.clone());
        let boot = media.begin_boot()?;
        bulk.share_power(media.power_lost());
        let display = DisplayDevice::new(width, height);
        let input = model.input();
        let mut input = input;
        let power = PowerDevice::new(Availability::Ready);
        if let Some(world) = &twin {
            let method = |capability, name| {
                let method = crate::twin::script::named(capability, name);
                (world.clone(), method.expect("a method of a shell device"))
            };
            display.0.borrow_mut().refusals.twin = Some(method("Display", "submit"));
            input.twin = Some(method("InputSource", "take_edge"));
            let mut power = power.0.borrow_mut();
            power.refusals.twin = Some(method("Power", "brightness"));
            power.twin = Some(method("Power", "availability"));
        }
        let ram = Volatile::boot(media.device_handle(), &model);
        let settings = Lent::new(ram.handle(media.device_handle()));
        let log = Trace::log(twin.is_some());
        // Before the shell loads its settings, which hold the Wi-Fi password.
        if redacted {
            log.borrow_mut().redact();
        }
        log.borrow_mut().open(memory);
        let mut shell = firmware_shell::shell::Shell::new(
            device_api::display::DisplayRecorder::new(display.clone(), log.clone(), Rc::default()),
            device_api::input::InputSourceRecorder::new(input.clone(), log.clone(), Rc::default()),
            device_api::power::PowerRecorder::new(power.clone(), log.clone(), Rc::default()),
            device_api::storage::OwnedFlashRecorder::new(
                match &twin {
                    Some(world) => Flash::Scripted(ScriptedFlash::new(world.clone())),
                    None => Flash::Simulated(ram),
                },
                log.clone(),
                Rc::default(),
            ),
            match &twin {
                Some(world) => Box::leak(Box::new(firmware_shell::storage::SettingsRecorder::new(
                    firmware_shell::storage::SettingsReplay::new(world.clone()),
                    log.clone(),
                    Kept::<firmware_shell::storage::SettingsObserved>::default(),
                ))) as &'static mut dyn Settings,
                None => Box::leak(Box::new(firmware_shell::storage::SettingsRecorder::new(
                    settings.clone(),
                    log.clone(),
                    Kept::<firmware_shell::storage::SettingsObserved>::default(),
                ))),
            },
            0,
        )
        .map_err(|_| io::Error::other("shell unavailable"))?;
        {
            shell.harness = firmware_shell::harness::State::new(boot);
            shell.harness.buffer = Some(firmware_shell::harness::CaptureBuffer::Owned(
                vec![0u8; firmware_shell::harness::FRAMES * width * height * 2].into_boxed_slice(),
            ));
        }
        shell.boot_id = boot;
        shell.present();
        let profile = if shell.settings.ble.is_some() {
            vana::sensors::ble_profile::Profile::from_u8(shell.settings.ble_profile)
        } else {
            vana::sensors::ble_profile::Profile::HeartRate
        };
        {
            let mut view =
                vana::screens::workout::View::new(vana::screens::workout::Page::Boot, 0, None);
            view.layout = model.layout;
            let scene = view.prepare();
            shell.draw_rows(|y, pixels| {
                firmware_shell::shell::Picture::paint(
                    &scene,
                    &mut vana::screens::chrome::Row::new(y, pixels),
                );
            });
        }
        shell.forget_frame();
        let mut sdk = vana::app::Runtime::new(profile, model.product, model.layout);
        {
            use vana::ride::directory::{ENTRY, RIDES, RideIndex};
            let bytes = || Box::leak(vec![0u8; RIDES * ENTRY].into_boxed_slice());
            if memory.ride_index {
                sdk.attach_ride_index(Box::leak(Box::new(RideIndex::new(bytes(), bytes()))));
            }
            let facts = vec![0u8; vana::kept::facts::record::SHELF_BYTES].into_boxed_slice();
            if memory.ride_facts {
                sdk.attach_ride_facts(Box::leak(facts));
                let deleted = vec![0u8; vana::kept::deleted::SHELF_BYTES].into_boxed_slice();
                sdk.attach_deleted_rides(Box::leak(deleted));
            }
        }
        sdk.load_preferences(&shell.settings.app, shell.settings_source);
        sdk.set_micros_clock(host_micros);
        // The look-ahead and its tile cache, which the device keeps in PSRAM;
        // each boot starts them over, as device memory does.
        if memory.terrain {
            sdk.attach_terrain(terrain_block());
        }
        if memory.phone {
            sdk.attach_phone(Box::leak(Box::new(vana::app::PhoneWork::new())));
        }
        if memory.track {
            let crumbs = Vec::leak(vec![vana::app::Crumb::default(); vana::app::TRACK_CRUMBS]);
            let map = Box::leak(Box::new(vana::app::MapFrame::default()));
            let line = Vec::leak(vec![vana::app::Crumb::default(); vana::app::COURSE_POINTS]);
            let turns = Vec::leak(vec![vana::app::Turn::default(); vana::app::COURSE_TURNS]);
            let course = Box::leak(Box::new(vana::app::Course::empty()));
            let finder = Box::leak(Box::new(vana::app::CourseFinder::new()));
            let track = Box::new(vana::app::RideTrack::new(
                crumbs, line, turns, map, course, finder,
            ));
            sdk.attach_track(Box::leak(track));
        }
        let trace = Trace::new(
            Start {
                device: model.name,
                boot,
                redacted: false,
                terrain: origin::Found::Absent,
            },
            log,
        );
        if let Some(tap) = &tap {
            trace.blocks().tap(Box::new(tap.clone()));
        }
        let session = Self {
            model,
            shell,
            now: 0,
            boot,
            input,
            power,
            off: false,
            display,
            media,
            settings,
            bulk,
            sdk,
            trace,
            trace_is_the_record: false,
            unrecorded: None,
            unseen_ms: 0,
            rest_ms: 0,
            edges_seen: 0,
            host: false,
            devices: Devices::Simulated(Box::new(Simulated::new(model.fitted))),
            tap,
            passes_since_start,
        };
        session.assert_invariants();
        Ok(session)
    }

    fn assert_invariants(&self) {
        if let Some(broken) = self.sdk.broken() {
            panic!(
                "broken after pass {} (boot {}): {broken}",
                self.passes_since_start, self.boot
            );
        }
    }

    /// Drive it with `replay_step`. A replay that leaves the recorded world
    /// before its first pass fails with a [`LeftAtBoot`] inside the error.
    pub fn replaying(trace: &[u8], options: Options) -> io::Result<Self> {
        Self::replaying_with(trace, options, &crate::trace::card::pack_store())
    }

    /// As `replaying`, reading the TERRAIN pack the trace's header names
    /// from the pack files in `packs`. A replay starts from blank flash at
    /// the header's power-on, with the card the trace says enough of: the
    /// twin serves every byte VANA reads from the trace.
    pub fn replaying_with(trace: &[u8], options: Options, packs: &Path) -> io::Result<Self> {
        let (header, _) = firmware_services::trace::decode(trace)
            .map_err(|error| io::Error::other(format!("trace: {error}")))?;
        let model = *Model::named(header.device).ok_or_else(|| {
            let known: Vec<_> = crate::models::all()
                .iter()
                .map(|model| model.name)
                .collect();
            io::Error::other(format!(
                "trace: recorded on {:?}, which the simulator has no model of (it has {})",
                header.device,
                known.join(", ")
            ))
        })?;
        let boot = header.boot.max(1);
        let flash = FileFlash::from_image(boot - 1, crate::storage::FlashImage::erased());
        let crate::trace::world::Opened { memory, redacted } = crate::trace::world::opened(trace);
        let pack = crate::trace::card::pack(trace, packs)?;
        let world = Recorded::open(trace.to_vec(), pack, options)
            .map_err(|error| io::Error::other(format!("trace: {error}")))?;
        let world = Rc::new(RefCell::new(world));
        let card = crate::trace::card::of(trace);
        let booted = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            Self::with_media(
                model,
                flash,
                card,
                memory,
                None,
                Boot {
                    passes_since_start: 0,
                    redacted,
                    twin: Some(world.clone()),
                },
            )
        }));
        let mut session = match booted {
            Ok(session) => session?,
            Err(panic) if panic.is::<Left>() => {
                let world = world.borrow();
                let why = world.parted().unwrap_or_default();
                return Err(io::Error::other(LeftAtBoot(why.to_string())));
            }
            Err(panic) => std::panic::resume_unwind(panic),
        };
        if redacted {
            session.redact_secrets();
        }
        if world.borrow_mut().take_redacting() {
            session.redact_secrets();
        }
        session.devices = Devices::Twin(Box::new(Servers::new(world)));
        Ok(session)
    }
    fn within_the_recording(
        &mut self,
        world: &Scripted,
        step: impl FnOnce(&mut Self) -> Step,
    ) -> Result<Step, Stopped> {
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| step(self))) {
            Ok(step) => Ok(step),
            Err(panic) if panic.is::<Left>() => Err(Stopped::LeftTheRecording),
            Err(panic) => {
                let why = (panic.downcast_ref::<String>().map(String::as_str))
                    .or_else(|| panic.downcast_ref::<&str>().copied())
                    .unwrap_or("a panic without a message");
                world.borrow_mut().panicked(why);
                Err(Stopped::Panicked)
            }
        }
    }
    fn twin_step(&mut self, world: Scripted) -> Step {
        let next = world.borrow_mut().advance();
        let step = match next {
            Next::Pass => self.within_the_recording(&world, |session| {
                session.tick();
                Step::Pass
            }),
            Next::Command(line) => self.within_the_recording(&world, |session| {
                let mut reply = String::new();
                let status = session
                    .command(line.as_bytes(), &mut reply)
                    .map_or("INVALID", |(_, status)| status);
                Step::Command {
                    line,
                    status,
                    reply,
                }
            }),
            Next::End => Ok(Step::End),
        };
        if world.borrow_mut().take_redacting() {
            self.redact_secrets();
        }
        step.unwrap_or(Step::End)
    }
    pub fn replay_step(&mut self) -> Step {
        match &self.devices {
            Devices::Twin(servers) => self.twin_step(servers.world.clone()),
            Devices::Simulated(_) => Step::End,
        }
    }
    /// Where a replay parted from its recording so far.
    pub fn report(&self) -> Option<Report> {
        match &self.devices {
            Devices::Twin(servers) => Some(servers.world.borrow().report(&self.trace())),
            Devices::Simulated(_) => None,
        }
    }
    /// In a replay, `region` as the replayed build left it, over `out`.
    /// A replay's card as the replayed build left it: every sector the
    /// trace read or the build wrote. `None` outside a replay.
    pub fn replayed_bulk(&self) -> Option<crate::bulk::SimBulk> {
        let Devices::Twin(servers) = &self.devices else {
            return None;
        };
        let written = servers
            .world
            .borrow_mut()
            .pages(firmware_services::trace::Medium::Bulk);
        let card = crate::bulk::SimBulk::default();
        card.restore(&crate::bulk::Image {
            presence: crate::bulk::Presence::Ready,
            sectors: crate::bulk::C606_SECTORS,
            entropy: 0,
            written,
        });
        Some(card)
    }

    pub fn replayed_media(&self, region: device_api::storage::Region, out: &mut [u8]) {
        if let Devices::Twin(servers) = &self.devices {
            servers.world.borrow_mut().contents(region.into(), out);
        }
    }
    /// Open a replay's window: its next divergence becomes the report's
    /// first. Returns the counts before the window.
    pub fn open_replay_window(&mut self) -> std::collections::BTreeMap<crate::replay::Kind, u32> {
        match &mut self.devices {
            Devices::Twin(servers) => servers.world.borrow_mut().open_window(),
            Devices::Simulated(_) => Default::default(),
        }
    }
    /// The trace a replay of this session needs: all of it, unless
    /// something changed what VANA read unseen by the trace.
    pub fn capture(&self) -> Result<Vec<u8>, String> {
        if let Some((what, at)) = self.unrecorded {
            return Err(format!("{what} at {at} ms, unseen by the trace"));
        }
        if !self.trace_is_the_record {
            return Err("no pass ran yet".into());
        }
        Ok(self.trace())
    }
    /// The bulk medium as a host writes it over USB. After the first pass
    /// such a write changes what VANA reads unseen by the trace, so this
    /// session no longer has a capture.
    pub fn host_bulk(&mut self) -> crate::bulk::SimBulk {
        self.unrecorded("a host wrote the bulk medium");
        self.bulk.host()
    }
    fn unrecorded(&mut self, what: &'static str) {
        if self.trace_is_the_record {
            self.unrecorded.get_or_insert((what, self.now));
        }
    }
    /// Record secrets redacted, as the C606 does, from the next pass on and
    /// after every restart; see `firmware_services::trace::redact`.
    pub fn redact_secrets(&mut self) {
        self.trace.start.redacted = true;
        self.trace.recorder.borrow_mut().redact();
    }
    /// The trace of this power-on session so far.
    pub fn trace(&self) -> Vec<u8> {
        self.trace.bytes()
    }
    /// One pass of the firmware loop, in the order C606 composition runs it.
    pub fn tick(&mut self) {
        match &mut self.devices {
            Devices::Twin(servers) => self.now = servers.world.borrow().now(),
            Devices::Simulated(sim) => {
                self.trace_is_the_record = true;
                if !self.off {
                    sim.environment(self.now, self.shell.settings.ble.is_some());
                }
            }
        }
        crate::logs::enter_device(self.tap.as_ref(), self.now);
        if !self.trace.spooling {
            self.trace.spool(&self.bulk);
        }
        self.edges_seen = self.input.edges.borrow().pushed();
        let quiet = self.quiet();
        // A replay runs only the passes the recording kept, so it keeps
        // each one, also one kept for its health and activity alone.
        let quietly = quiet && !self.is_replay();
        self.trace
            .recorder
            .borrow_mut()
            .begin_pass(self.now, quietly);
        self.display.0.borrow_mut().refusals.begin_pass();
        self.power.0.borrow_mut().refusals.begin_pass();
        self.media.fault.borrow_mut().begin_pass();
        self.bulk.begin_pass();
        firmware_services::why::pass(self.now, |out| self.sdk.state(out));
        let stayed_quiet = self.pass(quiet);
        {
            let power = self.trace.status.state;
            let host = core::mem::take(&mut self.host);
            self.rest_ms = if self.off {
                0
            } else {
                self.sdk.pass_rest_ms(&self.shell, power, host, self.now)
            };
        }
        self.trace.window.passes += 1;
        self.passes_since_start += 1;
        self.assert_invariants();
        firmware_services::why::deltas(|out| self.sdk.state(out));
        if !self.off && self.trace.vitals.due(self.now, quiet) {
            self.record_vitals();
        }
        if stayed_quiet {
            self.forget_as_recorded();
            return;
        }
        self.record_seen();
        let card = spool::Card::of(
            self.card_admits(),
            self.sdk.recorder_busy(),
            self.sdk.shutdown_due(),
        );
        if self.sdk.wipes_traces() {
            self.trace.file.formatted();
            self.trace.blocks().sink_mut().spool = None;
        }
        self.trace.drain(&self.bulk, card, self.now);
        self.forget_as_recorded();
    }
    /// Whether the card admits work after this pass, as the C606's power
    /// owner admits it: while work is ready or the companion starts, and
    /// not from the moment a power request was accepted.
    fn card_admits(&self) -> bool {
        use device_api::power::PowerControl as _;
        let status = match &self.devices {
            Devices::Simulated(sim) => sim.switch.status(),
            Devices::Twin(servers) => servers.control.status(),
        };
        !self.off && (status.ready || status.state == device_api::power::State::Initializing)
    }
    /// Append everything the trace staged to its file and sync it, as the
    /// C606 does before a host takes the card; false when it has no file.
    pub fn sync_trace(&mut self) -> bool {
        self.trace.sync(&self.bulk, self.now)
    }
    /// The most the trace's spool held so far, and its size; none while
    /// nothing is staged.
    pub fn trace_spool_peak(&self) -> Option<(usize, usize)> {
        let recorder = self.trace.blocks();
        let spool = recorder.sink().spool.as_ref()?;
        Some((spool.peak(), spool.capacity()))
    }
    /// The name of this session's trace file in the card's STORE volume.
    pub fn trace_file(&self) -> Option<String> {
        match &self.trace.file {
            spool::File::Open(_, _, drain) => Some(drain.name().as_str().to_string()),
            _ => None,
        }
    }
    /// Health and activity as the simulator knows them: its own passes,
    /// what its devices report and the trace's spool. What it cannot know,
    /// such as memory, CPU and pass times or signal, stays absent. A replay
    /// records none: its device is the recording's.
    fn record_vitals(&mut self) {
        let Devices::Simulated(sim) = &self.devices else {
            return;
        };
        let now = self.now;
        let gps = sim.position.snapshot(now).map(|position| position.gps);
        let input = sim.battery.snapshot(now);
        let (spool_used, spool_size) = self
            .trace
            .blocks()
            .sink()
            .spool
            .as_ref()
            .map_or((0, 0), |spool| (spool.len(), spool.capacity()));
        let count = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        let health = Health {
            simulated: true,
            gps_sentences: gps.map_or(0, |gps| gps.valid_sentences),
            gps_overflows: gps.map_or(0, |gps| gps.overflows),
            gps_uart_errors: gps.map_or(0, |gps| gps.uart_errors),
            companion_errors: input.map_or(0, |input| {
                input
                    .companion_bad_crc
                    .saturating_add(input.uart_errors)
                    .saturating_add(input.input_lost)
            }),
            spool_used: count(spool_used),
            spool_size: count(spool_size),
            ..Health::default()
        };
        let frames = self.display.0.borrow().submissions;
        let window = core::mem::replace(
            &mut self.trace.window,
            Window {
                start_ms: now,
                passes: 0,
                frames,
            },
        );
        let ant_channels = sim
            .ant
            .channels(now)
            .iter()
            .flatten()
            .filter(|channel| channel.link == device_api::ant::LinkState::Connected)
            .count();
        let access_point = sim.access_point.state();
        let activity = Activity {
            simulated: true,
            window_ms: u32::try_from(now - window.start_ms).unwrap_or(u32::MAX),
            wakes: window.passes,
            consumers: Consumers {
                backlight: self.power.0.borrow().brightness,
                frames: frames.wrapping_sub(window.frames),
                gnss: match (&sim.gps, sim.position.missing) {
                    (Some(_), false) => Gnss::Receiving,
                    _ => Gnss::Off,
                },
                access_point,
                ble: sim.ble.snapshot.link,
                ant: sim.ant.availability,
                ant_scanning: sim.ant.scanning(),
                ant_channels: u8::try_from(ant_channels).unwrap_or(u8::MAX),
                ..Consumers::default()
            },
            ..Activity::default()
        };
        let mut recorder = self.trace.recorder.borrow_mut();
        recorder.record(&Event::Health(health));
        recorder.record(&Event::Activity(activity));
        recorder.record(&Event::Task {
            name: "console",
            polls: window.passes,
            busy_us: None,
        });
    }
    fn startup(&self) -> Startup {
        match &self.devices {
            Devices::Simulated(sim) => sim.startup,
            Devices::Twin(servers) => {
                use device_api::sensors::Sensors as _;
                servers.sensors.startup_status()
            }
        }
    }
    /// A pass in dark charging standby, as the C606 runs it: only the
    /// startup and power status are read, and only a change records it.
    fn quiet(&self) -> bool {
        self.trace.status.state == device_api::power::State::Charging && self.sdk.standby_dark()
    }
    /// One pass; true when it stayed quiet.
    fn pass(&mut self, quiet: bool) -> bool {
        let now = self.now;
        let read_companion = |session: &mut Self| {
            let trace = &session.trace;
            (session.devices).companion(&trace.observed.companion, now, &trace.recorder)
        };
        let companion = (!quiet).then(|| read_companion(self));
        if self.off {
            return false;
        }
        let startup = {
            use device_api::sensors::Sensors as _;
            let sensed = Sensed {
                startup: self.startup(),
                physical: None,
                light: Observation::Unavailable,
                fitted: self.model.fitted.sensors,
            };
            let trace = &self.trace;
            device_api::sensors::SensorsRecorder::new(
                sensed,
                &*trace.recorder,
                &trace.observed.sensors,
            )
            .startup_status()
        };
        self.trace.startup = startup;
        let power = self.devices.power(
            &self.trace.observed.control,
            now,
            startup,
            &self.trace.recorder,
        );
        let Some(power) = power else {
            self.trace.record(&Event::PoweredOff);
            self.turn_off();
            return false;
        };
        self.trace.status = power;
        if quiet && power.state == device_api::power::State::Charging {
            return true;
        }
        let companion = companion.unwrap_or_else(|| read_companion(self));
        self.power.0.borrow_mut().battery = companion.battery();
        let admits = power.ready || power.state == device_api::power::State::Initializing;
        if admits && matches!(self.trace.file, spool::File::Waiting) {
            self.trace.stage(&self.bulk);
            if let Devices::Twin(servers) = &self.devices {
                self.trace.start.terrain = servers.world.borrow().terrain();
            }
            let media = self.devices.bulk(&mut self.bulk);
            let log: &RefCell<TraceRecorder> = &self.trace.recorder;
            let mut media =
                device_api::bulk::ReadWriteRecorder::new(media, log, &self.trace.observed.bulk);
            self.trace.read_volumes(&mut media);
        }
        // C606 composition scans the ride journal while the companion starts
        // and runs the shell and VANA only once power work is ready; until
        // then the backlight stays dark.
        if power.state == device_api::power::State::Initializing {
            let log: &RefCell<TraceRecorder> = &self.trace.recorder;
            let mut bulk = device_api::bulk::ReadWriteRecorder::new(
                self.devices.bulk(&mut self.bulk),
                log,
                &self.trace.observed.bulk,
            );
            self.sdk
                .prepare(&mut self.shell.data_storage(), &mut bulk, now);
        }
        if power.state == device_api::power::State::Charging {
            self.standby(companion);
        }
        if !power.ready {
            return false;
        }
        let position =
            (self.devices).position(&self.trace.observed.position, now, &self.trace.recorder);
        self.bind_and_pass(companion, position, startup);
        false
    }
    fn bind_and_pass(
        &mut self,
        companion: CompanionView,
        position: PositionView,
        startup: Startup,
    ) {
        let now = self.now;
        let model = self.model.fitted.sensors;
        let (physical, light, network_time, fitted) = match &mut self.devices {
            Devices::Simulated(sim) => {
                let mut physical = sim.physical(now);
                if !model.barometer {
                    physical.pressure = Observation::Unavailable;
                }
                let light = if model.light {
                    sim.light(now)
                } else {
                    Observation::Unavailable
                };
                let network_time = sim
                    .network_time
                    .map_or_else(Default::default, |anchor| anchor.snapshot(now, 0, true));
                (physical, light, network_time, model)
            }
            Devices::Twin(servers) => {
                use device_api::sensors::Sensors as _;
                let sensed = (
                    servers.sensors.snapshot(now),
                    servers.sensors.light(now),
                    servers.sensors.fitted(),
                );
                let network_time = servers.world.borrow().network_time();
                (sensed.0, sensed.1, network_time, sensed.2)
            }
        };
        let record: &RefCell<TraceRecorder> = &self.trace.recorder;
        let (physical, light, fitted) = {
            use device_api::sensors::Sensors as _;
            let sensed = Sensed {
                startup,
                physical: Some(physical),
                light,
                fitted,
            };
            let sensors = device_api::sensors::SensorsRecorder::new(
                sensed,
                record,
                &self.trace.observed.sensors,
            );
            (sensors.snapshot(now), sensors.light(now), sensors.fitted())
        };
        if self.trace.network_time.changed(network_time, now) {
            self.trace.record(&Event::NetworkTime(network_time));
        }
        let token = self.sdk.phone_status().map(|status| status.token);
        if self.trace.start.redacted {
            let token = token.as_ref().map(|token| token.bytes());
            self.trace.recorder.borrow_mut().session_token(token);
        }
        match &mut self.devices {
            Devices::Twin(servers) => servers.world.borrow_mut().token = token,
            Devices::Simulated(_) => {}
        }
        let Trace {
            recorder,
            observed,
            started,
            status,
            ..
        } = &mut self.trace;
        let record: &RefCell<TraceRecorder> = recorder;
        let raw = self.devices.raw(&mut self.bulk);
        let mut ant = device_api::ant::AntRecorder::new(raw.0, record, &observed.ant);
        let mut ble = device_api::ble_transport::BleRecorder::new(raw.1, record, &observed.ble);
        let mut bulk = device_api::bulk::ReadWriteRecorder::new(raw.2, record, &observed.bulk);
        let mut access_point =
            device_api::server::AccessPointRecorder::new(raw.3, record, &observed.access_point);
        let mut server = device_api::server::ServerRecorder::new(raw.4, record, &observed.server);
        let mut sound = device_api::sound::SoundRecorder::new(raw.5, record, &observed.sound);
        let mut control =
            device_api::power::PowerControlRecorder::new(raw.6, record, &observed.control);
        ant.sample();
        ble.sample();
        access_point.sample();
        server.sample();
        sound.sample();
        control.sample();
        vana::app::pass(
            &mut self.sdk,
            &mut self.shell,
            startup,
            light,
            vana::app::Observations {
                position: &position,
                network_time,
                input: &companion,
                physical,
                fitted,
            },
            now,
            vana::app::Handles {
                ant: &mut ant,
                ble: &mut ble,
                bulk: &mut bulk,
                access_point: &mut access_point,
                server: &mut server,
                sound: Started {
                    sound: &mut sound,
                    started,
                },
                control: &mut control,
            },
            &mut |point, shell| {
                if point == vana::app::Point::InputsReady {
                    record_synthetic(shell, record);
                }
            },
        );
        *status = control.status();
    }
    fn record_seen(&mut self) {
        let mut notes = Vec::new();
        let mut recorder = self.trace.recorder.borrow_mut();
        let mut record = |event: Event<'_>| recorder.record(&event);
        self.sdk.report_seen(self.now, &mut |shown| match shown {
            vana::app::Seen::Page(page) => record(Event::Page(page)),
            vana::app::Seen::Toast(text) => record(Event::Toast(text)),
            vana::app::Seen::RideState(state) => record(Event::RideState(state)),
            vana::app::Seen::Shown(fields) => record(Event::Shown(fields)),
            vana::app::Seen::Mark {
                count,
                pressed_ms,
                page,
            } => record(Event::Mark {
                count,
                pressed_ms,
                page,
            }),
            vana::app::Seen::Note {
                cue,
                pattern,
                note,
                of,
            } => notes.push((pattern, cue.to_string(), note, of)),
        });
        for pattern in core::mem::take(&mut self.trace.started) {
            let played = notes
                .iter()
                .position(|played| played.0 == pattern)
                .map(|at| notes.remove(at));
            let (cue, note, of) =
                played.map_or((String::new(), 0, 0), |(_, cue, note, of)| (cue, note, of));
            record(Event::Sound {
                pattern,
                cue: &cue,
                note,
                of,
            });
        }
        if let Some(digest) = self.shell.take_frame() {
            record(Event::Panel(digest));
        }
        drop(recorder);
        self.record_logs();
    }
    fn record_logs(&self) {
        let mut recorder = self.trace.recorder.borrow_mut();
        crate::logs::drain(|level, target, text| {
            recorder.record(&Event::Log {
                level,
                target,
                text,
            });
        });
    }
    fn standby(&mut self, companion: CompanionView) {
        let lit = self.sdk.standby(&mut self.shell, &companion, self.now);
        self.power.0.borrow_mut().brightness = if lit {
            self.shell.settings.brightness
        } else {
            0
        };
    }
    /// The device rests in charging standby, VANA off.
    pub fn charging_standby(&self) -> bool {
        matches!(&self.devices, Devices::Simulated(sim) if sim.switch.standby())
    }
    /// The panel shows something while VANA is off: charging standby.
    pub fn standby_lit(&self) -> bool {
        self.charging_standby() && self.power.0.borrow().brightness > 0
    }
    fn turn_off(&mut self) {
        self.off = true;
        self.power.0.borrow_mut().brightness = 0;
        self.shell.forget_panel();
        let mut frame = self.display.0.borrow_mut();
        frame.pixels.fill(0);
        let height = frame.geometry.height;
        frame.shown(height, &firmware_shell::rendering::work::Work::default());
    }
    /// Whether the panel is lit: the shell set a brightness since power-on.
    pub fn lit(&self) -> bool {
        !self.off && !self.charging_standby() && self.power.0.borrow().brightness > 0
    }
    /// Hold the top-left button of a device that is off: it starts again,
    /// companion included; in charging standby, it wakes in the same
    /// session. A running device ignores this.
    pub fn power_button(&mut self) -> io::Result<()> {
        if self.off {
            self.reboot(true)?;
        } else if self.charging_standby() {
            let now = self.now;
            self.sim_mut().wake(now);
        }
        Ok(())
    }
    fn start_charging(&mut self) -> io::Result<()> {
        self.reboot(true)?;
        self.sim_mut().companion_charging();
        Ok(())
    }
    /// Advance device and world time together in 10 ms slices, with a pass
    /// in each slice where VANA's rest (`Runtime::pass_rest_ms`) is over or
    /// an input edge or a radar page arrived, as they wake the C606 console;
    /// the device clock then reads the last pass.
    /// Off on USB power, the companion starts the device again for
    /// charging, and the rest of `ms` runs on the new session's clock.
    pub fn step(&mut self, ms: u64) {
        let mut left = ms;
        while left > 0 {
            let step = left.min(10);
            left -= step;
            self.sim_mut().world.advance(step);
            self.unseen_ms += step;
            if self.unseen_ms < self.rest_ms
                && self.input.edges.borrow().pushed() == self.edges_seen
                && !self.woken_by_ant()
            {
                continue;
            }
            self.now += core::mem::take(&mut self.unseen_ms);
            self.tick();
            if self.sdk.restart_due() {
                self.restart().expect("the media reopens");
            }
            if self.off && self.sim().battery.charging {
                self.start_charging().expect("the media reopens");
            }
        }
    }
    fn woken_by_ant(&self) -> bool {
        let sim = self.sim();
        sim.ant
            .page_due(vana::app::WAKING_ANT_TYPE, sim.world.time_ms)
    }
    /// The device clock between passes: the last pass's time plus the world
    /// time since, as the C606's input task stamps an edge when it arrives.
    pub fn clock_ms(&self) -> u64 {
        self.now + self.unseen_ms
    }
    /// End the rest under way with a pass at the device clock, so a request
    /// that follows runs at the time it arrives, not the last pass's.
    pub fn close_rest(&mut self) {
        self.now += core::mem::take(&mut self.unseen_ms);
        self.tick();
    }
    pub fn advance(&mut self, ms: u64) -> Result<(), Error> {
        if ms > 120000 {
            return Err(Error::Invalid);
        }
        self.now.checked_add(ms).ok_or(Error::Invalid)?;
        self.step(ms);
        Ok(())
    }
    /// A terminal request line: recorded into the trace, then executed, as
    /// the C606 runs one after the pass before it, in no pass of its own.
    /// One sent during a rest ends it first.
    pub fn command(
        &mut self,
        line: &[u8],
        out: &mut String,
    ) -> Result<(Request, &'static str), firmware_console::protocol::Error> {
        let request = firmware_console::protocol::parse(line)?;
        if self.unseen_ms > 0 {
            self.close_rest();
        }
        if let Ok(text) = core::str::from_utf8(line) {
            let mut redacted = [0u8; 256];
            let text = match self.trace.start.redacted {
                true => redact::command(text, &mut redacted),
                false => text,
            };
            self.trace.record(&Event::Command(text));
        }
        self.freeze_power();
        let status = self.execute_gated(request, out);
        self.forget_as_recorded();
        Ok((request, status))
    }
    /// Run a pass, then a parsed request, as the terminal does. Unlike
    /// `command`, the trace does not see the request, so a replay of this
    /// session will not either.
    pub fn execute(&mut self, request: Request, out: &mut String) -> &'static str {
        self.unrecorded("a request ran unrecorded");
        self.close_rest();
        self.freeze_power();
        self.execute_gated(request, out)
    }
    fn freeze_power(&mut self) {
        let now = self.now;
        self.trace.recorder.borrow_mut().begin_command();
        self.trace.status =
            (self.devices).command_power(&self.trace.observed.control, now, &self.trace.recorder);
    }
    fn forget_as_recorded(&mut self) {
        let Devices::Twin(servers) = &mut self.devices else {
            return;
        };
        let forgotten = servers.world.borrow_mut().forget();
        if forgotten.is_empty() {
            return;
        }
        for medium in forgotten {
            self.trace.recorder.borrow_mut().forget(medium);
        }
        let log: &RefCell<TraceRecorder> = &self.trace.recorder;
        let mut media = device_api::bulk::ReadWriteRecorder::new(
            &mut servers.bulk,
            log,
            &self.trace.observed.bulk,
        );
        self.trace.read_volumes(&mut media);
    }
    fn execute_gated(&mut self, request: Request, out: &mut String) -> &'static str {
        crate::logs::enter_device(self.tap.as_ref(), self.now);
        // A host talking on the terminal keeps the usual pace.
        self.rest_ms = 0;
        {
            self.host = true;
        }
        self.respond(request.command, out)
    }
    fn respond(&mut self, command: Command, out: &mut String) -> &'static str {
        let now = self.now;
        let trace_card = self.bulk.host();
        let frozen = self.trace.status;
        let (startup, network_time) = match &self.devices {
            Devices::Simulated(sim) => (sim.startup, sim.network_time),
            Devices::Twin(_) => (self.trace.startup, None),
        };
        let mut stand_ins = StandIns {
            ant: Absent,
            power: UnrecordedConsolePower(frozen),
        };
        let (devices, media) =
            self.devices
                .unrecorded_console(&mut self.bulk, &mut stand_ins, command, frozen);
        // The radios' control status decides what a SELECT, FORGET or
        // CONFIG answers, and the card what a VOLUME or STORE command
        // does, so the world log holds what the command read of them.
        let (log, observed) = (self.trace.recorder.clone(), self.trace.observed.clone());
        let mut ble =
            device_api::ble_transport::BleRecorder::new(devices.ble, &*log, &observed.ble);
        let mut wifi =
            device_api::connectivity::WifiRecorder::new(devices.wifi, &*log, &observed.wifi);
        let mut media = device_api::bulk::ReadWriteRecorder::new(media, &*log, &observed.bulk);
        let devices = firmware_console::commands::Capabilities {
            ble: &mut ble,
            wifi: &mut wifi,
            ..devices
        };
        firmware_console::commands::execute(
            command,
            now,
            devices,
            &mut self.shell,
            &mut self.sdk,
            &mut Twin {
                media: &mut media,
                trace: &mut self.trace,
                trace_card,
                startup,
                network_time,
            },
            out,
        )
    }
    /// A terminal fixture line: the world or a simulated device changes. A
    /// replay has neither; its trace already holds what they did.
    pub fn fixture(&mut self, line: &str, out: &mut String) -> &'static str {
        if self.is_replay() {
            return "UNSUPPORTED";
        }
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
                if w.next().is_some() {
                    return "INVALID";
                }
                if self.advance(ms).is_err() {
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
                let mut fault = self.media.fault.borrow_mut();
                fault.after = Some(after);
                fault.partial = partial;
            }
            Some("POSITION") => {
                let now = self.now;
                let sim = self.sim_mut();
                match w.next() {
                    Some("NONE") => sim.position = PositionFixture::default(),
                    Some("UNSUPPORTED") => sim.position.missing = true,
                    Some("NMEA") => {
                        let Some(sentence) = w.next() else {
                            return "INVALID";
                        };
                        if sentence.len() > 100 {
                            return "INVALID";
                        }
                        let mut bytes = sentence.as_bytes().to_vec();
                        bytes.extend_from_slice(b"\r\n");
                        sim.position.missing = false;
                        sim.position.acquisition.ingest(
                            now,
                            &bytes,
                            sim.position.dma_losses,
                            sim.position.uart_errors,
                        );
                    }
                    Some("LOSS") => {
                        sim.position.dma_losses = sim.position.dma_losses.saturating_add(1);
                        sim.position.uart_errors = sim.position.uart_errors.saturating_add(1);
                        sim.position.acquisition.ingest(
                            now,
                            &[],
                            sim.position.dma_losses,
                            sim.position.uart_errors,
                        );
                    }
                    _ => return "INVALID",
                };
                if w.next().is_some() {
                    return "INVALID";
                }
            }
            Some("BLE") => {
                let now = self.now;
                let sim = self.sim_mut();
                match w.next() {
                    Some("CONNECT") => {
                        sim.ble.snapshot.link = device_api::ble_transport::Link::Connected;
                        sim.ble.snapshot.connections += 1;
                        sim.ble.packets.clear();
                    }
                    Some("DISCONNECT") => {
                        sim.ble.snapshot.link = device_api::ble_transport::Link::Off;
                        sim.ble.snapshot.disconnections += 1;
                        sim.ble.packets.clear();
                    }
                    Some("LOSS") => {
                        sim.ble.snapshot.dropped += 1;
                    }
                    Some("HEART") => {
                        let Some(bpm) = w.next().and_then(|s| s.parse::<u8>().ok()) else {
                            return "INVALID";
                        };
                        if sim.ble.snapshot.link != device_api::ble_transport::Link::Connected {
                            return "UNAVAILABLE";
                        }
                        sim.ble.snapshot.notifications += 1;
                        if sim.ble.packets.len() == 4 {
                            sim.ble.snapshot.dropped += 1;
                        } else {
                            let mut bytes = [0; 60];
                            bytes[1] = bpm;
                            sim.ble
                                .packets
                                .push_back(device_api::ble_transport::Packet {
                                    connection: sim.ble.snapshot.connections,
                                    sequence: sim.ble.snapshot.notifications,
                                    dropped: sim.ble.snapshot.dropped,
                                    received_ms: now,
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
        self.sync_trace();
        self.reboot(false)
    }
    /// Store `app` as VANA's preferences, as a save would; VANA reads them
    /// when it next boots.
    pub fn keep_app_preferences(&mut self, app: firmware_shell::preferences::AppPreferences) {
        self.settings.save_app(app).expect("memory media saves");
    }
    /// Power lost and restored: the companion boots again too.
    pub fn power_cycle(&mut self) -> io::Result<()> {
        self.reboot(true)
    }
    /// A panic or a watchdog reset of the main CPU: a restart without the
    /// sync before it, after which memory still holds the trace's spool and
    /// open block, and the next boot salvages them into the trace before.
    pub fn crash(&mut self) -> io::Result<()> {
        let remains = self.trace.remains();
        self.reboot(false)?;
        self.trace.remains = remains;
        Ok(())
    }
    /// Reboot on the same media. The world, sensors and receiver presence
    /// persist; everything held in device memory starts over.
    fn reboot(&mut self, power: bool) -> io::Result<()> {
        self.record_logs();
        if let Some(tap) = &self.tap {
            tap.end_session();
        }
        self.bulk.begin_boot();
        let mut next = Self::with_media(
            self.model,
            self.media.untapped(),
            self.bulk.untapped(),
            Memory::ALL,
            self.tap.clone(),
            Boot {
                passes_since_start: self.passes_since_start,
                redacted: self.trace.start.redacted,
                twin: None,
            },
        )?;
        if self.display.0.borrow().hashes.is_some() {
            next.display.0.borrow_mut().hashes = Some(Vec::new());
        }
        if self.trace.start.redacted {
            next.redact_secrets();
        }
        let uptime = self.now;
        let next_sim = next.sim_mut();
        self.sim_mut().carry_into(next_sim, power, uptime);
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
pub fn terminal(root: &Path, width: usize, height: usize, restart_probe_ms: u64) -> io::Result<()> {
    let mut session = Session::new(root, width, height)?;
    session.sim_mut().restart_probe_ms = restart_probe_ms;
    let mut line = Vec::new();
    let mut overflow = false;
    let stdin = io::stdin();
    let mut stdout = io::stdout().lock();
    // The host starts its first reply clock only after this line, so boot time
    // under load never turns a request into an uncertain mutation.
    stdout.write_all(b"{\"type\":\"ready\"}\n")?;
    stdout.flush()?;
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
            session.close_rest();
            match session.command(&line, &mut out) {
                Ok((request, status)) => {
                    id = request.id;
                    restart = request.command == Command::Restart && status == "ACCEPTED";
                    status
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

fn host_micros() -> u64 {
    static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    START
        .get_or_init(std::time::Instant::now)
        .elapsed()
        .as_micros() as u64
}

/// A replay left the recorded world as VANA booted: why.
#[derive(Debug)]
pub struct LeftAtBoot(pub String);

impl std::fmt::Display for LeftAtBoot {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "trace: at boot, {}", self.0)
    }
}

impl std::error::Error for LeftAtBoot {}
