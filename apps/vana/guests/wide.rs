//! Draws a bar a pixel wide for each 10 W: inside its box at an easy power,
//! past it at the top of the range, so accept's trial readings refuse it.
#![no_std]
mod vana;

#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(vana::unit(96), vana::unit(24), 1, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, _width: i32, height: i32) {
    let power = vana::metric(vana::POWER);
    if power != vana::NONE {
        vana::bar(0, 0, power / 10, height, 0xfbe0);
    }
}
