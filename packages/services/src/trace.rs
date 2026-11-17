//! Ride trace: every input VANA consumed during one power-on session, and a
//! thin layer of what the rider saw, as a compact binary stream.
//!
//! A trace is a run of blocks, each `[payload length: u16 LE][payload][CRC32
//! LE of the payload]`. The first block holds the [`Header`]; every later one
//! holds whole event records. A block cut short or failing its CRC ends the
//! trace, so a torn tail is dropped rather than misread. Each event block opens
//! with the pass clock, so a block decodes without the ones before it.
//!
//! A record byte with the top bit set is a [`Event::Pass`] 0 to 127 ms after
//! the previous pass. Any other record is a tag byte, the body length as a
//! varint and the body. Times inside a record are stored relative to the
//! current pass.
use crate::checksum::crc32;
use device_api::ant;
use device_api::ble_transport;
use device_api::input::{Edge, Input, InputSnapshot};
use device_api::observation::{Availability, Observation};
use device_api::positioning;
use device_api::power;
use device_api::sensors;
use device_api::server::{self, AccessPointState, Method, Reach};
use device_api::sound;

pub mod determined;
pub mod frozen;
pub mod origin;
pub mod redact;
pub mod shell;
pub mod spool;
#[cfg(feature = "testing")]
pub mod tapped;
#[cfg(test)]
mod tests;
pub mod wire;

use wire::{Reader, Wire, Writer};

/// The header's first byte.
pub const VERSION: u8 = 5;
pub const MAX_BLOCK: usize = u16::MAX as usize + FRAME;
const FRAME: usize = 2 + 4;

/// What a replay needs before the first pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header<'a> {
    /// The recording build's content hash (`cycling_devtools.build_hash`),
    /// zero when unknown.
    pub build: [u8; 20],
    /// The manifest name of the device that recorded it, such as `c606`.
    pub device: &'a str,
    pub boot: u32,
    /// The BLE profile selector the shell settings stored.
    pub profile: u8,
    /// Preferences, shell settings and saved sensors, as the owners encode them.
    pub config: &'a [u8],
    /// Where each journal and file store stood at power-on.
    pub media: &'a [u8],
}

/// The ANT radio's capabilities; the decoded type list borrows the trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AntCapabilities<'a> {
    pub supported_types: &'a [u8],
    pub connection_capacity: u8,
    pub one_peer_per_type: bool,
    pub concurrent_scan: bool,
    pub acknowledged_send: bool,
    pub radio_delivery_feedback: bool,
    pub burst: bool,
    pub max_pending_sends: u8,
}

impl From<ant::Capabilities> for AntCapabilities<'static> {
    fn from(c: ant::Capabilities) -> Self {
        Self {
            supported_types: c.supported_types,
            connection_capacity: c.connection_capacity,
            one_peer_per_type: c.one_peer_per_type,
            concurrent_scan: c.concurrent_scan,
            acknowledged_send: c.acknowledged_send,
            radio_delivery_feedback: c.radio_delivery_feedback,
            burst: c.burst,
            max_pending_sends: c.max_pending_sends,
        }
    }
}

impl AntCapabilities<'_> {
    /// The device-api form, with `supported_types` standing in for the list.
    pub fn with_types(&self, supported_types: &'static [u8]) -> ant::Capabilities {
        ant::Capabilities {
            supported_types,
            connection_capacity: self.connection_capacity,
            one_peer_per_type: self.one_peer_per_type,
            concurrent_scan: self.concurrent_scan,
            acknowledged_send: self.acknowledged_send,
            radio_delivery_feedback: self.radio_delivery_feedback,
            burst: self.burst,
            max_pending_sends: self.max_pending_sends,
        }
    }
}

/// The radio-wide ANT state VANA reads, recorded when it changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AntRadio<'a> {
    pub availability: Availability,
    pub capabilities: AntCapabilities<'a>,
    pub scanning: bool,
    pub scan: ant::ScanSnapshot,
}

/// A request head as the phone server presents it; its text borrows the trace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ServerRequest<'a> {
    pub id: u32,
    pub method: Method,
    pub path: &'a str,
    pub query: &'a str,
    pub session: &'a str,
    pub gzip: bool,
    pub body: u32,
}

impl<'a> From<&'a server::Request> for ServerRequest<'a> {
    fn from(request: &'a server::Request) -> Self {
        Self {
            id: request.id,
            method: request.method,
            path: &request.path,
            query: &request.query,
            session: &request.session,
            gzip: request.gzip,
            body: request.body,
        }
    }
}

impl ServerRequest<'_> {
    /// The device-api form. Text longer than the server holds is cut, as
    /// the transport would.
    pub fn to_request(&self) -> server::Request {
        fn cut<const N: usize>(text: &str) -> heapless::String<N> {
            let mut out = heapless::String::new();
            for c in text.chars() {
                if out.push(c).is_err() {
                    break;
                }
            }
            out
        }
        server::Request {
            id: self.id,
            method: self.method,
            path: cut(self.path),
            query: cut(self.query),
            session: cut(self.session),
            gzip: self.gzip,
            body: self.body,
        }
    }
}

/// Where a storage operation failed: a region of internal flash, or the
/// owned bulk medium.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Medium {
    Configuration,
    Data,
    Crash,
    Bulk,
    Intent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageOp {
    Read,
    Write,
    Erase,
    /// The bulk medium's size and place.
    Info,
}

/// A storage error, whichever medium gave it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError {
    Unsupported,
    Unavailable,
    Failed,
    Invalid,
    Range,
}

