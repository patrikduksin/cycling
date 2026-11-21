//! The last minute of power as twelve bars: `series` does the looping.
#![no_std]
mod vana;

const BARS: i32 = 12;
const POWER_COLOR: i32 = 0xfbe0;

#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(vana::unit(96), vana::unit(24), BARS, 0);
    vana::form(vana::unit(48), vana::unit(12), BARS, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, width: i32, height: i32) {
    vana::series(vana::POWER, 60, BARS, 0, 0, width, height, POWER_COLOR);
}
