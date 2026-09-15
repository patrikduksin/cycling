use core::cell::RefCell;
use device_api::observation::Error;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use firmware_services::sound::Player;
static PLAYER: Mutex<CriticalSectionRawMutex, RefCell<Player>> =
    Mutex::new(RefCell::new(Player::new()));
pub fn snapshot() -> device_api::sound::Snapshot {
    PLAYER.lock(|p| p.borrow().snapshot())
}
pub fn play(id: u8, now: u64) -> Result<(), Error> {
    let _access = super::power::ACCESS.enter()?;
    let pattern = crate::drivers::sound_protocol::PATTERNS
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
            let frame = crate::drivers::sound_protocol::encode(id).ok_or(())?;
            crate::drivers::companion_uart::send(&frame)
        })
    });
}
