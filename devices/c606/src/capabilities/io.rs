//! Continuous physical input and companion observations. No gesture, navigation
//! or injected-input policy belongs here. Single-consumer edges have a 16-entry
//! queue with explicit cancellation after loss; status is latest-value only.
//!
//! The task runs when the companion's receive interrupt fires, when another
//! task queues companion work (`wake`), and at the earliest deadline of that
//! work or of the next touch read while the panel is lit and open.
use core::cell::RefCell;
use core::sync::atomic::AtomicBool;
use core::sync::atomic::Ordering;

use device_api::input::Edge;
use device_api::input::Input;
use firmware_services::input::Edges;

use crate::drivers::companion::Decoder;
use crate::drivers::companion::Event;
use crate::drivers::companion::Status;
use crate::drivers::input::Report;
use embassy_sync::blocking_mutex::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Instant;
use embassy_time::Timer;
use firmware_services::input::{Poll, TouchContact};

/// With nothing due, the task still runs this often: the bound on anything
/// a `wake` misses.
const IDLE_MS: u64 = 1_000;
/// A deadline its tick did not move is retried at this pace, never in a spin.
const RETRY_MS: u64 = 10;

static EDGES: Mutex<CriticalSectionRawMutex, RefCell<Edges>> =
    Mutex::new(RefCell::new(Edges::new()));
static LATEST: Mutex<CriticalSectionRawMutex, RefCell<Option<State>>> =
    Mutex::new(RefCell::new(None));
