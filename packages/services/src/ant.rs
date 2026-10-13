use device_api::ant::*;
use heapless::Deque;

/// Zero is a wildcard on the wire; a selected channel must be explicit.
fn explicit(identity: Identity) -> bool {
    identity.device_type != 0 && identity.device_number != 0 && identity.transmission_type != 0
}

/// Retain the most recent pages; consumers detect gaps from loss_count.
/// Returns whether the oldest page was displaced.
fn push_latest(queue: &mut Deque<Packet, PACKET_CAPACITY>, packet: Packet) -> bool {
    let displaced = queue.is_full() && queue.pop_front().is_some();
    // Cannot fail: a full queue just released a slot.
    let _ = queue.push_back(packet);
    displaced
}

/// One receive channel for a single selected peer. Data has no sender
/// identifier, so pages are admitted only while the matching link is open.
pub struct Channel {
    selected: Option<Identity>,
    link: LinkState,
    connect_until: u64,
    generation: u32,
    queue: Deque<Packet, PACKET_CAPACITY>,
    diagnostics: Deque<Packet, PACKET_CAPACITY>,
    diagnostic_losses: u32,
    last_packet: Option<u64>,
    packets: u32,
    dropped_packets: u32,
    transport_losses: u32,
    tx_failures: u32,
}

impl Default for Channel {
    fn default() -> Self {
        Self::new()
    }
}

impl Channel {
    pub const fn new() -> Self {
        Self {
            selected: None,
            link: LinkState::Idle,
            connect_until: 0,
            generation: 0,
            queue: Deque::new(),
            diagnostics: Deque::new(),
            diagnostic_losses: 0,
            last_packet: None,
            packets: 0,
            dropped_packets: 0,
            transport_losses: 0,
            tx_failures: 0,
        }
    }

    fn busy(&self) -> bool {
        matches!(
            self.link,
            LinkState::Connecting | LinkState::Connected | LinkState::Disconnecting
        )
    }

    pub fn connect(&mut self, identity: Identity, now: u64) -> Result<Request, Error> {
        if !explicit(identity) {
            return Err(Error::InvalidIdentity);
        }
        if self.busy() {
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
        self.queue.clear();
        self.diagnostics.clear();
        self.last_packet = None;
    }

    pub fn tick(&mut self, now: u64) {
        if matches!(self.link, LinkState::Connecting | LinkState::Disconnecting)
            && now >= self.connect_until
        {
            self.invalidate();
            self.link = LinkState::TimedOut;
        }
    }

    /// Receive a normalized radio observation, never raw UART bytes.
    /// Discovery events belong to the scanner and are ignored here.
    pub fn receive(&mut self, event: Event, now: u64) {
        self.tick(now);
        match event {
            Event::Discovery { .. } | Event::ScanEnded => {}
            Event::Connected(identity) => {
                if self.selected == Some(identity) && self.link == LinkState::Connecting {
                    self.link = LinkState::Connected;
                }
            }
            Event::Disconnected(identity) | Event::Timeout(identity) => {
                let closed = matches!(event, Event::Disconnected(_));
                // A late close still confirms cleanup after loss; a late timeout does not.
                let pending = self.busy()
                    || closed
                        && matches!(self.link, LinkState::TimedOut | LinkState::TransportLost);
                if self.selected == Some(identity) && pending {
                    self.invalidate();
                    self.link = if closed {
                        LinkState::Disconnected
                    } else {
                        LinkState::TimedOut
                    };
                }
            }
            Event::Data { device_type, data } => self.data(device_type, data, now),
        }
    }

    fn data(&mut self, device_type: u8, data: [u8; 8], now: u64) {
        let Some(identity) = self.selected else {
            return;
        };
        if self.link != LinkState::Connected || device_type != identity.device_type {
            return;
        }
        self.packets = self.packets.saturating_add(1);
        self.last_packet = Some(now);
        let packet = Packet {
            identity,
            data,
            received_ms: now,
            generation: self.generation,
            // Recomputed when popped from either stream.
            loss_count: 0,
        };
        if push_latest(&mut self.queue, packet) {
            self.dropped_packets = self.dropped_packets.saturating_add(1);
        }
        if push_latest(&mut self.diagnostics, packet) {
            self.diagnostic_losses = self.diagnostic_losses.saturating_add(1);
        }
    }

    pub fn transport_loss(&mut self, _now: u64) {
        self.transport_losses = self.transport_losses.saturating_add(1);
        self.invalidate();
        self.link = LinkState::TransportLost;
    }

    /// Delivery is uncertain. Do not replay a command or admit further pages.
    pub fn tx_failed(&mut self, now: u64) {
        self.tx_failures = self.tx_failures.saturating_add(1);
        self.transport_loss(now);
    }

    pub fn pop_packet(&mut self) -> Option<Packet> {
        let mut packet = self.queue.pop_front()?;
        packet.loss_count = self.dropped_packets.saturating_add(self.transport_losses);
        Some(packet)
    }

    pub fn pop_diagnostic_packet(&mut self) -> Option<Packet> {
        let mut packet = self.diagnostics.pop_front()?;
        packet.loss_count = self.diagnostic_losses.saturating_add(self.transport_losses);
        Some(packet)
    }

    pub fn snapshot(&self, now: u64) -> Snapshot {
        let age_ms = self.last_packet.map(|time| now.saturating_sub(time));
        Snapshot {
            scanning: false,
            link: self.link,
            selected: self.selected,
            generation: self.generation,
            packets: self.packets,
            dropped_packets: self.dropped_packets,
            discovery_overflows: 0,
            transport_losses: self.transport_losses,
            tx_failures: self.tx_failures,
            age_ms,
            stale: self.link != LinkState::Connected || age_ms.is_none_or(|age| age >= STALE_MS),
        }
    }
}

/// The local discovery window and its bounded, deduplicated results.
struct Scanner {
    until: Option<u64>,
    stopping: bool,
    discoveries: [Option<Discovery>; DISCOVERY_CAPACITY],
    overflows: u32,
}

impl Scanner {
    const fn new() -> Self {
        Self {
            until: None,
            stopping: false,
            discoveries: [None; DISCOVERY_CAPACITY],
            overflows: 0,
        }
    }

