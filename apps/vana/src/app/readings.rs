//! Sensor data intake and the live ride sample built from it.
use super::{Observations, Runtime};
use crate::ride::log::Sample;
use crate::ride::metrics::{Gradient, Odometer, Speed};
use crate::sensors::ant::{Epoch, HeartRate, Power, device_type};
use crate::sensors::ble_profile::{self, Profile};
use crate::sensors::radar::{Alert, Radar};
use device_api::ant::{Ant, CHANNEL_CAPACITY, LinkState, PACKET_CAPACITY, Packet};
use device_api::ble_transport::Ble;
use device_api::gps::FixState;
use device_api::input::InputObservation;
use device_api::observation::Observation;
use device_api::positioning::Positioning;

/// Decoded readings older than this no longer describe the rider.
const READING_FRESH_MS: u64 = 3000;
/// The BLE core queue holds two packets; never drain an unbounded producer here.
const BLE_PACKETS_PER_TICK: usize = 2;

/// A decoded value and when its packet arrived.
#[derive(Clone, Copy, Debug)]
pub(super) struct Timed<T> {
    pub(super) value: T,
    pub(super) received_ms: u64,
}

/// ANT profiles decoded here, whose state restarts with each channel session.
#[derive(Clone, Copy)]
enum Decoded {
    Radar,
    Heart,
    Power,
}

impl Decoded {
    const ALL: [Self; 3] = [Self::Radar, Self::Heart, Self::Power];

    const fn device_type(self) -> u8 {
        match self {
            Self::Radar => device_type::RADAR,
            Self::Heart => device_type::HEART_RATE,
            Self::Power => device_type::POWER,
        }
    }

    fn of(kind: u8) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|decoded| decoded.device_type() == kind)
    }
}

pub(super) struct Readings {
    /// The sample most recently offered to the recorder.
    pub(super) live: Sample,
    speed: Speed,
    odometer: Odometer,
    gradient: Gradient,
    pub(super) heart: Option<Timed<HeartRate>>,
    pub(super) power: Option<Timed<Power>>,
    pub(super) radar: Radar,
    pub(super) radar_alert: Alert,
    epochs: [Option<Epoch>; Decoded::ALL.len()],
}

impl Default for Readings {
    fn default() -> Self {
        Self {
            live: Sample::default(),
            speed: Speed::default(),
            odometer: Odometer::default(),
            gradient: Gradient::default(),
            heart: None,
            power: None,
            radar: Radar::new(),
            radar_alert: Alert::default(),
            epochs: [None; Decoded::ALL.len()],
        }
    }
}

impl Readings {
    fn restart(&mut self, decoded: Decoded) {
        match decoded {
            Decoded::Radar => self.radar.reset(),
            Decoded::Heart => self.heart = None,
            Decoded::Power => self.power = None,
        }
    }

    fn receive(&mut self, packet: Packet) {
        let kind = packet.identity.device_type;
        if matches!(kind, device_type::SPEED | device_type::SPEED_CADENCE) {
            self.speed.receive(packet);
            return;
        }
        let Some(decoded) = Decoded::of(kind) else {
            return;
        };
        let epoch = Some(Epoch::of(&packet));
        if self.epochs[decoded as usize] != epoch {
            self.restart(decoded);
            self.epochs[decoded as usize] = epoch;
        }
        let received_ms = packet.received_ms;
        match decoded {
            Decoded::Radar => {
                self.radar.receive(packet.data, received_ms);
            }
            Decoded::Heart => {
                let value = HeartRate::decode(packet.data);
                self.heart = Some(Timed { value, received_ms });
            }
            Decoded::Power => {
                if let Some(value) = Power::decode(packet.data) {
                    self.power = Some(Timed { value, received_ms });
                }
            }
        }
    }
}

impl Runtime {
    pub fn select_profile(&mut self, profile: Profile, previous_connection: u32) {
        self.sensors.select_profile(profile, previous_connection);
    }

