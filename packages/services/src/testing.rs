//! Host test media: sparse owned sectors with stale bytes, power cuts that
//! tear the interrupted write, and the deterministic random source every host
//! property test shares. Built for tests, or with the `testing` feature; never
//! part of a firmware build.

use device_api::bulk::{Error, OwnedInfo, ReadWrite, SECTOR_SIZE};
use std::collections::BTreeMap;
use std::vec::Vec;

pub type Sector = [u8; SECTOR_SIZE];

pub struct Media {
    pub sectors: u64,
    pub written: BTreeMap<u64, Sector>,
    pub writes: u64,
    pub reads: u64,
    /// Cut power at this write (counted from now); it leaves the sector torn.
    pub cut: Option<(u64, Tear)>,
    pub off: bool,
    entropy: u64,
}

/// How the interrupted write leaves its sector.
#[derive(Clone, Copy, Debug)]
pub enum Tear {
    /// Unchanged.
    Old,
    /// The first `n` bytes landed.
    Prefix(usize),
    /// Completely written, but the medium reported failure.
    New,
}

impl Tear {
    pub fn pick(rng: &mut Rng) -> Self {
        match rng.below(3) {
            0 => Self::Old,
            1 => Self::Prefix(rng.below(SECTOR_SIZE as u64) as usize),
            _ => Self::New,
        }
    }
}

impl Media {
    pub fn new(sectors: u64) -> Self {
        Self {
            sectors,
            written: BTreeMap::new(),
            writes: 0,
            reads: 0,
            cut: None,
            off: false,
            entropy: 0x5eed,
        }
    }

    /// Stale bytes a reused partition holds.
    pub fn stale(index: u64) -> Sector {
        let mut sector = [0u8; SECTOR_SIZE];
        let mut rng = Rng::new(index);
        for chunk in sector.chunks_mut(8) {
            chunk.copy_from_slice(&rng.next().to_le_bytes());
        }
        sector
    }

    pub fn get(&self, index: u64) -> Sector {
        self.written
            .get(&index)
            .copied()
            .unwrap_or_else(|| Self::stale(index))
    }

    pub fn set(&mut self, index: u64, sector: Sector) {
        self.written.insert(index, sector);
    }

    /// Power returns.
    pub fn reboot(&mut self) {
        self.cut = None;
        self.off = false;
    }
}

impl ReadWrite for Media {
    fn owned_info(&mut self) -> Result<OwnedInfo, Error> {
        if self.off {
            return Err(Error::Unavailable);
        }
        Ok(OwnedInfo {
            start_sector: 1000,
            sectors: self.sectors,
            sector_size: SECTOR_SIZE as u16,
        })
    }

    fn owned_read(&mut self, index: u64, output: &mut Sector) -> Result<(), Error> {
        if self.off {
            return Err(Error::Unavailable);
        }
        if index >= self.sectors {
            return Err(Error::Range);
        }
        self.reads += 1;
        *output = self.get(index);
        Ok(())
    }

    fn owned_write(&mut self, index: u64, input: &Sector) -> Result<(), Error> {
        if self.off {
            return Err(Error::Unavailable);
        }
        if index >= self.sectors {
            return Err(Error::Range);
        }
        if let Some((at, tear)) = self.cut {
            if at == 0 {
                let mut sector = self.get(index);
                match tear {
                    Tear::Old => {}
                    Tear::Prefix(n) => sector[..n].copy_from_slice(&input[..n]),
                    Tear::New => sector = *input,
                }
                self.set(index, sector);
                self.off = true;
                return Err(Error::Failed);
            }
            self.cut = Some((at - 1, tear));
        }
        self.writes += 1;
        self.set(index, *input);
        Ok(())
    }

    fn entropy(&mut self) -> u64 {
        let mut rng = Rng::new(self.entropy);
        self.entropy = rng.next();
        rng.next()
    }
}

/// xorshift64*, the one deterministic generator of the host property tests.
pub struct Rng(u64);

impl Rng {
    /// The generator for case `seed`; any seed, zero included, is usable.
    pub fn new(seed: u64) -> Self {
        let mut rng = Self(seed.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ 0x5eed_5eed_5eed_5eed | 1);
        rng.next();
        rng
    }

    #[allow(clippy::should_implement_trait)]
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
