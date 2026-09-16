//! ANT state shared with the console; the existing IO task owns all UART work.
use core::cell::RefCell;
use device_api::ant::Discovery;
use device_api::ant::Identity;
use device_api::ant::LinkState;
use device_api::ant::Packet;
use device_api::ant::Request;
use device_api::ant::Snapshot;
use device_api::ant::{
    Admission, Capabilities, Error as AntError, OperationId, ScanSnapshot, SendObservation,
};
use device_api::observation::Error;
use device_api::power::PeripheralState;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use firmware_services::ant::Channels;

const SEND_HISTORY_CAPACITY: usize = 8;

struct Shared {
    startup_activity: bool,
    state: Channels,
    pending: Option<Request>,
    send_keys: [Option<(u8, [u8; 2])>; SEND_HISTORY_CAPACITY],
    wire_send: Option<(OperationId, u8, [u8; 2])>,
    scan: ScanOwnership,
    power: PeripheralState,
    suspending: bool,
    power_active: bool,
    restore: [Option<Identity>; device_api::ant::CHANNEL_CAPACITY],
    reconnect_sent: [bool; device_api::ant::CHANNEL_CAPACITY],
    power_deadline: u64,
}
static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared {
    startup_activity: false,
    state: Channels::new(),
    pending: None,
    send_keys: [None; SEND_HISTORY_CAPACITY],
    wire_send: None,
    scan: ScanOwnership::Idle,
    power: PeripheralState::Running,
    suspending: false,
    power_active: false,
    restore: [None; device_api::ant::CHANNEL_CAPACITY],
    reconnect_sent: [false; device_api::ant::CHANNEL_CAPACITY],
    power_deadline: 0,
}));

/// Local scan deadlines release the discovery UI, not ownership of the radio.
#[derive(Clone, Copy)]
enum ScanOwnership {
    Idle,
    Active(u64),
    Stopping(u64, u8),
    Uncertain,
}
impl ScanOwnership {
    fn tick(&mut self, now: u64) {
        if matches!(*self, Self::Active(deadline) | Self::Stopping(deadline, _) if now >= deadline)
        {
            *self = Self::Uncertain;
        }
    }
    fn ended(&mut self, now: u64) {
        match *self {
            ScanOwnership::Active(deadline)
                if now >= deadline.saturating_sub(device_api::ant::SCAN_STOP_TIMEOUT_MS)
                    && now < deadline =>
            {
                *self = ScanOwnership::Idle
            }
            ScanOwnership::Stopping(deadline, remaining) if now < deadline => {
                // Explicit stop does not cancel the companion's timer.
                // Both replies must drain before a later scan can own it.
                if remaining > 1 {
                    *self = ScanOwnership::Stopping(deadline, remaining - 1);
                } else if now >= deadline.saturating_sub(device_api::ant::SCAN_STOP_TIMEOUT_MS) {
                    *self = ScanOwnership::Idle;
                }
            }
            _ => {}
        }
    }
    fn idle(self) -> bool {
        matches!(self, Self::Idle)
    }
}

pub fn power_status() -> PeripheralState {
    SHARED.lock(|s| s.borrow().power)
}

pub fn power_request(suspend: bool, now: u64) -> Result<(), Error> {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        if suspend {
            if s.power != PeripheralState::Running || s.pending.is_some() {
                return Err(Error::Unavailable);
            }
            s.restore = s.state.snapshots(now).map(|snapshot| {
                snapshot
                    .filter(|snapshot| {
                        matches!(snapshot.link, LinkState::Connected | LinkState::Connecting)
                    })
                    .and_then(|snapshot| snapshot.selected)
            });
            s.reconnect_sent.fill(false);
        } else if s.power == PeripheralState::Running {
            return Ok(());
        } else if s.power_active && !s.suspending {
            return Err(Error::Unavailable);
        }
        s.state.cancel_send(None);
        s.power_active = true;
        s.suspending = suspend;
        s.power = PeripheralState::Pending;
        s.power_deadline = now.saturating_add(device_api::ant::CONNECT_TIMEOUT_MS + 2_000);
        s.progress_power(now);
        Ok(())
    })
}

