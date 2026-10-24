//! Deterministic random input for host property tests.
//!
//! Property tests draw every case from a fixed seed, so a failure names the
//! case that reproduces it, and cap their cases so `mise run test` stays
//! quick. The generator is xorshift64*, as in `firmware_services`' private
//! test media; a hand-rolled one keeps the locked dependency set unchanged.

use crate::ride::log::{Altitude, Sample};
use std::vec::Vec;

pub struct Rng(u64);

impl Rng {
    /// The generator for case `seed`; any seed, zero included, is usable.
    pub fn new(seed: u64) -> Self {
        let mut rng = Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5eed_5eed_5eed_5eed | 1);
        rng.next();
        rng
    }

    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    /// Uniform in `0..n`; `n` must not be zero.
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    pub fn range(&mut self, low: u64, high: u64) -> u64 {
        low + self.below(high - low + 1)
    }

    pub fn one_in(&mut self, n: u64) -> bool {
        self.below(n) == 0
    }

    pub fn byte(&mut self) -> u8 {
        self.next() as u8
    }

    pub fn option<T>(&mut self, value: impl FnOnce(&mut Self) -> T) -> Option<T> {
        (!self.one_in(3)).then(|| value(self))
    }

    pub fn pick<T: Copy>(&mut self, values: &[T]) -> T {
        values[self.below(values.len() as u64) as usize]
    }

    pub fn bytes(&mut self, max: usize) -> Vec<u8> {
        let length = self.below(max as u64 + 1) as usize;
        (0..length).map(|_| self.byte()).collect()
    }

    /// Any reading a sensor or the GPS can report, at any active time.
    pub fn sample(&mut self) -> Sample {
        Sample {
            active_ms: self.next(),
            utc_ms: self.option(|rng| rng.range(1_500_000_000_000, 2_500_000_000_000)),
            location_e7: self.option(|rng| {
                (
                    rng.range(0, 1_800_000_000) as i32 - 900_000_000,
                    rng.range(0, 3_600_000_000) as u32 as i32,
                )
            }),
            demo_speed_mm_s: self.option(|rng| rng.next() as u32),
            heart_bpm: self.option(|rng| rng.next() as u16),
            cadence_tenths: self.option(|rng| rng.next() as u16),
            battery_percent: self.option(Self::byte),
            speed_mm_s: self.option(|rng| rng.next() as u32),
            power_watts: self.option(|rng| rng.next() as u16),
            gradient_tenths: self.option(|rng| rng.next() as i16),
            altitude: self.option(|rng| Altitude::from_dm(rng.next() as i32)),
        }
    }

    /// Damage `bytes` in place: flipped bits, overwritten bytes, a truncated
    /// or extended tail. Returns the possibly resized buffer.
    pub fn mutate(&mut self, mut bytes: Vec<u8>) -> Vec<u8> {
        for _ in 0..self.range(1, 4) {
            match self.below(5) {
                0 if !bytes.is_empty() => {
                    let at = self.below(bytes.len() as u64) as usize;
                    bytes[at] ^= 1 << self.below(8);
                }
                1 if !bytes.is_empty() => {
                    let at = self.below(bytes.len() as u64) as usize;
                    let random = self.byte();
                    bytes[at] = self.pick(&[0, 0xff, random]);
                }
                2 => {
                    let length = self.below(bytes.len() as u64 + 1) as usize;
                    bytes.truncate(length);
                }
                3 => {
                    let extra = self.bytes(8);
                    bytes.extend_from_slice(&extra);
                }
                _ if !bytes.is_empty() => {
                    let at = self.below(bytes.len() as u64) as usize;
                    bytes.insert(at, self.byte());
                }
                _ => {}
            }
        }
        bytes
    }
}
