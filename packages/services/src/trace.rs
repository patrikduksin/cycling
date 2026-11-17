//! Ride trace: every input VANA consumed during one power-on session, and a
//! thin layer of what the rider saw, as a compact binary stream.
//!
//! A trace is a run of blocks, each `[payload length: u16 LE][payload][CRC32
//! LE of the payload]`. The first block holds the [`Header`]; every later one
//! holds whole records. A block cut short or failing its CRC ends the
//! trace, so a torn tail is dropped rather than misread. Each event block opens
//! with the pass clock, so a block decodes without the ones before it.
//!
//! A record byte with the top bit set is a [`Event::Pass`] 0 to 127 ms after
//! the previous pass. Any other record is a capability id, an operation, the
//! body length as a varint and the body. Capability 1 to 127 is a device
//! capability and the operation is the method's place in its table
//! (`device_api::capability`): the record is an [`Event::Call`], what one
//! call of that method gave VANA, laid out as the method's kind says
//! (`device_api::journal`). Capability 0 is the trace's own records and the
//! operation is their tag. Times inside a record are stored relative to the
//! current pass.
use crate::checksum::crc32;
use device_api::ant;
use device_api::ble_transport;
use device_api::input::Input;
use device_api::observation::Availability;
use device_api::server::{self, AccessPointState, Method};

pub mod determined;
pub mod log;
pub mod media;
pub mod origin;
pub mod recorder;
pub mod redact;
#[cfg(feature = "testing")]
pub mod replayed;
pub mod spool;
pub mod tail;
#[cfg(feature = "testing")]
pub mod tapped;
#[cfg(test)]
mod tests;
pub mod wire;

use device_api::journal::MethodId;
use wire::{Reader, Wire, Writer};

/// The header's first byte.
pub const VERSION: u8 = 6;
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
    /// The TERRAIN pack the session found. The trace holds only digests of
    /// what VANA read of it, so a replay reads the pack this names.
    pub terrain: origin::Found<origin::Terrain>,
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

impl Medium {
    /// Bytes in one unit of a span on this medium.
    pub fn unit(self) -> usize {
        match self {
            Medium::Bulk => device_api::bulk::SECTOR_SIZE,
            _ => 1,
        }
    }
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
    Level { Error = 0, Warn = 1 }
    Gnss { Off = 0, Receiving = 1, Paused = 2, Failed = 3 }
}

wire::record! {
    crate::network_time::Snapshot { unix_seconds, millis, local_minutes, age_ms, status }
    crate::storage::Record { sequence, length }
    ServerRequest<'a> { id, method, path, query, session, gzip, body }
    AntCapabilities<'a> {
        supported_types, connection_capacity, one_peer_per_type, concurrent_scan,
        acknowledged_send, radio_delivery_feedback, burst, max_pending_sends,
    }
    FrameTime { frames, mean_us }
    Cost { evictions, high_water }
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

/// What one call of a capability method gave VANA.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Call<'a> {
    pub method: MethodId,
    /// The pass it was recorded in: times in the body are relative to it.
    pub now_ms: u64,
    pub body: &'a [u8],
}

impl<'a> Call<'a> {
    /// The body as a `T`; `None` unless it is exactly one.
    pub fn decode<T: Wire<'a>>(&self) -> Option<T> {
        let mut r = Reader::new(self.body, self.now_ms);
        let value = T::take(&mut r);
        (r.ok() && r.at == self.body.len()).then_some(value)
    }
}

