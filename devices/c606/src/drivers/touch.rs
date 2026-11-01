//! C606 I2C touch transport. No controller firmware, reset or calibration writes.

use crate::drivers::input::Report;
use esp_hal::Async;
use esp_hal::Blocking;
use esp_hal::i2c::master::Error;
use esp_hal::i2c::master::I2c;

/// Stock probes this register; its four bytes do not establish an exact part ID.
pub fn probe(bus: &mut I2c<'_, Blocking>) -> Result<[u8; 4], Error> {
    let mut data = [0; 4];
    bus.write_read(0x5au8, &[0xd0, 0x45], &mut data)?;
    Ok(data)
}

pub struct Touch<'d> {
    bus: I2c<'d, Async>,
}

impl<'d> Touch<'d> {
    /// Polls wait on the bus interrupt, so other tasks run during a transfer.
    pub fn new(bus: I2c<'d, Blocking>) -> Self {
        Self {
            bus: bus.into_async(),
        }
    }

    pub async fn poll(&mut self) -> Result<Report, Error> {
        let mut data = [0; 7];
        self.bus
            .write_read_async(0x5au8, &[0xd0, 0x00], &mut data)
            .await?;
        // End-of-read handshake, also used by stock at 0x4202b898.
        self.bus.write_async(0x5au8, &[0xd0, 0x00, 0xab]).await?;
        Ok(crate::drivers::input::decode(data))
    }
}
