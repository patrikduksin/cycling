//! Cycling speed, distance, gradient and training-zone policy.
use crate::sensors::ant::Epoch;
use device_api::ant::Packet;

pub const WHEEL_MM: u32 = 2136;
/// Speeds above this are treated as decoding noise (144 km/h).
const MAX_SPEED_MM_S: u64 = 40_000;
/// Speed is unknown once the sensor has been silent this long.
const SPEED_SILENT_MS: u64 = 3000;
/// Packets keep arriving but no new revolution: the wheel has stopped.
const WHEEL_STOPPED_MS: u64 = 4000;
/// A revolution this old cannot anchor a new speed measurement.
const REVOLUTION_EXPIRED_MS: u64 = 60_000;

/// The latest wheel revolution event of an ANT+ speed page.
#[derive(Clone, Copy)]
struct Revolution {
    /// Event time in 1/1024 s, wrapping.
    event_time: u16,
    /// Cumulative revolutions, wrapping.
    count: u16,
    received_ms: u64,
}

#[derive(Default)]
pub struct Speed {
    previous: Option<Revolution>,
    epoch: Option<Epoch>,
    value: Option<u32>,
    received: u64,
}

impl Speed {
    pub fn receive(&mut self, packet: Packet) {
        let epoch = Epoch::of(&packet);
        if self.epoch != Some(epoch) {
            *self = Self::default();
            self.epoch = Some(epoch);
        }
        let data = packet.data;
        let current = Revolution {
            event_time: u16::from_le_bytes([data[4], data[5]]),
            count: u16::from_le_bytes([data[6], data[7]]),
            received_ms: packet.received_ms,
        };
        self.received = current.received_ms;
        if let Some(previous) = self.previous {
            let dt = current.event_time.wrapping_sub(previous.event_time);
            let dr = current.count.wrapping_sub(previous.count);
            if current.received_ms.saturating_sub(previous.received_ms) > REVOLUTION_EXPIRED_MS {
                self.value = None;
            } else if dt != 0 && dr != 0 {
                let speed = u64::from(dr) * u64::from(WHEEL_MM) * 1024 / u64::from(dt);
                self.value = (speed <= MAX_SPEED_MM_S).then_some(speed as u32);
            } else {
                // Repeated event: keep timing from the last real revolution.
                return;
            }
        }
        self.previous = Some(current);
    }

    pub fn value(&self, now: u64) -> Option<u32> {
        if now.saturating_sub(self.received) > SPEED_SILENT_MS {
            return None;
        }
        self.previous.and_then(|previous| {
            if now.saturating_sub(previous.received_ms) > WHEEL_STOPPED_MS {
                Some(0)
            } else {
                self.value
            }
        })
    }
}

/// Ride distance, integrated from wheel speed about once per second.
#[derive(Default)]
pub struct Odometer {
    distance_mm: u64,
    last_step_ms: u64,
}

impl Odometer {
    const STEP_MS: u64 = 1000;
    /// Longer gaps, such as a stalled loop, are not extrapolated beyond this.
    const MAX_STEP_MS: u64 = 3000;

    /// Integrate the current speed if a step is due, reporting whether it was.
    pub fn step(&mut self, now: u64, speed_mm_s: Option<u32>) -> bool {
        if now < self.last_step_ms.saturating_add(Self::STEP_MS) {
            return false;
        }
        let elapsed = now.saturating_sub(self.last_step_ms).min(Self::MAX_STEP_MS);
        self.last_step_ms = now;
        self.distance_mm = self
            .distance_mm
            .saturating_add(u64::from(speed_mm_s.unwrap_or(0)) * elapsed / 1000);
        true
    }

    pub fn distance_mm(&self) -> u64 {
        self.distance_mm
    }
}

#[derive(Clone, Copy)]
struct PressureBase {
    pressure_centi_pa: u32,
    distance_mm: u64,
}

/// Road gradient from barometric pressure change over travelled distance.
#[derive(Default)]
pub struct Gradient {
    base: Option<PressureBase>,
    tenths: Option<i16>,
}