    fn active(&self) -> bool {
        self.until.is_some()
    }

    fn begin(&mut self, now: u64, duration_ms: u32) -> Result<Request, Error> {
        if duration_ms == 0 {
            return Err(Error::InvalidDuration);
        }
        if self.active() {
            return Err(Error::Busy);
        }
        self.discoveries.fill(None);
        self.until = Some(now.saturating_add(u64::from(duration_ms)));
        Ok(Request::Scan { duration_ms })
    }

    /// Keep discovery ownership while the untagged stop acknowledgment is pending.
    fn stop(&mut self, now: u64) -> Result<Request, Error> {
        if !self.active() || self.stopping {
            return Err(Error::Busy);
        }
        self.stopping = true;
        self.until = Some(now.saturating_add(SCAN_STOP_TIMEOUT_MS));
        Ok(Request::StopScan)
    }

    fn tick(&mut self, now: u64) {
        if self.until.is_some_and(|deadline| now >= deadline) {
            self.end();
        }
    }

    /// Close the local window; physical radio cleanup remains the caller's concern.
    fn end(&mut self) {
        self.until = None;
        self.stopping = false;
    }

    fn discover(&mut self, identity: Identity, rssi: i8, now: u64) {
        if !self.active() || self.stopping {
            return;
        }
        let slot = self
            .discoveries
            .iter()
            .position(|entry| entry.is_some_and(|d| d.identity == identity))
            .or_else(|| self.discoveries.iter().position(Option::is_none));
        match slot {
            Some(slot) => {
                self.discoveries[slot] = Some(Discovery {
                    identity,
                    rssi,
                    seen_ms: now,
                });
            }
            None => self.overflows = self.overflows.saturating_add(1),
        }
    }
}

#[derive(Clone, Copy)]
struct PendingSend {
    observation: SendObservation,
    data: [u8; 8],
    deadline: u64,
}

impl PendingSend {
    fn outstanding(&self) -> bool {
        matches!(
            self.observation.stage,
            SendStage::Queued | SendStage::UartSubmitted
        )
    }
}

/// The adapter identifies received pages by type, so only one peer of each type
/// can be selected. Slots with uncertain radio ownership are never reassigned.
pub struct Channels {
    scanner: Scanner,
    channels: [Channel; CHANNEL_CAPACITY],
    next_packet: usize,
    next_diagnostic: usize,
    capacity: usize,
    scan_state: ScanState,
    scan_generation: u32,
    send: Option<PendingSend>,
    next_send_id: u32,
}

impl Default for Channels {
    fn default() -> Self {
        Self::new()
    }
}

impl Channels {
    pub const fn new() -> Self {
        Self {
            scanner: Scanner::new(),
            channels: [const { Channel::new() }; CHANNEL_CAPACITY],
            next_packet: 0,
            next_diagnostic: 0,
            capacity: CHANNEL_CAPACITY,
            scan_state: ScanState::Idle,
            scan_generation: 0,
            send: None,
            next_send_id: 0,
        }
    }

