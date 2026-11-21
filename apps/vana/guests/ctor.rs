//! Spins in `__post_instantiate`, which WAMR would run while instantiating
//! it, outside any instruction limit: install refuses it.
#![no_std]
mod vana;
static mut N: u32 = 0;
#[unsafe(no_mangle)]
pub extern "C" fn __post_instantiate() {
    loop { unsafe { N = core::ptr::read_volatile(&raw const N).wrapping_add(1) }; }
}
#[unsafe(no_mangle)]
pub extern "C" fn forms() { vana::form(10, 10, 0, 0); }
#[unsafe(no_mangle)]
pub extern "C" fn draw(_form: i32, _width: i32, _height: i32) {}
