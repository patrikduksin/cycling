//! Presentation geometry and physical observations, without gesture policy.
/// Availability describes support and initialization, separately from observation freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Unsupported,
    Initializing,
    Unconfigured,
    Ready,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Unavailable,
    Failed,
    Invalid,
}
impl Availability {
    /// Reject operations before enqueueing when no initialized owner can act.
    pub fn require_ready(self) -> Result<(), Error> {
        match self {
            Self::Ready => Ok(()),
            Self::Unsupported => Err(Error::Unsupported),
            Self::Initializing | Self::Unconfigured => Err(Error::Unavailable),
            Self::Failed => Err(Error::Failed),
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rgb565,
}
/// Submission is synchronous. Return, including error, means all DMA access has
/// ended and the device owns its buffers again. No cancellable future is exposed.
pub trait Display {
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn geometry(&self) -> Geometry;
    fn format(&self) -> Format {
        Format::Rgb565
    }
    fn submit(&mut self, pixel: impl Fn(usize, usize) -> u16) -> Result<(), Error>;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    TopLeft,
    BottomLeft,
    BottomRight,
    Center,
}
impl Button {
    pub fn index(self) -> usize {
        match self {
            Self::TopLeft => 0,
            Self::BottomLeft => 1,
            Self::BottomRight => 2,
            Self::Center => 3,
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    pub touch: Availability,
    pub buttons: &'static [Button],
}
pub trait InputSource {
    fn controls(&self) -> Controls;
    fn take_edge(&mut self) -> Option<Edge>;
}
pub trait Power {
    fn availability(&self) -> Availability;
    fn battery(&self) -> Option<(u8, u16, u64)>;
    fn brightness(&mut self, percent: u8) -> Result<(), Error>;
}
pub trait Positioning {
    fn availability(&self) -> Availability;
    fn snapshot(&self, now_ms: u64) -> Option<crate::positioning::Snapshot>;
}
pub trait Ble {
    fn request(&mut self, _operation: crate::ble_transport::Operation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn control(&self) -> crate::connectivity::ControlStatus {
        crate::connectivity::ControlStatus::new()
    }
    fn discoveries(&self) -> [Option<crate::ble_transport::Discovery>; 8] {
        [None; 8]
    }

    fn availability(&self) -> Availability;
    fn snapshot(&self) -> crate::ble_transport::Snapshot;
    fn take_packet(&mut self) -> Option<crate::ble_transport::Packet>;
    fn reconnect(&mut self) -> Result<(), Error>;
}
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
#[derive(Clone, Copy)]
pub struct InputSnapshot {
    pub battery: Observation<(u8, u16)>,
    pub power: Observation<u8>,
    pub button_counts: [u32; 3],
    pub touch_available: bool,
    pub touch_errors: u32,
    pub companion_valid: u32,
    pub companion_bad_crc: u32,
    pub uart_errors: u32,
    pub input_lost: u32,
}
pub trait InputObservation {
    fn snapshot(&self, now_ms: u64) -> Option<InputSnapshot>;
}
pub trait Console {
    fn read(&mut self) -> Option<u8>;
    fn write(&mut self, byte: u8) -> bool;
    fn flush(&mut self);
}
/// The concrete network handle is Embassy's Stack. Transport existence does not
/// promise DHCP/link/internet readiness; compare generations around awaited IO.
#[cfg(feature = "network-stack")]
pub trait Network {
    fn request(&mut self, _operation: crate::connectivity::WifiOperation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn control(&self) -> crate::connectivity::ControlStatus {
        crate::connectivity::ControlStatus::new()
    }
    fn discoveries(&self) -> [Option<crate::connectivity::NetworkDiscovery>; 8] {
        [None; 8]
    }

    fn online(&self) -> bool;
    fn state(&self) -> u8;
    fn stats(&self) -> (u32, u32, u32, u8);
    fn availability(&self) -> Availability;
    fn stack(&self) -> Option<embassy_net::Stack<'static>>;
    fn connection_generation(&self) -> u32;
    fn reconnect(&mut self) -> Result<(), Error>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation<T> {
    Unavailable,
    Fresh { value: T, received_ms: u64 },
    Stale { value: T, received_ms: u64 },
}
pub fn observation<T: Copy>(value: Option<(T, u64)>, now: u64, stale_ms: u64) -> Observation<T> {
    match value {
        None => Observation::Unavailable,
        Some((value, received_ms)) if now.saturating_sub(received_ms) <= stale_ms => {
            Observation::Fresh { value, received_ms }
        }
        Some((value, received_ms)) => Observation::Stale { value, received_ms },
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Touch(Point),
    Release,
    /// Cancel all consumer-held input. Caused by invalid/lost touch or edge loss.
    Cancel,
    /// Known physical button with uninterpreted companion report code. Repeated
    /// code 1 reports do not establish a physical release; clients choose policy.
    Button {
        button: Button,
        code: u16,
    },
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub sequence: u32,
    pub received_ms: u64,
    pub input: Input,
}

/// Single-consumer bounded edge queue. On overflow discard queued history and
/// enqueue Cancel before the newest event, so consumers cannot retain old holds.
/// Acquisition never waits for a consumer. `lost` counts discarded input edges.
pub struct Edges {
    values: [Option<Edge>; 16],
    head: usize,
    len: usize,
    sequence: u32,
    pub lost: u32,
}
impl Default for Edges {
    fn default() -> Self {
        Self::new()
    }
}
impl Edges {
    pub const fn new() -> Self {
        Self {
            values: [None; 16],
            head: 0,
            len: 0,
            sequence: 0,
            lost: 0,
        }
    }
    pub fn push(&mut self, received_ms: u64, input: Input) {
        if self.len == self.values.len() {
            self.lost = self.lost.saturating_add(self.len as u32);
            self.head = 0;
            self.len = 0;
            self.insert(received_ms, Input::Cancel);
        }
        self.insert(received_ms, input);
    }
    fn insert(&mut self, received_ms: u64, input: Input) {
        self.sequence = self.sequence.wrapping_add(1);
        let index = (self.head + self.len) % self.values.len();
        self.values[index] = Some(Edge {
            sequence: self.sequence,
            received_ms,
            input,
        });
        self.len += 1;
    }
    pub fn pop(&mut self) -> Option<Edge> {
        if self.len == 0 {
            return None;
        }
        let value = self.values[self.head].take();
        self.head = (self.head + 1) % self.values.len();
        self.len -= 1;
        value
    }
}

/// Optional physical sensor observations; motion remains explicitly unscaled
/// until fitted hardware and axes are established.
pub trait Sensors {
    fn snapshot(&self, now: u64) -> crate::companion_sensors::Snapshot;
    fn identity_status(&self) -> &'static str;
    fn query_identity(&mut self, now: u64) -> Result<(), Error>;
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn slow_consumer_gets_cancel_before_new_input() {
        let mut edges = Edges::new();
        for t in 0..16 {
            edges.push(t, Input::Touch(Point { x: t as u16, y: 0 }));
        }
        edges.push(16, Input::Release);
        let cancel = edges.pop().unwrap();
        assert_eq!(cancel.input, Input::Cancel);
        assert_eq!(cancel.sequence, 17);
        assert_eq!(edges.pop().unwrap().input, Input::Release);
        assert!(edges.pop().is_none());
        assert_eq!(edges.lost, 16);
    }
    #[test]
    fn repeated_wraps_preserve_fifo() {
        let mut edges = Edges::new();
        for t in 0..100 {
            edges.push(t, Input::Release);
            assert_eq!(edges.pop().unwrap().received_ms, t);
        }
        assert_eq!(edges.lost, 0);
    }
    #[test]
    fn stale_values_keep_observation_time_and_unavailable_stays_absent() {
        assert_eq!(
            observation::<u8>(None, 9000, 5000),
            Observation::Unavailable
        );
        assert_eq!(
            observation(Some((50, 10)), 5011, 5000),
            Observation::Stale {
                value: 50,
                received_ms: 10
            }
        );
        assert_eq!(
            observation(Some((50, 10)), 5010, 5000),
            Observation::Fresh {
                value: 50,
                received_ms: 10
            }
        );
    }
}
