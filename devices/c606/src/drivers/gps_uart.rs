//! DMA-fed UART0 receiver for the C606's verified NMEA stream.

use embassy_time::Duration;
use embassy_time::Instant;
use esp_hal::Blocking;
use esp_hal::dma::DmaRxStreamBuf;
use esp_hal::dma_rx_stream_buffer;
use esp_hal::peripherals::DMA_CH1;
use esp_hal::peripherals::GPIO0;
use esp_hal::peripherals::UART0;
use esp_hal::peripherals::UHCI0;
use esp_hal::uart::Config;
use esp_hal::uart::RxConfig;
use esp_hal::uart::Uart;
use esp_hal::uart::uhci;
use esp_hal::uart::uhci::Uhci;
use esp_hal::uart::uhci::UhciDmaRxTransfer;
use esp_hal::uart::uhci::UhciRx;

const CAPACITY: usize = 8192;
const CHUNK: usize = 256;
const RETRY_MIN: Duration = Duration::from_millis(100);
const RETRY_MAX: Duration = Duration::from_secs(5);

/// Either the DMA owns the buffer through a running transfer, or reception is
/// stopped and the driver holds both halves until a restart succeeds.
enum Transport {
    Running(UhciDmaRxTransfer<'static, Blocking, DmaRxStreamBuf>),
    Stopped {
        receiver: UhciRx<'static, Blocking>,
        buffer: DmaRxStreamBuf,
        retry_at: Instant,
        backoff: Duration,
    },
}

/// Continuous DMA ownership for UART0. DMA_CH1 is separate from LCD DMA_CH0.
///
/// A failed DMA (re)start does not panic: reception stops, the failure counts
/// as a DMA loss so positioning reports a failed transport, and the restart is
/// retried with exponential backoff from `drain`.
pub struct Receiver {
    /// Always Some outside `start`, which moves the transport by value.
    transport: Option<Transport>,
    dma_losses: u32,
    uart_errors: u32,
    restart_failures: u32,
}

pub fn init(
    uart: UART0<'static>,
    rx: GPIO0<'static>,
    uhci: UHCI0<'static>,
    dma: DMA_CH1<'static>,
) -> Receiver {
    let config = Config::default()
        .with_baudrate(921600)
        .with_rx(RxConfig::default().with_fifo_full_threshold(32));
    let uart = Uart::new(uart, config)
        .expect("constant UART0 configuration is valid")
        .with_rx(rx);
    let mut uhci = Uhci::new(uart, uhci, dma);
    uhci.apply_rx_config(&uhci::RxConfig::default().with_chunk_limit(CHUNK as u16))
        .expect("constant UHCI chunk limit is valid");
    let (receiver, _unused_tx) = uhci.split();
    let buffer = dma_rx_stream_buffer!(CAPACITY, CHUNK);
    let mut result = Receiver {
        transport: None,
        dma_losses: 0,
        uart_errors: 0,
        restart_failures: 0,
    };
    result.start(receiver, buffer, RETRY_MIN);
    result
}

impl Receiver {
    pub fn sleep_boundary(&mut self) {
        self.restart(true);
    }

    /// Copy currently available bytes without waiting. Hardware UART or DMA
    /// faults restart reception and make the parser discard a partial sentence.
    pub fn drain(&mut self, output: &mut [u8]) -> (usize, u32, u32) {
        if let Some(Transport::Stopped { retry_at, .. }) = &self.transport {
            if Instant::now() >= *retry_at {
                self.restart(true);
            }
            return (0, self.dma_losses, self.uart_errors);
        }

        let uart = esp_hal::peripherals::UART0::regs();
        let uart_raw = uart.int_raw().read();
        let uart_error = uart_raw.rxfifo_ovf().bit_is_set()
            || uart_raw.glitch_det().bit_is_set()
            || uart_raw.frm_err().bit_is_set()
            || uart_raw.parity_err().bit_is_set();
        if uart_error {
            self.uart_errors = self.uart_errors.saturating_add(1);
            uart.int_clr().write(|w| {
                w.rxfifo_ovf().clear_bit_by_one();
                w.glitch_det().clear_bit_by_one();
                w.frm_err().clear_bit_by_one();
                w.parity_err().clear_bit_by_one()
            });
        }

        // SAFETY: DMA::ptr() is the fixed GDMA register block. This receiver
        // owns DMA_CH1 for its lifetime, and only channel 1 registers are
        // accessed; other channels belong to their own drivers.
        let dma = unsafe { &*esp32s3::DMA::ptr() };
        let dma_raw = dma.ch(1).in_int().raw().read();
        let dma_error = dma_raw.in_dscr_empty().bit_is_set()
            || dma_raw.in_dscr_err().bit_is_set()
            || dma_raw.in_err_eof().bit_is_set();
        if uart_error || dma_error {
            if dma_error {
                self.dma_losses = self.dma_losses.saturating_add(1);
            }
            dma.ch(1).in_int().clr().write(|w| {
                w.in_dscr_empty().clear_bit_by_one();
                w.in_dscr_err().clear_bit_by_one();
                w.in_err_eof().clear_bit_by_one()
            });
            self.restart(uart_raw.rxfifo_ovf().bit_is_set());
            return (0, self.dma_losses, self.uart_errors);
        }

        let Some(Transport::Running(transfer)) = self.transport.as_mut() else {
            return (0, self.dma_losses, self.uart_errors);
        };
        let mut count = 0;
        while count < output.len() {
            let available = transfer.peek();
            if available.is_empty() {
                break;
            }
            let copied = available.len().min(output.len() - count);
            output[count..count + copied].copy_from_slice(&available[..copied]);
            let consumed = transfer.consume(copied);
            debug_assert_eq!(consumed, copied);
            count += copied;
        }
        (count, self.dma_losses, self.uart_errors)
    }

