//! One-peer BLE data capabilities. Payload interpretation belongs to consumers.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Link {
    #[default]
    Off,
    Advertising,
    Scanning,
    Connecting,
    Connected,
    Retrying,
    Failed,
}
impl Link {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Advertising => "advertising",
            Self::Scanning => "scanning",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Retrying => "retrying",
            Self::Failed => "failed",
        }
    }
}
/// Match the supplied name and optional address, or an address alone with an
/// empty name. A client selection with neither is rejected. UUIDs are 16-bit.
#[derive(Clone, Copy, Eq, PartialEq)]
pub struct Selection {
    pub name: crate::connectivity::Text<32>,
    pub address: Option<[u8; 6]>,
    pub service: u16,
    pub characteristic: u16,
}
impl core::fmt::Debug for Selection {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Selection")
            .field("peer", &"<private>")
            .field("service", &self.service)
            .field("characteristic", &self.characteristic)
            .finish()
    }
}
#[derive(Clone, Copy, Debug, Default)]
pub struct Snapshot {
    pub link: Link,
    pub connections: u32,
    pub disconnections: u32,
    pub notifications: u32,
    pub dropped: u32,
    pub scan_reports: u32,
}
/// One complete notification, never truncated. Four packets fit in the producer
/// queue; later packets are dropped when full. Generation, sequence and drop
/// stamps expose discontinuity to consumers. No acknowledgment blocks the radio.
#[derive(Clone, Copy)]
pub struct Packet {
    pub connection: u32,
    pub sequence: u32,
    pub dropped: u32,
    pub received_ms: u64,
    pub bytes: [u8; 60],
    pub length: u8,
}

impl Packet {
    /// Reject malformed lengths rather than truncating a characteristic value.
    pub fn data(&self) -> Option<&[u8]> {
        self.bytes.get(..usize::from(self.length))
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Operation {
    Scan,
    Select(Option<Selection>),
    Connect,
    Disconnect,
    Echo,
}
#[derive(Clone, Copy)]
pub struct Discovery {
    pub name: crate::connectivity::Text<32>,
    pub address: [u8; 6],
    pub random: bool,
    pub rssi: i8,
}
