//! Interrupt-fed UART0 receiver for the C606's verified NMEA stream.

use core::cell::RefCell;
use critical_section::Mutex;
use esp_hal::{
    Blocking,
    peripherals::{GPIO0, UART0},
    uart::{Config, RxConfig, Uart, UartInterrupt},
};

const CAPACITY: usize = 8192;

struct State {
    receiver: Option<Uart<'static, Blocking>>,
    bytes: [u8; CAPACITY],
    read: usize,
    write: usize,
    overflow: u32,
    uart_errors: u32,
    discarding: bool,
}

static STATE: Mutex<RefCell<State>> = Mutex::new(RefCell::new(State {
    receiver: None,
    bytes: [0; CAPACITY],
    read: 0,
    write: 0,
    overflow: 0,
    uart_errors: 0,
    discarding: false,
}));

pub fn init(uart: UART0<'static>, rx: GPIO0<'static>) {
    let config = Config::default()
        .with_baudrate(921600)
        .with_rx(RxConfig::default().with_fifo_full_threshold(32));
    let mut receiver = Uart::new(uart, config).unwrap().with_rx(rx);
    receiver.set_interrupt_handler(interrupt_handler);
    critical_section::with(|cs| STATE.borrow_ref_mut(cs).receiver = Some(receiver));
    critical_section::with(|cs| {
        STATE
            .borrow_ref_mut(cs)
            .receiver
            .as_mut()
            .unwrap()
            .listen(UartInterrupt::RxFifoFull | UartInterrupt::RxTimeout)
    });
}

pub fn drain(output: &mut [u8]) -> (usize, u32, u32) {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        let mut count = 0;
        while count < output.len() && state.read != state.write {
            output[count] = state.bytes[state.read];
            state.read = (state.read + 1) % CAPACITY;
            count += 1;
        }
        (count, state.overflow, state.uart_errors)
    })
}

#[esp_hal::handler(priority = esp_hal::interrupt::Priority::Priority3)]
fn interrupt_handler() {
    critical_section::with(|cs| {
        let mut state = STATE.borrow_ref_mut(cs);
        let mut input = [0u8; 128];
        for _ in 0..64 {
            let result = match state.receiver.as_mut() {
                Some(receiver) => receiver.read_buffered(&mut input),
                None => return,
            };
            match result {
                Ok(0) => break,
                Ok(count) => {
                    for &byte in &input[..count] {
                        if state.discarding {
                            if byte != b'$' {
                                continue;
                            }
                            state.discarding = false;
                        }
                        let next = (state.write + 1) % CAPACITY;
                        if next == state.read {
                            state.overflow = state.overflow.saturating_add(1);
                            state.read = state.write;
                            state.discarding = true;
                        } else {
                            let write = state.write;
                            state.bytes[write] = byte;
                            state.write = next;
                        }
                    }
                }
                Err(_) => {
                    state.uart_errors = state.uart_errors.saturating_add(1);
                    state.read = state.write;
                    state.discarding = true;
                }
            }
        }
        if let Some(receiver) = state.receiver.as_mut() {
            let pending = receiver.interrupts();
            receiver.clear_interrupts(pending);
        }
    });
}