impl Gradient {
    /// Shorter baselines turn pressure noise into implausible grades.
    const MIN_BASELINE_MM: u64 = 30_000;
    /// Below walking pace the distance baseline is meaningless.
    const MIN_SPEED_MM_S: u32 = 1000;
    /// ±40.0 %; anything steeper is a measurement fault.
    const MAX_TENTHS: i64 = 400;

    /// `pressure_centi_pa` is a fresh reading only; anything else resets the baseline.
    pub fn update(
        &mut self,
        pressure_centi_pa: Option<u32>,
        distance_mm: u64,
        speed_mm_s: Option<u32>,
    ) {
        let Some(pressure) = pressure_centi_pa else {
            self.reset();
            return;
        };
        let base = PressureBase {
            pressure_centi_pa: pressure,
            distance_mm,
        };
        match self.base {
            Some(previous) => {
                let travelled = distance_mm.saturating_sub(previous.distance_mm);
                if travelled >= Self::MIN_BASELINE_MM {
                    // Local barometric derivative: dh = -8434 * dp / p, in tenths of a percent.
                    let grade = (i64::from(previous.pressure_centi_pa) - i64::from(pressure))
                        * 8_434_000_000i64
                        / i64::from(previous.pressure_centi_pa)
                        / travelled as i64;
                    self.tenths = (grade.abs() <= Self::MAX_TENTHS).then_some(grade as i16);
                    self.base = Some(base);
                }
            }
            None => self.base = Some(base),
        }
        if speed_mm_s.is_none_or(|speed| speed < Self::MIN_SPEED_MM_S) {
            self.reset();
        }
    }

    pub fn tenths(&self) -> Option<i16> {
        self.tenths
    }

    fn reset(&mut self) {
        self.base = None;
        self.tenths = None;
    }
}

pub fn power_zone(w: u16) -> u8 {
    match w {
        0..=118 => 1,
        119..=161 => 2,
        162..=194 => 3,
        195..=226 => 4,
        227..=258 => 5,
        259..=323 => 6,
        _ => 7,
    }
}
pub fn heart_zone(b: u16) -> u8 {
    match b {
        0..=138 => 1,
        139..=153 => 2,
        154..=171 => 3,
        172..=184 => 4,
        _ => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn speed_wrap_stop_and_loss() {
        fn packet(time: u16, rev: u16, at: u64, loss: u32) -> device_api::ant::Packet {
            let mut data = [0; 8];
            data[4..6].copy_from_slice(&time.to_le_bytes());
            data[6..8].copy_from_slice(&rev.to_le_bytes());
            device_api::ant::Packet {
                identity: device_api::ant::Identity {
                    device_type: 123,
                    device_number: 1,
                    transmission_type: 1,
                },
                data,
                received_ms: at,
                generation: 1,
                loss_count: loss,
            }
        }
        let mut speed = Speed::default();
        speed.receive(packet(65000, 65535, 100, 0));
        assert_eq!(speed.value(100), None);
        speed.receive(packet(488, 0, 1100, 0));
        assert_eq!(speed.value(1100), Some(WHEEL_MM));
        speed.receive(packet(488, 0, 5200, 0));
        assert_eq!(speed.value(5200), Some(0));
        assert_eq!(speed.value(8300), None);
        speed.receive(packet(1512, 1, 8500, 1));
        assert_eq!(speed.value(8500), None);
    }
    #[test]
    fn zones_cover_boundaries() {
        for (v, z) in [
            (118, 1),
            (119, 2),
            (161, 2),
            (162, 3),
            (194, 3),
            (195, 4),
            (226, 4),
            (227, 5),
            (258, 5),
            (259, 6),
            (323, 6),
            (324, 7),
        ] {
            assert_eq!(power_zone(v), z);
        }
        for (v, z) in [
            (138, 1),
            (139, 2),
            (153, 2),
            (154, 3),
            (171, 3),
            (172, 4),
            (184, 4),
            (185, 5),
        ] {
            assert_eq!(heart_zone(v), z);
        }
    }
}