static SLEEP_BOUNDARY: AtomicBool = AtomicBool::new(false);
/// Raised when an edge is queued and reset as a pass begins, so a rest
/// ends for an edge that pass has not taken. One queued while the pass
/// takes edges costs one early pass.
static EDGE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Run the IO task now: work for the companion is queued (a sound, a GNSS
/// stream command, an ANT request, an identity query or a wake), or the
/// panel was lit or opened to input.
pub fn wake() {
    WAKE.signal(());
}

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

pub use device_api::input::InputSnapshot as Snapshot;

pub fn snapshot(now: u64) -> Option<Snapshot> {
    let state = LATEST.lock(|latest| *latest.borrow());
    state.map(|state| Snapshot {
        battery: device_api::observation::observation(state.battery, now, 5_000),
        power: device_api::observation::observation(state.power, now, 5_000),
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
/// The edges' consumer begins a pass and takes the edges queued so far:
/// only one queued after this ends the rest after the pass.
pub fn pass_began() {
    EDGE.reset();
}
/// Wait `ms`, or less if an edge is queued first, for the edges' consumer.
pub async fn rest(ms: u64) {
    embassy_futures::select::select(Timer::after_millis(ms), EDGE.wait()).await;
}
/// Queue an edge; returns whether the queue overflowed and sent a Cancel,
/// so the touch contact in progress must be cancelled too.
#[must_use]
fn edge(now: u64, input: Input) -> bool {
    if super::power::ACCESS.closed() {
        return false;
    }
    EDGES.lock(|edges| queue(&mut edges.borrow_mut(), now, input))
}
pub fn power_boundary(now: u64) {
    EDGES.lock(|edges| {
        let mut edges = edges.borrow_mut();
        while edges.pop().is_some() {}
        // An empty queue cannot overflow.
        let _ = queue(&mut edges, now, Input::Cancel);
    });
}
/// Push `input` and wake a resting consumer if the queue took it; a touch
/// point of a cancelled contact is dropped and wakes nothing.
fn queue(edges: &mut Edges, now: u64, input: Input) -> bool {
    let before = edges.pushed();
    let overflowed = edges.push(now, input);
    if edges.pushed() != before {
        EDGE.signal(());
    }
    overflowed
}

/// Invalidate cached observations across intentional receive loss, including
/// configurations where MCU uptime does not count the sleep interval.
pub fn sleep_boundary(now: u64) {
    crate::drivers::companion_uart::sleep_boundary();
    SLEEP_BOUNDARY.store(true, Ordering::Release);
    LATEST.lock(|latest| {
        if let Some(state) = latest.borrow_mut().as_mut() {
            state.battery = None;
            state.power = None;
        }
    });
    super::sensors::loss();
    // ANT has already confirmed channel closure, which invalidates its samples.
    // A deliberate boundary must preserve that closure and reconnect identity.
    super::positioning::sleep_boundary();
    power_boundary(now);
    wake();
}

#[embassy_executor::task]
pub async fn run(mut touch: crate::drivers::touch::Touch<'static>, touch_available: bool) {
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
    let mut contact = TouchContact::default();
    // When to read the panel next; `None` while it is unread.
    let mut touch_at: Option<u64> = None;
    let mut hold = crate::drivers::hold_reboot::Hold::default();
    log::info!(target: "input", "acquisition started touch_ms={}/{} idle_ms={} edges=16",
        firmware_services::input::TOUCH_ACTIVE_POLL_MS, firmware_services::input::TOUCH_IDLE_POLL_MS, IDLE_MS);
    loop {
        let now = Instant::now().as_millis();
        if SLEEP_BOUNDARY.swap(false, Ordering::AcqRel) {
            decoder.reset();
            state.battery = None;
            state.power = None;
            // `power_boundary` sent the Cancel.
            contact.cancel(now);
            last_rx = now;
        }
        if now.saturating_sub(last_rx) > 250 {
            decoder.reset();
        }
        let mut bytes = [0u8; 256];
        loop {
            let (count, counters) = crate::drivers::companion_uart::drain(&mut bytes);
            if count > 0 {
                last_rx = now;
            }
            let errors = counters.errors();
            if errors != state.uart_errors {
                let _ = edge(now, Input::Cancel);
                contact.cancel(now);
                log::warn!(target: "input", "companion loss uart={} ring={}", errors, counters.ring_overflows);
            }
            state.uart_errors = crate::drivers::companion::feed_frames(
                &mut decoder,
                state.uart_errors,
                errors,
                &bytes[..count],
                |frame| {
                    let Ok(frame) = frame else {
                        super::ant::loss(now);
                        super::sensors::loss();
                        let _ = edge(now, Input::Cancel);
                        contact.cancel(now);
                        return;
                    };
                    if let Some(payload) = frame.identity_reply() {
                        super::sensors::identity_reply(payload, now);
                    }
                    if let Some((group, payload)) = frame.report_payload() {
                        super::sensors::receive(group, payload, now);
                    }
                    if let Some((group, payload)) = frame.report() {
                        super::ant::receive(group, payload, now);
                    }
                    if let Some((group, payload)) = frame.reply() {
                        super::ant::reply(group, payload, now);
                    }
                    if let Some(event) = frame.input() {
                        state.status.update(event);
                        match event {
                            Event::Battery {
                                percent,
                                millivolts,
                            } => state.battery = Some(((percent, millivolts), now)),
                            Event::Power { status } => state.power = Some((status, now)),
                            Event::Button { button, code } => {
                                log::info!(target: "button", "id={} code={}", button.index(), code);
                                if hold.report(button, code, now) {
                                    log::warn!(target: "input", "reboot hold");
                                    esp_hal::system::software_reset();
                                }
                                if edge(now, Input::Button { button, code }) {
                                    contact.cancel(now);
                                }
                            }
                        }
                    }
                },
            );
            if count < bytes.len() {
                break;
            }
        }
        // Startup compares fault counts between ticks: give it this tick's,
        // counted together with the framing losses decoded above.
        let due = [
            super::sensors::tick(now, state.uart_errors.saturating_add(decoder.bad_crc)),
            super::ant::tick(now),
            super::sound::tick(now),
            super::position_control::tick(now),
        ];
        state.companion_valid = decoder.valid_frames;
        state.companion_bad_crc = decoder.bad_crc;
        let was_available = state.touch_available;
        // Dimming never reaches 0%, so a touch still wakes a dimmed screen.
        let readable = super::power::lit() > 0 && !super::power::ACCESS.closed();
        let poll = if !readable {
            touch_at = None;
            Some(Poll::Unread)
        } else if touch_at.is_none_or(|at| now >= at) {
            if touch_at.is_none() {
                // Unread until now, so a finger held through the dark does not land again.
                let _ = contact.poll(Poll::Unread, now);
            }
            Some(match super::power::ACCESS.enter() {
                Ok(_lease) => match touch.poll().await {
                    Ok(report) => {
                        if report != Report::Invalid {
                            state.touch_available = true;
                        }
                        Poll::from(report)
                    }
                    Err(_) => {
                        state.touch_available = false;
                        state.touch_errors = state.touch_errors.saturating_add(1);
                        Poll::Failed
                    }
                },
                Err(_) => Poll::Unread,
            })
        } else {
            None
        };
        if let Some(poll) = poll {
            // See `Input::Cancel` for the contract these edges keep. An overflow
            // cancels the contact, as the queue's Cancel told the consumer.
            for input in contact.poll(poll, now).into_iter().flatten() {
                if edge(now, input) {
                    contact.cancel(now);
                }
            }
            touch_at = (poll != Poll::Unread).then(|| contact.next_read(now));
        }
        if state.touch_available != was_available {
            log::info!(target: "input", "touch available={} errors={}", state.touch_available, state.touch_errors);
        }
        LATEST.lock(|latest| *latest.borrow_mut() = Some(state));
        let end = Instant::now().as_millis();
        let until = match due.into_iter().flatten().chain(touch_at).min() {
            Some(due) if due <= end => end + RETRY_MS,
            Some(due) => due.min(end + IDLE_MS),
            None => end + IDLE_MS,
        };
        embassy_futures::select::select3(
            WAKE.wait(),
            crate::drivers::companion_uart::received(),
            Timer::at(Instant::from_millis(until)),
        )
        .await;
    }
}
