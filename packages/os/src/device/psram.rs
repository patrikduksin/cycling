//! C606 embedded PSRAM initialization and startup integrity test.
//!
//! External RAM has its own allocator. This prevents ordinary allocations from
//! spilling into cache-backed memory when they contain atomics or must remain
//! available while the flash and PSRAM cache is disabled.

use core::{
    alloc::{GlobalAlloc, Layout},
    ptr,
    sync::atomic::{Ordering, compiler_fence},
};

use esp_alloc::{EspHeap, HeapRegion, MemoryCapability};
use esp_hal::{
    peripherals::PSRAM,
    psram::{
        FlashFreq, Psram, PsramConfig, PsramMode, PsramSize, SpiRamFreq, SpiTimingConfigCoreClock,
    },
};

pub const CAPACITY: usize = 2 * 1024 * 1024;
const WORD_BYTES: usize = core::mem::size_of::<u32>();
const TEST_PASSES: usize = 2;
const ALLOCATOR_PROBE_BYTES: usize = 64 * 1024;
const ALLOCATOR_PROBE_ALIGNMENT: usize = 64;

static EXTERNAL_HEAP: EspHeap = EspHeap::empty();

#[derive(Clone, Copy, Debug)]
struct Failure {
    offset: usize,
    expected: u32,
    actual: u32,
}

pub struct Report {
    pub capacity: usize,
    pub tested: usize,
    pub passes: usize,
    pub internal_before: usize,
    pub internal_after: usize,
    pub external_free: usize,
    pub allocator_probe: usize,
    pub allocator_alignment: usize,
}

/// Initialize the C606's in-package quad PSRAM and test every mapped word.
pub fn init(peripheral: PSRAM<'static>) -> Report {
    let internal_before = internal_free();
    let config = PsramConfig {
        mode: PsramMode::QuadSpi,
        size: PsramSize::AutoDetect,
        core_clock: Some(SpiTimingConfigCoreClock::SpiTimingConfigCoreClock160m),
        flash_frequency: FlashFreq::FlashFreq80m,
        ram_frequency: SpiRamFreq::Freq40m,
    };
    let psram = Psram::new(peripheral, config);
    let (start, capacity) = psram.raw_parts();
    if start.is_null() || capacity != CAPACITY || !(start as usize).is_multiple_of(WORD_BYTES) {
        panic!("unexpected PSRAM mapping");
    }

    if let Err(failure) = unsafe { integrity_test(start, capacity) } {
        esp_println::println!(
            "CYCLING_PSRAM failed offset={} expected={:08x} actual={:08x}",
            failure.offset,
            failure.expected,
            failure.actual
        );
        panic!("PSRAM integrity test failed");
    }

    unsafe {
        EXTERNAL_HEAP.add_region(HeapRegion::new(
            start,
            capacity,
            MemoryCapability::External.into(),
        ));
    }
    let probe_layout = Layout::from_size_align(ALLOCATOR_PROBE_BYTES, ALLOCATOR_PROBE_ALIGNMENT)
        .expect("valid PSRAM allocator probe layout");
    let probe = unsafe { allocate_external(probe_layout) };
    if probe.is_null() || !(probe as usize).is_multiple_of(ALLOCATOR_PROBE_ALIGNMENT) {
        panic!("PSRAM allocator probe failed");
    }
    unsafe {
        ptr::write_volatile(probe, 0x5a);
        ptr::write_volatile(probe.add(ALLOCATOR_PROBE_BYTES - 1), 0xa5);
    }
    if unsafe { ptr::read_volatile(probe) } != 0x5a
        || unsafe { ptr::read_volatile(probe.add(ALLOCATOR_PROBE_BYTES - 1)) } != 0xa5
    {
        panic!("PSRAM allocator probe data mismatch");
    }
    unsafe { deallocate_external(probe, probe_layout) };
    let internal_after = internal_free();
    Report {
        capacity,
        tested: capacity,
        passes: TEST_PASSES,
        internal_before,
        internal_after,
        external_free: external_free(),
        allocator_probe: ALLOCATOR_PROBE_BYTES,
        allocator_alignment: ALLOCATOR_PROBE_ALIGNMENT,
    }
}

pub fn external_free() -> usize {
    EXTERNAL_HEAP.free_caps(MemoryCapability::External.into())
}

/// Allocate PSRAM explicitly. Callers own the returned block and must preserve
/// its layout for `deallocate_external`.
pub unsafe fn allocate_external(layout: Layout) -> *mut u8 {
    unsafe { EXTERNAL_HEAP.alloc(layout) }
}

/// Return a block obtained from `allocate_external`.
pub unsafe fn deallocate_external(pointer: *mut u8, layout: Layout) {
    unsafe { EXTERNAL_HEAP.dealloc(pointer, layout) }
}

fn internal_free() -> usize {
    esp_alloc::HEAP.free_caps(MemoryCapability::Internal.into())
}

fn pattern(word: usize, inverted: bool) -> u32 {
    let value = (word as u32)
        .wrapping_mul(0x9e37_79b9)
        .rotate_left((word & 31) as u32)
        ^ 0xa5c3_6f19;
    if inverted { !value } else { value }
}

unsafe fn integrity_test(start: *mut u8, capacity: usize) -> Result<(), Failure> {
    let words = capacity / WORD_BYTES;
    let memory = start.cast::<u32>();
    for inverted in [false, true] {
        for word in 0..words {
            unsafe { ptr::write_volatile(memory.add(word), pattern(word, inverted)) };
        }
        compiler_fence(Ordering::SeqCst);
        for word in 0..words {
            let expected = pattern(word, inverted);
            let actual = unsafe { ptr::read_volatile(memory.add(word)) };
            if actual != expected {
                return Err(Failure {
                    offset: word * WORD_BYTES,
                    expected,
                    actual,
                });
            }
        }
    }

    // Exercise every data bit at addresses spread across the mapped device.
    for bit in 0..u32::BITS as usize {
        let word = bit * (words - 1) / (u32::BITS as usize - 1);
        unsafe { ptr::write_volatile(memory.add(word), 1u32 << bit) };
    }
    compiler_fence(Ordering::SeqCst);
    for bit in 0..u32::BITS as usize {
        let word = bit * (words - 1) / (u32::BITS as usize - 1);
        let expected = 1u32 << bit;
        let actual = unsafe { ptr::read_volatile(memory.add(word)) };
        if actual != expected {
            return Err(Failure {
                offset: word * WORD_BYTES,
                expected,
                actual,
            });
        }
    }
    Ok(())
}
