//! C606 I2C touch transport. No controller firmware, reset or calibration writes.
use cycling_os::input::{self, Report};
use esp_hal::{
    Blocking,
    i2c::master::{Error, I2c},
};

pub struct Touch<'d> {
    bus: I2c<'d, Blocking>,
}

impl<'d> Touch<'d> {
    pub fn new(bus: I2c<'d, Blocking>) -> Self {
        Self { bus }
    }

    /// Stock probes this register; its four bytes do not establish an exact part ID.
    pub fn probe(&mut self) -> Result<[u8; 4], Error> {
        let mut data = [0; 4];
        self.bus.write_read(0x5au8, &[0xd0, 0x45], &mut data)?;
        Ok(data)
    }

    pub fn poll(&mut self) -> Result<Report, Error> {
        let mut data = [0; 7];
        self.bus.write_read(0x5au8, &[0xd0, 0x00], &mut data)?;
        // End-of-read handshake, also used by stock at 0x4202b898.
        self.bus.write(0x5au8, &[0xd0, 0x00, 0xab])?;
        Ok(input::decode(data))
    }
}
