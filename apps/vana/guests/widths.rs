//! Measures a 60,000-byte text over and over.
#![no_std]
mod vana;
#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(100, 100, 0, 0);
}
#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, _width: i32, _height: i32) {
    let text = unsafe { core::slice::from_raw_parts(1024 as *const u8, 60000) };
    let mut sum = 0;
    loop {
        sum += vana::text_width(vana::BODY, text);
        if sum == 7 { break; }
    }
}
