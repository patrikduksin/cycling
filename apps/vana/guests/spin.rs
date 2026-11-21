//! Loops for ever on every draw, so accept refuses it.
#![no_std]
mod vana;

#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(vana::unit(24), vana::unit(24), 0, 0);
}

#[unsafe(no_mangle)]
pub extern "C" fn draw(seed: i32, _width: i32, _height: i32) {
    let mut x = seed;
    loop {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        if x == 7 {
            return;
        }
    }
}
