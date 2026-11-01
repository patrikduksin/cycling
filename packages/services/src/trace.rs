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
//! varint and the body, so a reader skips tags it does not know. Times inside
//! a record are stored relative to the current pass.
use crate::checksum::crc32;
use device_api::ant;
use device_api::ble_transport;
use device_api::input::{Button, Edge, Input, InputSnapshot, Point};
use device_api::observation::{Availability, Observation};
use device_api::positioning;
use device_api::power;
use device_api::sensors;
use device_api::server::{self, AccessPointState, Method, Reach};
use device_api::sound;

pub mod frozen;
pub mod origin;
pub mod redact;
pub mod spool;
#[cfg(test)]
mod tests;

pub const VERSION: u16 = 1;
/// The oldest version that reads what this version writes. A later version
/// that only appends fields and adds tags keeps it; one that changes the
/// meaning of a field raises it.
pub const OLDEST_READER: u16 = 1;
pub const MAX_BLOCK: usize = u16::MAX as usize + FRAME;
const FRAME: usize = 2 + 4;

/// What a replay needs before the first pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header<'a> {
    pub version: u16,
    pub oldest_reader: u16,
    /// Git commit of the recording firmware, zero when unknown.
    pub build: [u8; 20],
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
    /// A record this version cannot read: its tag is unknown, or a later
    /// version wrote a value this one has no code for. Kept whole.
    Unknown {
        tag: u8,
        body: &'a [u8],
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
    /// The displayed fields, about once a second.
    Shown(&'a str),
    Log {
        level: Level,
        target: &'a str,
        text: &'a str,
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
    pub const PAGE: u8 = 64;
    pub const TOAST: u8 = 65;
    pub const SOUND: u8 = 66;
    pub const RIDE_STATE: u8 = 67;
    pub const SHOWN: u8 = 68;
    pub const LOG: u8 = 69;
}

const SHORT_PASS: u8 = 0x80;

pub struct Writer<'b> {
    buf: &'b mut [u8],
    len: usize,
    now: u64,
    overflow: bool,
}

impl<'b> Writer<'b> {
    fn new(buf: &'b mut [u8], now: u64) -> Self {
        Self {
            buf,
            len: 0,
            now,
            overflow: false,
        }
    }

    fn byte(&mut self, byte: u8) {
        match self.buf.get_mut(self.len) {
            Some(slot) => {
                *slot = byte;
                self.len += 1;
            }
            None => self.overflow = true,
        }
    }

    fn bytes(&mut self, bytes: &[u8]) {
        match self.buf.get_mut(self.len..self.len + bytes.len()) {
            Some(slot) => {
                slot.copy_from_slice(bytes);
                self.len += bytes.len();
            }
            None => self.overflow = true,
        }
    }

    fn varint(&mut self, mut value: u64) {
        while value >= 0x80 {
            self.byte(value as u8 | 0x80);
            value >>= 7;
        }
        self.byte(value as u8);
    }

    fn zigzag(&mut self, value: i64) {
        self.varint(((value << 1) ^ (value >> 63)) as u64);
    }

    fn record(&mut self, tag: u8, body: impl FnOnce(&mut Self)) {
        self.byte(tag);
        let at = self.len;
        self.byte(0);
        body(self);
        if self.overflow {
            return;
        }
        let mut prefix = [0u8; 10];
        let mut p = Writer::new(&mut prefix, 0);
        p.varint((self.len - at - 1) as u64);
        let width = p.len;
        if self.len + width - 1 > self.buf.len() {
            self.overflow = true;
            return;
        }
        self.buf.copy_within(at + 1..self.len, at + width);
        self.buf[at..at + width].copy_from_slice(&prefix[..width]);
        self.len += width - 1;
    }

    fn time(&mut self, at: u64) {
        self.zigzag(self.now.wrapping_sub(at) as i64);
    }

    fn times(&mut self, at: Option<u64>) {
        self.byte(at.is_some().into());
        if let Some(at) = at {
            self.time(at);
        }
    }
}

pub struct Reader<'a> {
    bytes: &'a [u8],
    at: usize,
    now: u64,
    failed: bool,
}

