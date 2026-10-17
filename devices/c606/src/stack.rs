//! Main stack high-water mark. Startup paints the unused stack, and STATUS
//! counts the words still painted, so a report shows the least free stack
//! seen since boot rather than a reserved size.
use core::ptr::{addr_of, read_volatile, write_volatile};

unsafe extern "C" {
    static _stack_end_cpu0: u32;
}

const PAINT: u32 = 0x5354_4b21;
/// esp-hal watches a guard word 60 bytes above the stack's end; never write it.
const GUARD_BYTES: usize = 64;
/// With interrupts disabled, nothing runs further than this below the
/// painter's own frame.
const MARGIN: usize = 512;

fn low() -> usize {
    addr_of!(_stack_end_cpu0) as usize + GUARD_BYTES
}

/// Paint the stack below the caller. Call once, first thing in `main`.
#[inline(never)]
pub fn paint() {
    let here = 0u32;
    let high = (addr_of!(here) as usize).saturating_sub(MARGIN) & !3;
    critical_section::with(|_| {
        let mut word = low();
        while word < high {
            // SAFETY: the words between the guard and `high` belong to the
            // main stack below every live frame, and interrupts that could
            // use them are disabled until painting ends.
            unsafe { write_volatile(word as *mut u32, PAINT) };
            word += 4;
        }
    });
}

/// Bytes at the bottom of the stack that nothing has written since `paint`.
pub fn untouched() -> usize {
    let mut word = low();
    // SAFETY: reads stay within the main stack and stop at the first written
    // word, which lies below every live frame.
    while unsafe { read_volatile(word as *const u32) } == PAINT {
        word += 4;
    }
    word - low()
}
