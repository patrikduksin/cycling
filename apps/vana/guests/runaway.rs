//! Loops for ever at exactly 8,334 mm/s (30.0 km/h) and draws a bar at any
//! other speed: accept's trial readings miss that one speed; a ride hits it.
#![no_std]
mod vana;

const TRAP: i32 = 8_334;

#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(vana::unit(24), vana::unit(24), 1, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, width: i32, height: i32) {
    let mut seed = vana::metric(vana::SPEED);
    while seed == TRAP {
        seed = core::hint::black_box(seed);
    }
    vana::bar(0, 0, width, height, 0xffff);
}
