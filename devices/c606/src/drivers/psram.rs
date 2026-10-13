//! C606 embedded PSRAM initialization and startup integrity test.
//!
//! External RAM has its own allocator. This prevents ordinary allocations from
//! spilling into cache-backed memory when they contain atomics or must remain
//! available while the flash and PSRAM cache is disabled.

use core::alloc::GlobalAlloc;
use core::alloc::Layout;
use core::ptr;
use core::sync::atomic::Ordering;
use core::sync::atomic::compiler_fence;

use esp_alloc::EspHeap;
use esp_alloc::HeapRegion;
use esp_alloc::MemoryCapability;
use esp_hal::peripherals::PSRAM;
use esp_hal::psram::FlashFreq;
use esp_hal::psram::Psram;
use esp_hal::psram::PsramConfig;
use esp_hal::psram::PsramMode;
use esp_hal::psram::PsramSize;
use esp_hal::psram::SpiRamFreq;
use esp_hal::psram::SpiTimingConfigCoreClock;

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

    // SAFETY: esp-hal mapped `capacity` bytes at the word-aligned `start`
    // (checked above), and nothing else references PSRAM before the external
    // heap is created below.
    if let Err(failure) = unsafe { integrity_test(start, capacity) } {
        esp_println::println!(
            "CYCLING_PSRAM failed offset={} expected={:08x} actual={:08x}",
            failure.offset,
            failure.expected,
            failure.actual
        );
        panic!("PSRAM integrity test failed");
    }

    // SAFETY: the mapped region is valid for the program lifetime, is added
    // exactly once (init consumes the PSRAM token) and overlaps no other heap.
    unsafe {
        EXTERNAL_HEAP.add_region(HeapRegion::new(
            start,
            capacity,
            MemoryCapability::External.into(),
        ));
    }
    let probe_layout = Layout::from_size_align(ALLOCATOR_PROBE_BYTES, ALLOCATOR_PROBE_ALIGNMENT)
        .expect("valid PSRAM allocator probe layout");
    // SAFETY: the probe layout has a non-zero size.
    let probe = unsafe { allocate_external(probe_layout) };
    if probe.is_null() || !(probe as usize).is_multiple_of(ALLOCATOR_PROBE_ALIGNMENT) {
        panic!("PSRAM allocator probe failed");
    }
    // SAFETY: probe is a live, non-null allocation of ALLOCATOR_PROBE_BYTES;
    // both offsets are inside it and no other reference to it exists.
    unsafe {
        ptr::write_volatile(probe, 0x5a);
        ptr::write_volatile(probe.add(ALLOCATOR_PROBE_BYTES - 1), 0xa5);
    }
    // SAFETY: same live allocation and offsets; both bytes were written above.
    let (first, last) = unsafe {
        (
            ptr::read_volatile(probe),
            ptr::read_volatile(probe.add(ALLOCATOR_PROBE_BYTES - 1)),
        )
    };
    if first != 0x5a || last != 0xa5 {
        panic!("PSRAM allocator probe data mismatch");
    }
    // SAFETY: probe came from allocate_external with this exact layout and is
    // not used afterwards.
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
/// its layout for `deallocate_external`. A null pointer means exhaustion.
///
/// # Safety
///
/// `layout` must have a non-zero size, as for [`GlobalAlloc::alloc`]. The
/// returned memory is uninitialized.
pub unsafe fn allocate_external(layout: Layout) -> *mut u8 {
    // SAFETY: the caller upholds GlobalAlloc::alloc's non-zero size contract.
    unsafe { EXTERNAL_HEAP.alloc(layout) }
}

/// Return a block obtained from `allocate_external`.
///
/// # Safety
///
/// `pointer` must come from `allocate_external` with this same `layout`, must
/// not have been freed already, and must not be used afterwards.
pub unsafe fn deallocate_external(pointer: *mut u8, layout: Layout) {
    // SAFETY: the caller upholds GlobalAlloc::dealloc's contract.
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

/// Write and verify every mapped word, then walk each data bit.
///
/// Limitation: accesses go through the data cache, and `compiler_fence` only
/// orders the compiler, not the cache. Each 2 MiB pass evicts all but the most
/// recently written lines of the much smaller data cache, so nearly every
/// read-back comes from PSRAM, but up to one cache's worth of the final words
/// (and the sparse bit-walk words) can be served from cache. A correct
/// write-back/invalidate needs the ROM cache routines that esp-hal 1.1.2 keeps
/// private (`soc::cache_writeback_addr`), so this test does not claim to
/// exercise those last lines on the external bus.
///
/// # Safety
///
/// `start` must be word-aligned and valid for reads and writes of `capacity`
/// bytes, with no other live references into that range.
unsafe fn integrity_test(start: *mut u8, capacity: usize) -> Result<(), Failure> {
    let words = capacity / WORD_BYTES;
    let memory = start.cast::<u32>();
    for inverted in [false, true] {
        for word in 0..words {
            // SAFETY: word < capacity / 4, inside the caller's valid range.
            unsafe { ptr::write_volatile(memory.add(word), pattern(word, inverted)) };
        }
        compiler_fence(Ordering::SeqCst);
        for word in 0..words {
            let expected = pattern(word, inverted);
            // SAFETY: word < capacity / 4, inside the caller's valid range.
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
        // SAFETY: word <= words - 1, inside the caller's valid range.
        unsafe { ptr::write_volatile(memory.add(word), 1u32 << bit) };
    }
    compiler_fence(Ordering::SeqCst);
    for bit in 0..u32::BITS as usize {
        let word = bit * (words - 1) / (u32::BITS as usize - 1);
        let expected = 1u32 << bit;
        // SAFETY: word <= words - 1, inside the caller's valid range.
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
