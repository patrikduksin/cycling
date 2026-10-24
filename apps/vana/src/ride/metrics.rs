//! Cycling speed, distance and gradient policy. Training zones live in `zones`.
use crate::sensors::ant::{Crank, Epoch};
use crate::sensors::ble_profile::CrankCadence;
use device_api::ant::Packet;

/// The circumference assumed until the rider or the calibration says
/// otherwise (`ride::wheel_size`).
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

/// Cadence is unknown once the sensor has been silent this long.
const CADENCE_SILENT_MS: u64 = 3000;
/// A crank that has turned and then sends the same revolution this long
/// is stopped.
const CRANK_STOPPED_MS: u64 = 4000;
/// Cadences above this are decoding noise; the BLE sensor's cap.
const MAX_CADENCE_TENTHS: u64 = CrankCadence::MAX_TENTHS as u64;

/// Crank cadence from the revolutions of an ANT+ cadence sensor. The caller
/// resets it when the channel session restarts.
#[derive(Clone, Copy, Default)]
pub struct Cadence {
    previous: Option<Revolution>,
    tenths: Option<u16>,
    /// A new revolution has arrived this session. A crank that never turns,
    /// such as a combined sensor without its cadence magnet, says nothing
    /// rather than zero.
    turned: bool,
    /// `previous` is a revolution seen arriving, not the sensor's first message.
    anchored: bool,
    received: u64,
}

impl Cadence {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn receive(&mut self, crank: Crank, received_ms: u64) {
        let current = Revolution {
            event_time: crank.event_time,
            count: crank.count,
            received_ms,
        };
        self.received = received_ms;
        let Some(previous) = self.previous else {
            // A sensor's first message repeats its last revolution, which may
            // be minutes old: a reference for nothing until a new one arrives.
            self.previous = Some(current);
            return;
        };
        let dt = current.event_time.wrapping_sub(previous.event_time);
        let dr = current.count.wrapping_sub(previous.count);
        // Only a real event counts; a repeat or a stray count does not.
        if dt == 0 || dr == 0 {
            return;
        }
        let age_ms = received_ms.saturating_sub(previous.received_ms);
        let measurable = self.anchored && age_ms <= CRANK_STOPPED_MS;
        self.tenths = if measurable {
            let tenths = u64::from(dr) * 614_400 / u64::from(dt);
            (tenths <= MAX_CADENCE_TENTHS).then_some(tenths as u16)
        } else {
            // Spanning a stop or the session start would average the rest in
            // with the stroke: anchor here, and keep zero if it had turned.
            self.turned.then_some(0)
        };
        self.anchored = true;
        self.turned = true;
        self.previous = Some(current);
    }

    /// Cadence in tenths of rpm. A crank that has turned and sends no new
    /// revolution decays with the wait, to zero once it has stood for
    /// `CRANK_STOPPED_MS`. The wait is measured to the latest packet, so a
    /// lost packet never stops the crank.
    pub fn tenths(&self, now: u64) -> Option<u16> {
        if now.saturating_sub(self.received) > CADENCE_SILENT_MS {
            return None;
        }
        let previous = self.previous?;
        let age_ms = self.received.saturating_sub(previous.received_ms);
        if self.turned && age_ms > CRANK_STOPPED_MS {
            return Some(0);
        }
        let tenths = self.tenths?;
        let overdue_ms = age_ms.saturating_sub(PACKET_SLACK_MS).max(1);
        let bound = 600_000 / overdue_ms;
        Some(u64::from(tenths).min(bound) as u16)
    }
}

pub struct Speed {
    /// Wheel circumference in mm, which turns revolutions into distance. It
    /// changes only between rides, so a ride's distance never jumps.
    wheel_mm: u32,
    /// Revolutions counted since the sensor session began.
    revolutions: u64,
    previous: Option<Revolution>,
    epoch: Option<Epoch>,
    value: Option<u32>,
    /// The wheel has stood so long that its last revolution is no reference.
    standing: bool,
    received: u64,
}

impl Default for Speed {
    fn default() -> Self {
        Self {
            wheel_mm: WHEEL_MM,
            revolutions: 0,
            previous: None,
            epoch: None,
            value: None,
            standing: false,
            received: 0,
        }
    }
}