    fn restart(&mut self, reset_fifo: bool) {
        let (receiver, buffer, backoff) = match self.transport.take() {
            // cancel() stops the channel before returning the buffer.
            Some(Transport::Running(transfer)) => {
                let (receiver, buffer) = transfer.cancel();
                (receiver, buffer, RETRY_MIN)
            }
            Some(Transport::Stopped {
                receiver,
                buffer,
                backoff,
                ..
            }) => {
                // drain() does not inspect errors while stopped, and the failed
                // start left its own DMA error bits. Clear the stale bits so
                // the first drain after a successful restart sees only new ones.
                clear_rx_errors();
                (receiver, buffer, backoff)
            }
            None => return,
        };
        if reset_fifo {
            let uart = esp_hal::peripherals::UART0::regs();
            uart.conf0().modify(|_, w| w.rxfifo_rst().set_bit());
            uart.conf0().modify(|_, w| w.rxfifo_rst().clear_bit());
        }
        self.start(receiver, buffer, backoff);
    }

    fn start(
        &mut self,
        receiver: UhciRx<'static, Blocking>,
        buffer: DmaRxStreamBuf,
        backoff: Duration,
    ) {
        self.transport = Some(match receiver.read(buffer) {
            Ok(transfer) => Transport::Running(transfer),
            Err((error, receiver, buffer)) => {
                // esp-hal 1.1.2 starts the RX link before it reports a
                // descriptor error, so stop it explicitly: the buffer is back in
                // our ownership and the DMA must no longer write into it.
                stop_rx_channel();
                // Report a loss and retry later instead of halting.
                self.dma_losses = self.dma_losses.saturating_add(1);
                self.restart_failures = self.restart_failures.saturating_add(1);
                log::warn!(target: "position",
                    "gps_dma_start_failed error={:?} failures={} retry_ms={}",
                    error, self.restart_failures, backoff.as_millis());
                Transport::Stopped {
                    receiver,
                    buffer,
                    retry_at: Instant::now() + backoff,
                    backoff: (backoff * 2).min(RETRY_MAX),
                }
            }
        });
    }
}

/// Clear the UART0 RX error and DMA channel 1 RX error interrupt bits that
/// `drain` inspects, using the same write-1-to-clear writes as its error path.
fn clear_rx_errors() {
    esp_hal::peripherals::UART0::regs().int_clr().write(|w| {
        w.rxfifo_ovf().clear_bit_by_one();
        w.glitch_det().clear_bit_by_one();
        w.frm_err().clear_bit_by_one();
        w.parity_err().clear_bit_by_one()
    });
    // SAFETY: DMA::ptr() is the fixed GDMA register block. The Receiver owns
    // DMA_CH1, and only channel 1's write-1-to-clear interrupt bits are written.
    let dma = unsafe { &*esp32s3::DMA::ptr() };
    dma.ch(1).in_int().clr().write(|w| {
        w.in_dscr_empty().clear_bit_by_one();
        w.in_dscr_err().clear_bit_by_one();
        w.in_err_eof().clear_bit_by_one()
    });
}

/// Stop DMA channel 1's RX link, as esp-hal's own `stop_transfer` does.
fn stop_rx_channel() {
    // SAFETY: DMA::ptr() is the fixed GDMA register block. The Receiver owns
    // DMA_CH1 and the HAL handle for it is idle (its read() just failed), so
    // setting channel 1's INLINK_STOP cannot disturb another driver.
    let dma = unsafe { &*esp32s3::DMA::ptr() };
    dma.ch(1).in_link().modify(|_, w| w.inlink_stop().set_bit());
}
