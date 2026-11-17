//! C606 embedded PSRAM initialization and startup integrity test.
//!
//! External RAM has its own allocator. This prevents ordinary allocations from
//! spilling into cache-backed memory when they contain atomics or must remain
//! available while the flash and PSRAM cache is disabled.
//!
//! A panic's software reset and a watchdog reset keep PSRAM's contents. The
//! trace notes where its spool ring and open block lie ([`keep`]), in RTC
//! fast memory, which those resets keep too. After one of them, [`init`]
//! leaves that span out of the integrity test and the heap, so the trace can
//! [`take_kept`] it and salvage it, then [`release`] tests it and gives it
//! to the heap. A power-on tests everything.

use core::alloc::GlobalAlloc;
use core::alloc::Layout;
use core::cell::RefCell;
use core::ops::Range;
use core::ptr;
use core::ptr::addr_of;
use core::ptr::addr_of_mut;
use core::sync::atomic::AtomicUsize;
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

static MAPPED: AtomicUsize = AtomicUsize::new(0);

/// What [`init`] kept for the trace, until it takes it.
static KEPT_SPAN: critical_section::Mutex<RefCell<Option<Kept>>> =
    critical_section::Mutex::new(RefCell::new(None));

/// Where the trace's spool ring and open block lie: their offsets and
/// lengths in PSRAM, then a check over them.
#[esp_hal::ram(unstable(rtc_fast, persistent))]
static mut KEPT: [u32; 5] = [0; 5];
const KEPT_SEAL: u32 = 0x5452_4b31;

/// A piece of PSRAM smaller than this beside a kept span is kept with it,
/// rather than made a heap region of its own.
const SMALLEST_REGION: usize = 64 * 1024;

#[derive(Clone, Copy, Debug)]
struct Failure {
    offset: usize,
    expected: u32,
    actual: u32,
}

pub struct Report {
    pub capacity: usize,
    pub tested: usize,
    /// Bytes left untested for the trace to salvage.
    pub kept: usize,
    pub passes: usize,
    pub internal_before: usize,
    pub internal_after: usize,
    pub external_free: usize,
    pub allocator_probe: usize,
    pub allocator_alignment: usize,
}

