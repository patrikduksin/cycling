use device_api::ant::*;

pub struct State {
    scan_until: Option<u64>,
    scan_stopping: bool,
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
            scan_stopping: false,
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

    /// Keep discovery ownership while the untagged stop acknowledgment is pending.
    pub fn stop_scan(&mut self, now: u64) -> Result<Request, Error> {
        if self.scan_until.is_none() || self.scan_stopping {
            return Err(Error::Busy);
        }
        self.scan_stopping = true;
        self.scan_until = Some(now.saturating_add(SCAN_STOP_TIMEOUT_MS));
        Ok(Request::StopScan)
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
            self.scan_stopping = false;
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
                if self.scan_until.is_none() || self.scan_stopping {
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
                self.scan_stopping = false;
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
        self.scan_stopping = false;
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

/// The adapter identifies received pages by type, so only one peer of each type
/// can be selected. Slots with uncertain radio ownership are never reassigned.
pub struct Channels {
    discovery: State,
    channels: [State; CHANNEL_CAPACITY],
    next_packet: usize,
}

impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}

impl Channels {
    pub const fn new() -> Self {
        Self {
            discovery: State::new(),
            channels: [const { State::new() }; CHANNEL_CAPACITY],
            next_packet: 0,
        }
    }

    pub fn begin_scan(&mut self, now: u64, duration_ms: u32) -> Result<Request, Error> {
        if self.channels.iter().any(|channel| {
            matches!(
                channel.link,
                LinkState::Connecting | LinkState::Connected | LinkState::Disconnecting
            )
        }) {
            return Err(Error::Busy);
        }
        self.discovery.begin_scan(now, duration_ms)
    }

    pub fn stop_scan(&mut self, now: u64) -> Result<Request, Error> {
        self.discovery.stop_scan(now)
    }

    pub fn scanning(&self) -> bool {
        self.discovery.scan_until.is_some()
    }

    fn index(&self, device_type: u8) -> Option<usize> {
        self.channels.iter().position(|channel| {
            channel
                .selected
                .is_some_and(|identity| identity.device_type == device_type)
        })
    }

    pub fn connect(&mut self, identity: Identity, now: u64) -> Result<Request, Error> {
        if identity.device_type == 0
            || identity.device_number == 0
            || identity.transmission_type == 0
        {
            return Err(Error::InvalidIdentity);
        }
        if self.scanning() {
            return Err(Error::Busy);
        }
        let slot = self
            .index(identity.device_type)
            .or_else(|| {
                self.channels
                    .iter()
                    .position(|channel| channel.selected.is_none())
            })
            .or_else(|| {
                self.channels
                    .iter()
                    .position(|channel| channel.link == LinkState::Disconnected)
            })
            .ok_or(Error::Busy)?;
        self.channels[slot].connect(identity, now)
    }

    pub fn disconnect(&mut self, device_type: u8, now: u64) -> Option<Request> {
        let slot = self.index(device_type)?;
        self.channels[slot].disconnect(now)
    }

    pub fn receive(&mut self, event: Event, now: u64) {
        self.tick(now);
        let device_type = match event {
            Event::Discovery { .. } | Event::ScanEnded => {
                self.discovery.receive(event, now);
                return;
            }
            Event::Connected(identity)
            | Event::Disconnected(identity)
            | Event::Timeout(identity) => identity.device_type,
            Event::Data { device_type, .. } => device_type,
        };
        if let Some(slot) = self.index(device_type) {
            self.channels[slot].receive(event, now);
        }
    }

    pub fn tick(&mut self, now: u64) {
        self.discovery.tick(now);
        for channel in &mut self.channels {
            channel.tick(now);
        }
    }

    pub fn transport_loss(&mut self, now: u64) {
        self.discovery.transport_loss(now);
        for channel in &mut self.channels {
            if channel.selected.is_some() {
                channel.transport_loss(now);
            }
        }
    }

    /// Use when the failed command is unknown or the shared transport failed.
    pub fn tx_failed(&mut self, now: u64) {
        self.discovery.tx_failed(now);
        for channel in &mut self.channels {
            if channel.selected.is_some() {
                channel.tx_failed(now);
            }
        }
    }

    /// A command-specific failure invalidates only the channel it addressed.
    pub fn request_failed(&mut self, request: Request, now: u64) {
        match request {
            Request::Scan { .. } | Request::StopScan => self.discovery.tx_failed(now),
            Request::Connect { identity } | Request::Disconnect { identity } => {
                if let Some(slot) = self.index(identity.device_type)
                    && self.channels[slot].selected == Some(identity)
                {
                    self.channels[slot].tx_failed(now);
                }
            }
        }
    }

    pub fn discoveries(&self) -> &[Option<Discovery>; DISCOVERY_CAPACITY] {
        self.discovery.discoveries()
    }

    /// Global discovery and transport diagnostics; per-peer state is in channel().
    pub fn snapshot(&self, now: u64) -> Snapshot {
        self.discovery.snapshot(now)
    }

    pub fn channel(&self, device_type: u8, now: u64) -> Option<Snapshot> {
        self.index(device_type)
            .map(|slot| self.channels[slot].snapshot(now))
    }

    pub fn snapshots(&self, now: u64) -> [Option<Snapshot>; CHANNEL_CAPACITY] {
        core::array::from_fn(|slot| {
            self.channels[slot]
                .selected
                .map(|_| self.channels[slot].snapshot(now))
        })
    }

    /// Round-robin draining keeps a busy sensor from starving the other queues.
    pub fn pop_packet(&mut self) -> Option<Packet> {
        for offset in 0..CHANNEL_CAPACITY {
            let slot = (self.next_packet + offset) % CHANNEL_CAPACITY;
            if let Some(packet) = self.channels[slot].pop_packet() {
                self.next_packet = (slot + 1) % CHANNEL_CAPACITY;
                return Some(packet);
            }
        }
        None
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

    const HR: Identity = Identity {
        device_type: 120,
        device_number: 456,
        transmission_type: 1,
    };
    const POWER: Identity = Identity {
        device_type: 11,
        device_number: 789,
        transmission_type: 1,
    };

    fn three_channels() -> Channels {
        let mut channels = Channels::new();
        for peer in [PEER, HR, POWER] {
            channels.connect(peer, 0).unwrap();
            channels.receive(Event::Connected(peer), 1);
        }
        channels
    }

    fn page(channels: &mut Channels, peer: Identity, value: u8, now: u64) {
        channels.receive(
            Event::Data {
                device_type: peer.device_type,
                data: [value; 8],
            },
            now,
        );
    }

    #[test]
    fn three_types_route_independently_and_drain_fairly() {
        let mut channels = three_channels();
        for value in 0..6 {
            page(&mut channels, PEER, value, 2);
        }
        page(&mut channels, HR, 90, 3);
        page(&mut channels, POWER, 200, 4);
        page(
            &mut channels,
            Identity {
                device_type: 99,
                ..PEER
            },
            99,
            4,
        );
        assert_eq!(channels.snapshots(4).iter().flatten().count(), 3);
        for (identity, value) in [(PEER, 2), (HR, 90), (POWER, 200), (PEER, 3)] {
            let packet = channels.pop_packet().unwrap();
            assert_eq!(packet.identity, identity);
            assert_eq!(packet.data, [value; 8]);
        }
        assert_eq!(channels.channel(40, 4).unwrap().dropped_packets, 2);
        assert_eq!(channels.channel(120, 4).unwrap().dropped_packets, 0);
        channels.pop_packet();
        assert_eq!(channels.pop_packet().unwrap().loss_count, 2);
        assert!(channels.pop_packet().is_none());
    }

    #[test]
    fn channel_loss_and_reconnect_preserve_other_peers() {
        let mut channels = three_channels();
        for peer in [PEER, HR, POWER] {
            page(&mut channels, peer, 1, 2);
        }
        let radar_generation = channels.channel(40, 2).unwrap().generation;
        let hr_generation = channels.channel(120, 2).unwrap().generation;
        channels.disconnect(40, 3).unwrap();
        channels.receive(Event::Disconnected(PEER), 4);
        channels.connect(PEER, 5).unwrap();
        page(&mut channels, PEER, 9, 6); // no open acknowledgement yet
        channels.receive(Event::Connected(PEER), 7);
        page(&mut channels, PEER, 2, 8);
        assert!(channels.channel(40, 8).unwrap().generation > radar_generation);
        assert_eq!(channels.channel(120, 8).unwrap().generation, hr_generation);
        let radar = channels.pop_packet().unwrap();
        assert_eq!(radar.identity, PEER);
        assert_eq!(radar.data, [2; 8]);
        assert_eq!(channels.pop_packet().unwrap().identity, HR);
        assert_eq!(channels.pop_packet().unwrap().identity, POWER);
        assert!(channels.pop_packet().is_none());
    }

    #[test]
    fn slots_and_same_type_replacement_require_confirmed_cleanup() {
        let mut channels = three_channels();
        let speed = Identity {
            device_type: 123,
            ..PEER
        };
        channels.connect(speed, 1).unwrap();
        channels.receive(Event::Connected(speed), 1);
        let replacement = Identity {
            device_number: 124,
            ..PEER
        };
        let fourth = Identity {
            device_type: 99,
            ..PEER
        };
        assert_eq!(channels.connect(replacement, 2), Err(Error::Busy));
        assert_eq!(channels.connect(fourth, 2), Err(Error::Busy));
        channels.disconnect(40, 3);
        channels.tick(CONNECT_TIMEOUT_MS + 3);
        assert_eq!(
            channels.connect(replacement, CONNECT_TIMEOUT_MS + 4),
            Err(Error::Busy)
        );
        assert_eq!(
            channels.connect(fourth, CONNECT_TIMEOUT_MS + 4),
            Err(Error::Busy)
        );
        channels.receive(Event::Disconnected(replacement), CONNECT_TIMEOUT_MS + 5);
        assert_eq!(
            channels.connect(replacement, CONNECT_TIMEOUT_MS + 6),
            Err(Error::Busy)
        );
        channels.receive(Event::Disconnected(PEER), CONNECT_TIMEOUT_MS + 7);
        channels
            .connect(replacement, CONNECT_TIMEOUT_MS + 8)
            .unwrap();
        channels.receive(Event::Connected(PEER), CONNECT_TIMEOUT_MS + 9);
        assert_eq!(
            channels.channel(40, CONNECT_TIMEOUT_MS + 9).unwrap().link,
            LinkState::Connecting
        );
        channels.disconnect(40, CONNECT_TIMEOUT_MS + 10);
        channels.receive(Event::Disconnected(replacement), CONNECT_TIMEOUT_MS + 11);
        channels.connect(fourth, CONNECT_TIMEOUT_MS + 12).unwrap();
        assert!(channels.channel(40, CONNECT_TIMEOUT_MS + 12).is_none());
        assert!(channels.channel(99, CONNECT_TIMEOUT_MS + 12).is_some());
    }

    #[test]
    fn scan_stop_holds_ownership_until_completion() {
        let mut channels = Channels::new();
        channels.begin_scan(0, 100).unwrap();
        channels.stop_scan(1).unwrap();
        assert!(channels.scanning());
        assert_eq!(channels.begin_scan(2, 100), Err(Error::Busy));
        assert_eq!(channels.connect(HR, 2), Err(Error::Busy));
        channels.receive(Event::ScanEnded, 3);
        channels.begin_scan(4, 100).unwrap();
        channels.receive(
            Event::Discovery {
                identity: HR,
                rssi: -50,
            },
            5,
        );
        assert!(channels.scanning());
        assert_eq!(channels.discoveries().iter().flatten().count(), 1);
    }

    #[test]
    fn scan_stop_timeout_is_bounded_and_duplicate_stops_are_rejected() {
        let mut channels = Channels::new();
        channels.begin_scan(0, 100).unwrap();
        channels.stop_scan(1).unwrap();
        assert_eq!(channels.stop_scan(2), Err(Error::Busy));
        channels.tick(100);
        assert!(channels.scanning()); // Original scan deadline must not release a pending stop.
        channels.receive(
            Event::Discovery {
                identity: HR,
                rssi: -50,
            },
            101,
        );
        assert_eq!(channels.discoveries().iter().flatten().count(), 0);
        channels.tick(SCAN_STOP_TIMEOUT_MS);
        assert!(channels.scanning());
        channels.tick(SCAN_STOP_TIMEOUT_MS + 1);
        assert!(!channels.scanning());
        channels.begin_scan(SCAN_STOP_TIMEOUT_MS + 2, 100).unwrap();
        channels.stop_scan(SCAN_STOP_TIMEOUT_MS + 3).unwrap();
        channels.transport_loss(SCAN_STOP_TIMEOUT_MS + 4);
        assert!(!channels.scanning());
    }

    #[test]
    fn scan_is_global_and_cannot_run_alongside_live_channels() {
        let mut channels = Channels::new();
        channels.begin_scan(0, 100).unwrap();
        for peer in [PEER, HR, POWER] {
            channels.receive(
                Event::Discovery {
                    identity: peer,
                    rssi: -50,
                },
                1,
            );
        }
        assert_eq!(channels.discoveries().iter().flatten().count(), 3);
        assert_eq!(channels.connect(HR, 2), Err(Error::Busy));
        channels.stop_scan(2).unwrap();
        channels.receive(Event::ScanEnded, 3);
        channels.connect(HR, 3).unwrap();
        assert_eq!(channels.begin_scan(4, 100), Err(Error::Busy));
        channels.receive(Event::Connected(HR), 5);
        assert_eq!(channels.begin_scan(6, 100), Err(Error::Busy));
        channels.disconnect(120, 7);
        assert_eq!(channels.begin_scan(8, 100), Err(Error::Busy));
        channels.receive(Event::Disconnected(HR), 9);
        channels.begin_scan(10, 100).unwrap();
        channels.tick(110);
        assert!(!channels.scanning());
    }

    #[test]
    fn request_failure_is_local_but_transport_loss_invalidates_all() {
        let mut channels = three_channels();
        for peer in [PEER, HR, POWER] {
            page(&mut channels, peer, 1, 2);
        }
        channels.request_failed(Request::Disconnect { identity: HR }, 3);
        assert_eq!(
            channels.channel(120, 3).unwrap().link,
            LinkState::TransportLost
        );
        assert_eq!(channels.channel(40, 3).unwrap().link, LinkState::Connected);
        assert_eq!(channels.channel(11, 3).unwrap().link, LinkState::Connected);
        assert_eq!(channels.pop_packet().unwrap().identity, PEER);
        assert_eq!(channels.pop_packet().unwrap().identity, POWER);
        assert!(channels.pop_packet().is_none());
        page(&mut channels, PEER, 2, 4);
        channels.transport_loss(5);
        for peer in [PEER, HR, POWER] {
            channels.receive(Event::Connected(peer), 6);
            page(&mut channels, peer, 3, 7);
            assert_eq!(
                channels.channel(peer.device_type, 7).unwrap().link,
                LinkState::TransportLost
            );
        }
        assert!(channels.pop_packet().is_none());
        channels.connect(PEER, 8).unwrap();
        channels.receive(Event::Connected(PEER), 9);
        page(&mut channels, PEER, 4, 10);
        assert_eq!(channels.pop_packet().unwrap().loss_count, 1);
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
