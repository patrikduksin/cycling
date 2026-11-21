#![no_std]

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}

#[path = "../src/compose/script/workload.rs"]
mod workload;

#[link(wasm_import_module = "bench")]
unsafe extern "C" {
    fn sample(i: i32) -> i32;
    fn ring(cx: i32, cy: i32, r: i32, frac_q16: i32, color: i32);
    fn text(digits: u32, len: i32, x: i32, y: i32, color: i32);
    fn line_to(x: i32, y: i32);
}

struct Imports;
impl workload::Host for Imports {
    fn sample(&mut self, i: i32) -> i32 { unsafe { sample(i) } }
    fn ring(&mut self, cx: i32, cy: i32, r: i32, f: i32, c: i32) { unsafe { ring(cx, cy, r, f, c) } }
    fn text(&mut self, d: u32, l: i32, x: i32, y: i32, c: i32) { unsafe { text(d, l, x, y, c) } }
    fn line_to(&mut self, x: i32, y: i32) { unsafe { line_to(x, y) } }
}

#[unsafe(no_mangle)]
pub extern "C" fn countdown(left_ms: i32, total_ms: i32) {
    workload::countdown(&mut Imports, left_ms, total_ms)
}

#[unsafe(no_mangle)]
pub extern "C" fn sparkline(n: i32, width: i32, height: i32) {
    workload::sparkline(&mut Imports, n, width, height)
}

/// A tile stuck in a loop: the host's work limit must stop it.
#[unsafe(no_mangle)]
pub extern "C" fn spin(seed: i32) -> i32 {
    let mut x = seed;
    loop {
        x = x.wrapping_mul(1103515245).wrapping_add(12345);
        if x == 7 {
            return x;
        }
    }
}
