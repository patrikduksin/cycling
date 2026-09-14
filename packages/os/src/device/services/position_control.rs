use core::cell::RefCell;
use cycling_os::{capabilities::Error, position_control::Controller};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
static CONTROL: Mutex<CriticalSectionRawMutex, RefCell<Controller>> =
    Mutex::new(RefCell::new(Controller::new()));
pub fn snapshot() -> cycling_os::position_control::Snapshot {
    CONTROL.lock(|c| c.borrow().snapshot())
}
pub fn pause(ms: u32, now: u64) -> Result<(), Error> {
    CONTROL.lock(|c| c.borrow_mut().pause(ms, now))
}
pub fn resume(now: u64) -> Result<(), Error> {
    CONTROL.lock(|c| c.borrow_mut().resume(now))
}
pub fn tick(now: u64) {
    let position = super::positioning::snapshot(now);
    let boundary = CONTROL.lock(|c| {
        c.borrow_mut().tick(
            now,
            position.map_or(0, |p| p.gps.valid_sentences),
            position.and_then(|p| p.received_at_ms),
            |open| {
                let frame =
                    cycling_os::companion::command(16, [0xe2, 2, 7, 0, 0, u8::from(open), 0, 0]);
                super::super::companion_uart::send(&frame)
            },
        )
    });
    if boundary {
        super::positioning::invalidate();
    }
}