impl<'a> Reader<'a> {
    fn new(bytes: &'a [u8], now: u64) -> Self {
        Self {
            bytes,
            at: 0,
            now,
            failed: false,
        }
    }

    fn fail(&mut self) {
        self.failed = true;
    }

    fn ok(&self) -> bool {
        !self.failed
    }

    fn byte(&mut self) -> u8 {
        match self.bytes.get(self.at) {
            Some(&byte) => {
                self.at += 1;
                byte
            }
            None => {
                self.fail();
                0
            }
        }
    }

    fn bytes(&mut self, len: usize) -> &'a [u8] {
        match self.bytes.get(self.at..self.at.saturating_add(len)) {
            Some(slice) => {
                self.at += len;
                slice
            }
            None => {
                self.fail();
                &[]
            }
        }
    }

    fn varint(&mut self) -> u64 {
        let mut value = 0u64;
        for shift in (0..64).step_by(7) {
            let byte = self.byte();
            value |= u64::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
        }
        self.fail();
        0
    }

    fn zigzag(&mut self) -> i64 {
        let value = self.varint();
        ((value >> 1) as i64) ^ -((value & 1) as i64)
    }

    fn time(&mut self) -> u64 {
        self.now.wrapping_sub(self.zigzag() as u64)
    }

    fn times(&mut self) -> Option<u64> {
        bool::take(self).then(|| self.time())
    }

    fn narrow<T: TryFrom<u64> + Default>(&mut self) -> T {
        let value = self.varint();
        T::try_from(value).unwrap_or_else(|_| {
            self.fail();
            T::default()
        })
    }
}

trait Wire<'a>: Sized {
    fn put(&self, w: &mut Writer);
    fn take(r: &mut Reader<'a>) -> Self;
}

impl Wire<'_> for u8 {
    fn put(&self, w: &mut Writer) {
        w.byte(*self);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        r.byte()
    }
}

macro_rules! unsigned {
    ($($ty:ty),*) => {$(
        impl Wire<'_> for $ty {
            fn put(&self, w: &mut Writer) {
                w.varint((*self).into());
            }
            fn take(r: &mut Reader<'_>) -> Self {
                r.narrow()
            }
        }
    )*};
}
unsigned!(u16, u32, u64);

macro_rules! signed {
    ($($ty:ty),*) => {$(
        impl Wire<'_> for $ty {
            fn put(&self, w: &mut Writer) {
                w.zigzag((*self).into());
            }
            fn take(r: &mut Reader<'_>) -> Self {
                let value = r.zigzag();
                <$ty>::try_from(value).unwrap_or_else(|_| {
                    r.fail();
                    0
                })
            }
        }
    )*};
}
signed!(i8, i16, i32);

impl Wire<'_> for bool {
    fn put(&self, w: &mut Writer) {
        w.byte((*self).into());
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => false,
            1 => true,
            _ => {
                r.fail();
                false
            }
        }
    }
}

impl<'a, T: Wire<'a>> Wire<'a> for Option<T> {
    fn put(&self, w: &mut Writer) {
        w.byte(self.is_some().into());
        if let Some(value) = self {
            value.put(w);
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        bool::take(r).then(|| T::take(r))
    }
}

impl<'a, T: Wire<'a>, const N: usize> Wire<'a> for [T; N] {
    fn put(&self, w: &mut Writer) {
        for value in self {
            value.put(w);
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        core::array::from_fn(|_| T::take(r))
    }
}

impl<'a, A: Wire<'a>, B: Wire<'a>> Wire<'a> for (A, B) {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
        self.1.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        (A::take(r), B::take(r))
    }
}

impl<'a> Wire<'a> for &'a [u8] {
    fn put(&self, w: &mut Writer) {
        w.varint(self.len() as u64);
        w.bytes(self);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        let len = r.narrow();
        r.bytes(len)
    }
}

impl<'a> Wire<'a> for &'a str {
    fn put(&self, w: &mut Writer) {
        self.as_bytes().put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        let bytes = <&[u8]>::take(r);
        core::str::from_utf8(bytes).unwrap_or_else(|_| {
            r.fail();
            ""
        })
    }
}

