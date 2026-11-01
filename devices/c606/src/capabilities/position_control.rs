use core::cell::RefCell;
use device_api::observation::Error;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use firmware_services::position_control::Controller;
static CONTROL: Mutex<CriticalSectionRawMutex, RefCell<Controller>> =
    Mutex::new(RefCell::new(Controller::new()));
pub fn snapshot() -> device_api::position_control::Snapshot {
    CONTROL.lock(|c| c.borrow().snapshot())
}
pub fn pause(ms: u32, now: u64) -> Result<(), Error> {
    let _access = super::power::ACCESS.enter()?;
    queued(CONTROL.lock(|c| c.borrow_mut().pause(ms, now)))
}
pub fn resume(now: u64) -> Result<(), Error> {
    let _access = super::power::ACCESS.enter()?;
    queued(CONTROL.lock(|c| c.borrow_mut().resume(now)))
}
pub fn power_suspend(now: u64) -> Result<(), Error> {
    queued(CONTROL.lock(|c| c.borrow_mut().suspend(now)))
}
pub fn power_resume(now: u64) -> Result<(), Error> {
    queued(CONTROL.lock(|c| c.borrow_mut().resume(now)))
}
/// A queued command is sent by the IO task's next tick.
fn queued(result: Result<(), Error>) -> Result<(), Error> {
    result?;
    super::io::wake();
    Ok(())
}
/// Send a queued command and observe the stream; when control is next due.
pub fn tick(now: u64) -> Option<u64> {
    let position = super::positioning::snapshot(now);
    let received = position.and_then(|p| p.received_at_ms);
    let (boundary, due) = CONTROL.lock(|c| {
        let mut c = c.borrow_mut();
        let boundary = c.tick(
            now,
            position.map_or(0, |p| p.gps.valid_sentences),
            received,
            |open| {
                crate::drivers::companion_uart::send(&crate::drivers::companion::gnss_power(open))
            },
        );
        (boundary, c.due(now, received))
    });
    if boundary {
        super::positioning::invalidate();
    }
    due
}
