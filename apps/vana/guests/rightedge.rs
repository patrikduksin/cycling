//! Right-aligns text in a box one pixel wide, narrower than a glyph's spill.
#![no_std]
mod vana;
#[unsafe(no_mangle)]
pub extern "C" fn forms() {
    vana::form(1, vana::text_height(vana::BODY) + 4, 0, 1);
}
#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, _width: i32, _height: i32) {
    vana::text(vana::BODY, b"1", vana::text_height(vana::BODY), vana::RIGHT, 0xffff);
}
