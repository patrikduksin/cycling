//! Calls the native its look names over and over, a few instructions
//! apart: each call costs the host more than an instruction.
#![no_std]
mod vana;

const TEXT: &[u8] = b"888888888888888888888888888888888888888888888888";

#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(200, 200, 255, 255);
}

/// `native` until it returns 7, which none does.
fn spin(native: impl Fn() -> i32) {
    while core::hint::black_box(native()) != 7 {}
}

#[unsafe(no_mangle)]
pub extern "C" fn draw(native: i32, width: i32, height: i32) {
    match native {
        0 => spin(|| vana::text_width(vana::HERO, TEXT)),
        1 => spin(|| vana::text_height(vana::BODY)),
        2 => spin(|| vana::metric(vana::POWER)),
        3 => spin(|| vana::zone(0)),
        4 => spin(|| vana::unit(10)),
        5 => spin(|| {
            vana::bar(0, 0, width, height, 0xffff);
            0
        }),
        6 => spin(|| {
            vana::ring(width / 2, height / 2, width / 3, 4, 1 << 15, 0xffff);
            0
        }),
        7 => spin(|| {
            vana::text(vana::HERO, TEXT, height, vana::LEFT, 0xffff);
            0
        }),
        _ => spin(|| {
            vana::series(vana::POWER, 600, 1, 0, 0, width, height, 0xffff);
            0
        }),
    }
}
