//! Interrupt-fed UART2 receiver for the installed companion controller.

use core::cell::RefCell;

use critical_section::Mutex;
use cycling_os::uart_ring::LossRing;
use esp_hal::{
    Blocking,
    peripherals::{GPIO41, GPIO42, UART2},
    uart::{Config, RxConfig, RxError, Uart, UartInterrupt},
};

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
}

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
}));

pub fn init(uart: UART2<'static>, rx: GPIO41<'static>, tx: GPIO42<'static>) -> Result<usize, ()> {
    let config = Config::default()
        .with_baudrate(115200)
        .with_rx(RxConfig::default().with_fifo_full_threshold(32));
    let mut uart = Uart::new(uart, config)
        .map_err(|_| ())?
        .with_rx(rx)
        .with_tx(tx);
    const GPS_OPEN: [u8; 16] = [
        0xa5, 0x0c, 0x6f, 0xf1, 0x02, 0x10, 0xe2, 0x02, 0x07, 0x00, 0x00, 0x01, 0x00, 0x00, 0x89,
        0xe5,
    ];
    let written = uart.write(&GPS_OPEN).map_err(|_| ());
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

pub fn drain(output: &mut [u8]) -> (usize, Counters) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        let count = state.ring.drain(output);
        (count, state.counters)
    })
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