impl Shared {
    fn release_settled_send(&mut self) {
        if let Some((id, kind, echoed)) = self.wire_send
            && self.state.send_status(id).is_some_and(|status| {
                matches!(
                    status.stage,
                    device_api::ant::SendStage::BridgeReplied { .. }
                        | device_api::ant::SendStage::Cancelled
                )
            })
        {
            // The examined bridge emits exactly one class-five reply per send.
            // A consumed reply settles that key; uncertain submissions retain it.
            if let Some(slot) = self
                .send_keys
                .iter_mut()
                .find(|key| **key == Some((kind, echoed)))
            {
                *slot = None;
            }
            self.wire_send = None;
        }
    }

    fn progress_power(&mut self, now: u64) {
        self.scan.tick(now);
        if !self.power_active || self.pending.is_some() {
            return;
        }
        let mut waiting = false;
        let mut failed = false;
        if self.suspending {
            match self.scan {
                ScanOwnership::Active(_) if !self.state.scanning() => waiting = true,
                ScanOwnership::Active(_) => match self.state.stop_scan(now) {
                    Ok(request) => {
                        let ScanOwnership::Active(deadline) = self.scan else {
                            unreachable!()
                        };
                        self.scan = ScanOwnership::Stopping(deadline, 2);
                        self.pending = Some(request);
                        return;
                    }
                    Err(_) => failed = true,
                },
                ScanOwnership::Stopping(..) => waiting = true,
                ScanOwnership::Uncertain => failed = true,
                ScanOwnership::Idle => {}
            }
            for snapshot in self.state.snapshots(now).into_iter().flatten() {
                match snapshot.link {
                    LinkState::Connecting | LinkState::Connected => {
                        if !failed {
                            self.pending = self
                                .state
                                .disconnect(snapshot.selected.unwrap().device_type, now);
                            if self.pending.is_some() {
                                return;
                            }
                            waiting = true;
                        }
                    }
                    LinkState::Disconnecting => waiting = true,
                    LinkState::TimedOut | LinkState::TransportLost => failed = true,
                    LinkState::Idle | LinkState::Disconnected => {}
                }
            }
        } else {
            // A submitted scan stop must settle even when preparation is aborted.
            waiting |= matches!(self.scan, ScanOwnership::Stopping(..));
            failed |= matches!(self.scan, ScanOwnership::Uncertain);
            for index in 0..self.restore.len() {
                let Some(identity) = self.restore[index] else {
                    continue;
                };
                let Some(snapshot) = self.state.channel(identity.device_type, now) else {
                    failed = true;
                    continue;
                };
                match snapshot.link {
                    LinkState::Connected if snapshot.selected == Some(identity) => {}
                    LinkState::Connecting | LinkState::Disconnecting => waiting = true,
                    LinkState::Disconnected if !self.reconnect_sent[index] && self.scan.idle() => {
                        self.reconnect_sent[index] = true;
                        match self.state.connect(identity, now) {
                            Ok(request) => {
                                self.pending = Some(request);
                                return;
                            }
                            Err(_) => failed = true,
                        }
                    }
                    _ => failed = true,
                }
            }
        }
        if failed || now >= self.power_deadline {
            self.power = PeripheralState::Failed;
            self.power_active = false;
        } else if waiting {
            self.power = PeripheralState::Pending;
        } else {
            self.power = if self.suspending {
                PeripheralState::Quiescent
            } else {
                PeripheralState::Running
            };
            self.power_active = false;
        }
    }
}