/// A storage operation that failed, and how many of its bytes reached the
/// medium first. `attempt` is its place among the medium's operations since
/// the pass began; `at` is a byte offset into a flash region, or a sector of
/// the bulk medium.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageFault {
    pub medium: Medium,
    pub attempt: u32,
    pub op: StorageOp,
    pub at: u64,
    pub landed: u32,
    pub error: StorageError,
}

impl From<device_api::storage::Region> for Medium {
    fn from(region: device_api::storage::Region) -> Self {
        match region {
            device_api::storage::Region::Configuration => Self::Configuration,
            device_api::storage::Region::Data => Self::Data,
            device_api::storage::Region::Crash => Self::Crash,
            device_api::storage::Region::Intent => Self::Intent,
        }
    }
}

/// Named rather than `From`, both ways: a `From` between these and the
/// device errors leaves `?` and `.into()` in firmware code with two
/// conversions to choose from (E0283).
impl StorageError {
    pub fn from_flash(error: device_api::observation::Error) -> Self {
        use device_api::observation::Error;
        match error {
            Error::Unsupported => Self::Unsupported,
            Error::Unavailable => Self::Unavailable,
            Error::Failed => Self::Failed,
            Error::Invalid => Self::Invalid,
        }
    }

    pub fn from_bulk(error: device_api::bulk::Error) -> Self {
        use device_api::bulk::Error;
        match error {
            Error::Unsupported => Self::Unsupported,
            Error::Unavailable => Self::Unavailable,
            Error::Range => Self::Range,
            Error::Failed => Self::Failed,
        }
    }

    pub fn flash(self) -> device_api::observation::Error {
        use device_api::observation::Error;
        match self {
            Self::Unsupported => Error::Unsupported,
            Self::Unavailable => Error::Unavailable,
            Self::Failed => Error::Failed,
            Self::Invalid | Self::Range => Error::Invalid,
        }
    }

    pub fn bulk(self) -> device_api::bulk::Error {
        use device_api::bulk::Error;
        match self {
            Self::Unsupported => Error::Unsupported,
            Self::Unavailable => Error::Unavailable,
            Self::Range | Self::Invalid => Error::Range,
            Self::Failed => Error::Failed,
        }
    }
}

/// An output the shell drives that can refuse.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    Panel,
    Backlight,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Error,
    Warn,
}

/// How often [`Health`] and [`Activity`] are recorded, and how often in
/// dark charging standby, where nothing else records and the card stays
/// closed: hours of standby still fit the spool.
pub const VITALS_MS: u64 = 5_000;
pub const QUIET_VITALS_MS: u64 = 60_000;

/// When the next [`Health`] and [`Activity`] records are due.
#[derive(Clone, Copy, Debug, Default)]
pub struct Vitals {
    last_ms: u64,
}

impl Vitals {
    /// Whether a pass at `now` records them; `quiet` for a pass in dark
    /// charging standby.
    pub fn due(&mut self, now: u64, quiet: bool) -> bool {
        let every = if quiet { QUIET_VITALS_MS } else { VITALS_MS };
        if now.saturating_sub(self.last_ms) < every {
            return false;
        }
        self.last_ms = now;
        true
    }
}

/// How the device fared since the record before. Not an input: a replay
/// reads past it and compares nothing. Fields a source cannot know, as the
/// simulator cannot know its heap, are absent.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// The simulator wrote it.
    pub simulated: bool,
    pub heap_free: Option<u32>,
    /// The least heap free sampled since power-on.
    pub heap_min: Option<u32>,
    pub psram_free: Option<u32>,
    /// The slowest frame the panel took and the slowest main-loop pass
    /// since the record before.
    pub frame_max_us: Option<u32>,
    pub pass_max_us: Option<u32>,
    /// Counts since power-on: NMEA sentences that decoded, receive DMA
    /// overflows and UART errors.
    pub gps_sentences: u32,
    pub gps_overflows: u32,
    pub gps_uart_errors: u32,
    /// Companion frames that failed their check, UART errors and lost
    /// input, since power-on.
    pub companion_errors: u32,
    /// The slowest card write since the record before, and the card's
    /// recoveries since power-on.
    pub card_write_max_us: Option<u32>,
    pub card_recoveries: Option<u32>,
    /// Bytes the trace's spool holds and can hold; zero without a spool.
    pub spool_used: u32,
    pub spool_size: u32,
    pub wifi: Option<device_api::connectivity::WifiState>,
    pub wifi_rssi: Option<i8>,
    /// The chosen BLE sensor's signal at its last advertisement.
    pub ble_rssi: Option<i8>,
}

/// The frames the panel took in the window of the [`Event::Health`] that
/// follows.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameTime {
    pub frames: u32,
    pub mean_us: u32,
}

impl FrameTime {
    pub fn of_window(frames: u32, total_us: u64) -> Option<Self> {
        let mean_us = total_us.checked_div(u64::from(frames))?;
        Some(Self {
            frames,
            mean_us: u32::try_from(mean_us).unwrap_or(u32::MAX),
        })
    }
}

/// Where the CPU's time went since the record before, and what drew power
/// as the window ended. Not an input. Its executor tasks follow it as
/// [`Event::Task`] records, then the main-loop pass's stages as
/// [`Event::Stage`] records. Times are µs; an interrupt counts in whatever
/// it interrupted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Activity {
    /// The simulator wrote it: its tasks are its own passes, and it has no
    /// CPU times.
    pub simulated: bool,
    pub window_ms: u32,
    /// Times the executor woke to poll its tasks, and how long it ran then.
    pub wakes: u32,
    pub awake_us: Option<u32>,
    /// The RTOS thread the executor runs in, the radio drivers' threads,
    /// and the idle hook waiting for an interrupt.
    pub main_us: Option<u32>,
    pub radio_us: Option<u32>,
    pub idle_us: Option<u32>,
    /// RTOS thread switches and timer interrupts.
    pub switches: Option<u32>,
    pub ticks: Option<u32>,
    /// What counting all this cost, estimated from the hooks' calls.
    pub hooks_us: Option<u32>,
    pub consumers: Consumers,
}

