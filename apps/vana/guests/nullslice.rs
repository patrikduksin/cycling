//! Measures the empty text at the very end of its memory.
#![no_std]
mod vana;
#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    let end = unsafe { core::slice::from_raw_parts(65536 as *const u8, 0) };
    let w = vana::text_width(vana::BODY, end);
    vana::form(100 + w, 100, 0, 0);
}
#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, _width: i32, _height: i32) {}
