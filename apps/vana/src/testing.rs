//! Deterministic random input for host property tests.
//!
//! Property tests draw every case from a fixed seed, so a failure names the
//! case that reproduces it, and cap their cases so `cycling test` stays
//! quick. The generator is `firmware_services::testing::Rng`, shared by every
//! crate's property tests through that crate's `testing` feature.

use crate::ride::log::{Altitude, LapStart, Sample};

pub use firmware_services::testing::Rng;

/// Ride-shaped draws on the shared generator.
pub trait Draw {
    fn lap(&mut self) -> LapStart;
    fn sample(&mut self) -> Sample;
}

impl Draw for Rng {
    fn lap(&mut self) -> LapStart {
        self.pick(&[
            LapStart::Ride,
            LapStart::Manual,
            LapStart::Distance,
            LapStart::Step,
            LapStart::DistanceStep,
        ])
    }

    /// Any reading a sensor or the GPS can report, at any active time.
    fn sample(&mut self) -> Sample {
        Sample {
            active_ms: self.next(),
            utc_ms: self.option(|rng| rng.range(1_500_000_000_000, 2_500_000_000_000)),
            location_e7: self.option(|rng| {
                (
                    rng.range(0, 1_800_000_000) as i32 - 900_000_000,
                    (rng.range(0, 3_600_000_000) as i64 - 1_800_000_000) as i32,
                )
            }),
            heart_bpm: self.option(|rng| rng.next() as u16),
            cadence_tenths: self.option(|rng| rng.next() as u16),
            battery_percent: self.option(Self::byte),
            speed_mm_s: self.option(|rng| rng.next() as u32),
            power_watts: self.option(|rng| rng.next() as u16),
            gradient_tenths: self.option(|rng| rng.next() as i16),
            altitude: self.option(|rng| Altitude::from_dm(rng.next() as i32)),
            lap: self.one_in(4).then(|| self.lap()),
        }
    }
}

/// One of a sweep's parts, split so nextest runs them on separate cores.
pub(crate) struct Share {
    k: usize,
    seen: usize,
}

impl Share {
    pub(crate) const fn new(k: usize) -> Self {
        assert!(k < 8);
        Self { k, seen: 0 }
    }

    pub(crate) fn takes(&mut self) -> bool {
        self.seen += 1;
        (self.seen - 1) % 8 == self.k
    }
}

macro_rules! split_in_eight {
    ($sweep:ident) => {
        mod $sweep {
            use $crate::testing::Share;
            #[test]
            fn share_0() {
                super::$sweep(Share::new(0));
            }
            #[test]
            fn share_1() {
                super::$sweep(Share::new(1));
            }
            #[test]
            fn share_2() {
                super::$sweep(Share::new(2));
            }
            #[test]
            fn share_3() {
                super::$sweep(Share::new(3));
            }
            #[test]
            fn share_4() {
                super::$sweep(Share::new(4));
            }
            #[test]
            fn share_5() {
                super::$sweep(Share::new(5));
            }
            #[test]
            fn share_6() {
                super::$sweep(Share::new(6));
            }
            #[test]
            fn share_7() {
                super::$sweep(Share::new(7));
            }
        }
    };
}
pub(crate) use split_in_eight;

/// How an interrupted sector erase leaves its sector. NOR erase only sets
/// bits, and one cut by power leaves cells across the whole sector partly
/// erased.
#[derive(Clone, Copy)]
pub(crate) struct EraseTear {
    pub(crate) progress: u16,
    pub(crate) seed: u64,
}

impl EraseTear {
    pub(crate) fn draw(rng: &mut Rng) -> Self {
        let progress = match rng.below(4) {
            0 => rng.pick(&[0, 1, 255, 256]),
            _ => rng.below(257) as u16,
        };
        Self {
            progress,
            seed: rng.next(),
        }
    }

