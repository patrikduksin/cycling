//! Bounded ANT discovery and one selected receive channel.
//!
//! The device translates control requests and classifies transport messages. Data
//! has no sender identifier, so it is admitted only after a matching connection
//! event and only for the selected device type. Profiles belong to consumers.

pub const DISCOVERY_CAPACITY: usize = 8;
pub const PACKET_CAPACITY: usize = 4;
pub const CONNECT_TIMEOUT_MS: u64 = 10_000;
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

pub struct State {
    scan_until: Option<u64>,
    discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
    selected: Option<Identity>,
    link: LinkState,
    connect_until: u64,
    generation: u32,
    queue: [Option<Packet>; PACKET_CAPACITY],
    queue_len: usize,
    last_packet: Option<u64>,
    packets: u32,
    dropped_packets: u32,
    discovery_overflows: u32,
    transport_losses: u32,
    tx_failures: u32,
}

impl Default for State {
    fn default() -> Self {
        Self::new()
    }
}

impl State {
    pub const fn new() -> Self {
        Self {
            scan_until: None,
            discoveries: [None; DISCOVERY_CAPACITY],
            selected: None,
            link: LinkState::Idle,
            connect_until: 0,
            generation: 0,
            queue: [None; PACKET_CAPACITY],
            queue_len: 0,
            last_packet: None,
            packets: 0,
            dropped_packets: 0,
            discovery_overflows: 0,
            transport_losses: 0,
            tx_failures: 0,
        }
    }

    pub fn begin_scan(&mut self, now: u64, duration_ms: u32) -> Result<Request, Error> {
        if duration_ms == 0 {
            return Err(Error::InvalidDuration);
        }
        if self.scan_until.is_some()
            || matches!(
                self.link,
                LinkState::Connecting | LinkState::Connected | LinkState::Disconnecting
            )
        {
            return Err(Error::Busy);
        }
        self.discoveries.fill(None);
        self.scan_until = Some(now.saturating_add(u64::from(duration_ms)));
        Ok(Request::Scan { duration_ms })
    }

    pub fn stop_scan(&mut self) -> Request {
        self.scan_until = None;
        Request::StopScan
    }

    pub fn connect(&mut self, identity: Identity, now: u64) -> Result<Request, Error> {
        // Zero is a wildcard on the wire; a selected channel must be explicit.
        if identity.device_type == 0
            || identity.device_number == 0
            || identity.transmission_type == 0
        {
            return Err(Error::InvalidIdentity);
        }
        if self.scan_until.is_some()
            || matches!(
                self.link,
                LinkState::Connecting | LinkState::Connected | LinkState::Disconnecting
            )
        {
            return Err(Error::Busy);
        }
        // A failed or timed-out command may still own a radio channel. Do not
        // relabel its identity until a matching close event confirms cleanup.
        if self.selected.is_some_and(|previous| previous != identity)
            && self.link != LinkState::Disconnected
        {
            return Err(Error::Busy);
        }
        self.invalidate();
        self.selected = Some(identity);
        self.link = LinkState::Connecting;
        self.connect_until = now.saturating_add(CONNECT_TIMEOUT_MS);
        Ok(Request::Connect { identity })
    }

    pub fn disconnect(&mut self, now: u64) -> Option<Request> {
        if self.link == LinkState::Disconnecting {
            return None;
        }
        let identity = self.selected?;
        self.invalidate();
        self.link = LinkState::Disconnecting;
        self.connect_until = now.saturating_add(CONNECT_TIMEOUT_MS);
        Some(Request::Disconnect { identity })
    }

    fn invalidate(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.queue.fill(None);
        self.queue_len = 0;
        self.last_packet = None;
    }

    pub fn tick(&mut self, now: u64) {
        if self.scan_until.is_some_and(|deadline| now >= deadline) {
            self.scan_until = None;
        }
        if matches!(self.link, LinkState::Connecting | LinkState::Disconnecting)
            && now >= self.connect_until
        {
            self.invalidate();
            self.link = LinkState::TimedOut;
        }
    }

