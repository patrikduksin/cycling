//! UART/DMA acquisition progresses independently of terminal, display and rides.
use core::cell::RefCell;
use cycling_os::positioning::{Acquisition, Snapshot};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Instant, Timer};

static LATEST: Mutex<CriticalSectionRawMutex, RefCell<Option<Acquisition>>> =
    Mutex::new(RefCell::new(None));

/// Latest state, with exact parser freshness recalculated at the read time.
/// A missing value means the acquisition task has not started yet.
pub fn snapshot(now: u64) -> Option<Snapshot> {
    LATEST.lock(|state| state.borrow().as_ref().map(|state| state.snapshot(now)))
}

#[embassy_executor::task]
pub async fn run(mut receiver: crate::device::gps_uart::Receiver) {
    let mut state = Acquisition::default();
    LATEST.lock(|latest| *latest.borrow_mut() = Some(state.clone()));
    log::info!(target: "position", "acquisition started poll_ms=10 batch=2048 state_bytes={}", core::mem::size_of::<Acquisition>());
    let mut previous = state.snapshot(Instant::now().as_millis());
    loop {
        let now = Instant::now().as_millis();
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
