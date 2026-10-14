//! A simulated GNSS receiver: the world's route position as NMEA bytes, once
//! per second, into the production acquisition.
use crate::route::Nmea;
use crate::world::World;
use firmware_services::positioning::Acquisition;

pub struct Receiver {
    pub nmea: Nmea,
    /// The sky is visible; without it the receiver reports no fix.
    pub fix: bool,
    /// The receiver is powered and its UART delivers bytes.
    pub transmitting: bool,
    pub satellites: u8,
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
        let bytes = match world.position() {
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

    /// The device restarted; publish again at the next world second.
    pub fn reset(&mut self) {
        self.next_ms = 0;
    }
}