/// Initialize the C606's in-package quad PSRAM and test every mapped word;
/// after a panic or a watchdog reset (`crashed`), every word but those the
/// trace noted with [`keep`], which [`take_kept`] then hands over.
pub fn init(peripheral: PSRAM<'static>, crashed: bool) -> Report {
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
    MAPPED.store(start as usize, Ordering::Relaxed);
    let kept = crashed.then(noted).flatten().map(|(ring, open)| {
        let low = ring.start.min(open.start) / WORD_BYTES * WORD_BYTES;
        let high = ring.end.max(open.end).next_multiple_of(WORD_BYTES);
        let low = if low < SMALLEST_REGION { 0 } else { low };
        let high = if capacity - high < SMALLEST_REGION {
            capacity
        } else {
            high
        };
        Kept {
            start: start as usize,
            span: low..high,
            ring,
            open,
        }
    });
    forget();
    let span = kept.as_ref().map_or(0..0, |kept| kept.span.clone());
    let pieces = match &kept {
        Some(kept) => [0..kept.span.start, kept.span.end..capacity],
        None => [0..capacity, 0..0],
    };
    for range in pieces.into_iter().filter(|range| !range.is_empty()) {
        // SAFETY: `range` lies in the `capacity` bytes esp-hal mapped at the
        // word-aligned `start` (checked above), outside the kept span, and
        // nothing else references it before it joins the heap here.
        unsafe { test_and_add(start, range) };
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
    critical_section::with(|cs| *KEPT_SPAN.borrow_ref_mut(cs) = kept);
    Report {
        capacity,
        tested: capacity - span.len(),
        kept: span.len(),
        passes: TEST_PASSES,
        internal_before,
        internal_after,
        external_free: external_free(),
        allocator_probe: ALLOCATOR_PROBE_BYTES,
        allocator_alignment: ALLOCATOR_PROBE_ALIGNMENT,
    }
}

/// What a panic or a watchdog reset left of the trace's spool, once.
pub fn take_kept() -> Option<Kept> {
    critical_section::with(|cs| KEPT_SPAN.borrow_ref_mut(cs).take())
}

/// # Safety
///
/// `range` lies in the mapped PSRAM, word-aligned, and nothing references
/// it or has added it to a heap.
unsafe fn test_and_add(start: *mut u8, range: Range<usize>) {
    // SAFETY: the caller hands over `range` of the mapped PSRAM.
    let at = unsafe { start.add(range.start) };
    // SAFETY: as above; nothing else references these bytes.
    if let Err(failure) = unsafe { integrity_test(at, range.len()) } {
        esp_println::println!(
            "CYCLING_PSRAM failed offset={} expected={:08x} actual={:08x}",
            range.start + failure.offset,
            failure.expected,
            failure.actual
        );
        panic!("PSRAM integrity test failed");
    }
    // SAFETY: the range is valid for the program lifetime and the caller
    // promises no heap holds it; the external heap takes at most three
    // regions, and the kept span splits PSRAM into at most three.
    unsafe {
        EXTERNAL_HEAP.add_region(HeapRegion::new(
            at,
            range.len(),
            MemoryCapability::External.into(),
        ));
    }
}

/// The span of PSRAM a panic or a watchdog reset left the trace's spool
/// ring and open block in, out of the heap until [`release`].
pub struct Kept {
    start: usize,
    span: Range<usize>,
    ring: Range<usize>,
    open: Range<usize>,
}

impl Kept {
    pub fn ring_and_open(&self) -> (&[u8], &[u8]) {
        // SAFETY: both ranges lie in the kept span (`noted` checks them
        // against the capacity), which no heap holds and which only this
        // value references until `release` consumes it.
        let slice = |range: &Range<usize>| unsafe {
            core::slice::from_raw_parts((self.start + range.start) as *const u8, range.len())
        };
        (slice(&self.ring), slice(&self.open))
    }
}

/// Test the kept span and give it to the heap.
pub fn release(kept: Kept) {
    // SAFETY: the span lies in the mapped PSRAM and is word-aligned; no heap
    // holds it, and `kept`, which this consumes, was its only reference.
    unsafe { test_and_add(kept.start as *mut u8, kept.span) };
}

/// Note where the trace's spool `ring` and open block lie, for the boot
/// after a panic or a watchdog reset.
pub fn keep(ring: &[u8], open: &[u8]) {
    let mapped = MAPPED.load(Ordering::Relaxed);
    let offset = |bytes: &[u8]| (bytes.as_ptr() as usize).wrapping_sub(mapped) as u32;
    let words = [
        offset(ring),
        ring.len() as u32,
        offset(open),
        open.len() as u32,
    ];
    write_kept(words, seal(&words));
}

/// The trace gave its spool back: a crash leaves nothing to salvage.
pub fn forget() {
    write_kept([0; 4], 0);
}

fn seal(words: &[u32; 4]) -> u32 {
    let mut bytes = [0u8; 16];
    for (chunk, word) in bytes.chunks_exact_mut(4).zip(words) {
        chunk.copy_from_slice(&word.to_le_bytes());
    }
    firmware_services::checksum::crc32(&bytes) ^ KEPT_SEAL
}

fn write_kept(words: [u32; 4], seal: u32) {
    let target = addr_of_mut!(KEPT).cast::<u32>();
    // SAFETY: every offset is below 5, inside KEPT. Only the main task
    // writes it, at boot and when the trace allocates or gives back its
    // spool; the seal is cleared first and written last, so a reset
    // mid-write leaves no valid record.
    unsafe {
        ptr::write_volatile(target.add(4), 0);
        for (index, word) in words.iter().enumerate() {
            ptr::write_volatile(target.add(index), *word);
        }
        ptr::write_volatile(target.add(4), seal);
    }
}

/// What [`keep`] noted, when its seal holds and both ranges lie in PSRAM.
fn noted() -> Option<(Range<usize>, Range<usize>)> {
    let source = addr_of!(KEPT).cast::<u32>();
    let mut words = [0u32; 5];
    for (index, word) in words.iter_mut().enumerate() {
        // SAFETY: index < 5, inside KEPT; boot reads it before any writer.
        *word = unsafe { ptr::read_volatile(source.add(index)) };
    }
    let [ring, ring_len, open, open_len, sealed] = words;
    if sealed != seal(&[ring, ring_len, open, open_len]) {
        return None;
    }
    let range = |offset: u32, len: u32| {
        let start = offset as usize;
        let end = start.checked_add(len as usize)?;
        (len > 0 && end <= CAPACITY).then_some(start..end)
    };
    Some((range(ring, ring_len)?, range(open, open_len)?))
}

/// `len` zeroed bytes of PSRAM, given back when the block drops.
pub fn owned(len: usize) -> Option<super::external::Owned> {
    let layout = Layout::from_size_align(len, 4)
        .ok()
        .filter(|layout| layout.size() > 0)?;
    // SAFETY: the layout's size is not zero.
    let pointer = unsafe { allocate_external(layout) };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the allocator returned `len` writable bytes. Zeroed here,
    // nothing else references them, and `deallocate_external` takes them
    // back with `layout`.
    unsafe {
        ptr::write_bytes(pointer, 0, len);
        super::external::Owned::from_raw(pointer, layout, deallocate_external)
    }
}

/// `len` zeroed bytes of external RAM for the program's lifetime, for
/// buffers only tasks touch (never interrupts: flash writes disable the
/// cache, but they also block the executor).
pub fn zeroed(len: usize) -> Option<&'static mut [u8]> {
    let layout = Layout::from_size_align(len, 4).ok()?;
    // SAFETY: callers pass a non-zero size; the block is never freed.
    let pointer = unsafe { allocate_external(layout) };
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the allocator returned `len` writable bytes, zeroed here; the
    // block is never deallocated and this is its only reference.
    Some(unsafe {
        ptr::write_bytes(pointer, 0, len);
        core::slice::from_raw_parts_mut(pointer, len)
    })
}

/// `value` moved into external RAM for the program's lifetime. Pass a
/// constant, so a large value is copied from flash, not built on the stack.
pub fn place<T>(value: T) -> Option<&'static mut T> {
    let layout = Layout::new::<T>();
    // SAFETY: callers place non-zero-sized values; the block is never freed.
    let pointer = unsafe { allocate_external(layout) }.cast::<T>();
    if pointer.is_null() {
        return None;
    }
    // SAFETY: the block is aligned and sized for one `T`, initialized here,
    // never freed and only referenced through this exclusive borrow.
    Some(unsafe {
        pointer.write(value);
        &mut *pointer
    })
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