/// What draws power, as it stood when an [`Activity`] window ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Consumers {
    pub cpu_mhz: Option<u16>,
    /// Percent; zero is a dark screen.
    pub backlight: u8,
    /// Frames the panel took in the window.
    pub frames: u32,
    pub gnss: Gnss,
    pub wifi: Option<device_api::connectivity::WifiState>,
    pub access_point: AccessPointState,
    pub ble: ble_transport::Link,
    pub ant: Availability,
    pub ant_scanning: bool,
    /// ANT channels linked to a sensor.
    pub ant_channels: u8,
    /// Light sleep in the window, ms.
    pub slept_ms: u32,
}

impl Default for Consumers {
    fn default() -> Self {
        Self {
            cpu_mhz: None,
            backlight: 0,
            frames: 0,
            gnss: Gnss::Off,
            wifi: None,
            access_point: AccessPointState::Off,
            ble: ble_transport::Link::Off,
            ant: Availability::Unsupported,
            ant_scanning: false,
            ant_channels: 0,
            slept_ms: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Gnss {
    Off,
    Receiving,
    /// Paused to save power, or its pause or resume is under way.
    Paused,
    Failed,
}

wire::codes! {
    crate::network_time::Status { Unavailable = 0, Syncing = 1, Fresh = 2, Offline = 3, Stale = 4 }
    Medium { Configuration = 0, Data = 1, Crash = 2, Bulk = 3, Intent = 4 }
    StorageOp { Read = 0, Write = 1, Erase = 2, Info = 3 }
    StorageError { Unsupported = 0, Unavailable = 1, Failed = 2, Invalid = 3, Range = 4 }
    Output { Panel = 0, Backlight = 1 }
    Level { Error = 0, Warn = 1 }
    Gnss { Off = 0, Receiving = 1, Paused = 2, Failed = 3 }
}

wire::record! {
    crate::network_time::Snapshot { unix_seconds, millis, local_minutes, age_ms, status }
    AntRadio<'a> { availability, capabilities, scanning, scan }
    ServerRequest<'a> { id, method, path, query, session, gzip, body }
    StorageFault { medium, attempt, op, at, landed, error }
    AntCapabilities<'a> {
        supported_types, connection_capacity, one_peer_per_type, concurrent_scan,
        acknowledged_send, radio_delivery_feedback, burst, max_pending_sends,
    }
    FrameTime { frames, mean_us }
    Health {
        simulated, heap_free, heap_min, psram_free, frame_max_us, pass_max_us, gps_sentences,
        gps_overflows, gps_uart_errors, companion_errors, card_write_max_us, card_recoveries,
        spool_used, spool_size, wifi, wifi_rssi, ble_rssi,
    }
    Consumers {
        cpu_mhz, backlight, frames, gnss, wifi, access_point, ble, ant, ant_scanning,
        ant_channels, slept_ms,
    }
    Activity {
        simulated, window_ms, wakes, awake_us, main_us, radio_us, idle_us, switches, ticks,
        hooks_us, consumers,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// One main-loop pass began at this uptime.
    Pass {
        now_ms: u64,
    },
    Input(Edge),
    AntRadio(AntRadio<'a>),
    AntDiscoveries([Option<ant::Discovery>; ant::DISCOVERY_CAPACITY]),
    AntLink {
        channel: u8,
        snapshot: Option<ant::Snapshot>,
    },
    AntPacket(ant::Packet),
    /// An ANT request VANA made and the radio's answer.
    AntRequest {
        operation: ant::AntOperation,
        result: Result<ant::Admission, ant::Error>,
    },
    /// The answer to the pass's next send status query.
    AntSend(Option<ant::SendObservation>),
    BleLink {
        availability: Availability,
        snapshot: ble_transport::Snapshot,
    },
    BlePacket(ble_transport::Packet),
    BleBattery(ble_transport::BatteryLevel),
    Position {
        availability: Availability,
        snapshot: Option<positioning::Snapshot>,
    },
    Physical(sensors::Snapshot),
    Light(Observation<sensors::Light>),
    Power(Option<InputSnapshot>),
    /// UTC from Wi-Fi synchronization, recorded when it no longer reads as
    /// the last recorded value advanced to this pass.
    NetworkTime(crate::network_time::Snapshot),
    Startup(sensors::Startup),
    /// A terminal request line, executed after the pass before it.
    Command(&'a str),
    StorageFault(StorageFault),
    /// A hardware random value VANA consumed, in the order it asked.
    Entropy(u64),
    /// A record with this tag was larger than a block and left out.
    TooLarge(u8),
    /// A host wrote media VANA reads, such as FILES or TERRAIN in a
    /// transfer session. The trace ends here: what follows ran on media no
    /// header names.
    HostWrote,
    /// Blocks of events the recorder sealed and its sink had no room for,
    /// since the last block it took. They are missing just before this.
    Lost {
        blocks: u32,
        bytes: u32,
    },
    /// What the phone access point reports, recorded when it changes.
    AccessPoint {
        state: AccessPointState,
        reach: Reach,
    },
    /// The access point's answer to VANA starting or stopping it.
    AccessPointRequest {
        start: bool,
        result: Result<(), device_api::observation::Error>,
    },
    /// The request the phone server holds and its room for the response,
    /// recorded when either changes.
    Server {
        request: Option<ServerRequest<'a>>,
        room: u32,
    },
    /// VANA's listen or finish, recorded when it changed what the server
    /// holds; the change follows.
    ServerListen(bool),
    ServerFinish(u32),
    /// Body bytes one read gave VANA, in parts that each fit a block; every
    /// part but the last has `more`.
    ServerRead {
        id: u32,
        part: &'a [u8],
        more: bool,
    },
    /// How many response bytes the server took from one write.
    ServerWrite {
        id: u32,
        taken: u32,
    },
    /// What the buzzer reports, recorded when it changes.
    SoundState(sound::Snapshot),
    /// VANA asked the buzzer to play a pattern, or to stop, and its answer.
    SoundRequest {
        play: Option<u8>,
        result: Result<(), device_api::observation::Error>,
    },
    /// What power control reports, recorded when it changes.
    PowerControl(power::Status),
    PowerRequest {
        operation: power::Operation,
        delay_ms: u32,
        result: Result<(), device_api::observation::Error>,
    },
    /// The device's power went away after an accepted shutdown.
    PoweredOff,
    /// Whether the backlight works, recorded when it changes.
    Backlight(Availability),
    /// The output refused the shell's request at this attempt of the pass.
    Refused {
        output: Output,
        attempt: u8,
    },
    /// The page the rider sees, by its wire name.
    Page(&'a str),
    Toast(&'a str),
    /// A sound pattern started, with the cue VANA played it for by name and
    /// the note's place in that cue (`note` of `of`, from 1). A pattern no
    /// cue played, such as a console `SOUND PLAY`, has an empty cue and
    /// zeros.
    Sound {
        pattern: u8,
        cue: &'a str,
        note: u8,
        of: u8,
    },
    RideState(&'a str),
    /// The rider held MOVE to mark a moment: the mark's number in the
    /// session, from 1, when the hold's first report came, and the page
    /// shown before it.
    Mark {
        count: u16,
        pressed_ms: u64,
        page: &'a str,
    },
    /// The displayed fields, about once a second.
    Shown(&'a str),
    /// A digest of the pixels on the panel at the end of a pass, when they
    /// changed since the last: each 4-row band digested as the shell last
    /// sent it, then the bands top to bottom. It does not depend on which
    /// rows a frame sent.
    Panel(u32),
    /// A diagnostic harness's synthetic press or touch the shell handed on
    /// like a physical one. A replay runs the harness command that
    /// scheduled it, so this is never replayed as an input.
    Synthetic(Input),
    Log {
        level: Level,
        target: &'a str,
        text: &'a str,
    },
    FrameTime(FrameTime),
    Health(Health),
    Activity(Activity),
    /// One executor task's share of the [`Event::Activity`] window before
    /// it. A task that never ran in the window has none.
    Task {
        name: &'a str,
        polls: u32,
        busy_us: Option<u32>,
    },
    /// One stage of the main-loop pass, a part of the `console` task's
    /// share of the [`Event::Activity`] window before it: the passes that
    /// ran it, their time in it and the slowest one's.
    Stage {
        name: &'a str,
        runs: u32,
        busy_us: u32,
        max_us: u32,
    },
}

mod tag {
    pub const RESUME: u8 = 0;
    pub const PASS: u8 = 1;
    pub const INPUT: u8 = 2;
    pub const ANT_RADIO: u8 = 3;
    pub const ANT_DISCOVERIES: u8 = 4;
    pub const ANT_LINK: u8 = 5;
    pub const ANT_PACKET: u8 = 6;
    pub const ANT_REQUEST: u8 = 7;
    pub const ANT_SEND: u8 = 8;
    pub const BLE_LINK: u8 = 9;
    pub const BLE_PACKET: u8 = 10;
    pub const BLE_BATTERY: u8 = 11;
    pub const POSITION: u8 = 12;
    pub const PHYSICAL: u8 = 13;
    pub const POWER: u8 = 14;
    pub const NETWORK_TIME: u8 = 16;
    pub const STARTUP: u8 = 17;
    pub const COMMAND: u8 = 18;
    pub const STORAGE_FAULT: u8 = 19;
    pub const ENTROPY: u8 = 20;
    pub const TOO_LARGE: u8 = 21;
    pub const ACCESS_POINT: u8 = 22;
    pub const ACCESS_POINT_REQUEST: u8 = 23;
    pub const SERVER: u8 = 24;
    pub const SERVER_LISTEN: u8 = 25;
    pub const SERVER_FINISH: u8 = 26;
    pub const SERVER_READ: u8 = 27;
    pub const SERVER_WRITE: u8 = 28;
    pub const SOUND_STATE: u8 = 29;
    pub const SOUND_REQUEST: u8 = 30;
    pub const POWER_CONTROL: u8 = 31;
    pub const POWER_REQUEST: u8 = 32;
    pub const POWERED_OFF: u8 = 33;
    pub const BACKLIGHT: u8 = 34;
    pub const REFUSED: u8 = 35;
    pub const LOST: u8 = 36;
    pub const HOST_WROTE: u8 = 37;
    pub const LIGHT: u8 = 38;
    pub const PAGE: u8 = 64;
    pub const TOAST: u8 = 65;
    pub const SOUND: u8 = 66;
    pub const RIDE_STATE: u8 = 67;
    pub const SHOWN: u8 = 68;
    pub const SYNTHETIC: u8 = 70;
    pub const LOG: u8 = 69;
    pub const PANEL: u8 = 72;
    pub const MARK: u8 = 73;
    pub const HEALTH: u8 = 80;
    pub const ACTIVITY: u8 = 81;
    pub const TASK: u8 = 82;
    pub const STAGE: u8 = 83;
    pub const FRAME_TIME: u8 = 84;
}

const SHORT_PASS: u8 = 0x80;

impl<'a> Wire<'a> for Header<'a> {
    fn put(&self, w: &mut Writer) {
        VERSION.put(w);
        w.bytes(&self.build);
        self.device.put(w);
        self.boot.put(w);
        self.profile.put(w);
        self.config.put(w);
        self.media.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        u8::take(r);
        Self {
            build: <[u8; 20]>::take(r),
            device: <&str>::take(r),
            boot: u32::take(r),
            profile: u8::take(r),
            config: <&[u8]>::take(r),
            media: <&[u8]>::take(r),
        }
    }
}

impl<'a> Event<'a> {
    pub fn tag(&self) -> u8 {
        match self {
            Self::Pass { .. } => tag::PASS,
            Self::Input(_) => tag::INPUT,
            Self::AntRadio(_) => tag::ANT_RADIO,
            Self::AntDiscoveries(_) => tag::ANT_DISCOVERIES,
            Self::AntLink { .. } => tag::ANT_LINK,
            Self::AntPacket(_) => tag::ANT_PACKET,
            Self::AntRequest { .. } => tag::ANT_REQUEST,
            Self::AntSend(_) => tag::ANT_SEND,
            Self::BleLink { .. } => tag::BLE_LINK,
            Self::BlePacket(_) => tag::BLE_PACKET,
            Self::BleBattery(_) => tag::BLE_BATTERY,
            Self::Position { .. } => tag::POSITION,
            Self::Physical(_) => tag::PHYSICAL,
            Self::Light(_) => tag::LIGHT,
            Self::Power(_) => tag::POWER,
            Self::NetworkTime(_) => tag::NETWORK_TIME,
            Self::Startup(_) => tag::STARTUP,
            Self::Command(_) => tag::COMMAND,
            Self::StorageFault(_) => tag::STORAGE_FAULT,
            Self::Entropy(_) => tag::ENTROPY,
            Self::TooLarge(_) => tag::TOO_LARGE,
            Self::AccessPoint { .. } => tag::ACCESS_POINT,
            Self::AccessPointRequest { .. } => tag::ACCESS_POINT_REQUEST,
            Self::Server { .. } => tag::SERVER,
            Self::ServerListen(_) => tag::SERVER_LISTEN,
            Self::ServerFinish(_) => tag::SERVER_FINISH,
            Self::ServerRead { .. } => tag::SERVER_READ,
            Self::ServerWrite { .. } => tag::SERVER_WRITE,
            Self::SoundState(_) => tag::SOUND_STATE,
            Self::SoundRequest { .. } => tag::SOUND_REQUEST,
            Self::PowerControl(_) => tag::POWER_CONTROL,
            Self::PowerRequest { .. } => tag::POWER_REQUEST,
            Self::PoweredOff => tag::POWERED_OFF,
            Self::Backlight(_) => tag::BACKLIGHT,
            Self::Refused { .. } => tag::REFUSED,
            Self::Lost { .. } => tag::LOST,
            Self::HostWrote => tag::HOST_WROTE,
            Self::Page(_) => tag::PAGE,
            Self::Toast(_) => tag::TOAST,
            Self::Sound { .. } => tag::SOUND,
            Self::RideState(_) => tag::RIDE_STATE,
            Self::Mark { .. } => tag::MARK,
            Self::Shown(_) => tag::SHOWN,
            Self::Synthetic(_) => tag::SYNTHETIC,
            Self::Panel(_) => tag::PANEL,
            Self::Log { .. } => tag::LOG,
            Self::FrameTime(_) => tag::FRAME_TIME,
            Self::Health(_) => tag::HEALTH,
            Self::Activity(_) => tag::ACTIVITY,
            Self::Task { .. } => tag::TASK,
            Self::Stage { .. } => tag::STAGE,
        }
    }

    fn put_body(&self, w: &mut Writer, previous_pass: u64) {
        match self {
            Self::Pass { now_ms } => w.zigzag(now_ms.wrapping_sub(previous_pass) as i64),
            Self::Input(edge) => edge.put(w),
            Self::AntRadio(radio) => radio.put(w),
            Self::AntDiscoveries(found) => found.put(w),
            Self::AntLink { channel, snapshot } => {
                channel.put(w);
                snapshot.put(w);
            }
            Self::AntPacket(packet) => packet.put(w),
            Self::AntRequest { operation, result } => {
                operation.put(w);
                result.put(w);
            }
            Self::AntSend(observation) => observation.put(w),
            Self::BleLink {
                availability,
                snapshot,
            } => {
                availability.put(w);
                snapshot.put(w);
            }
            Self::BlePacket(packet) => packet.put(w),
            Self::BleBattery(level) => level.put(w),
            Self::Position {
                availability,
                snapshot,
            } => {
                availability.put(w);
                snapshot.put(w);
            }
            Self::Physical(snapshot) => snapshot.put(w),
            Self::Light(light) => light.put(w),
            Self::Power(snapshot) => snapshot.put(w),
            Self::NetworkTime(snapshot) => snapshot.put(w),
            Self::Startup(startup) => startup.put(w),
            Self::Command(text)
            | Self::Page(text)
            | Self::Toast(text)
            | Self::RideState(text)
            | Self::Shown(text) => text.put(w),
            Self::StorageFault(fault) => fault.put(w),
            Self::AccessPoint { state, reach } => {
                state.put(w);
                reach.put(w);
            }
            Self::AccessPointRequest { start, result } => {
                start.put(w);
                result.put(w);
            }
            Self::Server { request, room } => {
                request.put(w);
                room.put(w);
            }
            Self::ServerListen(on) => on.put(w),
            Self::ServerFinish(id) => id.put(w),
            Self::ServerRead { id, part, more } => {
                id.put(w);
                part.put(w);
                more.put(w);
            }
            Self::ServerWrite { id, taken } => {
                id.put(w);
                taken.put(w);
            }
            Self::SoundState(snapshot) => snapshot.put(w),
            Self::SoundRequest { play, result } => {
                play.put(w);
                result.put(w);
            }
            Self::PowerControl(status) => status.put(w),
            Self::PowerRequest {
                operation,
                delay_ms,
                result,
            } => {
                operation.put(w);
                delay_ms.put(w);
                result.put(w);
            }
            Self::PoweredOff => {}
            Self::Backlight(availability) => availability.put(w),
            Self::HostWrote => {}
            Self::Lost { blocks, bytes } => {
                blocks.put(w);
                bytes.put(w);
            }
            Self::Refused { output, attempt } => {
                output.put(w);
                attempt.put(w);
            }
            Self::Sound {
                pattern,
                cue,
                note,
                of,
            } => {
                pattern.put(w);
                cue.put(w);
                note.put(w);
                of.put(w);
            }
            Self::Mark {
                count,
                pressed_ms,
                page,
            } => {
                count.put(w);
                w.time(*pressed_ms);
                page.put(w);
            }
            Self::TooLarge(pattern) => pattern.put(w),
            Self::Entropy(value) => value.put(w),
            Self::Log {
                level,
                target,
                text,
            } => {
                level.put(w);
                target.put(w);
                text.put(w);
            }
            Self::Synthetic(input) => input.put(w),
            Self::Panel(digest) => digest.put(w),
            Self::FrameTime(frame_time) => frame_time.put(w),
            Self::Health(health) => health.put(w),
            Self::Activity(activity) => activity.put(w),
            Self::Task {
                name,
                polls,
                busy_us,
            } => {
                name.put(w);
                polls.put(w);
                busy_us.put(w);
            }
            Self::Stage {
                name,
                runs,
                busy_us,
                max_us,
            } => {
                name.put(w);
                runs.put(w);
                busy_us.put(w);
                max_us.put(w);
            }
        }
    }

    fn take_body(tag: u8, r: &mut Reader<'a>) -> Option<Self> {
        Some(match tag {
            tag::PASS => Self::Pass {
                now_ms: r.now.wrapping_add(r.zigzag() as u64),
            },
            tag::INPUT => Self::Input(Wire::take(r)),
            tag::ANT_RADIO => Self::AntRadio(Wire::take(r)),
            tag::ANT_DISCOVERIES => Self::AntDiscoveries(Wire::take(r)),
            tag::ANT_LINK => Self::AntLink {
                channel: Wire::take(r),
                snapshot: Wire::take(r),
            },
            tag::ANT_PACKET => Self::AntPacket(Wire::take(r)),
            tag::ANT_REQUEST => Self::AntRequest {
                operation: Wire::take(r),
                result: Wire::take(r),
            },
            tag::ANT_SEND => Self::AntSend(Wire::take(r)),
            tag::BLE_LINK => Self::BleLink {
                availability: Wire::take(r),
                snapshot: Wire::take(r),
            },
            tag::BLE_PACKET => Self::BlePacket(Wire::take(r)),
            tag::BLE_BATTERY => Self::BleBattery(Wire::take(r)),
            tag::POSITION => Self::Position {
                availability: Wire::take(r),
                snapshot: Wire::take(r),
            },
            tag::PHYSICAL => Self::Physical(Wire::take(r)),
            tag::LIGHT => Self::Light(Wire::take(r)),
            tag::POWER => Self::Power(Wire::take(r)),
            tag::NETWORK_TIME => Self::NetworkTime(Wire::take(r)),
            tag::STARTUP => Self::Startup(Wire::take(r)),
            tag::COMMAND => Self::Command(Wire::take(r)),
            tag::STORAGE_FAULT => Self::StorageFault(Wire::take(r)),
            tag::ACCESS_POINT => Self::AccessPoint {
                state: Wire::take(r),
                reach: Wire::take(r),
            },
            tag::ACCESS_POINT_REQUEST => Self::AccessPointRequest {
                start: Wire::take(r),
                result: Wire::take(r),
            },
            tag::SERVER => Self::Server {
                request: Wire::take(r),
                room: Wire::take(r),
            },
            tag::SERVER_LISTEN => Self::ServerListen(Wire::take(r)),
            tag::SERVER_FINISH => Self::ServerFinish(Wire::take(r)),
            tag::SERVER_READ => Self::ServerRead {
                id: Wire::take(r),
                part: Wire::take(r),
                more: Wire::take(r),
            },
            tag::SERVER_WRITE => Self::ServerWrite {
                id: Wire::take(r),
                taken: Wire::take(r),
            },
            tag::SOUND_STATE => Self::SoundState(Wire::take(r)),
            tag::SOUND_REQUEST => Self::SoundRequest {
                play: Wire::take(r),
                result: Wire::take(r),
            },
            tag::POWER_CONTROL => Self::PowerControl(Wire::take(r)),
            tag::POWER_REQUEST => Self::PowerRequest {
                operation: Wire::take(r),
                delay_ms: Wire::take(r),
                result: Wire::take(r),
            },
            tag::POWERED_OFF => Self::PoweredOff,
            tag::BACKLIGHT => Self::Backlight(Wire::take(r)),
            tag::REFUSED => Self::Refused {
                output: Wire::take(r),
                attempt: Wire::take(r),
            },
            tag::ENTROPY => Self::Entropy(Wire::take(r)),
            tag::TOO_LARGE => Self::TooLarge(Wire::take(r)),
            tag::HOST_WROTE => Self::HostWrote,
            tag::LOST => Self::Lost {
                blocks: Wire::take(r),
                bytes: Wire::take(r),
            },
            tag::PAGE => Self::Page(Wire::take(r)),
            tag::TOAST => Self::Toast(Wire::take(r)),
            tag::SOUND => Self::Sound {
                pattern: Wire::take(r),
                cue: Wire::take(r),
                note: Wire::take(r),
                of: Wire::take(r),
            },
            tag::RIDE_STATE => Self::RideState(Wire::take(r)),
            tag::MARK => Self::Mark {
                count: Wire::take(r),
                pressed_ms: r.time(),
                page: Wire::take(r),
            },
            tag::SHOWN => Self::Shown(Wire::take(r)),
            tag::SYNTHETIC => Self::Synthetic(Wire::take(r)),
            tag::PANEL => Self::Panel(Wire::take(r)),
            tag::LOG => Self::Log {
                level: Wire::take(r),
                target: Wire::take(r),
                text: Wire::take(r),
            },
            tag::FRAME_TIME => Self::FrameTime(Wire::take(r)),
            tag::HEALTH => Self::Health(Wire::take(r)),
            tag::ACTIVITY => Self::Activity(Wire::take(r)),
            tag::TASK => Self::Task {
                name: Wire::take(r),
                polls: Wire::take(r),
                busy_us: Wire::take(r),
            },
            tag::STAGE => Self::Stage {
                name: Wire::take(r),
                runs: Wire::take(r),
                busy_us: Wire::take(r),
                max_us: Wire::take(r),
            },
            _ => return None,
        })
    }
}

/// The header as a complete block in `out`; `None` when it does not fit.
pub fn encode_header(header: &Header<'_>, out: &mut [u8]) -> Option<usize> {
    let end = out.len().min(MAX_BLOCK).checked_sub(4)?;
    let mut w = Writer::new(out.get_mut(2..end)?, 0);
    header.put(&mut w);
    let len = w.len;
    (!w.overflow).then_some(())?;
    seal(out, len)
}

fn seal(out: &mut [u8], len: usize) -> Option<usize> {
    let crc = crc32(out.get(2..2 + len)?);
    out.get_mut(2 + len..FRAME + len)?
        .copy_from_slice(&crc.to_le_bytes());
    out[..2].copy_from_slice(&(len as u16).to_le_bytes());
    Some(FRAME + len)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Full {
    /// The block has no room left; seal it and start the next.
    Block,
    /// The record is larger than an empty block.
    Record,
}

/// Records events into one block of a caller-provided buffer.
pub struct Encoder<B> {
    buf: B,
    len: usize,
    opening: usize,
    now: u64,
}

impl<B: AsMut<[u8]> + AsRef<[u8]>> Encoder<B> {
    /// `buf` holds one block, of 64 to [`MAX_BLOCK`] bytes.
    pub fn new(buf: B) -> Self {
        assert!(buf.as_ref().len() >= 64, "a trace block needs 64 bytes");
        let mut encoder = Self {
            buf,
            len: 0,
            opening: 0,
            now: 0,
        };
        encoder.restart();
        encoder
    }

    fn payload(&mut self) -> &mut [u8] {
        let end = self.buf.as_ref().len().min(MAX_BLOCK) - 4;
        &mut self.buf.as_mut()[2..end]
    }

    /// Start the next block, opening with the current pass clock.
    pub fn restart(&mut self) {
        let now = self.now;
        let mut w = Writer::new(self.payload(), now);
        w.record(tag::RESUME, |w| w.varint(now));
        let len = w.len;
        self.len = len;
        self.opening = len;
    }

    /// Append `event`, or leave the block untouched when it does not fit.
    pub fn push(&mut self, event: &Event<'_>) -> Result<(), Full> {
        let (len, previous) = (self.len, self.now);
        let mut w = Writer::new(&mut self.payload()[len..], previous);
        match event {
            Event::Pass { now_ms } if now_ms.wrapping_sub(previous) < 0x80 => {
                w.byte(SHORT_PASS | now_ms.wrapping_sub(previous) as u8);
            }
            _ => w.record(event.tag(), |w| event.put_body(w, previous)),
        }
        if w.overflow {
            return Err(if self.is_empty() {
                Full::Record
            } else {
                Full::Block
            });
        }
        self.len += w.len;
        if let Event::Pass { now_ms } = event {
            self.now = *now_ms;
        }
        Ok(())
    }

    /// Whether the block holds nothing beyond its opening clock.
    pub fn is_empty(&self) -> bool {
        self.len == self.opening
    }

    /// The block's size once sealed.
    pub fn len(&self) -> usize {
        FRAME + self.len
    }

    /// The block so far, framed. It stays open: later pushes extend it.
    pub fn sealed(&mut self) -> &[u8] {
        let len = self.len;
        let end = seal(self.buf.as_mut(), len).unwrap_or(0);
        &self.buf.as_ref()[..end]
    }
}

/// Why the events stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum End {
    /// Every block decoded.
    Complete,
    /// The last block was cut short or failed its check, as a write that
    /// power loss stopped leaves it; it is dropped.
    Torn,
    /// A block failed its check with bytes after it, or passed its check
    /// and did not decode: the trace was damaged, not cut, and everything
    /// from that block on is dropped.
    Corrupt,
}

impl End {
    pub fn name(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::Torn => "torn",
            Self::Corrupt => "corrupt",
        }
    }
}

/// A position in a trace's events, to resume decoding later, also over the
/// same trace grown since: the open block resealed longer, or more blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    block: usize,
    at: usize,
    now: u64,
    end: Option<End>,
    checked: Option<(usize, [u8; 4])>,
}

/// The trace's events in order, ending before a torn tail.
pub struct Events<'a> {
    bytes: &'a [u8],
    cursor: Cursor,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The header block is missing, torn or malformed.
    Header,
    /// The header opens with a version other than [`VERSION`].
    Version(u8),
}

impl core::fmt::Display for Error {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match *self {
            Self::Header => f.write_str("the header block is missing, torn or malformed"),
            Self::Version(version) => write!(
                f,
                "trace format {version} is not this build's ({VERSION}): read it with the build that recorded it"
            ),
        }
    }
}

/// Keep in step with `NAME` in `cycling devices`.
fn is_manifest_name(name: &str) -> bool {
    let mut bytes = name.bytes();
    bytes.next().is_some_and(|b| b.is_ascii_lowercase())
        && name.len() <= 16
        && bytes.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The header and the events after it.
pub fn decode(bytes: &[u8]) -> Result<(Header<'_>, Events<'_>), Error> {
    let (payload, next) = block(bytes, 0).ok_or(Error::Header)?;
    let version = *payload.first().ok_or(Error::Header)?;
    if version != VERSION {
        return Err(Error::Version(version));
    }
    let mut r = Reader::new(payload, 0);
    let header = Header::take(&mut r);
    if !r.ok() || r.at != payload.len() || !is_manifest_name(header.device) {
        return Err(Error::Header);
    }
    let cursor = Cursor {
        block: next,
        at: 0,
        now: 0,
        end: None,
        checked: None,
    };
    Ok((header, Events::resume(bytes, cursor)))
}

/// Why the block at `start` does not read: torn when it reaches the end of
/// `bytes`, corrupt when bytes follow it.
fn failed(bytes: &[u8], start: usize) -> End {
    let claimed = bytes
        .get(start..start.saturating_add(2))
        .map(|len| start + FRAME + usize::from(u16::from_le_bytes([len[0], len[1]])));
    match claimed {
        Some(end) if end < bytes.len() => End::Corrupt,
        _ => End::Torn,
    }
}

fn block(bytes: &[u8], start: usize) -> Option<(&[u8], usize)> {
    let (payload, crc) = unchecked_block(bytes, start)?;
    (crc32(payload).to_le_bytes() == crc).then_some((payload, start + FRAME + payload.len()))
}

fn unchecked_block(bytes: &[u8], start: usize) -> Option<(&[u8], [u8; 4])> {
    let len = usize::from(u16::from_le_bytes(
        bytes.get(start..start.checked_add(2)?)?.try_into().ok()?,
    ));
    let payload = bytes.get(start + 2..start + 2 + len)?;
    let crc = bytes.get(start + 2 + len..start + FRAME + len)?;
    Some((payload, crc.try_into().ok()?))
}

impl<'a> Events<'a> {
    /// Continue from a cursor taken from events over the same trace, which
    /// may have grown since. A trace that ended there is read on.
    pub fn resume(bytes: &'a [u8], cursor: Cursor) -> Self {
        Self {
            bytes,
            cursor: Cursor {
                end: None,
                ..cursor
            },
        }
    }

    pub fn cursor(&self) -> Cursor {
        self.cursor
    }

    /// Why the events ended; `None` while more may follow.
    pub fn end(&self) -> Option<End> {
        self.cursor.end
    }

    fn step(&mut self) -> Result<Option<Event<'a>>, End> {
        let c = &mut self.cursor;
        let payload = match unchecked_block(self.bytes, c.block) {
            Some((payload, crc)) if c.checked == Some((payload.len(), crc)) => payload,
            _ if c.block >= self.bytes.len() && c.at == 0 => return Err(End::Complete),
            Some((payload, crc)) if crc32(payload).to_le_bytes() == crc => {
                if c.at > payload.len() {
                    return Err(End::Torn);
                }
                c.checked = Some((payload.len(), crc));
                payload
            }
            _ => return Err(failed(self.bytes, c.block)),
        };
        let Some(&first) = payload.get(c.at) else {
            let next = c.block + FRAME + payload.len();
            if next >= self.bytes.len() {
                return Err(End::Complete);
            }
            c.block = next;
            c.at = 0;
            c.checked = None;
            return Ok(None);
        };
        if first & SHORT_PASS != 0 {
            c.at += 1;
            c.now = c.now.wrapping_add(u64::from(first & !SHORT_PASS));
            return Ok(Some(Event::Pass { now_ms: c.now }));
        }
        let mut r = Reader::new(&payload[c.at + 1..], c.now);
        let len = r.narrow();
        let body = r.bytes(len);
        if !r.ok() {
            return Err(End::Corrupt);
        }
        c.at += 1 + r.at;
        let mut r = Reader::new(body, c.now);
        let event = if first == tag::RESUME {
            c.now = r.varint();
            None
        } else {
            Some(Event::take_body(first, &mut r).ok_or(End::Corrupt)?)
        };
        if !r.ok() || r.at != body.len() {
            return Err(End::Corrupt);
        }
        if let Some(Event::Pass { now_ms }) = event {
            c.now = now_ms;
        }
        Ok(event)
    }
}

impl<'a> Iterator for Events<'a> {
    type Item = Event<'a>;

    fn next(&mut self) -> Option<Event<'a>> {
        while self.cursor.end.is_none() {
            match self.step() {
                Ok(Some(event)) => return Some(event),
                Ok(None) => {}
                Err(end) => self.cursor.end = Some(end),
            }
        }
        None
    }
}
