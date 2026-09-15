//! Bounded ANT discovery and independent receive channels by device type.
//!
//! The device translates control requests and classifies transport messages. Data
//! has no sender identifier, so it is admitted only after a matching connection
//! event and only for the selected device type. Profiles belong to consumers.
use crate::observation::Availability;

pub const DISCOVERY_CAPACITY: usize = 8;
pub const PACKET_CAPACITY: usize = 4;
pub const CONNECT_TIMEOUT_MS: u64 = 10_000;
pub const SCAN_STOP_TIMEOUT_MS: u64 = 2_000;
pub const STALE_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Identity {
    pub device_type: u8,
    pub device_number: u16,
    pub transmission_type: u8,
}

/// Transport-independent observations from the device radio adapter.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Discovery { identity: Identity, rssi: i8 },
    ScanEnded,
    Connected(Identity),
    Disconnected(Identity),
    Timeout(Identity),
    Data { device_type: u8, data: [u8; 8] },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Request {
    Scan { duration_ms: u32 },
    StopScan,
    Connect { identity: Identity },
    Disconnect { identity: Identity },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Busy,
    InvalidIdentity,
    InvalidDuration,
    InvalidState,
    UnsupportedType,
    Capacity,
    Unavailable,
    Disconnected,
    StaleGeneration,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LinkState {
    Idle,
    Connecting,
    Connected,
    Disconnecting,
    Disconnected,
    TimedOut,
    TransportLost,
}

impl LinkState {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Disconnecting => "disconnecting",
            Self::Disconnected => "disconnected",
            Self::TimedOut => "timed_out",
            Self::TransportLost => "transport_lost",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Discovery {
    pub identity: Identity,
    pub rssi: i8,
    pub seen_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Packet {
    pub identity: Identity,
    pub data: [u8; 8],
    pub received_ms: u64,
    pub generation: u32,
    pub loss_count: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub scanning: bool,
    pub link: LinkState,
    pub selected: Option<Identity>,
    pub generation: u32,
    pub packets: u32,
    pub dropped_packets: u32,
    pub discovery_overflows: u32,
    pub transport_losses: u32,
    pub tx_failures: u32,
    pub age_ms: Option<u64>,
    pub stale: bool,
}

pub const CHANNEL_CAPACITY: usize = 10;
pub const SEND_TIMEOUT_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Capabilities {
    pub supported_types: &'static [u8],
    /// Configured host bound, not measured simultaneous RF connections.
    pub connection_capacity: u8,
    pub one_peer_per_type: bool,
    pub concurrent_scan: bool,
    pub acknowledged_send: bool,
    pub radio_delivery_feedback: bool,
    pub burst: bool,
    /// Maximum admitted sends before the device requires a companion restart.
    pub max_sends_per_restart: u8,
}

impl Capabilities {
    pub const UNAVAILABLE: Self = Self {
        supported_types: &[],
        connection_capacity: 0,
        one_peer_per_type: true,
        concurrent_scan: false,
        acknowledged_send: false,
        radio_delivery_feedback: false,
        burst: false,
        max_sends_per_restart: 0,
    };
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    /// Host-selected identity; received data does not establish sender identity.
    pub identity: Identity,
    pub generation: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OperationId(pub u32);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Admission {
    Accepted,
    SendQueued(OperationId),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SendStage {
    Queued,
    /// All bytes reached the UART driver, not necessarily the radio.
    UartSubmitted,
    /// The bridge wrapper replied; radio acceptance and delivery remain unknown.
    BridgeReplied {
        accepted: bool,
    },
    /// A partial write, timeout or invalidated session makes delivery unknown.
    Uncertain,
    /// Cancelled before the UART owner took the operation.
    Cancelled,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SendObservation {
    pub id: OperationId,
    pub target: Target,
    pub stage: SendStage,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
/// Local discovery observation window, not proof of physical radio ownership.
/// Untagged scan-end reports never complete or clear an uncertain window.
pub enum ScanState {
    Idle,
    Starting,
    Active,
    Stopping,
    Uncertain,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScanSnapshot {
    pub state: ScanState,
    pub generation: u32,
    pub discovery_overflows: u32,
}

pub enum AntOperation {
    Scan(u32),
    StopScan,
    Connect(crate::ant::Identity),
    Disconnect(u8),
    Send { target: Target, data: [u8; 8] },
}

pub trait Ant {
    fn scanning(&self) -> bool;
    fn discoveries(&self) -> [Option<crate::ant::Discovery>; 8];
    fn request(&mut self, operation: AntOperation, now_ms: u64) -> Result<Admission, Error>;
    fn capabilities(&self) -> Capabilities {
        Capabilities::UNAVAILABLE
    }
    fn scan(&self) -> ScanSnapshot {
        ScanSnapshot {
            state: ScanState::Idle,
            generation: 0,
            discovery_overflows: 0,
        }
    }
    /// One outstanding send and one retained observation. Local IDs never correlate wire replies.
    fn send_status(&self, _id: OperationId) -> Option<SendObservation> {
        None
    }
    /// Independent bounded diagnostic stream; never consumes the production stream.
    fn take_diagnostic_packet(&mut self) -> Option<Packet> {
        None
    }
    fn channel(&self, kind: u8, now_ms: u64) -> Option<crate::ant::Snapshot> {
        self.channels(now_ms)
            .into_iter()
            .flatten()
            .find(|s| s.selected.is_some_and(|p| p.device_type == kind))
    }
    fn availability(&self) -> Availability;
    fn channels(&self, now_ms: u64)
    -> [Option<crate::ant::Snapshot>; crate::ant::CHANNEL_CAPACITY];
    /// The production consumer owns this destructive receive stream.
    fn take_packet(&mut self) -> Option<crate::ant::Packet>;
}
