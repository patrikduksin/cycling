//! UART/DMA acquisition progresses independently of terminal, display and rides.
use core::cell::RefCell;
use device_api::positioning::Snapshot;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_time::Instant;
use embassy_time::Timer;
use firmware_services::positioning::Acquisition;

static INVALIDATE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SLEEP_BOUNDARY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
pub fn sleep_boundary() {
    SLEEP_BOUNDARY.store(true, core::sync::atomic::Ordering::Release);
    LATEST.lock(|latest| {
        if let Some(state) = latest.borrow_mut().as_mut() {
            state.control_boundary();
        }
    });
    invalidate();
}
pub fn invalidate() {
    INVALIDATE.store(true, core::sync::atomic::Ordering::Release);
}

static LATEST: Mutex<CriticalSectionRawMutex, RefCell<Option<Acquisition>>> =
    Mutex::new(RefCell::new(None));

/// Latest state, with exact parser freshness recalculated at the read time.
/// A missing value means the acquisition task has not started yet.
pub fn snapshot(now: u64) -> Option<Snapshot> {
    LATEST.lock(|state| state.borrow().as_ref().map(|state| state.snapshot(now)))
}

#[embassy_executor::task]
pub async fn run(mut receiver: crate::drivers::gps_uart::Receiver) {
    let mut state = Acquisition::default();
    LATEST.lock(|latest| *latest.borrow_mut() = Some(state.clone()));
    log::info!(target: "position", "acquisition started poll_ms=10 batch=2048 state_bytes={}", core::mem::size_of::<Acquisition>());
    let mut previous = state.snapshot(Instant::now().as_millis());
    loop {
        let now = Instant::now().as_millis();
        if SLEEP_BOUNDARY.swap(false, core::sync::atomic::Ordering::AcqRel) {
            receiver.sleep_boundary();
        }
        if INVALIDATE.swap(false, core::sync::atomic::Ordering::AcqRel) {
            state.control_boundary();
            LATEST.lock(|latest| *latest.borrow_mut() = Some(state.clone()));
        }
        let mut bytes = [0u8; 2048];
        let (count, dma_losses, uart_errors) = receiver.drain(&mut bytes);
        if state.ingest(now, &bytes[..count], dma_losses, uart_errors) {
            // Parsing stays outside the lock. Copy the bounded parser state only
            // on received bytes or changed fault counters, never on idle polls.
            LATEST.lock(|latest| *latest.borrow_mut() = Some(state.clone()));
        }
        let current = state.snapshot(now);
        if (current.transport, current.gps.state) != (previous.transport, previous.gps.state) {
            log::info!(target: "position", "transport={} fix={} dma_losses={} uart_errors={}",
                current.transport.name(), current.gps.state.name(), current.gps.overflows, current.gps.uart_errors);
        }
        previous = current;
        Timer::after_millis(10).await;
    }
}