pub fn startup_activity() -> bool {
    SHARED.lock(|s| s.borrow().startup_activity)
}
pub fn snapshots(now: u64) -> [Option<Snapshot>; device_api::ant::CHANNEL_CAPACITY] {
    SHARED.lock(|s| s.borrow().state.snapshots(now))
}
pub fn scanning() -> bool {
    SHARED.lock(|s| s.borrow().state.scanning())
}
pub fn discoveries() -> [Option<Discovery>; 8] {
    SHARED.lock(|s| *s.borrow().state.discoveries())
}
pub fn take_packet() -> Option<Packet> {
    SHARED.lock(|s| s.borrow_mut().state.pop_packet())
}

pub use device_api::ant::AntOperation as Operation;
pub fn request(operation: Operation, now: u64) -> Result<Admission, AntError> {
    if matches!(
        operation,
        Operation::Scan(_) | Operation::Connect(_) | Operation::Send { .. }
    ) && !super::sensors::ant_allowed()
    {
        return Err(AntError::Busy);
    }
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.scan.tick(now);
        s.release_settled_send();
        if s.power != PeripheralState::Running
            || s.pending.is_some()
            || s.state.send_pending()
            || matches!(
                operation,
                Operation::Scan(_) | Operation::Connect(_) | Operation::Send { .. }
            ) && !s.scan.idle()
        {
            return Err(AntError::Busy);
        }
        let result = match operation {
            Operation::Scan(ms) => {
                if ms == 0 || ms > 60_000 {
                    return Err(AntError::InvalidDuration);
                }
                s.state.begin_scan(now, ms)
            }
            Operation::StopScan => s.state.stop_scan(now),
            Operation::Connect(peer) => {
                if !crate::drivers::ant_protocol::supports_type(peer.device_type) {
                    return Err(AntError::UnsupportedType);
                }
                s.state.connect(peer, now)
            }
            Operation::Send { target, data } => {
                if !crate::drivers::ant_protocol::supports_type(target.identity.device_type) {
                    return Err(AntError::UnsupportedType);
                }
                let key = (target.identity.device_type, [data[0], data[1]]);
                if s.send_keys.contains(&Some(key)) {
                    return Err(AntError::Uncertain);
                }
                let slot = s
                    .send_keys
                    .iter()
                    .position(Option::is_none)
                    .ok_or(AntError::Capacity)?;
                let id = s.state.queue_send(target, data, now)?;
                s.send_keys[slot] = Some(key);
                s.wire_send = Some((id, key.0, key.1));
                return Ok(Admission::SendQueued(id));
            }
            Operation::Disconnect(kind) => match s.state.disconnect(kind, now) {
                Some(request) => Ok(request),
                None => return Err(AntError::Disconnected),
            },
        };
        match result {
            Ok(request) => {
                match request {
                    Request::Scan { duration_ms } => {
                        // Allow the bounded report transit interval after the local window.
                        s.scan = ScanOwnership::Active(
                            now.saturating_add(
                                u64::from(duration_ms.div_ceil(1000).max(5) + 1) * 1000,
                            )
                            .saturating_add(device_api::ant::SCAN_STOP_TIMEOUT_MS),
                        )
                    }
                    Request::StopScan => {
                        let ScanOwnership::Active(deadline) = s.scan else {
                            return Err(AntError::Busy);
                        };
                        s.scan = ScanOwnership::Stopping(deadline, 2)
                    }
                    _ => {}
                }
                s.pending = Some(request);
                Ok(Admission::Accepted)
            }
            Err(AntError::Busy) => Err(AntError::InvalidState),
            Err(error) => Err(error),
        }
    })
}
pub fn receive(group: u8, payload: [u8; 8], now: u64) {
    if let Some(event) = crate::drivers::ant_protocol::decode(group, payload) {
        SHARED.lock(|s| {
            let mut shared = s.borrow_mut();
            shared.startup_activity = true;
            let selected = match event {
                device_api::ant::Event::Connected(peer)
                | device_api::ant::Event::Disconnected(peer)
                | device_api::ant::Event::Timeout(peer) => shared
                    .state
                    .channel(peer.device_type, now)
                    .and_then(|s| s.selected),
                _ => None,
            };
            let event = crate::drivers::ant_protocol::normalize(event, selected);
            if matches!(event, device_api::ant::Event::ScanEnded) {
                // Only one physical scan may be outstanding. A timed-out or lost
                // session cannot be recovered by an untagged late observation.
                // Ignore early endings during a newer natural-duration scan.
                shared.scan.ended(now);
            }
            shared.state.receive(event, now);
            shared.progress_power(now);
        });
    }
}
pub fn loss(now: u64) {
    SHARED.lock(|s| {
        let mut shared = s.borrow_mut();
        shared.pending = None;
        shared.state.transport_loss(now);
        if !shared.scan.idle() {
            shared.scan = ScanOwnership::Uncertain;
        }
        if shared.power != PeripheralState::Running {
            shared.power = PeripheralState::Failed;
            shared.power_active = false;
        }
    });
}
pub fn tick(now: u64) {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.state.tick(now);
        s.scan.tick(now);
        s.progress_power(now);
        if let Some(request) = s.pending.take() {
            let frame = crate::drivers::ant_protocol::encode(request);
            if crate::drivers::companion_uart::send(&frame).is_err() {
                s.state.request_failed(request, now);
                if matches!(request, Request::Scan { .. } | Request::StopScan) {
                    s.scan = ScanOwnership::Uncertain;
                }
                if s.power != PeripheralState::Running {
                    s.power = PeripheralState::Failed;
                    s.power_active = false;
                }
            }
        } else if let Some((id, target, data)) = s.state.pending_send() {
            // The same IO task serializes this with all companion control traffic.
            let frame =
                crate::drivers::ant_protocol::encode_send(target.identity.device_type, data);
            if frame.is_some_and(|frame| crate::drivers::companion_uart::send(&frame).is_ok()) {
                s.state.send_submitted(id, now);
            } else {
                s.state.send_failed(id);
            }
        }
    });
}

