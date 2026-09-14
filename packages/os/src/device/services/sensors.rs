//! Passive companion sensors and a single bounded read-only identity request.
use core::cell::RefCell;
use cycling_os::{
    capabilities::Error,
    companion_sensors::{Snapshot, State},
};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
struct Shared {
    state: Option<State>,
    query: &'static str,
    at: u64,
}
static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared {
    state: None,
    query: "idle",
    at: 0,
}));
pub fn snapshot(now: u64) -> Snapshot {
    SHARED.lock(|s| s.borrow().state.unwrap_or_default().snapshot(now, 5000))
}
pub fn query_status() -> &'static str {
    SHARED.lock(|s| s.borrow().query)
}
pub fn query(now: u64) -> Result<(), Error> {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        if matches!(s.query, "queued" | "waiting") {
            return Err(Error::Unavailable);
        }
        s.query = "queued";
        s.at = now;
        Ok(())
    })
}
pub fn receive(group: u8, payload: &[u8], now: u64) {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.state.get_or_insert_default().receive(group, payload, now);
    })
}
// Untagged query replies can establish receipt during this window, not a
// cryptographically or sequence-identified installed firmware version.
pub fn identity_reply(payload: &[u8], now: u64) {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        if s.query == "waiting" && now.saturating_sub(s.at) <= 2000 {
            s.state.get_or_insert_default().receive(1, payload, now);
            s.query = "reply_observed";
        }
    });
}

pub fn loss() {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.state.get_or_insert_default().loss();
        if matches!(s.query, "queued" | "waiting") {
            s.query = "failed";
        }
    });
}
pub fn tick(now: u64) {
    let send = SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        if matches!(s.query, "queued" | "waiting") && now.saturating_sub(s.at) > 2000 {
            s.query = "timeout";
        }
        if s.query == "queued" {
            s.query = "waiting";
            true
        } else {
            false
        }
    });
    if send {
        // N22 group1/page1 handler constructs an identity report only. No OTA
        // mode, reset, flash, network or calibration command is reachable here.
        let frame = cycling_os::companion::command(1, [1, 0, 0, 0, 0, 0, 0, 0]);
        if super::super::companion_uart::send(&frame).is_err() {
            SHARED.lock(|s| s.borrow_mut().query = "failed");
        }
    }
}
