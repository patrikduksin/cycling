//! Bounded nonblocking terminal transport; device owns USB HAL halves.
pub struct Usb {
    rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, esp_hal::Blocking>,
    tx: esp_hal::usb_serial_jtag::UsbSerialJtagTx<'static, esp_hal::Blocking>,
}
impl Usb {
    #[cfg(feature = "bulk-maintenance")]
    pub fn tx_ready(&self) -> bool {
        // Observe completion without writing WR_DONE again for an empty FIFO.
        unsafe { &*esp32s3::USB_DEVICE::ptr() }
            .ep1_conf()
            .read()
            .serial_in_ep_data_free()
            .bit_is_set()
    }

    pub fn new(usb: esp_hal::usb_serial_jtag::UsbSerialJtag<'static, esp_hal::Blocking>) -> Self {
        let (rx, tx) = usb.split();
        Self { rx, tx }
    }
}
impl device_api::console::Console for Usb {
    fn read(&mut self) -> Option<u8> {
        self.rx.read_byte().ok()
    }
    fn write(&mut self, byte: u8) -> bool {
        self.tx.write_byte_nb(byte).is_ok()
    }
    fn flush(&mut self) {
        let _ = self.tx.flush_tx_nb();
    }
}
