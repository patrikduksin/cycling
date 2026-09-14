//! Passive companion sensors and a single bounded read-only identity request.
use core::cell::RefCell;
use cycling_os::{
    capabilities::Error,
    companion_sensors::{Snapshot, State},
};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
struct Shared {
    startup: super::super::companion_startup::Startup,
    state: Option<State>,
    query: &'static str,
    at: u64,
}
static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared {
    startup: super::super::companion_startup::Startup::new(),
    state: None,
    query: "idle",
    at: 0,
}));
pub fn startup_begin(now: u64) {
    SHARED.lock(|s| s.borrow_mut().startup.begin(now));
}
pub fn startup_status() -> &'static str {
    let phase = startup_phase();
    if super::power::ACCESS.closed() && matches!(phase, "ready" | "already_running") {
        return "power_transition";
    }
    phase
}
pub fn startup_phase() -> &'static str {
    SHARED.lock(|s| s.borrow().startup.status())
}
pub fn power_request_wake() -> Result<(), Error> {
    SHARED.lock(|s| s.borrow_mut().startup.request_wake())
}
pub fn startup_reason() -> Option<u8> {
    SHARED.lock(|s| s.borrow().startup.reason())
}
pub fn ant_allowed() -> bool {
    SHARED.lock(|s| s.borrow().startup.ant_allowed())
}
pub fn snapshot(now: u64) -> Snapshot {
    SHARED.lock(|s| s.borrow().state.unwrap_or_default().snapshot(now, 5000))
}
pub fn query_status() -> &'static str {
    SHARED.lock(|s| s.borrow().query)
}
pub fn query(now: u64) -> Result<(), Error> {
    let _access = super::power::ACCESS.enter()?;
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
        s.startup.report(group, payload, now);
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
    // Ordinary recovery/query writers stop during a power transition. Explicit
    // charging wake and passive physical-startup observations still progress.
    let blocked = super::power::ACCESS.closed();
    use cycling_os::capabilities::Observation;
    let bridge = super::io::snapshot(now);
    let bridge_fresh = bridge.is_some_and(|s| {
        matches!(s.battery, Observation::Fresh { .. })
            && matches!(s.power, Observation::Fresh { .. })
    });
    let transport_clean = bridge.is_none_or(|s| s.uart_errors == 0 && s.companion_bad_crc == 0);
    let radio_seen = super::ant::startup_activity();
    let acknowledge = !blocked
        && SHARED.lock(|s| {
            let mut s = s.borrow_mut();
            let sample = s.state.unwrap_or_default().snapshot(now, 5000);
            let transport_clean = transport_clean && sample.losses == 0;
            s.startup
                .probe(sample, now, bridge_fresh, transport_clean, radio_seen)
        });
    if acknowledge {
        let result = super::super::companion_uart::send(&super::super::companion_startup::frame());
        SHARED.lock(|s| s.borrow_mut().startup.submitted(result.map(|()| 16), now));
    }
    let wake = SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        let sample = s.state.unwrap_or_default().snapshot(now, 5000);
        s.startup.wake(
            sample,
            bridge_fresh,
            transport_clean && sample.losses == 0,
            radio_seen,
        )
    });
    if wake {
        let result =
            super::super::companion_uart::send(&super::super::companion_startup::wake_frame());
        SHARED.lock(|s| {
            s.borrow_mut()
                .startup
                .wake_submitted(result.map(|()| 16), now)
        });
    }
    let send = SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        let sample = s.state.unwrap_or_default().snapshot(now, 5000);
        s.startup.observe(sample, now);
        if blocked {
            return false;
        }
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
