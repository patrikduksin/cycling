//! Draws a ring centred on its box's top-left corner, mostly outside it.
#![no_std]
mod vana;
#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(100, 100, 1, 0);
}
#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, _width: i32, _height: i32) {
    vana::ring(0, 0, 30, 2, 1 << 16, 0xffff);
}
