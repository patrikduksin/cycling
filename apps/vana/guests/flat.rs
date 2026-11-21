//! Draws a series into a box with no height (look 0) or no width (look 1).
#![no_std]
mod vana;
#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(100, 100, 2, 0);
}
#[unsafe(no_mangle)]
pub extern "C" fn draw(form: i32, width: i32, _height: i32) {
    if form == 0 { vana::series(vana::POWER, 60, 1, 0, 0, 10, 0, 0xffff); }
    else { vana::series(vana::POWER, 60, 1, width, 0, 0, 10, 0xffff); }
}
