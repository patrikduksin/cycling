//! UART/DMA acquisition progresses independently of terminal, display and rides.
//! The DMA fills its buffer by itself; the task drains it on the cadence
//! `firmware_services::positioning::drain_rest_ms` sets, and at once after a
//! stream boundary.
use core::cell::RefCell;
use device_api::positioning::Snapshot;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Instant;
use embassy_time::Timer;
use firmware_services::positioning::Acquisition;

static INVALIDATE: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static SLEEP_BOUNDARY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
/// Raised with a boundary, so the task handles it now.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
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
    WAKE.signal(());
}

static LATEST: Mutex<CriticalSectionRawMutex, RefCell<Option<Acquisition>>> =
    Mutex::new(RefCell::new(None));

/// Latest state, with exact parser freshness recalculated at the read time.
/// A missing value means the acquisition task has not started yet.
pub fn snapshot(now: u64) -> Option<Snapshot> {
    LATEST.lock(|state| state.borrow().as_ref().map(|state| state.snapshot(now)))
}

/// The parser's latest framing excerpts, as last published.
pub fn diagnostics() -> Option<device_api::gps::Diagnostics> {
    LATEST.lock(|state| state.borrow().as_ref().map(Acquisition::diagnostics))
}

#[embassy_executor::task]
pub async fn run(mut receiver: crate::drivers::gps_uart::Receiver) {
    let mut state = Acquisition::default();
    let published = state.clone();
    LATEST.lock(|latest| *latest.borrow_mut() = Some(published));
    log::info!(target: "position", "acquisition started drain_ms={}/{}/{} batch=2048 state_bytes={}",
        firmware_services::positioning::BURST_DRAIN_MS, firmware_services::positioning::LIVE_DRAIN_MS,
        firmware_services::positioning::QUIET_DRAIN_MS, core::mem::size_of::<Acquisition>());
    let mut previous = state.snapshot(Instant::now().as_millis());
    // When the receiver last sent a byte or its stream last paused or resumed.
    let mut heard = Instant::now().as_millis();
    loop {
        let now = Instant::now().as_millis();
        if SLEEP_BOUNDARY.swap(false, core::sync::atomic::Ordering::AcqRel) {
            receiver.sleep_boundary();
        }
        if INVALIDATE.swap(false, core::sync::atomic::Ordering::AcqRel) {
            state.control_boundary();
            let published = state.clone();
            LATEST.lock(|latest| *latest.borrow_mut() = Some(published));
            heard = now;
        }
        let mut bytes = [0u8; 2048];
        let (mut drained, mut changed) = (0, false);
        loop {
            let (count, dma_losses, uart_errors) = receiver.drain(&mut bytes);
            changed |= state.ingest(now, &bytes[..count], dma_losses, uart_errors);
            drained += count;
            if count < bytes.len() {
                break;
            }
        }
        if changed {
            // Parsing stays outside the lock. Copy the bounded parser state only
            // on received bytes or changed fault counters, never on idle drains.
            let published = state.clone();
            LATEST.lock(|latest| *latest.borrow_mut() = Some(published));
        }
        if drained > 0 {
            heard = now;
        }
        let current = state.snapshot(now);
        if (current.transport, current.gps.state) != (previous.transport, previous.gps.state) {
            log::info!(target: "position", "transport={} fix={} dma_losses={} uart_errors={}",
                current.transport.name(), current.gps.state.name(), current.gps.overflows, current.gps.uart_errors);
        }
        previous = current;
        let rest =
            firmware_services::positioning::drain_rest_ms(drained, now.saturating_sub(heard));
        embassy_futures::select::select(WAKE.wait(), Timer::after_millis(rest)).await;
    }
}