impl<'a, T: Wire<'a>> Wire<'a> for Observation<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Unavailable => w.byte(0),
            Self::Fresh { value, received_ms } => {
                w.byte(1);
                value.put(w);
                w.time(*received_ms);
            }
            Self::Stale { value, received_ms } => {
                w.byte(2);
                value.put(w);
                w.time(*received_ms);
            }
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        match r.byte() {
            0 => Self::Unavailable,
            kind @ (1 | 2) => {
                let value = T::take(r);
                let received_ms = r.time();
                if kind == 1 {
                    Self::Fresh { value, received_ms }
                } else {
                    Self::Stale { value, received_ms }
                }
            }
            _ => {
                r.fail();
                Self::Unavailable
            }
        }
    }
}

macro_rules! record {
    ($($ty:path { $($field:ident $(: $kind:ident)?),* $(,)? })*) => {$(
        impl<'a> Wire<'a> for $ty {
            fn put(&self, w: &mut Writer) {
                $(record!(@put w, self.$field $(, $kind)?);)*
            }
            fn take(r: &mut Reader<'a>) -> Self {
                Self { $($field: record!(@take r $(, $kind)?),)* }
            }
        }
    )*};
    (@put $w:ident, $value:expr) => { Wire::put(&$value, $w) };
    (@put $w:ident, $value:expr, time) => { $w.time($value) };
    (@put $w:ident, $value:expr, times) => { $w.times($value) };
    (@take $r:ident) => { Wire::take($r) };
    (@take $r:ident, time) => { $r.time() };
    (@take $r:ident, times) => { $r.times() };
}

macro_rules! codes {
    ($($ty:path { $first:ident = $first_code:literal $(, $variant:ident = $code:literal)* $(,)? })*) => {$(
        impl Wire<'_> for $ty {
            fn put(&self, w: &mut Writer) {
                w.byte(match self {
                    Self::$first => $first_code,
                    $(Self::$variant => $code,)*
                });
            }
            fn take(r: &mut Reader<'_>) -> Self {
                match r.byte() {
                    $first_code => Self::$first,
                    $($code => Self::$variant,)*
                    _ => {
                        r.fail();
                        Self::$first
                    }
                }
            }
        }
    )*};
}

codes! {
    Availability { Unsupported = 0, Initializing = 1, Unconfigured = 2, Ready = 3, Failed = 4 }
    Button { TopLeft = 0, BottomLeft = 1, BottomRight = 2, Center = 3 }
    ant::LinkState {
        Idle = 0, Connecting = 1, Connected = 2, Disconnecting = 3, Disconnected = 4,
        TimedOut = 5, TransportLost = 6,
    }
    ant::ScanState { Idle = 0, Starting = 1, Active = 2, Stopping = 3, Uncertain = 4 }
    ant::Error {
        Busy = 0, InvalidIdentity = 1, InvalidDuration = 2, InvalidState = 3, UnsupportedType = 4,
        Capacity = 5, Unavailable = 6, Disconnected = 7, StaleGeneration = 8, Uncertain = 9,
    }
    ble_transport::Link {
        Off = 0, Advertising = 1, Scanning = 2, Connecting = 3, Connected = 4, Retrying = 5,
        Failed = 6,
    }
    positioning::Transport { Unavailable = 0, Receiving = 1, Failed = 2, Silent = 3 }
    device_api::gps::FixState { NoData = 0, NoFix = 1, Fresh = 2, Stale = 3 }
    device_api::gps::Identity { Unknown = 0, Pair = 1, Pdt = 2, Conflicting = 3 }
    sensors::Startup {
        Unsupported = 0, NotSubmitted = 1, Probing = 2, Pending = 3, Waiting = 4, Charging = 5,
        WakePending = 6, AlreadyRunning = 7, RunningDegraded = 8, Ready = 9, Uncertain = 10,
        TimedOut = 11, Unavailable = 12, PowerTransition = 13,
    }
    crate::network_time::Status { Unavailable = 0, Syncing = 1, Fresh = 2, Offline = 3, Stale = 4 }
    Medium { Configuration = 0, Data = 1, Crash = 2, Bulk = 3 }
    StorageOp { Read = 0, Write = 1, Erase = 2, Info = 3 }
    StorageError { Unsupported = 0, Unavailable = 1, Failed = 2, Invalid = 3, Range = 4 }
    Output { Panel = 0, Backlight = 1 }
    Level { Error = 0, Warn = 1 }
    device_api::observation::Error { Unsupported = 0, Unavailable = 1, Failed = 2, Invalid = 3 }
    Method { Get = 0, Head = 1, Delete = 2, Put = 3, Post = 4, Options = 5 }
    sound::State {
        Idle = 0, Queued = 1, Submitted = 2, Elapsed = 3, StopQueued = 4, StopSubmitted = 5,
        Failed = 6,
    }
    power::Operation { Shutdown = 0, Sleep = 1, Wake = 2 }
    power::State {
        Initializing = 0, Idle = 1, Charging = 2, Requested = 3, Quiescing = 4, Submitted = 5,
        Sleeping = 6, Recovering = 7, Completed = 8, Failed = 9, Uncertain = 10,
    }
    power::Failure {
        Busy = 0, Storage = 1, Display = 2, Wifi = 3, Ble = 4, Ant = 5, Gnss = 6, Sound = 7,
        Companion = 8, Sleep = 9, PreparationTimeout = 10, Submission = 11,
        CompletionUnobservable = 12, Recovery = 13,
    }
}

