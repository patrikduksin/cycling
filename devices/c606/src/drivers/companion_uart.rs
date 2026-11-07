//! Interrupt-fed UART2 receiver for the installed companion controller.

use core::cell::RefCell;

use crate::drivers::uart_ring::LossRing;
use critical_section::Mutex;
use esp_hal::Blocking;
use esp_hal::peripherals::GPIO41;
use esp_hal::peripherals::GPIO42;
use esp_hal::peripherals::UART2;
use esp_hal::uart::Config;
use esp_hal::uart::RxConfig;
use esp_hal::uart::RxError;
use esp_hal::uart::Uart;
use esp_hal::uart::UartInterrupt;

const CAPACITY: usize = 2048;

#[derive(Clone, Copy, Default)]
pub struct Counters {
    pub ring_overflows: u32,
    pub fifo_overflows: u32,
    pub glitches: u32,
    pub frame_errors: u32,
    pub parity_errors: u32,
    pub other_errors: u32,
}

impl Counters {
    pub const fn errors(self) -> u32 {
        self.ring_overflows
            .saturating_add(self.fifo_overflows)
            .saturating_add(self.glitches)
            .saturating_add(self.frame_errors)
            .saturating_add(self.parity_errors)
            .saturating_add(self.other_errors)
    }
}

struct State {
    uart: Option<Uart<'static, Blocking>>,
    ring: LossRing<CAPACITY, 0xa5>,
    counters: Counters,
    #[cfg(companion_update)]
    update: Update,
}

#[cfg(companion_update)]
struct Update {
    session: super::companion_update::Session,
    pending: Option<(super::companion_update::Packet, usize, u64)>,
    #[cfg(companion_trial)]
    trial: super::companion_update::Trial,
    #[cfg(companion_trial)]
    trial_image: bool,
    #[cfg(companion_trial)]
    probe_packet: bool,
}

#[cfg(companion_update)]
#[cfg(companion_trial)]
static TRIAL_IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/companion-trial.bin"));

#[cfg(companion_update)]
impl Update {
    fn exclusive(&self) -> bool {
        #[cfg(companion_trial)]
        return self.trial.exclusive();
        #[cfg(not(companion_trial))]
        return self.session.phase != super::companion_update::Phase::Ready;
    }
}

#[cfg(companion_update)]
static STOCK_IMAGE: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/companion-stock.bin"));

static STATE: Mutex<RefCell<State>> = Mutex::new(RefCell::new(State {
    uart: None,
    ring: LossRing::new(),
    counters: Counters {
        ring_overflows: 0,
        fifo_overflows: 0,
        glitches: 0,
        frame_errors: 0,
        parity_errors: 0,
        other_errors: 0,
    },
    #[cfg(companion_update)]
    update: Update {
        session: super::companion_update::Session::new(),
        pending: None,
        #[cfg(companion_trial)]
        trial: super::companion_update::Trial::new(),
        #[cfg(companion_trial)]
        trial_image: false,
        #[cfg(companion_trial)]
        probe_packet: false,
    },
}));

pub fn init(uart: UART2<'static>, rx: GPIO41<'static>, tx: GPIO42<'static>) -> Result<usize, ()> {
    let config = Config::default()
        .with_baudrate(115200)
        .with_rx(RxConfig::default().with_fifo_full_threshold(32));
    let mut uart = Uart::new(uart, config)
        .map_err(|_| ())?
        .with_rx(rx)
        .with_tx(tx);
    let written = uart
        .write(&crate::drivers::companion::gnss_power(true))
        .map_err(|_| ());
    uart.set_interrupt_handler(interrupt_handler);
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        state.uart = Some(uart);
        state
            .uart
            .as_mut()
            .unwrap()
            .listen(UartInterrupt::RxFifoFull | UartInterrupt::RxTimeout);
    });
    written
}

/// Single bounded FIFO attempt. A short write is an uncertain command failure;
/// callers invalidate the operation and do not replay it automatically.
pub fn send(frame: &[u8; 16]) -> Result<(), ()> {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        #[cfg(companion_update)]
        if state.update.exclusive() {
            return Err(());
        }
        let uart = state.uart.as_mut().ok_or(())?;
        if !uart.write_ready() {
            return Err(());
        }
        match uart.write(frame) {
            Ok(16) => Ok(()),
            _ => Err(()),
        }
    })
}

/// Exclusive ownership persists through success or failure, until the main CPU
/// is deliberately restored/rebooted. Normal traffic cannot reach the loader.
pub fn updating() -> bool {
    #[cfg(companion_update)]
    return critical_section::with(|cs| STATE.borrow_ref(cs).update.exclusive());
    #[cfg(not(companion_update))]
    false
}

#[cfg(companion_update)]
pub fn start_update() -> bool {
    #[cfg(companion_trial)]
    return false;
    #[cfg(not(companion_trial))]
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        state.uart.is_some() && state.update.session.start(STOCK_IMAGE.len())
    })
}

#[cfg(companion_trial)]
pub fn start_trial() -> bool {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        if state.uart.is_none()
            || state.update.pending.is_some()
            || state.update.session.phase != super::companion_update::Phase::Ready
            || !state.update.trial.start()
        {
            return false;
        }
        state.update.trial_image = true;
        state.update.session.start(TRIAL_IMAGE.len())
    })
}

/// The host recorded the custom identity; restore stock now.
#[cfg(companion_trial)]
pub fn acknowledge_trial() -> bool {
    critical_section::with(|cs| STATE.borrow_ref_mut(cs).update.trial.acknowledge())
}