/// What the determined set cost so far: see [`determined::Determined`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cost {
    pub evictions: u32,
    pub high_water: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event<'a> {
    /// One main-loop pass began at this uptime.
    Pass {
        now_ms: u64,
    },
    Call(Call<'a>),
    /// A host wrote this medium unseen: the trace determines none of it
    /// from here on, and the next read of it records its bytes.
    Forget(Medium),
    Cost(Cost),
    /// The last record of this method came after `count` calls of it in
    /// the pass that were not recorded: each repeated the method's record
    /// before them and changed nothing its capability observes.
    Repeated {
        method: MethodId,
        count: u8,
    },
    /// What composition gave this session, recorded as the trace opens and
    /// again when it changes: the working memory VANA has, and whether
    /// secrets are replaced as they are recorded (see [`redact`]).
    Session {
        memory: origin::Memory,
        redacted: bool,
    },
    /// UTC from Wi-Fi synchronization, recorded when it no longer reads as
    /// the last recorded value advanced to this pass.
    NetworkTime(crate::network_time::Snapshot),
    /// A terminal request line, executed after the pass before it.
    Command(&'a str),
    /// A record of this capability and operation was larger than a block
    /// and left out.
    TooLarge {
        capability: u8,
        operation: u8,
    },
    /// Blocks of events the recorder sealed and its sink had no room for,
    /// since the last block it took. They are missing just before this.
    Lost {
        blocks: u32,
        bytes: u32,
    },
    /// The device's power went away after an accepted shutdown.
    PoweredOff,
    /// The durable horizon of the session's trace before this one, read
    /// back once this one opened: `previous`'s last pass whose records
    /// survived, and how many whole blocks it holds after its last synced
    /// record (see [`tail`]).
    Horizon {
        previous: &'a str,
        last_pass_ms: Option<u64>,
        blocks: u32,
    },
    /// After a panic or a watchdog reset: the blocks this boot appended to
    /// the trace before it, from what the crash left in memory.
    Salvaged {
        blocks: u32,
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
    pub const NETWORK_TIME: u8 = 16;
    pub const COMMAND: u8 = 18;
    pub const TOO_LARGE: u8 = 21;
    pub const POWERED_OFF: u8 = 33;
    pub const LOST: u8 = 36;
    pub const FORGET: u8 = 39;
    pub const COST: u8 = 40;
    pub const HORIZON: u8 = 41;
    pub const SALVAGED: u8 = 42;
    pub const REPEATED: u8 = 43;
    pub const SESSION: u8 = 44;
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
        self.terrain.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        u8::take(r);
        Self {
            build: <[u8; 20]>::take(r),
            device: <&str>::take(r),
            boot: u32::take(r),
            terrain: Wire::take(r),
        }
    }
}

impl<'a> Event<'a> {
    /// Whether the block holding this event is sealed as soon as it is
    /// recorded: a named cue's first note or a page change. The world log
    /// seals a button report and a storage fault itself.
    pub fn seals(&self) -> bool {
        match self {
            Self::Sound { cue, note, .. } => !cue.is_empty() && *note == 1,
            Self::Page(_) => true,
            _ => false,
        }
    }

    /// The capability and operation its record opens with.
    pub fn id(&self) -> (u8, u8) {
        match self {
            Self::Call(call) => (call.method.capability, call.method.operation),
            _ => (0, self.tag()),
        }
    }

    fn tag(&self) -> u8 {
        match self {
            Self::Pass { .. } => tag::PASS,
            Self::Call(call) => call.method.operation,
            Self::Forget(_) => tag::FORGET,
            Self::Cost(_) => tag::COST,
            Self::Repeated { .. } => tag::REPEATED,
            Self::Session { .. } => tag::SESSION,
            Self::NetworkTime(_) => tag::NETWORK_TIME,
            Self::Command(_) => tag::COMMAND,
            Self::TooLarge { .. } => tag::TOO_LARGE,
            Self::PoweredOff => tag::POWERED_OFF,
            Self::Horizon { .. } => tag::HORIZON,
            Self::Salvaged { .. } => tag::SALVAGED,
            Self::Lost { .. } => tag::LOST,
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

    fn put_record(&self, w: &mut Writer, previous_pass: u64) {
        let (capability, operation) = self.id();
        w.byte(capability);
        w.record(operation, |w| self.put_body(w, previous_pass));
    }

    fn put_body(&self, w: &mut Writer, previous_pass: u64) {
        match self {
            Self::Pass { now_ms } => w.zigzag(now_ms.wrapping_sub(previous_pass) as i64),
            Self::Call(call) => w.bytes(call.body),
            Self::Forget(medium) => medium.put(w),
            Self::Cost(cost) => cost.put(w),
            Self::Repeated { method, count } => {
                method.capability.put(w);
                method.operation.put(w);
                count.put(w);
            }
            Self::Session { memory, redacted } => {
                memory.put(w);
                redacted.put(w);
            }
            Self::NetworkTime(snapshot) => snapshot.put(w),
            Self::Command(text)
            | Self::Page(text)
            | Self::Toast(text)
            | Self::RideState(text)
            | Self::Shown(text) => text.put(w),
            Self::PoweredOff => {}
            Self::Horizon {
                previous,
                last_pass_ms,
                blocks,
            } => {
                previous.put(w);
                last_pass_ms.put(w);
                blocks.put(w);
            }
            Self::Salvaged { blocks } => blocks.put(w),
            Self::Lost { blocks, bytes } => {
                blocks.put(w);
                bytes.put(w);
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
            Self::TooLarge {
                capability,
                operation,
            } => {
                capability.put(w);
                operation.put(w);
            }
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
            tag::NETWORK_TIME => Self::NetworkTime(Wire::take(r)),
            tag::COMMAND => Self::Command(Wire::take(r)),
            tag::POWERED_OFF => Self::PoweredOff,
            tag::HORIZON => Self::Horizon {
                previous: Wire::take(r),
                last_pass_ms: Wire::take(r),
                blocks: Wire::take(r),
            },
            tag::SALVAGED => Self::Salvaged {
                blocks: Wire::take(r),
            },
            tag::TOO_LARGE => Self::TooLarge {
                capability: Wire::take(r),
                operation: Wire::take(r),
            },
            tag::FORGET => Self::Forget(Wire::take(r)),
            tag::COST => Self::Cost(Wire::take(r)),
            tag::REPEATED => Self::Repeated {
                method: MethodId {
                    capability: Wire::take(r),
                    operation: Wire::take(r),
                },
                count: Wire::take(r),
            },
            tag::LOST => Self::Lost {
                blocks: Wire::take(r),
                bytes: Wire::take(r),
            },
            tag::SESSION => Self::Session {
                memory: Wire::take(r),
                redacted: Wire::take(r),
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
        w.byte(0);
        w.record(tag::RESUME, |w| w.varint(now));
        let len = w.len;
        self.len = len;
        self.opening = len;
        self.note_len();
    }

    /// Keep the block's length prefix current, so that memory a crash
    /// leaves holds the open block's extent (see [`tail::Remains`]).
    fn note_len(&mut self) {
        let len = self.len as u16;
        self.buf.as_mut()[..2].copy_from_slice(&len.to_le_bytes());
    }

    /// Append `event`, or leave the block untouched when it does not fit.
    pub fn push(&mut self, event: &Event<'_>) -> Result<(), Full> {
        let (len, previous) = (self.len, self.now);
        let mut w = Writer::new(&mut self.payload()[len..], previous);
        match event {
            Event::Pass { now_ms } if now_ms.wrapping_sub(previous) < 0x80 => {
                w.byte(SHORT_PASS | now_ms.wrapping_sub(previous) as u8);
            }
            _ => event.put_record(&mut w, previous),
        }
        if w.overflow {
            return Err(if self.is_empty() {
                Full::Record
            } else {
                Full::Block
            });
        }
        self.len += w.len;
        self.note_len();
        if let Event::Pass { now_ms } = event {
            self.now = *now_ms;
        }
        Ok(())
    }

    /// The buffer as it stands: the length prefix is current, the CRC is
    /// not.
    pub fn memory(&self) -> &[u8] {
        self.buf.as_ref()
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
        if c.at >= payload.len() {
            let next = c.block + FRAME + payload.len();
            if next >= self.bytes.len() {
                return Err(End::Complete);
            }
            c.block = next;
            c.at = 0;
            c.checked = None;
            return Ok(None);
        }
        next_record(payload, &mut c.at, &mut c.now)
    }
}

/// The record at `at` of a block's payload, read after the pass clock
/// `now`; both move past it. `None` for the clock a block opens with.
fn next_record<'a>(
    payload: &'a [u8],
    at: &mut usize,
    now: &mut u64,
) -> Result<Option<Event<'a>>, End> {
    let Some(&first) = payload.get(*at) else {
        return Err(End::Corrupt);
    };
    if first & SHORT_PASS != 0 {
        *at += 1;
        *now = now.wrapping_add(u64::from(first & !SHORT_PASS));
        return Ok(Some(Event::Pass { now_ms: *now }));
    }
    let Some(&operation) = payload.get(*at + 1) else {
        return Err(End::Corrupt);
    };
    let mut r = Reader::new(&payload[*at + 2..], *now);
    let len = r.narrow();
    let body = r.bytes(len);
    if !r.ok() {
        return Err(End::Corrupt);
    }
    *at += 2 + r.at;
    if first != 0 {
        return Ok(Some(Event::Call(Call {
            method: MethodId {
                capability: first,
                operation,
            },
            now_ms: *now,
            body,
        })));
    }
    let mut r = Reader::new(body, *now);
    let event = if operation == tag::RESUME {
        *now = r.varint();
        None
    } else {
        Some(Event::take_body(operation, &mut r).ok_or(End::Corrupt)?)
    };
    if !r.ok() || r.at != body.len() {
        return Err(End::Corrupt);
    }
    if let Some(Event::Pass { now_ms }) = event {
        *now = now_ms;
    }
    Ok(event)
}

/// What an event block says of its clock: the pass it opens at, the last
/// pass it holds, and how many events it holds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Clocks {
    pub opening: u64,
    pub end: u64,
    pub events: u32,
    /// Its first event is [`Event::Lost`]: blocks before it are missing.
    pub after_loss: bool,
}

/// The clocks of an event block's payload; `None` when it does not open
/// with its clock or does not decode to its end.
pub fn clocks(payload: &[u8]) -> Option<Clocks> {
    if payload.get(..2)? != [0, tag::RESUME] {
        return None;
    }
    let (mut at, mut now) = (0, 0);
    next_record(payload, &mut at, &mut now).ok()?;
    let mut clocks = Clocks {
        opening: now,
        end: now,
        events: 0,
        after_loss: false,
    };
    while at < payload.len() {
        let event = next_record(payload, &mut at, &mut now).ok()?;
        if clocks.events == 0 {
            clocks.after_loss = matches!(event, Some(Event::Lost { .. }));
        }
        clocks.events += u32::from(event.is_some());
    }
    clocks.end = now;
    Some(clocks)
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
