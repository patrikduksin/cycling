//! ANT state shared with the console; the existing IO task owns all UART work.
use core::cell::RefCell;
use cycling_os::ant::{Channels, Discovery, Packet, Request, Snapshot};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

struct Shared {
    startup_activity: bool,
    state: Channels,
    pending: Option<Request>,
}
static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared {
    startup_activity: false,
    state: Channels::new(),
    pending: None,
}));

pub fn startup_activity() -> bool {
    SHARED.lock(|s| s.borrow().startup_activity)
}
pub fn snapshots(now: u64) -> [Option<Snapshot>; cycling_os::ant::CHANNEL_CAPACITY] {
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

pub use cycling_os::capabilities::AntOperation as Operation;
pub fn request(operation: Operation, now: u64) -> &'static str {
    if matches!(operation, Operation::Scan(_) | Operation::Connect(_))
        && !super::sensors::ant_allowed()
    {
        return "BUSY";
    }
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        if s.pending.is_some() {
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
                s.pending = Some(request);
                "ACCEPTED"
            }
            Err(_) => "STATE",
        }
    })
}
pub fn receive(group: u8, payload: [u8; 8], now: u64) {
    if let Some(event) = crate::device::ant_protocol::decode(group, payload) {
        SHARED.lock(|s| {
            let mut shared = s.borrow_mut();
            shared.startup_activity = true;
            let selected = match event {
                cycling_os::ant::Event::Connected(peer)
                | cycling_os::ant::Event::Disconnected(peer)
                | cycling_os::ant::Event::Timeout(peer) => shared
                    .state
                    .channel(peer.device_type, now)
                    .and_then(|s| s.selected),
                _ => None,
            };
            let event = crate::device::ant_protocol::normalize(event, selected);
            shared.state.receive(event, now);
        });
    }
}
pub fn loss(now: u64) {
    SHARED.lock(|s| {
        let mut shared = s.borrow_mut();
        shared.pending = None;
        shared.state.transport_loss(now);
    });
}
pub fn tick(now: u64) {
    let pending = SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.state.tick(now);
        s.pending.take()
    });
    let Some(request) = pending else {
        return;
    };
    let frame = crate::device::ant_protocol::encode(request);
    if crate::device::companion_uart::send(&frame).is_err() {
        SHARED.lock(|s| s.borrow_mut().state.request_failed(request, now));
        log::warn!(target: "ant", "command transmit failed; completion uncertain");
    }
}
