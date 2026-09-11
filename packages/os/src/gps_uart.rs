//! DMA-fed UART0 receiver for the C606's verified NMEA stream.

use esp_hal::{
    Blocking,
    dma::DmaRxStreamBuf,
    dma_rx_stream_buffer,
    peripherals::{DMA_CH1, GPIO0, UART0, UHCI0},
    uart::{
        Config, RxConfig, Uart,
        uhci::{self, Uhci, UhciDmaRxTransfer},
    },
};

const CAPACITY: usize = 8192;
const CHUNK: usize = 256;

/// Continuous DMA ownership for UART0. DMA_CH1 is separate from LCD DMA_CH0.
pub struct Receiver {
    transfer: Option<UhciDmaRxTransfer<'static, Blocking, DmaRxStreamBuf>>,
    dma_losses: u32,
    uart_errors: u32,
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
    let uart = Uart::new(uart, config).unwrap().with_rx(rx);
    let mut receiver = Uhci::new(uart, uhci, dma);
    receiver
        .apply_rx_config(&uhci::RxConfig::default().with_chunk_limit(CHUNK as u16))
        .unwrap();
    let (receiver, _unused_tx) = receiver.split();
    let buffer = dma_rx_stream_buffer!(CAPACITY, CHUNK);
    let transfer = receiver
        .read(buffer)
        .unwrap_or_else(|_| panic!("GPS DMA start failed"));
    Receiver {
        transfer: Some(transfer),
        dma_losses: 0,
        uart_errors: 0,
    }
}

impl Receiver {
    /// Copy currently available bytes without waiting. Hardware UART or DMA
    /// faults restart reception and make the parser discard a partial sentence.
    pub fn drain(&mut self, output: &mut [u8]) -> (usize, u32, u32) {
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

        let mut count = 0;
        while count < output.len() {
            let available = self.transfer.as_ref().unwrap().peek();
            if available.is_empty() {
                break;
            }
            let copied = available.len().min(output.len() - count);
            output[count..count + copied].copy_from_slice(&available[..copied]);
            let consumed = self.transfer.as_mut().unwrap().consume(copied);
            debug_assert_eq!(consumed, copied);
            count += copied;
        }
        (count, self.dma_losses, self.uart_errors)
    }

    fn restart(&mut self, reset_fifo: bool) {
        let (receiver, buffer) = self.transfer.take().unwrap().cancel();
        if reset_fifo {
            let uart = esp_hal::peripherals::UART0::regs();
            uart.conf0().modify(|_, w| w.rxfifo_rst().set_bit());
            uart.conf0().modify(|_, w| w.rxfifo_rst().clear_bit());
        }
        self.transfer = Some(
            receiver
                .read(buffer)
                .unwrap_or_else(|_| panic!("GPS DMA restart failed")),
        );
    }
}