#[cfg(companion_update)]
pub fn update_status(output: &mut impl core::fmt::Write) {
    critical_section::with(|cs| {
        let state = STATE.borrow_ref(cs);
        let update = &state.update;
        let _ = write!(
            output,
            "phase={:?} retries={} packets={} tx_pending={} image=1.902 bytes={}",
            update.session.phase,
            update.session.retries,
            update.session.count(),
            update.pending.is_some(),
            STOCK_IMAGE.len()
        );
        #[cfg(companion_trial)]
        let _ = write!(
            output,
            " trial={:?} custom_seen={} custom_at={} custom_version={}.{} stock_seen={} target={}",
            update.trial.phase,
            update.trial.custom_seen,
            update.trial.custom_at.map_or(-1, |at| at as i64),
            update.trial.custom_version[0],
            update.trial.custom_version[1],
            update.trial.stock_seen,
            if update.trial_image { "trial" } else { "stock" }
        );
    });
}

#[cfg(companion_update)]
pub fn update_reply(frame: &[u8], _now: u64) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        if state.update.pending.is_none() {
            #[cfg(companion_trial)]
            state.update.trial.reply(frame, _now);
            state.update.session.reply(frame);
        }
    });
}

#[cfg(companion_update)]
pub fn update_tick(now: u64) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        let State { uart, update, .. } = &mut *state;
        #[cfg(companion_trial)]
        if update.pending.is_none() {
            use super::companion_update::{Packet, Session, TrialAction};
            match update.trial.tick(now, update.session.phase) {
                Some(TrialAction::Restore) => {
                    update.trial_image = false;
                    update.probe_packet = false;
                    update.session = Session::new();
                    let _ = update.session.start(STOCK_IMAGE.len());
                }
                Some(TrialAction::Probe) => {
                    update.probe_packet = true;
                    update.pending = Some((Packet::identity(), 0, now));
                }
                None => {}
            }
        }
        if update.pending.is_none() && update.session.due(now) {
            #[cfg(companion_trial)]
            let image = if update.trial_image {
                TRIAL_IMAGE
            } else {
                STOCK_IMAGE
            };
            #[cfg(not(companion_trial))]
            let image = STOCK_IMAGE;
            update.pending = update.session.packet(image).map(|p| (p, 0, now));
            #[cfg(companion_trial)]
            {
                update.probe_packet = false;
            }
        }
        if let Some((packet, offset, began)) = &mut update.pending {
            let Some(uart) = uart.as_mut() else {
                update.session.fail();
                update.pending = None;
                return;
            };
            // HAL write is partial; never wait inside the critical section for
            // the 141-byte packet to fit the 128-byte FIFO.
            if now.saturating_sub(*began) >= 1000 {
                update.session.fail();
                update.pending = None;
            } else if uart.write_ready() {
                match uart.write(&packet.as_bytes()[*offset..]) {
                    Ok(n) if n > 0 => {
                        *offset += n;
                        if *offset == packet.as_bytes().len() {
                            update.pending = None;
                            #[cfg(companion_trial)]
                            if update.probe_packet {
                                update.trial.probe_sent(now);
                                update.probe_packet = false;
                            }
                        }
                    }
                    _ => {
                        update.session.fail();
                        update.pending = None;
                    }
                }
            }
        }
    });
}

pub fn drain(output: &mut [u8]) -> (usize, Counters) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        let count = state.ring.drain(output);
        (count, state.counters)
    })
}

/// Drop bytes straddling a sleep boundary. The acquisition owner separately
/// resets its decoder and observations before accepting post-wake reports.
pub fn sleep_boundary() {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        state.ring.discard_partial();
        if let Some(uart) = state.uart.as_mut() {
            let mut bytes = [0u8; 128];
            for _ in 0..16 {
                if matches!(uart.read_buffered(&mut bytes), Ok(0)) {
                    break;
                }
            }
            let pending = uart.interrupts();
            uart.clear_interrupts(pending);
        }
    });
}

#[esp_hal::handler(priority = esp_hal::interrupt::Priority::Priority3)]
fn interrupt_handler() {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        let mut input = [0u8; 128];
        for _ in 0..16 {
            let result = match state.uart.as_mut() {
                Some(uart) => uart.read_buffered(&mut input),
                None => return,
            };
            match result {
                Ok(0) => break,
                Ok(count) => {
                    for &byte in &input[..count] {
                        if !state.ring.push(byte) {
                            state.counters.ring_overflows =
                                state.counters.ring_overflows.saturating_add(1);
                        }
                    }
                }
                Err(error) => {
                    match error {
                        RxError::FifoOverflowed => {
                            state.counters.fifo_overflows =
                                state.counters.fifo_overflows.saturating_add(1)
                        }
                        RxError::GlitchOccurred => {
                            state.counters.glitches = state.counters.glitches.saturating_add(1)
                        }
                        RxError::FrameFormatViolated => {
                            state.counters.frame_errors =
                                state.counters.frame_errors.saturating_add(1)
                        }
                        RxError::ParityMismatch => {
                            state.counters.parity_errors =
                                state.counters.parity_errors.saturating_add(1)
                        }
                        _ => {
                            state.counters.other_errors =
                                state.counters.other_errors.saturating_add(1)
                        }
                    }
                    state.ring.discard_partial();
                }
            }
        }
        if let Some(uart) = state.uart.as_mut() {
            let pending = uart.interrupts();
            uart.clear_interrupts(pending);
        }
    });
}
