//! Passive companion sensors and a single bounded read-only identity request.
use crate::drivers::companion_sensors::State;
use core::cell::RefCell;
use device_api::observation::Error;
use device_api::sensors::{IdentityQuery, Snapshot, Startup};
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
struct Shared {
    startup: crate::drivers::companion_startup::Startup,
    state: Option<State>,
    query: IdentityQuery,
    at: u64,
}
static SHARED: Mutex<CriticalSectionRawMutex, RefCell<Shared>> = Mutex::new(RefCell::new(Shared {
    startup: crate::drivers::companion_startup::Startup::new(),
    state: None,
    query: IdentityQuery::Idle,
    at: 0,
}));
pub fn startup_begin(now: u64) {
    SHARED.lock(|s| s.borrow_mut().startup.begin(now));
}
pub fn startup_status() -> Startup {
    let phase = startup_phase();
    if super::power::ACCESS.closed() && matches!(phase, Startup::Ready | Startup::AlreadyRunning) {
        return Startup::PowerTransition;
    }
    phase
}
pub fn startup_phase() -> Startup {
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
pub fn query_status() -> IdentityQuery {
    SHARED.lock(|s| s.borrow().query)
}
pub fn query(now: u64) -> Result<(), Error> {
    let _access = super::power::ACCESS.enter()?;
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        if s.query.outstanding() {
            return Err(Error::Unavailable);
        }
        s.query = IdentityQuery::Queued;
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
        if s.query == IdentityQuery::Waiting && now.saturating_sub(s.at) <= 2000 {
            s.state.get_or_insert_default().receive(1, payload, now);
            s.query = IdentityQuery::ReplyObserved;
        }
    });
}

pub fn loss() {
    SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        s.state.get_or_insert_default().loss();
        if s.query.outstanding() {
            s.query = IdentityQuery::Failed;
        }
    });
}
/// `link_faults` is this acquisition tick's cumulative UART error and bad-CRC
/// count. The published bridge snapshot lags one tick behind framing losses
/// counted here, which would split one fault across two ticks.
pub fn tick(now: u64, link_faults: u32) {
    // Ordinary recovery/query writers stop during a power transition. Explicit
    // charging wake and passive physical-startup observations still progress.
    let blocked = super::power::ACCESS.closed();
    use device_api::observation::Observation;
    let bridge = super::io::snapshot(now);
    let bridge_fresh = bridge.is_some_and(|s| {
        matches!(s.battery, Observation::Fresh { .. })
            && matches!(s.power, Observation::Fresh { .. })
    });
    let radio_seen = super::ant::startup_activity();
    let acknowledge = !blocked
        && SHARED.lock(|s| {
            let mut s = s.borrow_mut();
            let sample = s.state.unwrap_or_default().snapshot(now, 5000);
            // Cumulative, so startup judges only faults counted since it began observing.
            let faults = link_faults.saturating_add(sample.losses);
            s.startup
                .probe(sample, now, bridge_fresh, faults, radio_seen)
        });
    if acknowledge {
        let result =
            crate::drivers::companion_uart::send(&crate::drivers::companion_startup::frame());
        SHARED.lock(|s| s.borrow_mut().startup.submitted(result.map(|()| 16), now));
    }
    let wake = SHARED.lock(|s| {
        let mut s = s.borrow_mut();
        let sample = s.state.unwrap_or_default().snapshot(now, 5000);
        s.startup.wake(
            sample,
            bridge_fresh,
            link_faults.saturating_add(sample.losses),
            radio_seen,
        )
    });
    if wake {
        let result =
            crate::drivers::companion_uart::send(&crate::drivers::companion_startup::wake_frame());
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
        if s.query.outstanding() && now.saturating_sub(s.at) > 2000 {
            s.query = IdentityQuery::Timeout;
        }
        if s.query == IdentityQuery::Queued {
            s.query = IdentityQuery::Waiting;
            true
        } else {
            false
        }
    });
    if send {
        // N22 group1/page1 handler constructs an identity report only. No OTA
        // mode, reset, flash, network or calibration command is reachable here.
        let frame = crate::drivers::companion::command(1, [1, 0, 0, 0, 0, 0, 0, 0]);
        if crate::drivers::companion_uart::send(&frame).is_err() {
            SHARED.lock(|s| s.borrow_mut().query = IdentityQuery::Failed);
        }
    }
}