    /// Receive a normalized radio observation, never raw UART bytes.
    pub fn receive(&mut self, event: Event, now: u64) {
        self.tick(now);
        let (device_type, data) = match event {
            Event::Discovery { identity, rssi } => {
                if self.scan_until.is_none() {
                    return;
                }
                let slot = self
                    .discoveries
                    .iter()
                    .position(|entry| entry.is_some_and(|d| d.identity == identity))
                    .or_else(|| self.discoveries.iter().position(Option::is_none));
                if let Some(slot) = slot {
                    self.discoveries[slot] = Some(Discovery {
                        identity,
                        rssi,
                        seen_ms: now,
                    });
                } else {
                    self.discovery_overflows = self.discovery_overflows.saturating_add(1);
                }
                return;
            }
            Event::ScanEnded => {
                self.scan_until = None;
                return;
            }
            Event::Connected(identity) => {
                if self.selected == Some(identity) && self.link == LinkState::Connecting {
                    self.link = LinkState::Connected;
                }
                return;
            }
            Event::Disconnected(identity) | Event::Timeout(identity) => {
                if self.selected == Some(identity)
                    && (matches!(
                        self.link,
                        LinkState::Connecting | LinkState::Connected | LinkState::Disconnecting
                    ) || matches!(event, Event::Disconnected(_))
                        && matches!(self.link, LinkState::TimedOut | LinkState::TransportLost))
                {
                    self.invalidate();
                    self.link = if matches!(event, Event::Disconnected(_)) {
                        LinkState::Disconnected
                    } else {
                        LinkState::TimedOut
                    };
                }
                return;
            }
            Event::Data { device_type, data } => (device_type, data),
        };
        let Some(identity) = self.selected else {
            return;
        };
        if self.link != LinkState::Connected || device_type != identity.device_type {
            return;
        }
        self.packets = self.packets.saturating_add(1);
        self.last_packet = Some(now);
        // Retain the most recent pages; consumers detect gaps from loss_count.
        if self.queue_len == PACKET_CAPACITY {
            self.pop_packet();
            self.dropped_packets = self.dropped_packets.saturating_add(1);
        }
        self.queue[self.queue_len] = Some(Packet {
            identity,
            data,
            received_ms: now,
            generation: self.generation,
            loss_count: self.dropped_packets.saturating_add(self.transport_losses),
        });
        self.queue_len += 1;
    }

    pub fn transport_loss(&mut self, _now: u64) {
        self.transport_losses = self.transport_losses.saturating_add(1);
        self.scan_until = None;
        self.invalidate();
        self.link = LinkState::TransportLost;
    }

    /// Delivery is uncertain. Do not replay a command or admit further pages.
    pub fn tx_failed(&mut self, now: u64) {
        self.tx_failures = self.tx_failures.saturating_add(1);
        self.transport_loss(now);
    }

    pub fn discoveries(&self) -> &[Option<Discovery>; DISCOVERY_CAPACITY] {
        &self.discoveries
    }

    pub fn pop_packet(&mut self) -> Option<Packet> {
        if self.queue_len == 0 {
            return None;
        }
        let packet = self.queue[0];
        self.queue.rotate_left(1);
        self.queue_len -= 1;
        self.queue[self.queue_len] = None;
        packet
    }

