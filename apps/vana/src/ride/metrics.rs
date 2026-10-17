//! Cycling speed, distance and gradient policy. Training zones live in `zones`.
use crate::sensors::ant::Epoch;
use device_api::ant::Packet;

pub const WHEEL_MM: u32 = 2136;
/// Speeds above this are treated as decoding noise (144 km/h).
const MAX_SPEED_MM_S: u64 = 40_000;
/// Speed is unknown once the sensor has been silent this long.
const SPEED_SILENT_MS: u64 = 3000;
/// Revolutions reach us in packets about every 250 ms; a revolution can wait
/// that long, and a little more, before it is seen.
const PACKET_SLACK_MS: u64 = 300;
/// Below this (2 km/h) a wheel that has not completed a revolution is stopped.
const WHEEL_STOPPED_MM_S: u64 = 556;
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
    /// The wheel has stood so long that its last revolution is no reference.
    standing: bool,
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
            let repeat = dt == 0 && dr == 0;
            if self.standing {
                // The reference is a minute old or more: the wheel stands. Keep
                // reading zero rather than lose the sensor, until a new
                // revolution anchors a measurement. Its event time is stale, so
                // no speed comes from comparing with it.
                self.value = repeat.then_some(0);
                self.standing = repeat;
            } else if current.received_ms.saturating_sub(previous.received_ms)
                > REVOLUTION_EXPIRED_MS
            {
                self.value = repeat.then_some(0);
                self.standing = repeat;
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

    /// The wheel speed now. Once the time since the last revolution exceeds
    /// what the last measurement implies, the wheel cannot be going faster
    /// than one circumference per that time, so the reading decays with it
    /// instead of holding until the wheel is declared stopped. The age is
    /// measured from when the packet carrying the last revolution arrived, so a
    /// lost repeat packet does not create a decay. Under 2 km/h it reads zero.
    pub fn value(&self, now: u64) -> Option<u32> {
        if now.saturating_sub(self.received) > SPEED_SILENT_MS {
            return None;
        }
        let previous = self.previous?;
        // Measured to the latest packet, not the clock: only packets that repeat the
        // old revolution show the wheel has not turned. Silence proves nothing.
        let age_ms = self.received.saturating_sub(previous.received_ms).max(1);
        // A newer revolution may sit in a packet not yet received.
        let overdue_ms = age_ms.saturating_sub(PACKET_SLACK_MS).max(1);
        let bound = u64::from(WHEEL_MM) * 1000 / overdue_ms;
        let speed = match self.value {
            Some(speed) => u64::from(speed).min(bound),
            None => bound,
        };
        if speed < WHEEL_STOPPED_MM_S {
            return Some(0);
        }
        // Without a measured speed the bound is only a ceiling: unknown, not
        // moving, until a revolution is measured or the wheel is plainly stopped.
        self.value.map(|_| speed as u32)
    }
}