record! {
    Point { x, y }
    Edge { sequence, received_ms: time, input }
    ant::Identity { device_type, device_number, transmission_type }
    ant::Packet { identity, data, received_ms: time, generation, loss_count }
    ant::Snapshot {
        scanning, link, selected, generation, packets, dropped_packets, discovery_overflows,
        transport_losses, tx_failures, age_ms, stale,
    }
    ant::Discovery { identity, rssi, seen_ms: time }
    ant::ScanSnapshot { state, generation, discovery_overflows }
    ant::Target { identity, generation }
    ant::SendObservation { id, target, stage }
    ble_transport::Snapshot { link, connections, disconnections, notifications, dropped, scan_reports }
    ble_transport::BatteryLevel { connection, raw, received_ms: time }
    device_api::gps::Snapshot {
        state, identity, latitude_e7, longitude_e7, satellites, hdop_tenths, speed_mm_s,
        epoch_ms, utc, utc_ms, age_ms, bytes, valid_sentences, checksum_errors, parse_errors,
        overflows, line_overflows, long_proprietary, uart_errors, binary_bytes, non_nmea_frames,
    }
    positioning::Snapshot { gps, transport, sequence, received_at_ms: times, fault_at_ms: times }
    sensors::Pressure { pressure_centi_pa, temperature_centi_c }
    sensors::Motion { axes }
    sensors::Identity { fields }
    sensors::Snapshot { pressure, motion, identity, reports, invalid_reports, losses }
    InputSnapshot {
        battery, power, button_counts, touch_available, touch_errors, companion_valid,
        companion_bad_crc, uart_errors, input_lost,
    }
    crate::network_time::Snapshot { unix_seconds, millis, local_minutes, age_ms, status }
    AntRadio<'a> { availability, capabilities, scanning, scan }
    ServerRequest<'a> { id, method, path, query, session, gzip, body }
    Reach { access_point, station }
    sound::Snapshot { state, pattern, requested_ms: time, submitted_ms: times, until_ms: time }
    power::Status { sequence, operation, state, at_ms: time, prepare_at_ms: times, failure, ready }
    StorageFault { medium, attempt, op, at, landed, error }
    AntCapabilities<'a> {
        supported_types, connection_capacity, one_peer_per_type, concurrent_scan,
        acknowledged_send, radio_delivery_feedback, burst, max_pending_sends,
    }
}

impl Wire<'_> for Input {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Touch(point) => {
                w.byte(0);
                point.put(w);
            }
            Self::Release => w.byte(1),
            Self::Cancel => w.byte(2),
            Self::Button { button, code } => {
                w.byte(3);
                button.put(w);
                code.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Touch(Point::take(r)),
            1 => Self::Release,
            2 => Self::Cancel,
            3 => Self::Button {
                button: Button::take(r),
                code: u16::take(r),
            },
            _ => {
                r.fail();
                Self::Cancel
            }
        }
    }
}