    pub fn snapshot(&self, now: u64) -> Snapshot {
        let age_ms = self.last_packet.map(|time| now.saturating_sub(time));
        Snapshot {
            scanning: self.scan_until.is_some(),
            link: self.link,
            selected: self.selected,
            generation: self.generation,
            packets: self.packets,
            dropped_packets: self.dropped_packets,
            discovery_overflows: self.discovery_overflows,
            transport_losses: self.transport_losses,
            tx_failures: self.tx_failures,
            age_ms,
            stale: self.link != LinkState::Connected || age_ms.is_none_or(|age| age >= STALE_MS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    const PEER: Identity = Identity {
        device_type: 40,
        device_number: 123,
        transmission_type: 5,
    };
    fn connected(state: &mut State) {
        state.connect(PEER, 0).unwrap();
        state.receive(Event::Connected(PEER), 1);
    }

    #[test]
    fn requires_selected_connection_and_filters_other_types() {
        let mut state = State::new();
        state.connect(PEER, 0).unwrap();
        state.receive(
            Event::Data {
                device_type: 40,
                data: [1; 8],
            },
            1,
        );
        state.receive(
            Event::Connected(Identity {
                device_number: 124,
                ..PEER
            }),
            2,
        );
        assert!(state.pop_packet().is_none());
        assert_eq!(state.snapshot(2).link, LinkState::Connecting);
        state.receive(Event::Connected(PEER), 3);
        state.receive(
            Event::Data {
                device_type: 41,
                data: [2; 8],
            },
            4,
        );
        state.receive(
            Event::Data {
                device_type: 40,
                data: [3; 8],
            },
            5,
        );
        assert_eq!(state.pop_packet().unwrap().data, [3; 8]);
        assert!(state.pop_packet().is_none());
        assert!(!state.snapshot(5).stale);
        assert!(state.snapshot(5 + STALE_MS).stale);
    }

    #[test]
    fn scan_is_bounded_deduplicates_and_ends() {
        let mut state = State::new();
        state.begin_scan(0, 100).unwrap();
        for number in 1..=9 {
            state.receive(
                Event::Discovery {
                    identity: Identity {
                        device_number: number,
                        ..PEER
                    },
                    rssi: -56,
                },
                1,
            );
        }
        state.receive(
            Event::Discovery {
                identity: Identity {
                    device_number: 1,
                    ..PEER
                },
                rssi: -66,
            },
            2,
        );
        assert_eq!(state.discoveries()[0].unwrap().rssi, -66);
        assert_eq!(state.snapshot(2).discovery_overflows, 1);
        assert_eq!(state.connect(PEER, 2), Err(Error::Busy));
        state.receive(Event::ScanEnded, 3);
        assert!(!state.snapshot(3).scanning);
        state.begin_scan(4, 10).unwrap();
        state.tick(14);
        assert!(!state.snapshot(14).scanning);
    }

    #[test]
    fn packet_overflow_exposes_loss_and_retains_latest() {
        let mut state = State::new();
        connected(&mut state);
        for value in 0..6 {
            state.receive(
                Event::Data {
                    device_type: 40,
                    data: [value; 8],
                },
                2,
            );
        }
        assert_eq!(state.snapshot(2).dropped_packets, 2);
        for value in 2..6 {
            let packet = state.pop_packet().unwrap();
            assert_eq!(packet.data, [value; 8]);
            if value == 5 {
                assert_eq!(packet.loss_count, 2);
            }
        }
        assert!(state.pop_packet().is_none());
    }

    #[test]
    fn transport_loss_invalidates_pages_and_late_events() {
        let mut state = State::new();
        connected(&mut state);
        state.receive(
            Event::Data {
                device_type: 40,
                data: [0; 8],
            },
            2,
        );
        let generation = state.snapshot(2).generation;
        state.tx_failed(3);
        state.receive(Event::Connected(PEER), 4);
        state.receive(
            Event::Data {
                device_type: 40,
                data: [1; 8],
            },
            5,
        );
        assert!(state.pop_packet().is_none());
        let snapshot = state.snapshot(5);
        assert_eq!(snapshot.link, LinkState::TransportLost);
        assert_eq!(snapshot.generation, generation + 1);
        assert_eq!(snapshot.tx_failures, 1);
        assert!(snapshot.stale);
    }

    #[test]
    fn timeout_and_disconnect_never_replay_or_accept_late_data() {
        let mut state = State::new();
        state.connect(PEER, 0).unwrap();
        state.tick(CONNECT_TIMEOUT_MS);
        state.receive(Event::Connected(PEER), CONNECT_TIMEOUT_MS + 1);
        assert_eq!(state.snapshot(CONNECT_TIMEOUT_MS).link, LinkState::TimedOut);
        state.connect(PEER, CONNECT_TIMEOUT_MS + 2).unwrap();
        state.receive(Event::Connected(PEER), CONNECT_TIMEOUT_MS + 3);
        assert_eq!(
            state.disconnect(CONNECT_TIMEOUT_MS + 4),
            Some(Request::Disconnect { identity: PEER })
        );
        state.receive(
            Event::Data {
                device_type: 40,
                data: [0; 8],
            },
            CONNECT_TIMEOUT_MS + 5,
        );
        assert!(state.pop_packet().is_none());
        assert_eq!(
            state.snapshot(CONNECT_TIMEOUT_MS + 5).link,
            LinkState::Disconnecting
        );
    }
    #[test]
    fn switching_peer_requires_confirmed_close() {
        let mut state = State::new();
        let other = Identity {
            device_number: 124,
            ..PEER
        };
        connected(&mut state);
        state.disconnect(2);
        assert_eq!(state.connect(other, 3), Err(Error::Busy));
        state.tick(CONNECT_TIMEOUT_MS + 2);
        assert_eq!(
            state.snapshot(CONNECT_TIMEOUT_MS + 2).link,
            LinkState::TimedOut
        );
        assert_eq!(
            state.connect(other, CONNECT_TIMEOUT_MS + 3),
            Err(Error::Busy)
        );
        state.receive(Event::Disconnected(other), CONNECT_TIMEOUT_MS + 5);
        assert_eq!(
            state.connect(other, CONNECT_TIMEOUT_MS + 6),
            Err(Error::Busy)
        );
        state.receive(Event::Disconnected(PEER), CONNECT_TIMEOUT_MS + 7);
        assert_eq!(
            state.connect(other, CONNECT_TIMEOUT_MS + 8),
            Ok(Request::Connect { identity: other })
        );
    }
    #[test]
    fn late_close_after_loss_confirms_cleanup_but_late_open_does_not() {
        let mut state = State::new();
        connected(&mut state);
        state.transport_loss(2);
        state.receive(Event::Connected(PEER), 3);
        assert_eq!(state.snapshot(3).link, LinkState::TransportLost);
        state.receive(Event::Disconnected(PEER), 4);
        let other = Identity {
            device_number: 124,
            ..PEER
        };
        assert_eq!(
            state.connect(other, 5),
            Ok(Request::Connect { identity: other })
        );
    }

    #[test]
    fn repeated_disconnect_does_not_send_or_extend_pending_close() {
        let mut state = State::new();
        connected(&mut state);
        assert_eq!(
            state.disconnect(2),
            Some(Request::Disconnect { identity: PEER })
        );
        let generation = state.snapshot(2).generation;
        assert_eq!(state.disconnect(CONNECT_TIMEOUT_MS), None);
        assert_eq!(state.snapshot(CONNECT_TIMEOUT_MS).generation, generation);
        state.tick(CONNECT_TIMEOUT_MS + 2);
        assert_eq!(
            state.snapshot(CONNECT_TIMEOUT_MS + 2).link,
            LinkState::TimedOut
        );
    }
}
