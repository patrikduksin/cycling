//! Position acquisition state. Publication replaces old state; consumers do not
//! acknowledge updates and cannot backpressure the receiver.
use crate::gps;
use crate::observation::Availability;

/// Silence is a transport observation, distinct from a receiver reporting no fix.
pub const SILENT_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    Unavailable,
    Receiving,
    Failed,
    Silent,
}
impl Transport {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Receiving => "receiving",
            Self::Failed => "failed",
            Self::Silent => "silent",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub gps: gps::Snapshot,
    pub transport: Transport,
    pub sequence: u32,
    pub received_at_ms: Option<u64>,
    pub fault_at_ms: Option<u64>,
}

pub trait Positioning {
    fn availability(&self) -> Availability;
    fn snapshot(&self, now_ms: u64) -> Option<crate::positioning::Snapshot>;
}
