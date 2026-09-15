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

pub const CHANNEL_CAPACITY: usize = 4;

pub enum AntOperation {
    Scan(u32),
    StopScan,
    Connect(crate::ant::Identity),
    Disconnect(u8),
}

pub trait Ant {
    fn scanning(&self) -> bool;
    fn discoveries(&self) -> [Option<crate::ant::Discovery>; 8];
    fn request(&mut self, operation: AntOperation, now_ms: u64) -> &'static str;
    fn channel(&self, kind: u8, now_ms: u64) -> Option<crate::ant::Snapshot> {
        self.channels(now_ms)
            .into_iter()
            .flatten()
            .find(|s| s.selected.is_some_and(|p| p.device_type == kind))
    }
    fn availability(&self) -> Availability;
    fn channels(&self, now_ms: u64)
    -> [Option<crate::ant::Snapshot>; crate::ant::CHANNEL_CAPACITY];
    fn take_packet(&mut self) -> Option<crate::ant::Packet>;
}
