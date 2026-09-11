//! Fixed receive ring with explicit loss resynchronization.

pub struct LossRing<const N: usize, const START: u8> {
    bytes: [u8; N],
    read: usize,
    write: usize,
    discarding: bool,
}

impl<const N: usize, const START: u8> Default for LossRing<N, START> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize, const START: u8> LossRing<N, START> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            read: 0,
            write: 0,
            discarding: false,
        }
    }

    /// Returns false when this byte caused a full-ring loss.
    pub fn push(&mut self, byte: u8) -> bool {
        if self.discarding {
            if byte != START {
                return true;
            }
            self.discarding = false;
        }
        let next = (self.write + 1) % N;
        if next == self.read {
            self.read = self.write;
            self.discarding = true;
            return false;
        }
        self.bytes[self.write] = byte;
        self.write = next;
        true
    }

    pub fn discard_partial(&mut self) {
        self.read = self.write;
        self.discarding = true;
    }

    pub fn drain(&mut self, output: &mut [u8]) -> usize {
        let mut count = 0;
        while count < output.len() && self.read != self.write {
            output[count] = self.bytes[self.read];
            self.read = (self.read + 1) % N;
            count += 1;
        }
        count
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::companion::{Decoder, Event};

    fn crc16(bytes: &[u8]) -> u16 {
        let mut crc = 0u16;
        for byte in bytes {
            crc ^= u16::from(*byte) << 8;
            for _ in 0..8 {
                crc = if crc & 0x8000 != 0 {
                    (crc << 1) ^ 0x1021
                } else {
                    crc << 1
                };
            }
        }
        crc
    }

    fn battery() -> [u8; 16] {
        let mut packet = [0u8; 16];
        packet[..14].copy_from_slice(&[
            0xa5, 12, 0x6f, 0xf1, 4, 0, 0x52, 0xff, 0xff, 0xff, 0xa0, 0x0f, 73, 0xff,
        ]);
        let crc = crc16(&packet[..14]);
        packet[14..].copy_from_slice(&crc.to_le_bytes());
        packet
    }

    #[test]
    fn explicit_loss_discards_partial_and_resynchronizes_decoder() {
        let valid = battery();
        let mut ring = LossRing::<64, 0xa5>::new();
        for byte in &valid[..7] {
            assert!(ring.push(*byte));
        }
        ring.discard_partial();
        for byte in [1, 2, 3] {
            assert!(ring.push(byte));
        }
        for byte in valid {
            assert!(ring.push(byte));
        }
        let mut bytes = [0; 64];
        let count = ring.drain(&mut bytes);
        let mut decoder = Decoder::default();
        let event = bytes[..count].iter().find_map(|byte| decoder.push(*byte));
        assert_eq!(
            event,
            Some(Event::Battery {
                percent: 73,
                millivolts: 4000
            })
        );
        assert_eq!(decoder.bad_crc, 0);
    }

    #[test]
    fn full_ring_reports_loss_and_accepts_next_frame_start() {
        let mut ring = LossRing::<5, 0xa5>::new();
        assert!([1, 2, 3, 4].into_iter().all(|byte| ring.push(byte)));
        assert!(!ring.push(5));
        assert!(ring.push(9));
        assert!(ring.push(0xa5));
        assert!(ring.push(7));
        let mut output = [0; 4];
        assert_eq!(ring.drain(&mut output), 2);
        assert_eq!(&output[..2], &[0xa5, 7]);
    }
}