    pub(crate) fn apply(self, stored: &mut [u8]) {
        let mut rng = Rng::new(self.seed);
        for byte in stored.iter_mut() {
            for bit in 0..8 {
                if rng.below(256) < u64::from(self.progress) {
                    *byte |= 1 << bit;
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Cut {
    pub(crate) at: u64,
    pub(crate) seed: u64,
}

/// The two NOR sectors of `Region::Intent`, with power cut at any mutation.
/// It models an interrupted NOR erase and program conservatively: a cut
/// erase leaves bits across the sector partly erased, a cut program leaves
/// its bytes partly programmed, and a cut commit word is marginal, so each
/// later read returns it whole or as it was left until a program completes
/// over it or the sector is erased.
pub(crate) struct IntentFlash {
    pub(crate) bytes: std::vec::Vec<u8>,
    pub(crate) mutations: u64,
    pub(crate) cut: Option<Cut>,
    pub(crate) off: bool,
    marginal: std::vec::Vec<(usize, u8)>,
    reads: Rng,
}

impl Default for IntentFlash {
    fn default() -> Self {
        Self {
            bytes: std::vec![0xff; 2 * 4096],
            mutations: 0,
            cut: None,
            off: false,
            marginal: std::vec::Vec::new(),
            reads: Rng::new(0),
        }
    }
}

impl IntentFlash {
    pub(crate) fn power_on(&mut self, read_seed: u64) {
        self.off = false;
        self.cut = None;
        self.reads = Rng::new(read_seed);
    }

    fn mutation(&mut self) -> Result<Option<Rng>, ()> {
        if self.off {
            return Err(());
        }
        let at = self.mutations;
        self.mutations += 1;
        match self.cut {
            Some(cut) if cut.at == at => {
                self.off = true;
                Ok(Some(Rng::new(cut.seed)))
            }
            _ => Ok(None),
        }
    }
}

impl device_api::storage::OwnedFlash for IntentFlash {
    type Error = ();
    fn geometry(&self, region: device_api::storage::Region) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: if region == device_api::storage::Region::Intent {
                self.bytes.len()
            } else {
                0
            },
            program_size: 4,
            erase_size: 4096,
        }
    }
    fn read(
        &mut self,
        _: device_api::storage::Region,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), ()> {
        if self.off {
            return Err(());
        }
        output.copy_from_slice(&self.bytes[offset..offset + output.len()]);
        for &(at, whole) in &self.marginal {
            if let Some(byte) = at.checked_sub(offset).and_then(|i| output.get_mut(i))
                && self.reads.one_in(2)
            {
                *byte = whole;
            }
        }
        Ok(())
    }
    fn program(
        &mut self,
        _: device_api::storage::Region,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), ()> {
        let Some(mut tear) = self.mutation()? else {
            for (stored, new) in self.bytes[offset..].iter_mut().zip(bytes) {
                *stored &= new;
            }
            self.marginal
                .retain(|&(at, _)| !(offset..offset + bytes.len()).contains(&at));
            return Ok(());
        };
        let landed = tear.below(bytes.len() as u64 + 1) as usize;
        for (i, (stored, new)) in self.bytes[offset..].iter_mut().zip(bytes).enumerate() {
            let target = *stored & new;
            *stored &= match i.cmp(&landed) {
                core::cmp::Ordering::Less => *new,
                core::cmp::Ordering::Equal => new | tear.byte(),
                core::cmp::Ordering::Greater => 0xff,
            };
            if bytes.len() == 4 && *stored != target {
                self.marginal.push((offset + i, target));
            }
        }
        Err(())
    }
    fn erase(
        &mut self,
        _: device_api::storage::Region,
        offset: usize,
        length: usize,
    ) -> Result<(), ()> {
        let tear = self.mutation()?;
        self.marginal
            .retain(|&(at, _)| !(offset..offset + length).contains(&at));
        let Some(mut tear) = tear else {
            self.bytes[offset..offset + length].fill(0xff);
            return Ok(());
        };
        EraseTear {
            progress: tear.below(257) as u16,
            seed: tear.next(),
        }
        .apply(&mut self.bytes[offset..offset + length]);
        Err(())
    }
}
