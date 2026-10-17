//! A simulated GNSS receiver: the world's route position as NMEA bytes, once
//! per second, into the production acquisition.
use crate::route::{Nmea, Point};
use crate::world::World;
use firmware_services::positioning::Acquisition;

pub struct Receiver {
    pub nmea: Nmea,
    /// The sky is visible; without it the receiver reports no fix.
    pub fix: bool,
    /// The receiver is powered and its UART delivers bytes.
    pub transmitting: bool,
    pub satellites: u8,
    /// Each fix lands up to this far from the true position, per axis (m).
    pub jitter_m: f64,
    seed: u32,
    next_ms: u64,
}

impl Default for Receiver {
    fn default() -> Self {
        Self {
            // 10:00 UTC on 14 October 2026.
            nmea: Nmea::new(10 * 3600, (14, 10, 26)),
            fix: true,
            transmitting: true,
            satellites: 9,
            jitter_m: 0.0,
            seed: 0x2545_f491,
            next_ms: 0,
        }
    }
}

impl Receiver {
    /// Publish one epoch at each whole world second.
    pub fn tick(&mut self, world: &World, now: u64, acquisition: &mut Acquisition) {
        if world.time_ms < self.next_ms {
            return;
        }
        self.next_ms = (world.time_ms / 1000 + 1) * 1000;
        if !self.transmitting {
            return;
        }
        let position = world.position().map(|point| self.jittered(point));
        let bytes = match position {
            Some(point) if self.fix => self.nmea.fix(
                world.time_ms,
                point,
                world.speed_kmh / 3.6,
                world.course(),
                self.satellites,
            ),
            _ => self.nmea.no_fix(world.time_ms),
        };
        acquisition.ingest(now, &bytes, 0, 0);
    }

    /// A deterministic position error within `jitter_m` in each axis.
    fn jittered(&mut self, mut point: Point) -> Point {
        let mut offset = || {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            (f64::from(self.seed % 2001) / 1000.0 - 1.0) * self.jitter_m
        };
        let (north, east) = (offset(), offset());
        point.lat += north / 111_319.49;
        point.lon += east / (111_319.49 * point.lat.to_radians().cos());
        point
    }

    /// The device restarted; publish again at the next world second.
    pub fn reset(&mut self) {
        self.next_ms = 0;
    }
}
