//! Bounded nonblocking terminal transport; device owns USB HAL halves.
pub struct Usb {
    rx: esp_hal::usb_serial_jtag::UsbSerialJtagRx<'static, esp_hal::Blocking>,
    tx: esp_hal::usb_serial_jtag::UsbSerialJtagTx<'static, esp_hal::Blocking>,
}
impl Usb {
    pub fn new(usb: esp_hal::usb_serial_jtag::UsbSerialJtag<'static, esp_hal::Blocking>) -> Self {
        let (rx, tx) = usb.split();
        Self { rx, tx }
    }
}
impl cycling_os::capabilities::Console for Usb {
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