pub fn reply(group: u8, payload: [u8; 8], now: u64) {
    if let Some(reply) = crate::drivers::ant_protocol::decode_send_reply(group, payload) {
        SHARED.lock(|s| {
            let mut s = s.borrow_mut();
            if let Some((id, kind, echoed)) = s.wire_send
                && kind == reply.device_type
                && echoed == reply.echoed
            {
                s.state.send_reply(id, reply.accepted, now);
                s.release_settled_send();
            }
        });
    }
}

pub struct Ant;
impl device_api::ant::Ant for Ant {
    fn availability(&self) -> device_api::observation::Availability {
        if super::sensors::ant_allowed() {
            device_api::observation::Availability::Ready
        } else {
            device_api::observation::Availability::Initializing
        }
    }
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            supported_types: crate::drivers::ant_protocol::SUPPORTED_TYPES,
            connection_capacity: 10,
            one_peer_per_type: true,
            concurrent_scan: true,
            acknowledged_send: true,
            radio_delivery_feedback: false,
            burst: false,
            max_pending_sends: 1,
        }
    }
    fn scan(&self) -> ScanSnapshot {
        SHARED.lock(|s| {
            let s = s.borrow();
            let mut snapshot = s.state.scan();
            if matches!(s.scan, ScanOwnership::Uncertain) {
                snapshot.state = device_api::ant::ScanState::Uncertain;
            } else if !s.scan.idle() && snapshot.state == device_api::ant::ScanState::Idle {
                snapshot.state = device_api::ant::ScanState::Stopping;
            }
            snapshot
        })
    }
    fn channels(&self, now: u64) -> [Option<Snapshot>; device_api::ant::CHANNEL_CAPACITY] {
        snapshots(now)
    }
    fn take_packet(&mut self) -> Option<Packet> {
        take_packet()
    }
    fn take_diagnostic_packet(&mut self) -> Option<Packet> {
        SHARED.lock(|s| s.borrow_mut().state.pop_diagnostic_packet())
    }
    fn send_status(&self, id: OperationId) -> Option<SendObservation> {
        SHARED.lock(|s| s.borrow().state.send_status(id))
    }
    fn scanning(&self) -> bool {
        scanning()
    }
    fn discoveries(&self) -> [Option<Discovery>; 8] {
        discoveries()
    }
    fn request(&mut self, operation: Operation, now: u64) -> Result<Admission, AntError> {
        let Ok(_access) = super::power::ACCESS.enter() else {
            return Err(AntError::Busy);
        };
        request(operation, now)
    }
}

