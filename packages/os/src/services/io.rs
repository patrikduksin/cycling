//! Continuous physical input and companion observations. No gesture, navigation
//! or injected-input policy belongs here. Single-consumer edges have a 16-entry
//! queue with explicit cancellation after loss; status is latest-value only.
use core::cell::RefCell;
use cycling_os::{
    capabilities::{self, Edge, Edges, Input, Observation},
    companion::{self, Decoder, Event, Status},
    input::Report,
};
use embassy_sync::blocking_mutex::{Mutex, raw::CriticalSectionRawMutex};
use embassy_time::{Instant, Timer};

static EDGES: Mutex<CriticalSectionRawMutex, RefCell<Edges>> =
    Mutex::new(RefCell::new(Edges::new()));
static LATEST: Mutex<CriticalSectionRawMutex, RefCell<Option<State>>> =
    Mutex::new(RefCell::new(None));

#[derive(Clone, Copy)]
struct State {
    status: Status,
    battery: Option<((u8, u16), u64)>,
    power: Option<(u8, u64)>,
    touch_available: bool,
    touch_errors: u32,
    companion_valid: u32,
    companion_bad_crc: u32,
    uart_errors: u32,
}

#[derive(Clone, Copy)]
pub struct Snapshot {
    pub battery: Observation<(u8, u16)>,
    /// Zero means charging in stock; all other values remain uninterpreted.
    pub power: Observation<u8>,
    pub button_counts: [u32; 3],
    pub touch_available: bool,
    pub touch_errors: u32,
    pub companion_valid: u32,
    pub companion_bad_crc: u32,
    pub uart_errors: u32,
    pub input_lost: u32,
}

pub fn snapshot(now: u64) -> Option<Snapshot> {
    let state = LATEST.lock(|latest| *latest.borrow());
    state.map(|state| Snapshot {
        battery: capabilities::observation(state.battery, now),
        power: capabilities::observation(state.power, now),
        button_counts: state.status.button_counts,
        touch_available: state.touch_available,
        touch_errors: state.touch_errors,
        companion_valid: state.companion_valid,
        companion_bad_crc: state.companion_bad_crc,
        uart_errors: state.uart_errors,
        input_lost: EDGES.lock(|edges| edges.borrow().lost),
    })
}

/// One input consumer drains edges. Additional clients should read snapshots.
pub fn take_edge() -> Option<Edge> {
    EDGES.lock(|edges| edges.borrow_mut().pop())
}
fn edge(now: u64, input: Input) {
    EDGES.lock(|edges| edges.borrow_mut().push(now, input));
}

#[embassy_executor::task]
pub async fn run(mut touch: crate::device::touch::Touch<'static>, touch_available: bool) {
    let mut state = State {
        status: Status::default(),
        battery: None,
        power: None,
        touch_available,
        touch_errors: 0,
        companion_valid: 0,
        companion_bad_crc: 0,
        uart_errors: 0,
    };
    let mut decoder = Decoder::default();
    let mut last_rx = Instant::now().as_millis();
    let mut last_touch = last_rx;
    let mut point = None;
    log::info!(target: "input", "acquisition started poll_ms=10 edges=16");
    loop {
        let now = Instant::now().as_millis();
        if now.saturating_sub(last_rx) > 250 {
            decoder.reset();
        }
        let mut bytes = [0u8; 256];
        let (count, counters) = crate::device::companion_uart::drain(&mut bytes);
        if count > 0 {
            last_rx = now;
        }
        let errors = counters.errors();
        if errors != state.uart_errors {
            edge(now, Input::Cancel);
            point = None;
            log::warn!(target: "input", "companion loss uart={} ring={}", errors, counters.ring_overflows);
        }
        state.uart_errors = companion::feed_batch(
            &mut decoder,
            state.uart_errors,
            errors,
            &bytes[..count],
            |event| {
                state.status.update(event);
                match event {
                    Event::Battery {
                        percent,
                        millivolts,
                    } => state.battery = Some(((percent, millivolts), now)),
                    Event::Power { status } => state.power = Some((status, now)),
                    Event::Button { button, code } => edge(now, Input::Button { button, code }),
                }
            },
        );
        state.companion_valid = decoder.valid_frames;
        state.companion_bad_crc = decoder.bad_crc;
        let was_available = state.touch_available;
        match touch.poll() {
            Ok(Report::Press(next)) => {
                state.touch_available = true;
                last_touch = now;
                if point != Some(next) {
                    edge(now, Input::Touch(next));
                }
                point = Some(next);
            }
            Ok(Report::Release) => {
                state.touch_available = true;
                if point.take().is_some() {
                    edge(now, Input::Release);
                }
            }
            Ok(Report::Invalid) => {}
            Err(_) => {
                state.touch_available = false;
                state.touch_errors = state.touch_errors.saturating_add(1);
                if point.take().is_some() {
                    edge(now, Input::Cancel);
                }
            }
        }
        if now.saturating_sub(last_touch) > 250 && point.take().is_some() {
            edge(now, Input::Cancel);
        }
        if state.touch_available != was_available {
            log::info!(target: "input", "touch available={} errors={}", state.touch_available, state.touch_errors);
        }
        LATEST.lock(|latest| *latest.borrow_mut() = Some(state));
        Timer::after_millis(10).await;
    }
}
