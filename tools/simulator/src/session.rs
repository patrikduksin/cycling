//! Headless backend: production parser, shell handlers, journals and SDK runtime.
use crate::ant::SimulatedAnt;
use crate::devices::Battery;
use crate::devices::DisplayDevice;
use crate::devices::InputDevice;
use crate::devices::PowerDevice;
use crate::devices::PowerSwitch;
use crate::devices::Speaker;
use crate::gps::Receiver;
use crate::replay::{Capture, Mode, Next, Origin, Replay, Report, Seen, Shelled, Sources};
use crate::storage::FileFlash;
use crate::world::World;
use device_api::ant::Ant;
use device_api::ant::AntOperation;
use device_api::ble_transport::Ble;
use device_api::bulk::ReadWrite;
use device_api::display::Display;
use device_api::input::Button;
use device_api::input::InputObservation;
use device_api::input::InputSnapshot;
use device_api::observation::Availability;
use device_api::observation::Error;
use device_api::observation::Observation;
use device_api::positioning::Positioning;
use device_api::sensors::Startup;
use device_api::server::AccessPoint;
use device_api::{power::PowerControl, server::Server, sound::Sound};
use firmware_console::protocol::Command;
use firmware_console::protocol::Request;
use firmware_services::trace::frozen::CompanionView;
use firmware_services::trace::frozen::Sink;
use firmware_services::trace::frozen::{FrozenAccessPoint, FrozenServer, FrozenSound};
use firmware_services::trace::frozen::{
    FrozenCompanion, FrozenPosition, FrozenPowerControl, Latest, Record, RecordedBulk,
    RecordedEdges, Recorder,
};
use firmware_services::trace::origin::{self, Config, Media, Memory};
use firmware_services::trace::redact;
use firmware_services::trace::spool::{self, Spool, Staging};
use firmware_services::trace::{Activity, Consumers, Event, Gnss, Health, Output, Vitals};
use std::cell::RefCell;
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
    fn take_battery(&mut self) -> Option<device_api::ble_transport::BatteryLevel> {
        self.battery.take()
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
const ASSUMED_BAROMETER_MS: u64 = 100;
/// Uptime before a powered-on companion reports ready. Modelled on C606
/// startup: three seconds of passive observation before the startup
/// acknowledgment, then an operating reason and two advancing sensor samples.
pub const COMPANION_BOOT_MS: u64 = 3_300;
/// How long C606's startup check watches a running companion after a
/// main-CPU restart before it admits work: one passive probe window.
pub const RESTART_PROBE_MS: u64 = 3_000;
pub type VirtualShell =
    firmware_shell::shell::Shell<DisplayDevice, RecordedEdges<InputDevice>, PowerDevice, FileFlash>;

const TRACE_BLOCK: usize = 4096;

type TraceRecorder = Recorder<Box<[u8]>, Staged>;

/// The trace's sealed blocks: kept for a capture, and staged for its file
/// while the card's STORE volume records it, as the C606 stages them. A
/// block the spool has no room for is lost from both.
#[derive(Default)]
struct Staged {
    kept: Vec<u8>,
    spool: Option<Spool<Vec<u8>>>,
}

impl Sink for Staged {
    fn block(&mut self, bytes: &[u8]) -> bool {
        let taken = self.spool.as_mut().is_none_or(|spool| spool.block(bytes));
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

struct Trace {
    recorder: RefCell<TraceRecorder>,
    ant: firmware_services::trace::frozen::FrozenAnt,
    ble: firmware_services::trace::frozen::FrozenBle,
    position: FrozenPosition,
    companion: FrozenCompanion,
    startup: Latest<Startup>,
    physical: Latest<device_api::sensors::Snapshot>,
    network_time: Latest<firmware_services::network_time::Snapshot>,
    power: FrozenPowerControl,
    backlight: Latest<Availability>,
    sound: FrozenSound,
    access_point: FrozenAccessPoint,
    server: FrozenServer,
    start: Start,
    file: spool::File<Vec<u8>>,
    spooling: bool,
    vitals: Vitals,
    window: Window,
}

/// What the current activity window has counted so far.
#[derive(Clone, Copy, Default)]
struct Window {
    start_ms: u64,
    passes: u32,
    /// The display's frame count as the window began.
    frames: u32,
}

/// What the device started from, for the trace's header: the settings and
/// working memory at power-on, FILES and TERRAIN as the first pass found
/// them, and the ride journals once VANA's boot scans ended.
struct Start {
    boot: u32,
    profile: u8,
    settings: firmware_shell::preferences::Settings,
    memory: Memory,
    redacted: bool,
    volumes: Option<Volumes>,
    journals: Option<Journals>,
}

#[derive(Clone)]
struct Volumes {
    /// FILES' store id, generation and entries.
    files: origin::Found<(u64, u64, Vec<u8>)>,
    facts: Option<u32>,
    terrain: origin::Found<origin::Terrain>,
}
type Journals = (Option<origin::Journal>, Option<origin::RideLog>);

impl Start {
    fn read_volumes(&mut self, bulk: &crate::bulk::SimBulk) {
        let mut entries = vec![0u8; origin::FILE_ENTRIES];
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let (files, terrain) = origin::volumes(&mut bulk.host(), &mut entries, &mut sector);
        let files = files.map(|files| (files.store, files.generation, files.entries.to_vec()));
        let facts = vana::ride::facts::digest(&mut bulk.host()).ok().flatten();
        self.volumes = Some(Volumes {
            files,
            facts,
            terrain,
        });
    }

    fn header(&self) -> Vec<u8> {
        let Volumes {
            files,
            facts,
            terrain,
        } = self.volumes.clone().unwrap_or(Volumes {
            files: origin::Found::Absent,
            facts: None,
            terrain: origin::Found::Absent,
        });
        let (journal, rides) = self.journals.unwrap_or((None, None));
        let mut settings = self.settings;
        if self.redacted
            && let Some(wifi) = &mut settings.wifi
        {
            let mut password = [0u8; 63];
            let len = redact::password(wifi.password.bytes(), &mut password);
            wifi.password =
                device_api::connectivity::Text::new(&password[..len]).expect("a password's length");
        }
        let config = Config {
            settings: &settings.encode(),
            memory: self.memory,
            redacted: self.redacted,
        };
        let media = Media {
            journal,
            rides,
            files: match &files {
                origin::Found::Read((store, generation, entries)) => {
                    origin::Found::Read(origin::Files {
                        store: *store,
                        generation: *generation,
                        entries,
                    })
                }
                origin::Found::Absent => origin::Found::Absent,
                origin::Found::Unreadable(failure) => origin::Found::Unreadable(*failure),
            },
            facts,
            terrain,
        };
        let (mut scratch, mut out) = (vec![0u8; 16 * 1024], vec![0u8; 16 * 1024]);
        let commit = option_env!("CYCLING_BUILD_COMMIT").unwrap_or("");
        let len = origin::header(
            commit,
            self.boot,
            self.profile,
            &config,
            &media,
            &mut scratch,
            &mut out,
        )
        .expect("the header fits a block");
        out.truncate(len);
        out
    }
}

impl Trace {
    fn new(start: Start) -> Self {
        let buffer = vec![0u8; TRACE_BLOCK].into_boxed_slice();
        Self {
            start,
            recorder: RefCell::new(Recorder::new(buffer, Staged::default())),
            file: spool::File::Waiting,
            spooling: false,
            vitals: Vitals::default(),
            window: Window::default(),
            ant: Default::default(),
            ble: Default::default(),
            position: FrozenPosition::default(),
            companion: FrozenCompanion::default(),
            startup: Latest::default(),
            physical: Latest::default(),
            network_time: Latest::default(),
            power: FrozenPowerControl::default(),
            backlight: Latest::default(),
            sound: FrozenSound::default(),
            access_point: FrozenAccessPoint::default(),
            server: FrozenServer::default(),
        }
    }

    fn record(&self, event: &Event<'_>) {
        self.recorder.borrow_mut().record(event);
    }

    /// From the first pass, as the C606 spools from power-on, when the
    /// card holds a STORE its trace will stage for: the card does not
    /// change while it is closed.
    fn spool(&mut self, bulk: &crate::bulk::SimBulk) {
        self.spooling = true;
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let file: spool::File<Vec<u8>> = spool::File::stage(&mut bulk.host(), &mut sector);
        if let spool::File::Staging(..) = file {
            self.recorder.borrow_mut().sink_mut().spool =
                Some(Spool::new(vec![0u8; spool::RING_BYTES]));
        }
    }

    fn stage(&mut self, bulk: &crate::bulk::SimBulk) {
        self.start.read_volumes(bulk);
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        self.file = spool::File::stage(&mut bulk.host(), &mut sector);
        if !matches!(self.file, spool::File::Staging(..)) {
            self.recorder.borrow_mut().sink_mut().spool = None;
        }
    }

    fn drain(&mut self, bulk: &crate::bulk::SimBulk, card: spool::Card, now: u64) {
        if card != spool::Card::Closed && matches!(self.file, spool::File::Waiting) {
            self.stage(bulk);
        }
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let start = &self.start;
        let header = || start.journals.map(|_| (start.header(), start.boot));
        let mut recorder = self.recorder.borrow_mut();
        let passed = self.file.pass(
            &mut recorder,
            &mut bulk.host(),
            card,
            header,
            now,
            &mut sector,
        );
        if passed.is_err() {
            recorder.sink_mut().spool = None;
        }
    }

    fn sync(&mut self, bulk: &crate::bulk::SimBulk, now: u64) -> bool {
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let mut recorder = self.recorder.borrow_mut();
        let open = self.file.is_open();
        let synced = self
            .file
            .sync(&mut recorder, &mut bulk.host(), now, &mut sector);
        if synced.is_err() {
            recorder.sink_mut().spool = None;
        }
        open && synced.is_ok()
    }

    fn bytes(&self) -> Vec<u8> {
        let mut recorder = self.recorder.borrow_mut();
        let mut bytes = self.start.header();
        bytes.extend_from_slice(&recorder.sink().kept);
        bytes.extend_from_slice(recorder.open_block());
        bytes
    }
}

/// The checkout paths whose tracked files build VANA, as `build.rs` lists
/// them: an uncommitted change under one means no commit holds this build.
pub fn build_sources() -> Vec<&'static str> {
    env!("CYCLING_BUILD_SOURCES").split(' ').collect()
}

fn build_id() -> [u8; 20] {
    origin::build(option_env!("CYCLING_BUILD_COMMIT").unwrap_or(""))
}

fn recorded_bulk<'a>(
    bulk: &'a mut crate::bulk::SimBulk,
    devices: &'a mut Devices,
    record: &'a RefCell<TraceRecorder>,
) -> RecordedBulk<'a, dyn ReadWrite + 'a, TraceRecorder> {
    let bulk: &mut dyn ReadWrite = match devices {
        Devices::Replayed(replay) => &mut replay.sources.bulk,
        Devices::Simulated(_) => bulk,
    };
    RecordedBulk::new(bulk, record)
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
    wifi_control: device_api::connectivity::ControlStatus,
    ble_control: device_api::connectivity::ControlStatus,
    pub phone_server: crate::phone::SimServer,
    pub access_point: crate::phone::SimAccessPoint,
}

impl Simulated {
    fn new() -> Self {
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
            switch: PowerSwitch::default(),
            barometer: None,
            wifi_control: device_api::connectivity::ControlStatus::new(),
            ble_control: device_api::connectivity::ControlStatus::new(),
            phone_server: Default::default(),
            access_point: Default::default(),
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
        self.switch = PowerSwitch::charging();
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
    Replayed(Box<Replay>),
}

/// The simulated device, composed like C606 firmware, and what it observes.
pub struct Session {
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
    /// The owned MMC region; absent unless a scenario fits one.
    pub bulk: crate::bulk::SimBulk,
    pub sdk: vana::app::Runtime,
    trace: Trace,
    origin: Option<Origin>,
    unrecorded: Option<(&'static str, u64)>,
    unseen_ms: u64,
    rest_ms: u64,
    /// `Edges::pushed` when the last pass began: an edge since ends a rest.
    edges_seen: u32,
    /// A terminal request ran since the last pass, as the C606's terminal
    /// reports traffic.
    host: bool,
    devices: Devices,
}
impl Session {
    pub fn new(root: &Path, width: usize, height: usize) -> io::Result<Self> {
        if !(16..=320).contains(&width) || !(16..=320).contains(&height) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "geometry must be 16..320",
            ));
        }
        Self::with_media(FileFlash::open(root)?, width, height, Memory::ALL)
    }
    /// A C606-sized device on memory media whose companion and ANT radio
    /// start shortly after boot.
    pub fn device() -> io::Result<Self> {
        let mut session = Self::with_media(FileFlash::memory(), 240, 320, Memory::ALL)?;
        let sim = session.sim_mut();
        sim.companion_starting(Availability::Ready);
        sim.restart_probe_ms = RESTART_PROBE_MS;
        Ok(session)
    }
    /// The world and the simulated devices. A replay has none.
    pub fn sim(&self) -> &Simulated {
        match &self.devices {
            Devices::Simulated(sim) => sim,
            Devices::Replayed(_) => panic!("a replay has no simulated world"),
        }
    }
    pub fn sim_mut(&mut self) -> &mut Simulated {
        match &mut self.devices {
            Devices::Simulated(sim) => sim,
            Devices::Replayed(_) => panic!("a replay has no simulated world"),
        }
    }
    pub fn is_replay(&self) -> bool {
        matches!(self.devices, Devices::Replayed(_))
    }
    fn with_media(
        media: FileFlash,
        width: usize,
        height: usize,
        memory: Memory,
    ) -> io::Result<Self> {
        crate::logs::install();
        let boot = media.begin_boot()?;
        let display = DisplayDevice::new(width, height);
        let input = InputDevice::new(
            &[Button::TopLeft, Button::BottomLeft, Button::BottomRight],
            Availability::Ready,
        );
        let power = PowerDevice::new(Availability::Ready);
        let mut shell = firmware_shell::shell::Shell::new(
            display.clone(),
            RecordedEdges::new(input.clone()),
            power.clone(),
            media.clone(),
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
            let view =
                vana::screens::workout::View::new(vana::screens::workout::Page::Boot, 0, None);
            shell.draw_canvas(240, 320, |row| view.paint(row));
        }
        shell.forget_frame();
        let mut sdk = vana::app::Runtime::new(profile);
        {
            use vana::ride::directory::{ENTRY, RIDES, RideIndex};
            let bytes = || Box::leak(vec![0u8; RIDES * ENTRY].into_boxed_slice());
            if memory.ride_index {
                sdk.attach_ride_index(Box::leak(Box::new(RideIndex::new(bytes(), bytes()))));
            }
            let facts = vec![0u8; vana::ride::facts::SHELF_BYTES].into_boxed_slice();
            if memory.ride_facts {
                sdk.attach_ride_facts(Box::leak(facts));
                let deleted = vec![0u8; vana::ride::deleted::SHELF_BYTES].into_boxed_slice();
                sdk.attach_deleted_rides(Box::leak(deleted));
            }
        }
        sdk.load_preferences(&shell.settings.app);
        // The look-ahead and its tile cache, which the device keeps in PSRAM;
        // each boot starts them over, as device memory does.
        if memory.terrain {
            sdk.attach_terrain(terrain_block());
        }
        if memory.phone {
            sdk.attach_phone(Box::leak(Box::new(vana::app::PhoneWork::new())));
        }
        // Back to start's breadcrumb, which the device keeps in PSRAM; each
        // boot gets a fresh one, as device memory starts over.
        if memory.back_to_start {
            let crumbs = Vec::leak(vec![vana::app::Crumb::default(); vana::app::BACK_CRUMBS]);
            let map = Box::leak(Box::new(vana::app::MapFrame::default()));
            let line = Vec::leak(vec![vana::app::Crumb::default(); vana::app::COURSE_POINTS]);
            let turns = Vec::leak(vec![vana::app::Turn::default(); vana::app::COURSE_TURNS]);
            let course = Box::leak(Box::new(vana::app::Course::empty()));
            let finder = Box::leak(Box::new(vana::app::CourseFinder::new()));
            let back = Box::new(vana::app::BackToStart::new(
                crumbs, line, turns, map, course, finder,
            ));
            sdk.attach_back(Box::leak(back));
        }
        let trace = Trace::new(Start {
            boot,
            profile: shell.settings.ble_profile,
            settings: shell.settings,
            memory,
            redacted: false,
            volumes: None,
            journals: None,
        });
        Ok(Self {
            shell,
            now: 0,
            boot,
            input,
            power,
            off: false,
            display,
            media,
            bulk: crate::bulk::SimBulk::default(),
            sdk,
            trace,
            origin: None,
            unrecorded: None,
            unseen_ms: 0,
            rest_ms: 0,
            edges_seen: 0,
            host: false,
            devices: Devices::Simulated(Box::new(Simulated::new())),
        })
    }
    /// A C606-sized device on the captured media, with the capture's trace
    /// standing in for the world. Drive it with `replay_step`.
    pub fn replaying(capture: &Capture, mode: Mode) -> io::Result<Self> {
        let origin = &capture.origin;
        let flash = origin.flash.copy_before_boot(origin.boot).ok_or_else(|| {
            io::Error::other("the capture's media is file-backed and cannot be replayed")
        })?;
        let config = firmware_services::trace::decode(&capture.trace)
            .ok()
            .and_then(|(header, _)| Config::decode(header.config));
        let memory = config.map_or(Memory::ALL, |config| config.memory);
        let mut session = Self::with_media(flash, 240, 320, memory)?;
        if config.is_some_and(|config| config.redacted) {
            session.redact_secrets();
        }
        session.bulk = origin.bulk.scripted();
        let shelled = Shelled {
            input: session.input.clone(),
            display: session.display.clone(),
            power: session.power.clone(),
            flash: session.media.clone(),
            bulk: session.bulk.clone(),
        };
        let replay = Replay::new(capture.trace.clone(), shelled, mode, build_id())
            .map_err(|error| io::Error::other(format!("trace: {error}")))?;
        session.devices = Devices::Replayed(Box::new(replay));
        Ok(session)
    }
    /// Run what the trace holds next: a pass or a terminal request. False
    /// once the trace is spent.
    pub fn replay_step(&mut self) -> bool {
        let Devices::Replayed(replay) = &mut self.devices else {
            return false;
        };
        match replay.step() {
            Next::Pass => self.tick(),
            Next::Command(line) => {
                let _ = self.command(line.as_bytes(), &mut String::new());
            }
            Next::End => {
                let trace = self.trace();
                if let Devices::Replayed(replay) = &mut self.devices {
                    replay.rerecorded(&trace);
                }
                return false;
            }
        }
        true
    }
    /// Where a replay parted from its recording so far.
    pub fn report(&self) -> Option<Report> {
        match &self.devices {
            Devices::Replayed(replay) => Some(replay.sources.report()),
            Devices::Simulated(_) => None,
        }
    }
    /// The devices a replay presents, to set up before it runs.
    pub fn replay_sources(&mut self) -> Option<&mut Sources> {
        match &mut self.devices {
            Devices::Replayed(replay) => Some(&mut replay.sources),
            Devices::Simulated(_) => None,
        }
    }
    /// What a replay of this session needs.
    pub fn capture(&self) -> Result<Capture, String> {
        if let Some((what, at)) = self.unrecorded {
            return Err(format!("{what} at {at} ms, unseen by the trace"));
        }
        Ok(Capture {
            origin: self.origin.clone().ok_or("no pass ran yet")?,
            trace: self.trace(),
        })
    }
    /// The bulk medium as a host writes it over USB. After the first pass
    /// such a write changes what VANA reads unseen by the trace, so this
    /// session no longer has a capture.
    pub fn host_bulk(&mut self) -> crate::bulk::SimBulk {
        self.unrecorded("a host wrote the bulk medium");
        self.bulk.host()
    }
    fn unrecorded(&mut self, what: &'static str) {
        if self.origin.is_some() {
            self.unrecorded.get_or_insert((what, self.now));
        }
    }
    /// Record secrets redacted, as the C606 does, from the next pass on and
    /// after every restart; see `firmware_services::trace::redact`.
    pub fn redact_secrets(&mut self) {
        self.trace.start.redacted = true;
        self.trace.access_point.redact();
    }
    /// The trace of this power-on session so far.
    pub fn trace(&self) -> Vec<u8> {
        self.trace.bytes()
    }
    /// One pass of the firmware loop, in the order C606 composition runs it.
    pub fn tick(&mut self) {
        match &mut self.devices {
            Devices::Replayed(replay) => self.now = replay.load_pass(),
            Devices::Simulated(sim) => {
                if self.origin.is_none() {
                    self.origin = self.media.copy_before_boot(self.boot).map(|flash| Origin {
                        flash,
                        bulk: self.bulk.copy(),
                        boot: self.boot,
                    });
                }
                if !self.off {
                    sim.environment(self.now, self.shell.settings.ble.is_some());
                }
            }
        }
        if !self.trace.spooling {
            self.trace.spool(&self.bulk);
        }
        self.edges_seen = self.input.edges.borrow().pushed();
        let quiet = self.quiet();
        // A replay runs only the passes the recording kept, so it keeps
        // each one, also one kept for its health and activity alone.
        if quiet && !self.is_replay() {
            self.trace.recorder.borrow_mut().quiet_pass(self.now);
        } else {
            self.trace.record(&Event::Pass { now_ms: self.now });
        }
        self.display.0.borrow_mut().refusals.begin_pass();
        self.power.0.borrow_mut().refusals.begin_pass();
        self.media.fault.borrow_mut().begin_pass();
        self.bulk.begin_pass();
        let stayed_quiet = self.pass(quiet);
        {
            let power = self.trace.power.frozen().state;
            let host = core::mem::take(&mut self.host);
            self.rest_ms = if self.off {
                0
            } else {
                self.sdk.pass_rest_ms(&self.shell, power, host, self.now)
            };
        }
        self.trace.window.passes += 1;
        if !self.off && self.trace.vitals.due(self.now, quiet) {
            self.record_vitals();
        }
        if stayed_quiet {
            return;
        }
        if self.trace.start.journals.is_none() {
            let journals = self
                .sdk
                .journals_at_boot()
                .map(|journals| (Some(journals.internal), Some(journals.bulk)));
            self.trace.start.journals = journals;
        }
        self.record_faults();
        let seen = self.record_seen();
        if let Devices::Replayed(replay) = &mut self.devices {
            replay.saw(&seen);
        }
        let card = spool::Card::of(
            self.card_admits(),
            self.sdk.recording(),
            self.sdk.shutdown_due(),
        );
        self.trace.drain(&self.bulk, card, self.now);
    }
    /// Whether the card admits work after this pass, as the C606's power
    /// owner admits it: while work is ready or the companion starts, and
    /// not from the moment a power request was accepted.
    fn card_admits(&self) -> bool {
        use device_api::power::PowerControl as _;
        let status = match &self.devices {
            Devices::Simulated(sim) => sim.switch.status(),
            Devices::Replayed(replay) => replay.sources.power.status(),
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
        let recorder = self.trace.recorder.borrow();
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
        let input = InputObservation::snapshot(&sim.battery, now);
        let (spool_used, spool_size) = self
            .trace
            .recorder
            .borrow()
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
        let access_point = AccessPoint::state(&sim.access_point);
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
                ant_scanning: Ant::scanning(&sim.ant),
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
    fn record_faults(&mut self) {
        let panel = core::mem::take(&mut self.display.0.borrow_mut().refusals.refused);
        let backlight = core::mem::take(&mut self.power.0.borrow_mut().refusals.refused);
        let flash = core::mem::take(&mut self.media.fault.borrow_mut().log);
        let bulk = self.bulk.take_log();
        let refused = (panel.into_iter().map(|attempt| (Output::Panel, attempt))).chain(
            backlight
                .into_iter()
                .map(|attempt| (Output::Backlight, attempt)),
        );
        for (output, attempt) in refused {
            self.trace.record(&Event::Refused { output, attempt });
        }
        for fault in flash.into_iter().chain(bulk) {
            self.trace.record(&Event::StorageFault(fault));
        }
    }
    fn startup(&self) -> Startup {
        match &self.devices {
            Devices::Simulated(sim) => sim.startup,
            Devices::Replayed(replay) => replay.sources.startup(),
        }
    }
    /// A pass in dark charging standby, as the C606 runs it: only the
    /// startup and power status are read, and only a change records it.
    fn quiet(&self) -> bool {
        self.trace.power.frozen().state == device_api::power::State::Charging
            && self.sdk.standby_dark()
    }
    /// One pass; true when it stayed quiet.
    fn pass(&mut self, quiet: bool) -> bool {
        let now = self.now;
        let freeze_companion = |session: &mut Self| {
            let record = &session.trace.recorder;
            match &session.devices {
                Devices::Replayed(replay) => {
                    session
                        .trace
                        .companion
                        .freeze(&replay.sources.companion, now, record)
                }
                Devices::Simulated(sim) => {
                    session.trace.companion.freeze(&sim.battery, now, record)
                }
            }
        };
        let companion = (!quiet).then(|| freeze_companion(self));
        if self.off {
            return false;
        }
        let startup = self.startup();
        let startup =
            self.trace
                .startup
                .observe(startup, now, &self.trace.recorder, Event::Startup);
        let record = &self.trace.recorder;
        let power = match &mut self.devices {
            Devices::Simulated(sim) => {
                sim.speaker.tick(now);
                observe_startup(&mut sim.switch, now, startup);
                (!sim.switch.off_due()).then(|| self.trace.power.freeze(&sim.switch, now, record))
            }
            Devices::Replayed(replay) => (!replay.sources.power.powered_off())
                .then(|| self.trace.power.freeze(&replay.sources.power, now, record)),
        };
        let Some(power) = power else {
            self.trace.record(&Event::PoweredOff);
            self.turn_off();
            return false;
        };
        if quiet && power.state == device_api::power::State::Charging {
            return true;
        }
        let companion = companion.unwrap_or_else(|| freeze_companion(self));
        self.power.0.borrow_mut().battery = firmware_services::trace::frozen::battery(&companion);
        // C606 composition scans the ride journal while the companion starts
        // and runs the shell and VANA only once power work is ready; until
        // then the backlight stays dark.
        if power.state == device_api::power::State::Initializing {
            self.sdk.prepare(
                &mut self.shell.data_storage(),
                &mut recorded_bulk(&mut self.bulk, &mut self.devices, &self.trace.recorder),
                now,
            );
        }
        if power.state == device_api::power::State::Charging {
            self.standby(companion);
        }
        if !power.ready {
            return false;
        }
        let record = &self.trace.recorder;
        self.trace.backlight.observe(
            self.power.0.borrow().available,
            now,
            record,
            Event::Backlight,
        );
        let position = match &self.devices {
            Devices::Replayed(replay) => {
                self.trace
                    .position
                    .freeze(&replay.sources.position, now, record)
            }
            Devices::Simulated(sim) => self.trace.position.freeze(&sim.position, now, record),
        };
        self.vana_pass(companion, position, startup);
        false
    }
    fn vana_pass(
        &mut self,
        companion: CompanionView,
        position: firmware_services::trace::frozen::PositionView,
        startup: Startup,
    ) {
        let now = self.now;
        let (physical, network_time) = match &mut self.devices {
            Devices::Replayed(replay) => (replay.sources.physical(), replay.sources.network_time()),
            Devices::Simulated(sim) => (
                sim.physical(now),
                sim.network_time
                    .map_or_else(Default::default, |anchor| anchor.snapshot(now, 0, true)),
            ),
        };
        let record = &self.trace.recorder;
        let physical = self
            .trace
            .physical
            .observe(physical, now, record, Event::Physical);
        let network_time =
            self.trace
                .network_time
                .observe(network_time, now, record, Event::NetworkTime);
        if self.trace.start.redacted {
            let token = self.sdk.phone_status().map(|status| status.token);
            self.trace
                .server
                .redact(token.as_ref().map(|token| token.bytes()));
        }
        if let Devices::Replayed(replay) = &mut self.devices {
            replay.sources.server.token = self.sdk.phone_status().map(|status| status.token);
        }
        type Handles<'a> = (
            &'a mut dyn Ant,
            &'a mut dyn Ble,
            &'a mut dyn ReadWrite,
            &'a mut dyn AccessPoint,
            &'a mut dyn Server,
            &'a mut dyn Sound,
            &'a mut dyn PowerControl,
        );
        let (ant, ble, bulk, access_point, server, sound, control): Handles =
            match &mut self.devices {
                Devices::Replayed(replay) => {
                    let sources = &mut replay.sources;
                    (
                        &mut sources.ant,
                        &mut sources.ble,
                        &mut sources.bulk,
                        &mut sources.access_point,
                        &mut sources.server,
                        &mut sources.sound,
                        &mut sources.power,
                    )
                }
                Devices::Simulated(sim) => (
                    &mut sim.ant,
                    &mut sim.ble,
                    &mut self.bulk,
                    &mut sim.access_point,
                    &mut sim.phone_server,
                    &mut sim.speaker,
                    &mut sim.switch,
                ),
            };
        let mut ant = self.trace.ant.bind(ant, now, record);
        let mut ble = self.trace.ble.bind(ble, now, record);
        let mut bulk = RecordedBulk::new(bulk, record);
        let mut access_point = self.trace.access_point.bind(access_point, now, record);
        let mut server = self.trace.server.bind(server, now, record);
        let mut sound = self.trace.sound.bind(sound, now, record);
        let mut control = self.trace.power.bind(control, now, record);
        self.sdk.set_startup_status(startup);
        self.shell.set_app_active(self.sdk.input_active());
        self.shell.tick(now);
        self.shell
            .input_source()
            .record(&mut *self.trace.recorder.borrow_mut());
        record_synthetic(&mut self.shell, &self.trace.recorder);
        for _ in 0..16 {
            let Some(edge) = self.shell.take_app_input() else {
                break;
            };
            self.sdk
                .input_received(edge.input, edge.received_ms, now, &mut ant);
        }
        self.shell.observe_position(&position, now);
        self.shell.present();
        self.sdk.tick(
            &mut self.shell.data_storage(),
            &mut bulk,
            now,
            &mut ant,
            &mut ble,
            vana::app::Observations {
                position: &position,
                network_time,
                input: &companion,
                physical,
            },
        );
        self.sdk.look_ahead(&mut bulk, now);
        self.sdk.serve_phone(
            &mut server,
            &mut access_point,
            &mut self.shell.data_storage(),
            &mut bulk,
            now,
        );
        self.sdk.present(&mut self.shell, now, &ant, &position);
        self.sdk.save_preferences(&mut self.shell, now);
        self.sdk.alert(&mut sound, now);
        self.sdk.power(&mut self.shell, &mut control, now);
    }
    fn record_seen(&mut self) -> Vec<Seen> {
        let mut seen = Vec::new();
        let mut notes = Vec::new();
        let mut recorder = self.trace.recorder.borrow_mut();
        let mut record = |event: Event<'_>| {
            recorder.record(&event);
            seen.extend(crate::replay::seen(&event).flatten());
        };
        self.sdk.report_seen(self.now, &mut |shown| match shown {
            vana::app::Seen::Page(page) => record(Event::Page(page)),
            vana::app::Seen::Toast(text) => record(Event::Toast(text)),
            vana::app::Seen::RideState(state) => record(Event::RideState(state)),
            vana::app::Seen::Shown(fields) => record(Event::Shown(fields)),
            vana::app::Seen::Note {
                cue,
                pattern,
                note,
                of,
            } => notes.push((pattern, cue.to_string(), note, of)),
        });
        for pattern in self.trace.sound.take_started() {
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
            record(Event::Frame(digest));
        }
        crate::logs::drain(|level, target, text| {
            recorder.record(&Event::Log {
                level,
                target,
                text,
            });
        });
        seen
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
        let mut frame = self.display.0.borrow_mut();
        frame.pixels.fill(0);
        frame.shown();
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
        let power = self.gate();
        Ok((request, self.execute_gated(request, power, out)))
    }
    /// Run a pass, then a parsed request, as the terminal does. Unlike
    /// `command`, the trace does not see the request, so a replay of this
    /// session will not either.
    pub fn execute(&mut self, request: Request, out: &mut String) -> &'static str {
        self.unrecorded("a request ran unrecorded");
        self.close_rest();
        let power = self.gate();
        self.execute_gated(request, power, out)
    }
    fn gate(&mut self) -> device_api::power::Status {
        let now = self.now;
        let record = &self.trace.recorder;
        match &mut self.devices {
            Devices::Simulated(sim) => {
                observe_startup(&mut sim.switch, now, sim.startup);
                self.trace.power.freeze(&sim.switch, now, record)
            }
            Devices::Replayed(replay) => {
                self.trace.power.freeze(&replay.sources.power, now, record)
            }
        }
    }
    fn execute_gated(
        &mut self,
        request: Request,
        power: device_api::power::Status,
        out: &mut String,
    ) -> &'static str {
        // A host talking on the terminal keeps the usual pace.
        self.rest_ms = 0;
        {
            self.host = true;
        }
        let status = self.respond(request, power, out);
        self.record_faults();
        status
    }
    fn respond(
        &mut self,
        request: Request,
        power: device_api::power::Status,
        out: &mut String,
    ) -> &'static str {
        let now = self.now;
        if !power.ready && !firmware_console::protocol::admitted_while_not_ready(request.command) {
            let startup = match &self.devices {
                Devices::Simulated(sim) => sim.startup,
                Devices::Replayed(_) => self.trace.startup.at(now).unwrap_or(Startup::Unsupported),
            };
            firmware_console::protocol::write_not_ready(out, power.state, startup);
            return "BUSY";
        }
        let status = match &mut self.devices {
            Devices::Simulated(sim) => {
                firmware_console::commands::ant::execute(request.command, &mut sim.ant, now, out)
            }
            Devices::Replayed(replay) => {
                let status =
                    firmware_console::commands::ant::execute(request.command, &mut NoAnt, now, out);
                if status.is_some() {
                    replay
                        .sources
                        .unrecorded("the ANT radio the terminal reads");
                }
                status
            }
        };
        if let Some(status) = status {
            return status;
        }
        if let Some(status) = firmware_console::commands::shell::execute(
            request.command,
            &mut self.shell,
            self.now,
            out,
        ) {
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
                out.push_str(",RIDE,EXPORT,RADAR,PROFILE,WORKOUT");
            }
            return status;
        }
        let (wifi_control, ble_control) = match &self.devices {
            Devices::Simulated(sim) => (sim.wifi_control, sim.ble_control),
            Devices::Replayed(replay) => {
                if matches!(request.command, Command::Wifi | Command::Ble) {
                    replay
                        .sources
                        .unrecorded("the radio control status the terminal reads");
                }
                (
                    device_api::connectivity::ControlStatus::new(),
                    device_api::connectivity::ControlStatus::new(),
                )
            }
        };
        match request.command {
            // The simulator has no binary USB session; tests install volume
            // contents through `Sim` instead.
            Command::Volume(firmware_console::commands::volumes::Command::Transfer) => {
                return "UNSUPPORTED";
            }
            Command::Volume(command) => {
                return firmware_console::commands::volumes::execute(
                    command,
                    &mut recorded_bulk(&mut self.bulk, &mut self.devices, &self.trace.recorder),
                    out,
                );
            }
            Command::Info => {
                let _ = write!(
                    out,
                    "board=virtual commit={} protocol=1 boot={} physical_evidence=unsupported",
                    option_env!("CYCLING_BUILD_COMMIT").unwrap_or("unknown"),
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
                    wifi_control.operation.name(),
                    wifi_control.sequence,
                    wifi_control.error_name()
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
                if let Devices::Simulated(sim) = &mut self.devices {
                    sim.wifi_control.sequence = sim.wifi_control.sequence.wrapping_add(1);
                    sim.wifi_control.operation = device_api::connectivity::Operation::Completed;
                }
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
                if let Devices::Simulated(sim) = &mut self.devices {
                    sim.ble.packets.clear();
                    sim.ble.snapshot.link = device_api::ble_transport::Link::Off;
                    sim.ble_control.sequence = sim.ble_control.sequence.wrapping_add(1);
                    sim.ble_control.operation = device_api::connectivity::Operation::Completed;
                }
                self.sdk.select_profile(
                    vana::sensors::ble_profile::Profile::Echo,
                    self.trace.ble.snapshot().connections,
                );
                return "ACCEPTED";
            }
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
                if let Devices::Simulated(sim) = &mut self.devices {
                    sim.ble.packets.clear();
                    sim.ble.snapshot.link = device_api::ble_transport::Link::Off;
                    sim.ble_control.sequence = sim.ble_control.sequence.wrapping_add(1);
                    sim.ble_control.operation = device_api::connectivity::Operation::Completed;
                }
                self.sdk
                    .select_profile(profile, self.trace.ble.snapshot().connections);
                return "ACCEPTED";
            }
            Command::Ble => {
                let s = match &self.devices {
                    Devices::Simulated(sim) => sim.ble.snapshot(),
                    Devices::Replayed(_) => self.trace.ble.snapshot(),
                };
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
                    ble_control.operation.name(),
                    ble_control.sequence,
                    ble_control.error_name()
                );
            }
            Command::Domain { bytes, length } => {
                return self.sdk.command(
                    core::str::from_utf8(&bytes[..length]).unwrap_or(""),
                    &mut self.shell.data_storage(),
                    &mut recorded_bulk(&mut self.bulk, &mut self.devices, &self.trace.recorder),
                    self.now,
                    out,
                );
            }
            Command::Restart => return "ACCEPTED",
            // The simulator never panics into a record.
            Command::Crash => {
                let _ = write!(out, "none");
            }
            Command::CrashClear => {}
            _ => return "UNSUPPORTED",
        }
        "OK"
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
    /// Power lost and restored: the companion boots again too.
    pub fn power_cycle(&mut self) -> io::Result<()> {
        self.reboot(true)
    }
    /// Reboot on the same media. The world, sensors and receiver presence
    /// persist; everything held in device memory starts over.
    fn reboot(&mut self, power: bool) -> io::Result<()> {
        let g = self.display.geometry();
        let mut next = Self::with_media(self.media.clone(), g.width, g.height, Memory::ALL)?;
        if self.display.0.borrow().hashes.is_some() {
            next.display.0.borrow_mut().hashes = Some(Vec::new());
        }
        if self.trace.start.redacted {
            next.redact_secrets();
        }
        self.bulk.begin_boot();
        next.bulk = self.bulk.clone();
        let uptime = self.now;
        let (sim, next_sim) = (self.sim_mut(), next.sim_mut());
        next_sim.world = core::mem::take(&mut sim.world);
        let ready = sim.ant_ready_availability();
        core::mem::swap(&mut next_sim.ant, &mut sim.ant);
        if power {
            next_sim.ant.power_cycled();
            if sim.companion_boot_ms.is_some()
                || matches!(sim.startup, Startup::Ready | Startup::AlreadyRunning)
            {
                next_sim.companion_starting(ready);
            }
        } else {
            next_sim.ant.host_restarted();
            next_sim.startup = if sim.startup == Startup::Ready {
                Startup::AlreadyRunning
            } else {
                sim.startup
            };
            // A CPU reset does not stop a companion that is still booting;
            // the new uptime starts at zero, so carry what is left.
            next_sim.companion_boot_ms = sim
                .companion_boot_ms
                .map(|(at, ant)| (at.saturating_sub(uptime), ant));
            let companion_running = next_sim.companion_boot_ms.is_none();
            if companion_running && sim.restart_probe_ms > 0 {
                next_sim.restart_probe = Some((sim.restart_probe_ms, next_sim.startup));
                next_sim.startup = Startup::Probing;
            }
        }
        next_sim.restart_probe_ms = sim.restart_probe_ms;
        next_sim.gps = sim.gps.take().map(|mut gps| {
            gps.reset();
            gps
        });
        next_sim.position.missing = sim.position.missing;
        next_sim.battery = sim.battery;
        next_sim.ble_strap = sim.ble_strap.take().map(|mut strap| {
            strap.reset();
            strap
        });
        next_sim.speaker.played = core::mem::take(&mut sim.speaker.played);
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
                    restart = request.command == Command::Restart;
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