    pub fn with_capacity(capacity: usize) -> Self {
        let mut channels = Self::new();
        channels.capacity = capacity.min(CHANNEL_CAPACITY);
        channels
    }

    pub fn begin_scan(&mut self, now: u64, duration_ms: u32) -> Result<Request, Error> {
        if self.channels.iter().any(|channel| {
            matches!(
                channel.link,
                LinkState::Connecting | LinkState::Disconnecting
            )
        }) {
            return Err(Error::Busy);
        }
        if self.scan_state == ScanState::Uncertain {
            return Err(Error::Uncertain);
        }
        let request = self.scanner.begin(now, duration_ms)?;
        self.scan_state = ScanState::Starting;
        self.scan_generation = self.scan_generation.wrapping_add(1);
        Ok(request)
    }

    pub fn stop_scan(&mut self, now: u64) -> Result<Request, Error> {
        let request = self.scanner.stop(now)?;
        self.scan_state = ScanState::Stopping;
        Ok(request)
    }

    pub fn scanning(&self) -> bool {
        self.scanner.active()
    }

    pub fn scan(&self) -> ScanSnapshot {
        ScanSnapshot {
            state: self.scan_state,
            generation: self.scan_generation,
            discovery_overflows: self.scanner.overflows,
        }
    }

    pub fn discoveries(&self) -> &[Option<Discovery>; DISCOVERY_CAPACITY] {
        &self.scanner.discoveries
    }

    fn index(&self, device_type: u8) -> Option<usize> {
        self.channels.iter().position(|channel| {
            channel
                .selected
                .is_some_and(|identity| identity.device_type == device_type)
        })
    }

    pub fn connect(&mut self, identity: Identity, now: u64) -> Result<Request, Error> {
        if !explicit(identity) {
            return Err(Error::InvalidIdentity);
        }
        if self.scanning() || self.scan_state == ScanState::Uncertain {
            return Err(Error::Busy);
        }
        let usable = &self.channels[..self.capacity];
        let slot = self
            .index(identity.device_type)
            .or_else(|| usable.iter().position(|channel| channel.selected.is_none()))
            .or_else(|| {
                usable
                    .iter()
                    .position(|channel| channel.link == LinkState::Disconnected)
            })
            .ok_or(Error::Capacity)?;
        self.channels[slot].connect(identity, now)
    }

    pub fn disconnect(&mut self, device_type: u8, now: u64) -> Option<Request> {
        if self.scanning() || self.scan_state == ScanState::Uncertain {
            return None;
        }
        self.cancel_send(Some(device_type));
        let slot = self.index(device_type)?;
        self.channels[slot].disconnect(now)
    }