impl Wire<'_> for ant::OperationId {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        Self(u32::take(r))
    }
}

impl Wire<'_> for ant::AntOperation {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Scan(duration_ms) => {
                w.byte(0);
                duration_ms.put(w);
            }
            Self::StopScan => w.byte(1),
            Self::Connect(identity) => {
                w.byte(2);
                identity.put(w);
            }
            Self::Disconnect(device_type) => {
                w.byte(3);
                device_type.put(w);
            }
            Self::Send { target, data } => {
                w.byte(4);
                target.put(w);
                data.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Scan(u32::take(r)),
            1 => Self::StopScan,
            2 => Self::Connect(ant::Identity::take(r)),
            3 => Self::Disconnect(u8::take(r)),
            4 => Self::Send {
                target: ant::Target::take(r),
                data: <[u8; 8]>::take(r),
            },
            _ => {
                r.fail();
                Self::StopScan
            }
        }
    }
}

impl Wire<'_> for ant::SendStage {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Queued => w.byte(0),
            Self::UartSubmitted => w.byte(1),
            Self::BridgeReplied { accepted } => {
                w.byte(2);
                accepted.put(w);
            }
            Self::Uncertain => w.byte(3),
            Self::Cancelled => w.byte(4),
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Queued,
            1 => Self::UartSubmitted,
            2 => Self::BridgeReplied {
                accepted: bool::take(r),
            },
            3 => Self::Uncertain,
            4 => Self::Cancelled,
            _ => {
                r.fail();
                Self::Uncertain
            }
        }
    }
}

impl Wire<'_> for Result<ant::Admission, ant::Error> {
    fn put(&self, w: &mut Writer) {
        match self {
            Ok(ant::Admission::Accepted) => w.byte(0),
            Ok(ant::Admission::SendQueued(id)) => {
                w.byte(1);
                id.put(w);
            }
            Err(error) => {
                w.byte(2);
                error.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Ok(ant::Admission::Accepted),
            1 => Ok(ant::Admission::SendQueued(ant::OperationId::take(r))),
            2 => Err(ant::Error::take(r)),
            _ => {
                r.fail();
                Err(ant::Error::Uncertain)
            }
        }
    }
}

impl Wire<'_> for Result<(), device_api::observation::Error> {
    fn put(&self, w: &mut Writer) {
        match self {
            Ok(()) => w.byte(0),
            Err(error) => {
                w.byte(1);
                error.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Ok(()),
            1 => Err(Wire::take(r)),
            _ => {
                r.fail();
                Ok(())
            }
        }
    }
}

impl Wire<'_> for AccessPointState {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Off => w.byte(0),
            Self::Starting => w.byte(1),
            Self::Up { clients } => {
                w.byte(2);
                clients.put(w);
            }
            Self::Stopping => w.byte(3),
            Self::Failed => w.byte(4),
            Self::Unsupported => w.byte(5),
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Off,
            1 => Self::Starting,
            2 => Self::Up {
                clients: u8::take(r),
            },
            3 => Self::Stopping,
            4 => Self::Failed,
            5 => Self::Unsupported,
            _ => {
                r.fail();
                Self::Failed
            }
        }
    }
}

impl Wire<'_> for ble_transport::Packet {
    fn put(&self, w: &mut Writer) {
        self.connection.put(w);
        self.sequence.put(w);
        self.dropped.put(w);
        w.time(self.received_ms);
        self.length.put(w);
        w.bytes(&self.bytes[..usize::from(self.length).min(self.bytes.len())]);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        let connection = u32::take(r);
        let sequence = u32::take(r);
        let dropped = u32::take(r);
        let received_ms = r.time();
        let length = u8::take(r);
        let mut bytes = [0; 60];
        let data = r.bytes(usize::from(length).min(bytes.len()));
        bytes[..data.len()].copy_from_slice(data);
        Self {
            connection,
            sequence,
            dropped,
            received_ms,
            bytes,
            length,
        }
    }
}

