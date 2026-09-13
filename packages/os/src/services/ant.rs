//! ANT state shared with the console; the existing IO task owns all UART work.
use core::cell::RefCell;
use cycling_os::ant::{Discovery, Identity, Packet, Request, Snapshot, State};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};

struct Shared {
    state: State,
    pending: Option<Request>,
}
static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared {
    state: State::new(),
    pending: None,
}));

pub fn snapshot(now: u64) -> Snapshot {
    SHARED.lock(|s| s.borrow().state.snapshot(now))
}
pub fn discoveries() -> [Option<Discovery>; 8] {
    SHARED.lock(|s| *s.borrow().state.discoveries())
}
pub fn take_packet() -> Option<Packet> {
    SHARED.lock(|s| s.borrow_mut().state.pop_packet())
}

pub enum Operation {
    Scan(u32),
    StopScan,
    Connect(Identity),
    Disconnect,
}
pub fn request(operation: Operation, now: u64) -> &'static str {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        if s.pending.is_some() {
            return "BUSY";
        }
        let result = match operation {
            Operation::Scan(ms) => s.state.begin_scan(now, ms),
            Operation::StopScan => Ok(s.state.stop_scan()),
            Operation::Connect(peer) => s.state.connect(peer, now),
            Operation::Disconnect => match s.state.disconnect(now) {
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
            let selected = shared.state.snapshot(now).selected;
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
        SHARED.lock(|s| s.borrow_mut().state.tx_failed(now));
        log::warn!(target: "ant", "command transmit failed; completion uncertain");
    }
}