    pub fn receive(&mut self, event: Event, now: u64) {
        self.tick(now);
        let device_type = match event {
            // Untagged scan-end reports never complete or clear a window.
            Event::ScanEnded => return,
            Event::Discovery { identity, rssi } => {
                if self.scan_state == ScanState::Starting {
                    self.scan_state = ScanState::Active;
                }
                self.scanner.discover(identity, rssi, now);
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
        self.tick(now);
    }

    pub fn tick(&mut self, now: u64) {
        let scanning = self.scanning();
        self.scanner.tick(now);
        if scanning && !self.scanning() {
            self.scan_state = ScanState::Idle;
        }
        if let Some(send) = &mut self.send
            && send.observation.stage == SendStage::UartSubmitted
            && now >= send.deadline
        {
            send.observation.stage = SendStage::Uncertain;
        }
        for channel in &mut self.channels {
            channel.tick(now);
        }
        if let Some(send) = self.send {
            let target = send.observation.target;
            if self
                .channel(target.identity.device_type, now)
                .is_none_or(|channel| {
                    channel.link != LinkState::Connected
                        || channel.generation != target.generation
                        || channel.selected != Some(target.identity)
                })
            {
                self.cancel_send(Some(target.identity.device_type));
            }
        }
    }

    /// Invalidate everything that shares the failed transport.
    fn lose_transport(&mut self, now: u64, lose: fn(&mut Channel, u64)) {
        self.cancel_send(None);
        if self.scanning() {
            self.scan_state = ScanState::Uncertain;
        }
        self.scanner.end();
        for channel in &mut self.channels {
            if channel.selected.is_some() {
                lose(channel, now);
            }
        }
    }

    pub fn transport_loss(&mut self, now: u64) {
        self.lose_transport(now, Channel::transport_loss);
    }

    /// Use when the failed command is unknown or the shared transport failed.
    pub fn tx_failed(&mut self, now: u64) {
        self.lose_transport(now, Channel::tx_failed);
    }

    /// A command-specific failure invalidates only the channel it addressed.
    pub fn request_failed(&mut self, request: Request, now: u64) {
        match request {
            Request::Scan { .. } | Request::StopScan => {
                self.scan_state = ScanState::Uncertain;
                self.scanner.end();
            }
            Request::Connect { identity } | Request::Disconnect { identity } => {
                self.cancel_send(Some(identity.device_type));
                if let Some(slot) = self.index(identity.device_type)
                    && self.channels[slot].selected == Some(identity)
                {
                    self.channels[slot].tx_failed(now);
                }
            }
        }
    }

    /// Only call after a confirmed companion restart. A framing reset,
    /// reconnect or wake does not establish physical discovery cleanup.
    pub fn companion_restarted(&mut self, now: u64) {
        self.transport_loss(now);
        self.scan_state = ScanState::Idle;
    }

    pub fn channel(&self, device_type: u8, now: u64) -> Option<Snapshot> {
        self.index(device_type)
            .map(|slot| self.channels[slot].snapshot(now))
    }

    pub fn snapshots(&self, now: u64) -> [Option<Snapshot>; CHANNEL_CAPACITY] {
        self.channels
            .each_ref()
            .map(|channel| channel.selected.map(|_| channel.snapshot(now)))
    }

    /// Round-robin draining keeps a busy sensor from starving the other queues.
    fn pop_fair(
        channels: &mut [Channel; CHANNEL_CAPACITY],
        next: &mut usize,
        pop: fn(&mut Channel) -> Option<Packet>,
    ) -> Option<Packet> {
        for offset in 0..CHANNEL_CAPACITY {
            let slot = (*next + offset) % CHANNEL_CAPACITY;
            if let Some(packet) = pop(&mut channels[slot]) {
                *next = (slot + 1) % CHANNEL_CAPACITY;
                return Some(packet);
            }
        }
        None
    }

    pub fn pop_packet(&mut self) -> Option<Packet> {
        Self::pop_fair(
            &mut self.channels,
            &mut self.next_packet,
            Channel::pop_packet,
        )
    }

    pub fn pop_diagnostic_packet(&mut self) -> Option<Packet> {
        Self::pop_fair(
            &mut self.channels,
            &mut self.next_diagnostic,
            Channel::pop_diagnostic_packet,
        )
    }

    pub fn queue_send(
        &mut self,
        target: Target,
        data: [u8; 8],
        now: u64,
    ) -> Result<OperationId, Error> {
        self.tick(now);
        let channel = self
            .channel(target.identity.device_type, now)
            .ok_or(Error::Disconnected)?;
        if channel.selected != Some(target.identity) || channel.generation != target.generation {
            return Err(Error::StaleGeneration);
        }
        if channel.link != LinkState::Connected {
            return Err(Error::Disconnected);
        }
        if self.send_pending() {
            return Err(Error::Busy);
        }
        self.next_send_id = self.next_send_id.checked_add(1).ok_or(Error::Capacity)?;
        let id = OperationId(self.next_send_id);
        self.send = Some(PendingSend {
            observation: SendObservation {
                id,
                target,
                stage: SendStage::Queued,
            },
            data,
            deadline: 0,
        });
        Ok(id)
    }

    pub fn send_pending(&self) -> bool {
        self.send.is_some_and(|send| send.outstanding())
    }

    pub fn pending_send(&self) -> Option<(OperationId, Target, [u8; 8])> {
        let send = self.send?;
        (send.observation.stage == SendStage::Queued).then_some((
            send.observation.id,
            send.observation.target,
            send.data,
        ))
    }

    pub fn send_submitted(&mut self, id: OperationId, now: u64) {
        if let Some(send) = &mut self.send
            && send.observation.id == id
            && send.observation.stage == SendStage::Queued
        {
            send.observation.stage = SendStage::UartSubmitted;
            send.deadline = now.saturating_add(SEND_TIMEOUT_MS);
        }
    }

    pub fn send_failed(&mut self, id: OperationId) {
        if let Some(send) = &mut self.send
            && send.observation.id == id
        {
            send.observation.stage = SendStage::Uncertain;
        }
    }

    /// The device adapter correlates its wire reply before identifying a local operation.
    pub fn send_reply(&mut self, id: OperationId, accepted: bool, now: u64) {
        self.tick(now);
        if let Some(send) = &mut self.send
            && send.observation.stage == SendStage::UartSubmitted
            && send.observation.id == id
        {
            send.observation.stage = SendStage::BridgeReplied { accepted };
        }
    }

    pub fn send_status(&self, id: OperationId) -> Option<SendObservation> {
        self.send
            .map(|send| send.observation)
            .filter(|observation| observation.id == id)
    }

    pub fn cancel_send(&mut self, device_type: Option<u8>) {
        if let Some(send) = &mut self.send
            && device_type.is_none_or(|kind| kind == send.observation.target.identity.device_type)
        {
            send.observation.stage = match send.observation.stage {
                SendStage::Queued => SendStage::Cancelled,
                SendStage::UartSubmitted => SendStage::Uncertain,
                stage => stage,
            };
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
    fn connected(state: &mut Channel) {
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
        channels.capacity = 4;
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
        assert_eq!(channels.connect(fourth, 2), Err(Error::Capacity));
        channels.disconnect(40, 3);
        channels.tick(CONNECT_TIMEOUT_MS + 3);
        assert_eq!(
            channels.connect(replacement, CONNECT_TIMEOUT_MS + 4),
            Err(Error::Busy)
        );
        assert_eq!(
            channels.connect(fourth, CONNECT_TIMEOUT_MS + 4),
            Err(Error::Capacity)
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
    fn discovery_and_diagnostics_preserve_production_reception() {
        let mut channels = three_channels();
        channels.begin_scan(2, 100).unwrap();
        assert_eq!(channels.scan().state, ScanState::Starting);
        channels.receive(
            Event::Discovery {
                identity: HR,
                rssi: -50,
            },
            3,
        );
        assert_eq!(channels.scan().state, ScanState::Active);
        page(&mut channels, PEER, 9, 4);
        let diagnostic = channels.pop_diagnostic_packet().unwrap();
        assert_eq!(channels.pop_packet(), Some(diagnostic));
        assert_eq!(channels.disconnect(40, 5), None);
        channels.stop_scan(6).unwrap();
        channels.receive(Event::ScanEnded, 7);
        assert_eq!(channels.channel(40, 7).unwrap().link, LinkState::Connected);
        assert_eq!(channels.scan().state, ScanState::Stopping);
        channels.tick(SCAN_STOP_TIMEOUT_MS + 6);
        channels.begin_scan(SCAN_STOP_TIMEOUT_MS + 7, 100).unwrap();
        channels.receive(Event::ScanEnded, SCAN_STOP_TIMEOUT_MS + 8);
        assert!(channels.scanning());
    }

    #[test]
    fn scan_failures_leave_discovery_uncertain_until_companion_restart() {
        let mut channels = three_channels();
        channels.begin_scan(2, 1_000).unwrap();
        channels.receive(
            Event::Discovery {
                identity: HR,
                rssi: -50,
            },
            3,
        );
        channels.request_failed(Request::StopScan, 4);
        assert!(!channels.scanning());
        assert_eq!(channels.scan().state, ScanState::Uncertain);
        assert_eq!(channels.begin_scan(5, 100), Err(Error::Uncertain));
        assert_eq!(channels.connect(HR, 5), Err(Error::Busy));
        assert_eq!(channels.channel(40, 5).unwrap().link, LinkState::Connected);
        // Discoveries stay visible, and late reports cannot reopen the window.
        assert_eq!(channels.discoveries()[0].unwrap().identity, HR);
        channels.receive(
            Event::Discovery {
                identity: POWER,
                rssi: -40,
            },
            6,
        );
        assert!(channels.discoveries()[1].is_none());
        channels.companion_restarted(7);
        assert_eq!(channels.scan().state, ScanState::Idle);
        for snapshot in channels.snapshots(7).into_iter().flatten() {
            assert!(!snapshot.scanning);
            assert_eq!(snapshot.link, LinkState::TransportLost);
        }
        channels.begin_scan(8, 100).unwrap();
        assert!(channels.scanning());
    }

    #[test]
    fn ten_receivers_drain_fairly_with_independent_diagnostic_overflow() {
        let mut channels = Channels::new();
        let types = [40, 120, 11, 122, 123, 121, 34, 17, 128, 35];
        for device_type in types {
            let identity = Identity {
                device_type,
                ..PEER
            };
            channels.connect(identity, 0).unwrap();
            channels.receive(Event::Connected(identity), 1);
            page(&mut channels, identity, device_type, 2);
        }
        assert_eq!(
            channels.connect(
                Identity {
                    device_type: 99,
                    ..PEER
                },
                3
            ),
            Err(Error::Capacity)
        );
        for device_type in types {
            assert_eq!(
                channels.pop_packet().unwrap().identity.device_type,
                device_type
            );
        }
        // A console that falls behind cannot remove production pages.
        for value in 0..6 {
            page(&mut channels, PEER, value, 4);
            assert_eq!(channels.pop_packet().unwrap().data, [value; 8]);
        }
        let diagnostic = channels.pop_diagnostic_packet().unwrap();
        assert_eq!(diagnostic.data, [2; 8]);
        assert!(diagnostic.loss_count > 0);
        channels.disconnect(40, 5).unwrap();
        while let Some(packet) = channels.pop_diagnostic_packet() {
            assert_ne!(packet.identity.device_type, 40);
        }
    }

    #[test]
    fn sends_report_bridge_evidence_with_bounded_work_and_local_ids() {
        let mut channels = three_channels();
        let target = Target {
            identity: PEER,
            generation: channels.channel(40, 2).unwrap().generation,
        };
        let id = channels.queue_send(target, [1; 8], 2).unwrap();
        assert_eq!(channels.pending_send(), Some((id, target, [1; 8])));
        assert_eq!(channels.queue_send(target, [2; 8], 2), Err(Error::Busy));
        channels.send_submitted(id, 3);
        channels.send_reply(id, true, 4);
        assert_eq!(
            channels.send_status(id).unwrap().stage,
            SendStage::BridgeReplied { accepted: true }
        );
        let next = channels.queue_send(target, [2; 8], 6).unwrap();
        channels.send_submitted(next, 7);
        channels.send_reply(id, true, 8);
        assert_eq!(
            channels.send_status(next).unwrap().stage,
            SendStage::UartSubmitted
        );
        channels.tick(SEND_TIMEOUT_MS + 7);
        assert_eq!(
            channels.send_status(next).unwrap().stage,
            SendStage::Uncertain
        );
        assert_eq!(
            channels.channel(40, SEND_TIMEOUT_MS + 7).unwrap().link,
            LinkState::Connected
        );
        channels.disconnect(40, SEND_TIMEOUT_MS + 8).unwrap();
        assert_eq!(
            channels.queue_send(target, [3; 8], SEND_TIMEOUT_MS + 9),
            Err(Error::StaleGeneration)
        );
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
        let mut state = Channel::new();
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
        let mut state = Channels::new();
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
        assert_eq!(state.scan().discovery_overflows, 1);
        assert_eq!(state.connect(PEER, 2), Err(Error::Busy));
        // Untagged scan-end reports never close the window; its deadline does.
        state.receive(Event::ScanEnded, 3);
        assert!(state.scanning());
        state.tick(100);
        assert!(!state.scanning());
        assert_eq!(state.scan().state, ScanState::Idle);
        state.begin_scan(101, 10).unwrap();
        assert!(state.discoveries().iter().all(Option::is_none));
        state.tick(111);
        assert!(!state.scanning());
    }

    #[test]
    fn packet_overflow_exposes_loss_and_retains_latest() {
        let mut state = Channel::new();
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
        let mut state = Channel::new();
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
        let mut state = Channel::new();
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
        let mut state = Channel::new();
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
        let mut state = Channel::new();
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
        let mut state = Channel::new();
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