    pub fn radar(&self, now: u64) -> crate::sensors::radar::Snapshot {
        self.readings.radar.snapshot(now)
    }

    pub(super) fn receive_ant(&mut self, ant: &mut impl Ant, now: u64) {
        for decoded in Decoded::ALL {
            let epoch = &mut self.readings.epochs[decoded as usize];
            let channel = ant.channel(decoded.device_type(), now);
            if channel.is_none_or(|s| {
                s.link != LinkState::Connected
                    || epoch.is_some_and(|epoch| epoch.generation != s.generation)
            }) {
                *epoch = None;
                self.readings.restart(decoded);
            }
        }
        for _ in 0..PACKET_CAPACITY * CHANNEL_CAPACITY {
            let Some(packet) = ant.take_packet() else {
                break;
            };
            self.readings.receive(packet);
        }
    }

    pub(super) fn receive_ble(&mut self, ble: &mut impl Ble) {
        self.sensors.update(ble.snapshot());
        for _ in 0..BLE_PACKETS_PER_TICK {
            let Some(packet) = ble.take_packet() else {
                break;
            };
            self.sensors.packet(packet);
        }
    }

    pub(super) fn sample(
        &mut self,
        now: u64,
        ant: &impl Ant,
        observations: &Observations<'_, impl Positioning, impl InputObservation>,
    ) -> Sample {
        let ble = self.sensors.snapshot(now);
        let (ble_heart_bpm, cadence_tenths) =
            ble_profile::ride_fields(ble, self.ride.recorder.source());
        let location_e7 = observations.position.snapshot(now).and_then(|snapshot| {
            (snapshot.gps.state == FixState::Fresh)
                .then(|| Some((snapshot.gps.latitude_e7?, snapshot.gps.longitude_e7?)))
                .flatten()
        });
        let clock = firmware_services::network_time::snapshot(now, 0, observations.network_online);
        let utc_ms = clock.unix_seconds.and_then(|seconds| {
            seconds
                .checked_mul(1_000)?
                .checked_add(u64::from(clock.millis))
        });
        // Keep existing last-observed battery behavior. Core status still exposes
        // its age; these ride format bytes have no battery-age field.
        let battery_percent = observations
            .input
            .snapshot(now)
            .and_then(|snapshot| match snapshot.battery {
                Observation::Fresh {
                    value: (percent, _),
                    ..
                }
                | Observation::Stale {
                    value: (percent, _),
                    ..
                } => Some(percent),
                Observation::Unavailable => None,
            });
        let speed_mm_s = [device_type::SPEED, device_type::SPEED_CADENCE]
            .into_iter()
            .any(|kind| {
                ant.channel(kind, now)
                    .is_some_and(|s| s.link == LinkState::Connected && !s.stale)
            })
            .then(|| self.readings.speed.value(now))
            .flatten();
        let readings = &mut self.readings;
        if readings.odometer.step(now, speed_mm_s) {
            let pressure = match observations.physical.pressure {
                Observation::Fresh { value, .. } => Some(value.pressure_centi_pa),
                _ => None,
            };
            let distance_mm = readings.odometer.distance_mm();
            readings.gradient.update(pressure, distance_mm, speed_mm_s);
        }
        let fresh = |received_ms: u64| now.saturating_sub(received_ms) <= READING_FRESH_MS;
        let sample = Sample {
            active_ms: 0,
            utc_ms,
            location_e7,
            demo_speed_mm_s: None,
            heart_bpm: readings
                .heart
                .filter(|heart| fresh(heart.received_ms))
                .and_then(|heart| heart.value.bpm)
                .map(u16::from)
                .or(ble_heart_bpm),
            cadence_tenths,
            battery_percent,
            speed_mm_s,
            power_watts: readings
                .power
                .filter(|power| fresh(power.received_ms) && power.value.watts != u16::MAX)
                .map(|power| power.value.watts),
            gradient_tenths: readings.gradient.tenths(),
        };
        readings.live = sample;
        sample
    }
}
