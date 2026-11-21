//! Fills 60,000 bytes of its memory over and over.
#![no_std]
mod vana;
#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(100, 100, 0, 0);
}
#[unsafe(no_mangle)]
pub extern "C" fn draw(form: i32, _width: i32, _height: i32) {
    let mut i: u8 = form as u8;
    let base = 1024 as *mut u8;
    loop {
        unsafe { core::ptr::write_bytes(base, i, 60000) };
        let seen = unsafe { core::ptr::read_volatile(base.add(59999)) };
        i = i.wrapping_add(1);
        if seen == 0xfe && vana::metric(vana::SPEED) == 12345 { break; }
    }
}