/// One odometer integration: the time it covered and the distance it added.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Step {
    pub elapsed_ms: u64,
    pub distance_mm: u64,
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

    /// Integrate the current speed if a step is due, reporting the step taken.
    pub fn step(&mut self, now: u64, speed_mm_s: Option<u32>) -> Option<Step> {
        if now < self.last_step_ms.saturating_add(Self::STEP_MS) {
            return None;
        }
        let elapsed_ms = now.saturating_sub(self.last_step_ms).min(Self::MAX_STEP_MS);
        self.last_step_ms = now;
        let distance_mm = u64::from(speed_mm_s.unwrap_or(0)) * elapsed_ms / 1000;
        self.distance_mm = self.distance_mm.saturating_add(distance_mm);
        Some(Step {
            elapsed_ms,
            distance_mm,
        })
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

/// The 3 s power a rider reads: the mean of the last reading in each of the
/// last three seconds, zeros included. A lost meter clears it, so the value
/// never outlives the readings it averages.
#[derive(Default)]
pub struct Power3s {
    /// The last reading of each sampled second, and that second.
    slots: [Option<(u32, u16)>; 3],
    next: u8,
}

impl Power3s {
    pub fn update(&mut self, now_ms: u64, watts: Option<u16>) -> Option<u16> {
        let Some(watts) = watts else {
            self.slots = [None; 3];
            return None;
        };
        let second = (now_ms / 1000) as u32;
        let last = usize::from((self.next + 2) % 3);
        if matches!(self.slots[last], Some((at, _)) if at == second) {
            // A later reading in the same second replaces that second's.
            self.slots[last] = Some((second, watts));
        } else {
            self.slots[usize::from(self.next)] = Some((second, watts));
            self.next = (self.next + 1) % 3;
        }
        // Seconds skipped without a sample (ticks suspended) leave their
        // older buckets out of the window.
        let (sum, len) = self
            .slots
            .iter()
            .flatten()
            .filter(|(at, _)| second - at < 3)
            .fold((0u32, 0u32), |(sum, len), (_, w)| {
                (sum + u32::from(*w), len + 1)
            });
        Some(((sum + len / 2) / len) as u16)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn three_second_power_averages_one_reading_per_second() {
        let mut power = Power3s::default();
        assert_eq!(power.update(0, Some(300)), Some(300));
        // A second reading in the same second replaces the first.
        assert_eq!(power.update(500, Some(240)), Some(240));
        assert_eq!(power.update(1_000, Some(120)), Some(180));
        assert_eq!(power.update(2_000, Some(0)), Some(120));
        // The oldest second drops out.
        assert_eq!(power.update(3_000, Some(600)), Some(240));
        assert_eq!(power.update(3_250, Some(0)), Some(40));
        // A lost meter clears the average; it restarts from the next reading.
        assert_eq!(power.update(4_000, None), None);
        assert_eq!(power.update(5_000, Some(200)), Some(200));
        assert_eq!(power.update(6_000, Some(2_000)), Some(1_100));
    }

    #[test]
    fn three_second_power_forgets_seconds_without_a_sample() {
        let mut power = Power3s::default();
        for second in 0..3 {
            power.update(second * 1000, Some(600));
        }
        // Ticks stopped for seven seconds: the old buckets are out of the window.
        assert_eq!(power.update(10_000, Some(0)), Some(0));
        // One second skipped: only the bucket from two seconds ago remains.
        assert_eq!(power.update(12_000, Some(300)), Some(150));
        // The zero from second 10 has left the window.
        assert_eq!(power.update(13_000, Some(300)), Some(300));
    }
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
    #[test]
    fn speed_wrap_stop_and_loss() {
        let mut speed = Speed::default();
        speed.receive(packet(65000, 65535, 100, 0));
        assert_eq!(speed.value(100), None);
        speed.receive(packet(488, 0, 1100, 0));
        assert_eq!(speed.value(1100), Some(WHEEL_MM));
        speed.receive(packet(488, 0, 5300, 0));
        assert_eq!(speed.value(5300), Some(0));
        assert_eq!(speed.value(8400), None);
        speed.receive(packet(1512, 1, 8500, 1));
        assert_eq!(speed.value(8500), None);
    }
    #[test]
    fn speed_decays_when_braking_and_reads_zero_when_stopped() {
        let mut speed = Speed::default();
        // One revolution per second: 7.7 km/h.
        speed.receive(packet(0, 0, 0, 0));
        speed.receive(packet(1024, 1, 1000, 0));
        assert_eq!(speed.value(1000), Some(WHEEL_MM));
        // Repeat packets within a revolution period change nothing.
        speed.receive(packet(1024, 1, 1250, 0));
        assert_eq!(speed.value(1250), Some(WHEEL_MM));
        assert_eq!(speed.value(1999), Some(WHEEL_MM));
        // Overdue: bounded by one circumference per elapsed time.
        speed.receive(packet(1024, 1, 2500, 0));
        assert_eq!(speed.value(2500), Some(WHEEL_MM * 1000 / 1200));
        speed.receive(packet(1024, 1, 3000, 0));
        let at_three = speed.value(3000).unwrap();
        assert!(at_three < WHEEL_MM * 1000 / 1200, "{at_three}");
        // Under 2 km/h (556 mm/s) is zero, a little under four seconds on.
        speed.receive(packet(1024, 1, 4900, 0));
        assert!(speed.value(4900).unwrap() > 0);
        speed.receive(packet(1024, 1, 5200, 0));
        assert_eq!(speed.value(5300), Some(0));
    }
    #[test]
    fn repeated_packets_after_a_connect_are_unknown_then_stopped_never_fast() {
        let mut speed = Speed::default();
        for at in (0..4000).step_by(250) {
            speed.receive(packet(1024, 5, at, 0));
            let value = speed.value(at);
            assert!(value.is_none() || value == Some(0), "{value:?} at {at}");
        }
        speed.receive(packet(1024, 5, 4250, 0));
        assert_eq!(speed.value(4250), Some(0));
    }
    #[test]
    fn a_wheel_standing_for_minutes_keeps_reading_zero() {
        let mut speed = Speed::default();
        speed.receive(packet(0, 0, 0, 0));
        speed.receive(packet(1024, 1, 1000, 0));
        let mut zero_from = None;
        for at in (1250..400_000).step_by(250) {
            speed.receive(packet(1024, 1, at, 0));
            let value = speed.value(at);
            if value == Some(0) {
                zero_from.get_or_insert(at);
            }
            if zero_from.is_some() {
                assert_eq!(value, Some(0), "{value:?} at {at}");
            }
        }
        assert!(zero_from.is_some());
        // Pedalling off again is measured from the next revolution.
        speed.receive(packet(2048, 2, 400_500, 0));
        speed.receive(packet(3072, 3, 401_500, 0));
        assert_eq!(speed.value(401_500), Some(WHEEL_MM));
    }
    #[test]
    fn one_revolution_after_a_long_stand_is_not_a_speed() {
        let mut speed = Speed::default();
        speed.receive(packet(0, 0, 0, 0));
        speed.receive(packet(1024, 1, 1000, 0));
        for at in (1250..65_000).step_by(250) {
            speed.receive(packet(1024, 1, at, 0));
        }
        assert_eq!(speed.value(64_750), Some(0));
        // One revolution 64.25 s on (the 16-bit event time wraps), then still.
        speed.receive(packet(1280, 2, 65_250, 0));
        assert_eq!(speed.value(65_250), None);
        for at in (65_500..70_000).step_by(250) {
            speed.receive(packet(1280, 2, at, 0));
            let value = speed.value(at);
            assert!(value.is_none() || value == Some(0), "{value:?} at {at}");
        }
        // The next revolution anchors a real measurement.
        speed.receive(packet(2304, 3, 71_000, 0));
        assert_eq!(speed.value(71_000), Some(WHEEL_MM));
    }
    #[test]
    fn a_dropout_during_a_slow_revolution_is_not_a_stop() {
        let mut speed = Speed::default();
        // 3.6 km/h: a revolution every 2136 ms, packets every 250 ms.
        let mut at = 0;
        let mut count = 0u16;
        let mut event = 0u32;
        while at < 20_000 {
            if at >= (u64::from(count) + 1) * 2136 {
                count += 1;
                event = u32::from(count) * 2187;
            }
            speed.receive(packet(event as u16, count, at, 0));
            at += 250;
        }
        let silent_from = at - 250;
        let before = speed.value(silent_from).unwrap();
        assert!(before > 833, "{before}");
        // Packets stop late in a revolution: the clock alone is no evidence.
        for now in (silent_from..silent_from + 3000).step_by(100) {
            let value = speed.value(now).unwrap();
            assert!(
                value > 833,
                "{value} mm/s {} ms into the dropout",
                now - silent_from
            );
        }
        assert_eq!(speed.value(silent_from + 3100), None);
    }
    #[test]
    fn a_lost_packet_does_not_fake_a_decay() {
        let mut speed = Speed::default();
        speed.receive(packet(0, 0, 0, 0));
        speed.receive(packet(1024, 1, 1000, 0));
        // Packets at 1250 and 1500 are lost; the revolution at 2024 arrives late.
        speed.receive(packet(2048, 2, 2100, 0));
        assert_eq!(speed.value(2100), Some(WHEEL_MM));
        speed.receive(packet(3072, 3, 3100, 0));
        assert_eq!(speed.value(3100), Some(WHEEL_MM));
    }
    #[test]
    fn speed_rejects_implausible_and_expired_revolutions() {
        let mut speed = Speed::default();
        speed.receive(packet(0, 0, 0, 0));
        // 20 revolutions in one second is about 154 km/h.
        speed.receive(packet(1024, 20, 1000, 0));
        assert_eq!(speed.value(1000), None);
        speed.receive(packet(2048, 22, 2000, 0));
        assert_eq!(speed.value(2000), Some(2 * WHEEL_MM));
        // A revolution after more than a minute has no usable reference.
        speed.receive(packet(3072, 23, 63_000, 0));
        assert_eq!(speed.value(63_000), None);
        speed.receive(packet(4096, 24, 64_000, 0));
        assert_eq!(speed.value(64_000), Some(WHEEL_MM));
    }
    #[test]
    fn odometer_integrates_speed_once_per_second_and_bounds_gaps() {
        let step = |elapsed_ms, distance_mm| {
            Some(Step {
                elapsed_ms,
                distance_mm,
            })
        };
        let mut odometer = Odometer::default();
        assert_eq!(odometer.step(999, Some(10_000)), None);
        assert_eq!(odometer.step(1000, Some(10_000)), step(1000, 10_000));
        assert_eq!(odometer.distance_mm(), 10_000);
        assert_eq!(odometer.step(1999, Some(10_000)), None);
        assert_eq!(odometer.step(2500, Some(10_000)), step(1500, 15_000));
        assert_eq!(odometer.distance_mm(), 25_000);
        // A ten-second stall counts as three seconds of travel.
        assert_eq!(odometer.step(12_500, Some(10_000)), step(3000, 30_000));
        assert_eq!(odometer.distance_mm(), 55_000);
        assert_eq!(odometer.step(13_500, None), step(1000, 0));
        assert_eq!(odometer.distance_mm(), 55_000);
    }
    #[test]
    fn gradient_needs_thirty_metres_of_moving_baseline() {
        const SEA_LEVEL: u32 = 10_130_000;
        // About 60.8 Pa less pressure over 100 m is a 5.0% climb.
        const CLIMB: u32 = 6080;
        let mut gradient = Gradient::default();
        gradient.update(Some(SEA_LEVEL), 0, Some(5000));
        gradient.update(Some(SEA_LEVEL - 3000), 29_999, Some(5000));
        assert_eq!(gradient.tenths(), None);
        gradient.update(Some(SEA_LEVEL - CLIMB), 100_000, Some(5000));
        assert_eq!(gradient.tenths(), Some(50));
        // The baseline moved, so returning to the old pressure is a descent.
        gradient.update(Some(SEA_LEVEL), 200_000, Some(5000));
        assert_eq!(gradient.tenths(), Some(-50));
        // Short of the next baseline, the last grade remains.
        gradient.update(Some(SEA_LEVEL - 100), 210_000, Some(5000));
        assert_eq!(gradient.tenths(), Some(-50));
    }
    #[test]
    fn gradient_resets_when_slow_unpressurised_or_implausible() {
        const SEA_LEVEL: u32 = 10_130_000;
        let mut gradient = Gradient::default();
        gradient.update(Some(SEA_LEVEL), 0, Some(5000));
        gradient.update(Some(SEA_LEVEL - 6080), 100_000, Some(5000));
        assert_eq!(gradient.tenths(), Some(50));
        gradient.update(Some(SEA_LEVEL - 6080), 101_000, Some(999));
        assert_eq!(gradient.tenths(), None);
        // The slow step also discarded the baseline.
        gradient.update(Some(SEA_LEVEL), 200_000, Some(5000));
        assert_eq!(gradient.tenths(), None);
        gradient.update(Some(SEA_LEVEL - 6080), 300_000, Some(5000));
        assert_eq!(gradient.tenths(), Some(50));
        gradient.update(None, 310_000, Some(5000));
        assert_eq!(gradient.tenths(), None);
        // A 49.9% reading is a measurement fault, not a road.
        gradient.update(Some(SEA_LEVEL), 400_000, Some(5000));
        gradient.update(Some(SEA_LEVEL - 60_000), 500_000, Some(5000));
        assert_eq!(gradient.tenths(), None);
    }
}