#[cfg(test)]
mod power_tests {
    use super::*;
    use device_api::ant::Event;

    const PEER: Identity = Identity {
        device_type: 120,
        device_number: 123,
        transmission_type: 1,
    };

    fn shared() -> Shared {
        Shared {
            startup_activity: false,
            state: Channels::new(),
            pending: None,
            send_keys: [None; SEND_HISTORY_CAPACITY],
            wire_send: None,
            scan: ScanOwnership::Idle,
            power: PeripheralState::Pending,
            suspending: true,
            power_active: true,
            restore: [None; device_api::ant::CHANNEL_CAPACITY],
            reconnect_sent: [false; device_api::ant::CHANNEL_CAPACITY],
            power_deadline: 12_000,
        }
    }

    #[test]
    fn local_scan_deadline_cannot_complete_power_quiescence() {
        let mut s = shared();
        s.state.begin_scan(0, 100).unwrap();
        s.scan = ScanOwnership::Active(100);
        s.state.tick(100);
        s.progress_power(100);
        assert_eq!(s.power, PeripheralState::Failed);
        assert!(s.pending.is_none());
    }

    #[test]
    fn scan_stop_waits_for_observation_without_replaying() {
        let mut s = shared();
        s.state.begin_scan(0, 100).unwrap();
        s.scan = ScanOwnership::Active(8_000);
        s.progress_power(1);
        assert_eq!(s.pending.take(), Some(Request::StopScan));
        s.progress_power(100);
        assert_eq!(s.power, PeripheralState::Pending);
        assert!(s.pending.is_none());
        s.scan.ended(101);
        s.progress_power(101);
        assert_eq!(s.power, PeripheralState::Pending);
        s.state.tick(6_000);
        s.scan.ended(6_000);
        s.progress_power(6_000);
        assert_eq!(s.power, PeripheralState::Quiescent);
    }

    #[test]
    fn confirmed_close_is_required_before_abort_reconnect() {
        let mut s = shared();
        s.state.connect(PEER, 0).unwrap();
        s.state.receive(Event::Connected(PEER), 1);
        s.restore[0] = Some(PEER);
        s.progress_power(2);
        assert_eq!(
            s.pending.take(),
            Some(Request::Disconnect { identity: PEER })
        );
        s.suspending = false;
        s.progress_power(3);
        assert!(s.pending.is_none());
        assert_eq!(s.power, PeripheralState::Pending);
        s.state.receive(Event::Disconnected(PEER), 4);
        s.progress_power(4);
        assert_eq!(s.pending.take(), Some(Request::Connect { identity: PEER }));
        s.progress_power(5);
        assert_eq!(s.power, PeripheralState::Pending);
        s.state.receive(Event::Connected(PEER), 6);
        s.progress_power(6);
        assert_eq!(s.power, PeripheralState::Running);
        assert!(s.state.channel(PEER.device_type, 6).unwrap().stale);
    }

    #[test]
    fn uncertain_close_is_never_replayed_or_reconnected() {
        let mut s = shared();
        s.state.connect(PEER, 0).unwrap();
        s.state.receive(Event::Connected(PEER), 1);
        s.restore[0] = Some(PEER);
        s.progress_power(2);
        let request = s.pending.take().unwrap();
        s.state.request_failed(request, 3);
        s.suspending = false;
        s.progress_power(4);
        assert_eq!(s.power, PeripheralState::Failed);
        assert!(s.pending.is_none());
        s.progress_power(11_000);
        assert!(s.pending.is_none());
    }
}
