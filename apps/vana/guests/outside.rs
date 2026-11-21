//! Draws one pixel past its box, so accept refuses it.
#![no_std]
mod vana;

#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(vana::unit(24), vana::unit(24), 1, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, width: i32, height: i32) {
    vana::bar(0, 0, width + 1, height, 0xffff);
}
