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