impl Speed {
    /// Measure with this circumference from now on.
    pub fn set_wheel_mm(&mut self, wheel_mm: u32) {
        self.wheel_mm = wheel_mm;
    }

    pub fn wheel_mm(&self) -> u32 {
        self.wheel_mm
    }

    /// Wheel revolutions counted since the sensor session began, to compare
    /// with GPS distance.
    pub fn revolutions(&self) -> u64 {
        self.revolutions
    }

    pub fn receive(&mut self, packet: Packet) {
        let epoch = Epoch::of(&packet);
        if self.epoch != Some(epoch) {
            *self = Self {
                wheel_mm: self.wheel_mm,
                epoch: Some(epoch),
                ..Self::default()
            };
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
            // Only a real event counts; a repeat or a stray count does not.
            if dt != 0 {
                self.revolutions += u64::from(dr);
            }
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
                let speed = u64::from(dr) * u64::from(self.wheel_mm) * 1024 / u64::from(dt);
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
        let bound = u64::from(self.wheel_mm) * 1000 / overdue_ms;
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

    /// A crank turning at `rpm` from `start_ms`, as an ANT+ sensor reports it
    /// in quarter-second packets, with event time and count starting anywhere.
    struct Crankset {
        rpm: u64,
        time0: u16,
        count0: u16,
        stops_at_ms: u64,
    }

    impl Crankset {
        fn packet(&self, at_ms: u64) -> Crank {
            let turning_ms = at_ms.min(self.stops_at_ms);
            let revs = turning_ms * self.rpm / 60_000;
            let last_ms = revs * 60_000 / self.rpm;
            Crank {
                event_time: self.time0.wrapping_add((last_ms * 1024 / 1000) as u16),
                count: self.count0.wrapping_add(revs as u16),
            }
        }
    }

    #[test]
    fn cadence_follows_a_crank_across_wraps_and_decays_when_it_stops() {
        use crate::testing::Rng;
        let mut rng = Rng::new(0xcad3);
        for case in 0..400 {
            let crank = Crankset {
                rpm: rng.range(25, 200),
                time0: rng.next() as u16,
                count0: if case % 2 == 0 {
                    65_500
                } else {
                    rng.next() as u16
                },
                stops_at_ms: rng.range(20_000, 200_000),
            };
            let mut cadence = Cadence::default();
            let mut now = 1_000_000 + rng.below(100_000);
            let start = now;
            // Long enough for the 16-bit event time to wrap more than once.
            while now - start < crank.stops_at_ms + 15_000 {
                cadence.receive(crank.packet(now - start), now);
                let measured = cadence.tenths(now);
                let since = now - start;
                // The first message and the first stroke only anchor.
                let settled = 3 * 60_000 / crank.rpm + 1_000;
                if since > settled && since < crank.stops_at_ms {
                    let want = crank.rpm * 10;
                    let got = u64::from(measured.expect("a turning crank reads"));
                    assert!(
                        got.abs_diff(want) <= want / 50 + 2,
                        "{case} {since} {got} {want}"
                    );
                }
                if since > crank.stops_at_ms + 5_000 {
                    assert_eq!(measured, Some(0), "{case} {since}");
                }
                now += 250;
            }
            // The sensor goes quiet: nothing is claimed.
            assert_eq!(cadence.tenths(now + 3_001), None);
        }
    }

    #[test]
    fn a_stopped_crank_only_slows_while_repeating_the_last_revolution() {
        let crank = Crankset {
            rpm: 90,
            time0: 100,
            count0: 7,
            stops_at_ms: 10_000,
        };
        let mut cadence = Cadence::default();
        let mut last = u16::MAX;
        for step in 0..60u64 {
            let at = step * 250;
            cadence.receive(crank.packet(at), 5_000 + at);
            let tenths = cadence.tenths(5_000 + at).unwrap_or(0);
            if at > 10_700 {
                assert!(tenths <= last, "{at} {tenths} {last}");
            }
            last = tenths;
        }
        assert_eq!(last, 0);
    }

    #[test]
    fn a_crank_that_never_turns_says_nothing_and_a_restart_forgets() {
        let still = Crank {
            event_time: 0,
            count: 0,
        };
        let mut cadence = Cadence::default();
        for step in 0..100u64 {
            cadence.receive(still, step * 250);
            assert_eq!(cadence.tenths(step * 250), None);
        }
        cadence.receive(
            Crank {
                event_time: 1024,
                count: 1,
            },
            25_000,
        );
        cadence.receive(
            Crank {
                event_time: 1024 + 683,
                count: 2,
            },
            25_667,
        );
        assert_eq!(cadence.tenths(25_700), Some(899));
        cadence.reset();
        assert_eq!(cadence.tenths(25_700), None);
    }

    #[test]
    fn the_first_stroke_after_a_stop_or_at_connection_is_not_an_average_of_the_rest() {
        let at = |event_time: u16, count: u16| Crank { event_time, count };
        // 90 rpm, a 20 s coast, one stroke: zero, not 3 rpm.
        let mut cadence = Cadence::default();
        cadence.receive(at(1000, 10), 0);
        cadence.receive(at(1683, 11), 667);
        cadence.receive(at(2366, 12), 1334);
        assert_eq!(cadence.tenths(1334), Some(899));
        for step in 1..=80u64 {
            cadence.receive(at(2366, 12), 1334 + step * 250);
        }
        assert_eq!(cadence.tenths(21_334), Some(0));
        cadence.receive(at(2366 + 20_480, 13), 21_500);
        assert_eq!(cadence.tenths(21_500), Some(0), "anchored, not measured");
        cadence.receive(at(2366 + 20_480 + 683, 14), 22_167);
        assert_eq!(cadence.tenths(22_167), Some(899));

        // A stop over a minute long reads zero throughout, with no flicker.
        let mut cadence = Cadence::default();
        cadence.receive(at(0, 0), 0);
        cadence.receive(at(683, 1), 667);
        cadence.receive(at(1366, 2), 1334);
        for step in 1..=400u64 {
            cadence.receive(at(1366, 2), 1334 + step * 250);
        }
        cadence.receive(at(1366 + 30_000, 3), 101_500);
        assert_eq!(cadence.tenths(101_500), Some(0));

        // Connecting to a sensor whose last stroke was long ago: the first
        // message is no reference, however its event time wraps.
        for old in [0u16, 3000, 40_000, 65_000] {
            let mut cadence = Cadence::default();
            cadence.receive(at(old, 500), 0);
            cadence.receive(at(old, 500), 250);
            assert_eq!(cadence.tenths(250), None);
            cadence.receive(at(old.wrapping_add(60_000), 501), 1000);
            assert_eq!(cadence.tenths(1000), None, "{old}");
            cadence.receive(at(old.wrapping_add(60_683), 502), 1667);
            assert_eq!(cadence.tenths(1667), Some(899), "{old}");
        }
    }

    #[test]
    fn hostile_crank_counters_stay_bounded_and_never_overflow() {
        use crate::testing::Rng;
        let mut rng = Rng::new(7);
        let mut cadence = Cadence::default();
        let mut now = u64::MAX - 400_000;
        for _ in 0..20_000 {
            let crank = Crank {
                event_time: rng.next() as u16,
                count: rng.next() as u16,
            };
            now = now.saturating_add(rng.below(90_000));
            cadence.receive(crank, rng.next().min(now));
            for at in [now, now.saturating_add(10_000), 0, u64::MAX] {
                if let Some(tenths) = cadence.tenths(at) {
                    assert!(u64::from(tenths) <= MAX_CADENCE_TENTHS);
                }
            }
        }
    }

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
    fn revolutions_count_across_the_wrap_and_the_size_scales_speed() {
        let mut speed = Speed::default();
        speed.receive(packet(0, 65534, 0, 0));
        speed.receive(packet(1024, 1, 1000, 0));
        assert_eq!(speed.revolutions(), 3);
        // A repeated event adds nothing.
        speed.receive(packet(1024, 1, 1250, 0));
        assert_eq!(speed.revolutions(), 3);
        speed.set_wheel_mm(2200);
        speed.receive(packet(2048, 4, 2000, 0));
        assert_eq!(speed.revolutions(), 6);
        assert_eq!(speed.value(2000), Some(3 * 2200));
        // A new sensor session restarts the count but keeps the size.
        let mut other = packet(0, 9, 3000, 0);
        other.generation = 2;
        speed.receive(other);
        assert_eq!(speed.revolutions(), 0);
        assert_eq!(speed.wheel_mm(), 2200);
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
