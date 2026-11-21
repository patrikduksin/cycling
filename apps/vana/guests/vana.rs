//! A script tile's side of VANA's host API (module `vana`). A tile exports
//! `forms()`, which declares its looks with [`form`], best first, and
//! `draw(form, width, height)`, which draws one of them into a box of that
//! size. Coordinates are pixels from the box's top-left corner; a draw
//! outside the box, or past the shapes and runs its look declared, stops
//! the tile.
#![allow(dead_code)]

/// `metric` without a reading.
pub const NONE: i32 = i32::MIN;

// A metric's id is its catalogue id, `Metric::id`.
pub const CADENCE: i32 = 0;
pub const DISTANCE: i32 = 1;
pub const SPEED: i32 = 9;
pub const POWER: i32 = 10;
pub const HEART: i32 = 11;
pub const ELAPSED: i32 = 12;
pub const GRADE: i32 = 13;

pub const CAPTION: i32 = 0;
pub const BODY: i32 = 1;
pub const TITLE: i32 = 2;
pub const VALUE: i32 = 3;
pub const LARGE: i32 = 4;
pub const HERO: i32 = 5;

pub const LEFT: i32 = 0;
pub const CENTRE: i32 = 1;
pub const RIGHT: i32 = 2;

mod raw {
    #[link(wasm_import_module = "vana")]
    unsafe extern "C" {
        pub fn form(width: i32, height: i32, shapes: i32, runs: i32);
        pub fn unit(n: i32) -> i32;
        pub fn text_width(face: i32, text: *const u8, len: usize) -> i32;
        pub fn text_height(face: i32) -> i32;
        pub fn metric(id: i32) -> i32;
        pub fn zone(effort: i32) -> i32;
        pub fn bar(left: i32, top: i32, width: i32, height: i32, color: i32);
        pub fn ring(cx: i32, cy: i32, radius: i32, width: i32, frac_q16: i32, color: i32);
        pub fn text(face: i32, text: *const u8, len: usize, base: i32, align: i32, color: i32);
        pub fn series(metric: i32, seconds: i32, bars: i32, left: i32, top: i32, width: i32, height: i32, color: i32);
    }
}

pub fn form(width: i32, height: i32, shapes: i32, runs: i32) {
    unsafe { raw::form(width, height, shapes, runs) }
}
pub fn unit(n: i32) -> i32 {
    unsafe { raw::unit(n) }
}
pub fn text_width(face: i32, text: &[u8]) -> i32 {
    unsafe { raw::text_width(face, text.as_ptr(), text.len()) }
}
pub fn text_height(face: i32) -> i32 {
    unsafe { raw::text_height(face) }
}
pub fn metric(id: i32) -> i32 {
    unsafe { raw::metric(id) }
}
pub fn zone(effort: i32) -> i32 {
    unsafe { raw::zone(effort) }
}
pub fn bar(left: i32, top: i32, width: i32, height: i32, color: i32) {
    unsafe { raw::bar(left, top, width, height, color) }
}
pub fn ring(cx: i32, cy: i32, radius: i32, width: i32, frac_q16: i32, color: i32) {
    unsafe { raw::ring(cx, cy, radius, width, frac_q16, color) }
}
pub fn text(face: i32, text: &[u8], base: i32, align: i32, color: i32) {
    unsafe { raw::text(face, text.as_ptr(), text.len(), base, align, color) }
}
/// The last `seconds` of `metric` as `bars` bars across the given box, from
/// the window's lowest reading to its highest.
pub fn series(metric: i32, seconds: i32, bars: i32, left: i32, top: i32, width: i32, height: i32, color: i32) {
    unsafe { raw::series(metric, seconds, bars, left, top, width, height, color) }
}

/// `value` in decimal, right-aligned in `out`; the digits written.
pub fn digits(mut value: u32, out: &mut [u8; 10]) -> &[u8] {
    let mut at = out.len();
    loop {
        at -= 1;
        out[at] = b'0' + (value % 10) as u8;
        value /= 10;
        if value == 0 || at == 0 {
            break;
        }
    }
    &out[at..]
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
