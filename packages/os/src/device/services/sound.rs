use core::cell::RefCell;
use cycling_os::{capabilities::Error, sound::Player};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
static PLAYER: Mutex<CriticalSectionRawMutex, RefCell<Player>> =
    Mutex::new(RefCell::new(Player::new()));
pub fn snapshot() -> cycling_os::sound::Snapshot {
    PLAYER.lock(|p| p.borrow().snapshot())
}
pub fn play(id: u8, now: u64) -> Result<(), Error> {
    let _access = super::power::ACCESS.enter()?;
    let pattern = super::super::sound_protocol::PATTERNS
        .iter()
        .find(|p| p.id == id)
        .ok_or(Error::Invalid)?;
    PLAYER.lock(|p| p.borrow_mut().play(*pattern, now))
}
pub fn stop(now: u64) -> Result<(), Error> {
    let _access = super::power::ACCESS.enter()?;
    power_stop(now)
}
pub fn power_stop(now: u64) -> Result<(), Error> {
    PLAYER.lock(|p| p.borrow_mut().stop(now))
}
pub fn tick(now: u64) {
    PLAYER.lock(|p| {
        p.borrow_mut().dispatch(now, |id| {
            let frame = super::super::sound_protocol::encode(id).ok_or(())?;
            super::super::companion_uart::send(&frame)
        })
    });
}
