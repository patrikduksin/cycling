use c606_firmware::drivers::external::Owned;
use core::alloc::Layout;
use core::sync::atomic::{AtomicUsize, Ordering};
use firmware_services::trace::Event;
use firmware_services::trace::frozen::{Record, Recorder};
use firmware_services::trace::spool::Spool;

static HELD: AtomicUsize = AtomicUsize::new(0);

/// # Safety
///
/// As `std::alloc::dealloc`.
unsafe fn free(pointer: *mut u8, layout: Layout) {
    HELD.fetch_sub(layout.size(), Ordering::SeqCst);
    // SAFETY: `pointer` came from `zeroed` below with this layout.
    unsafe { std::alloc::dealloc(pointer, layout) }
}

fn zeroed(len: usize) -> Owned {
    let layout = Layout::from_size_align(len, 4).unwrap();
    // SAFETY: `len` is not zero.
    let pointer = unsafe { std::alloc::alloc_zeroed(layout) };
    HELD.fetch_add(len, Ordering::SeqCst);
    // SAFETY: `pointer` holds `len` zeroed bytes from the global allocator
    // with `layout`, which `free` gives back.
    unsafe { Owned::from_raw(pointer, layout, free) }.unwrap()
}

#[test]
fn a_recording_that_stops_gives_its_spool_back() {
    let mut recording = Some(Recorder::new(zeroed(4096), Spool::new(zeroed(256 * 1024))));
    assert_eq!(HELD.load(Ordering::SeqCst), 260 * 1024);
    for now_ms in 0..2_000 {
        recording.record(&Event::Pass { now_ms });
    }
    recording.as_mut().unwrap().flush();
    assert!(
        !recording.as_ref().unwrap().sink().is_empty(),
        "blocks spooled"
    );
    recording = None;
    assert!(recording.is_none());
    assert_eq!(HELD.load(Ordering::SeqCst), 0, "PSRAM given back");
}
