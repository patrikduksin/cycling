//! ANT state shared with the console; the existing IO task owns all UART work.
use core::cell::RefCell;
use device_api::ant::Discovery;
use device_api::ant::Identity;
use device_api::ant::LinkState;
use device_api::ant::Packet;
use device_api::ant::Request;
use device_api::ant::Snapshot;
use device_api::observation::Error;
use device_api::power::PeripheralState;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use firmware_services::ant::Channels;

struct Shared {
    startup_activity: bool,
    state: Channels,
    pending: Option<Request>,
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
    Stopping(u64),
    Uncertain,
}
impl ScanOwnership {
    fn tick(&mut self, now: u64) {
        if matches!(*self, Self::Active(deadline) | Self::Stopping(deadline) if now >= deadline) {
            *self = Self::Uncertain;
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
        s.power_active = true;
        s.suspending = suspend;
        s.power = PeripheralState::Pending;
        s.power_deadline = now.saturating_add(device_api::ant::CONNECT_TIMEOUT_MS + 2_000);
        s.progress_power(now);
        Ok(())
    })
}

impl Shared {
    fn progress_power(&mut self, now: u64) {
        self.scan.tick(now);
        if !self.power_active || self.pending.is_some() {
            return;
        }
        let mut waiting = false;
        let mut failed = false;
        if self.suspending {
            match self.scan {
                ScanOwnership::Active(_) => match self.state.stop_scan(now) {
                    Ok(request) => {
                        self.scan = ScanOwnership::Stopping(
                            now.saturating_add(device_api::ant::SCAN_STOP_TIMEOUT_MS),
                        );
                        self.pending = Some(request);
                        return;
                    }
                    Err(_) => failed = true,
                },
                ScanOwnership::Stopping(_) => waiting = true,
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
                            return;
                        }
                    }
                    LinkState::Disconnecting => waiting = true,
                    LinkState::TimedOut | LinkState::TransportLost => failed = true,
                    LinkState::Idle | LinkState::Disconnected => {}
                }
            }
        } else {
            // A submitted scan stop must settle even when preparation is aborted.
            waiting |= matches!(self.scan, ScanOwnership::Stopping(_));
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
pub fn request(operation: Operation, now: u64) -> &'static str {
    if matches!(operation, Operation::Scan(_) | Operation::Connect(_))
        && !super::sensors::ant_allowed()
    {
        return "BUSY";
    }
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.scan.tick(now);
        if s.power != PeripheralState::Running
            || s.pending.is_some()
            || matches!(operation, Operation::Scan(_) | Operation::Connect(_)) && !s.scan.idle()
        {
            return "BUSY";
        }
        let result = match operation {
            Operation::Scan(ms) => s.state.begin_scan(now, ms),
            Operation::StopScan => s.state.stop_scan(now),
            Operation::Connect(peer) => s.state.connect(peer, now),
            Operation::Disconnect(kind) => match s.state.disconnect(kind, now) {
                Some(request) => Ok(request),
                None => return "STATE",
            },
        };
        match result {
            Ok(request) => {
                match request {
                    Request::Scan { duration_ms } => {
                        s.scan = ScanOwnership::Active(now.saturating_add(u64::from(duration_ms)))
                    }
                    Request::StopScan => {
                        s.scan = ScanOwnership::Stopping(
                            now.saturating_add(device_api::ant::SCAN_STOP_TIMEOUT_MS),
                        )
                    }
                    _ => {}
                }
                s.pending = Some(request);
                "ACCEPTED"
            }
            Err(_) => "STATE",
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
                shared.scan = ScanOwnership::Idle;
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
    let pending = SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.state.tick(now);
        s.scan.tick(now);
        s.progress_power(now);
        s.pending.take()
    });
    let Some(request) = pending else {
        return;
    };
    let frame = crate::drivers::ant_protocol::encode(request);
    if crate::drivers::companion_uart::send(&frame).is_err() {
        SHARED.lock(|s| {
            let mut s = s.borrow_mut();
            s.state.request_failed(request, now);
            if matches!(request, Request::Scan { .. } | Request::StopScan) {
                s.scan = ScanOwnership::Uncertain;
            }
            if s.power != PeripheralState::Running {
                s.power = PeripheralState::Failed;
                s.power_active = false;
            }
        });
        log::warn!(target: "ant", "command transmit failed; completion uncertain");
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
        s.scan = ScanOwnership::Active(100);
        s.progress_power(1);
        assert_eq!(s.pending.take(), Some(Request::StopScan));
        s.progress_power(100);
        assert_eq!(s.power, PeripheralState::Pending);
        assert!(s.pending.is_none());
        s.state.receive(Event::ScanEnded, 101);
        s.scan = ScanOwnership::Idle;
        s.progress_power(101);
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