impl<'a> Wire<'a> for Header<'a> {
    fn put(&self, w: &mut Writer) {
        self.version.put(w);
        self.oldest_reader.put(w);
        w.bytes(&self.build);
        self.boot.put(w);
        self.profile.put(w);
        self.config.put(w);
        self.media.put(w);
    }
    fn take(r: &mut Reader<'a>) -> Self {
        Self {
            version: u16::take(r),
            oldest_reader: u16::take(r),
            build: <[u8; 20]>::take(r),
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
            Self::Power(_) => tag::POWER,
            Self::NetworkTime(_) => tag::NETWORK_TIME,
            Self::Startup(_) => tag::STARTUP,
            Self::Command(_) => tag::COMMAND,
            Self::StorageFault(_) => tag::STORAGE_FAULT,
            Self::Entropy(_) => tag::ENTROPY,
            Self::TooLarge(_) => tag::TOO_LARGE,
            Self::Unknown { tag, .. } => *tag,
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
            Self::Shown(_) => tag::SHOWN,
            Self::Log { .. } => tag::LOG,
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
            Self::Power(snapshot) => snapshot.put(w),
            Self::NetworkTime(snapshot) => snapshot.put(w),
            Self::Startup(startup) => startup.put(w),
            Self::Command(text)
            | Self::Page(text)
            | Self::Toast(text)
            | Self::RideState(text)
            | Self::Shown(text) => text.put(w),
            Self::StorageFault(fault) => fault.put(w),
            Self::Unknown { body, .. } => w.bytes(body),
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
            tag::SHOWN => Self::Shown(Wire::take(r)),
            tag::LOG => Self::Log {
                level: Wire::take(r),
                target: Wire::take(r),
                text: Wire::take(r),
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

/// A position in a trace's events, to resume decoding later, also over the
/// same trace grown since: the open block resealed longer, or more blocks.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cursor {
    block: usize,
    at: usize,
    now: u64,
    later: bool,
    end: Option<End>,
}

/// The trace's events in order, ending before a torn tail.
pub struct Events<'a> {
    bytes: &'a [u8],
    cursor: Cursor,
    payload: Option<&'a [u8]>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The header block is missing, torn or malformed.
    Header,
    /// No version of the format has this number.
    Version(u16),
}

/// The header and the events after it. A later version may append fields to
/// the header and to any record; they are skipped.
pub fn decode(bytes: &[u8]) -> Result<(Header<'_>, Events<'_>), Error> {
    let (payload, next) = block(bytes, 0).ok_or(Error::Header)?;
    let mut r = Reader::new(payload, 0);
    let header = Header::take(&mut r);
    let later = header.version > VERSION;
    if !r.ok() || (r.at != payload.len() && !later) {
        return Err(Error::Header);
    }
    if header.version == 0 || header.oldest_reader > VERSION {
        return Err(Error::Version(header.version));
    }
    let cursor = Cursor {
        block: next,
        at: 0,
        now: 0,
        later,
        end: None,
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
    let len = usize::from(u16::from_le_bytes(
        bytes.get(start..start.checked_add(2)?)?.try_into().ok()?,
    ));
    let payload = bytes.get(start + 2..start + 2 + len)?;
    let crc = bytes.get(start + 2 + len..start + FRAME + len)?;
    (crc32(payload).to_le_bytes() == crc).then_some((payload, start + FRAME + len))
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
            payload: None,
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
        let payload = match self.payload {
            Some(payload) => payload,
            None if c.block >= self.bytes.len() && c.at == 0 => return Err(End::Complete),
            None => {
                let (payload, _) =
                    block(self.bytes, c.block).ok_or_else(|| failed(self.bytes, c.block))?;
                if c.at > payload.len() {
                    return Err(End::Torn);
                }
                *self.payload.insert(payload)
            }
        };
        let Some(&first) = payload.get(c.at) else {
            let next = c.block + FRAME + payload.len();
            if next >= self.bytes.len() {
                return Err(End::Complete);
            }
            c.block = next;
            c.at = 0;
            self.payload = None;
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
            let unknown = Event::Unknown { tag: first, body };
            match Event::take_body(first, &mut r) {
                Some(_) if c.later && !r.ok() => return Ok(Some(unknown)),
                Some(event) => Some(event),
                None => return Ok(Some(unknown)),
            }
        };
        if !r.ok() || (r.at != body.len() && !c.later) {
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
