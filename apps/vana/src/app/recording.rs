//! Sensor acquisition, metric snapshots and bounded recording work.
use super::Runtime;
use crate::{ride::log::Sample, sensors::ble_profile::Profile};
use device_api::{gps::FixState, observation::Observation};

impl Runtime {
    pub fn select_profile(&mut self, profile: Profile, previous_connection: u32) {
        self.sensors.select_profile(profile, previous_connection);
    }

    pub fn recording(&self) -> bool {
        // Protect accepted writes and active/paused rides from host-triggered restart.
        self.pending.is_some()
            || matches!(
                self.recorder.status(),
                crate::ride::log::Status::Recording
                    | crate::ride::log::Status::Paused
                    | crate::ride::log::Status::Formatting
                    | crate::ride::log::Status::Clearing
            )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn tick(
        &mut self,
        store: &mut impl crate::ride::storage::RideStorage,
        now: u64,
        ant: &mut impl device_api::ant::Ant,
        ble: &mut impl device_api::ble_transport::Ble,
        position: &impl device_api::positioning::Positioning,
        network_online: bool,
        input: &impl device_api::input::InputObservation,
        physical: device_api::sensors::Snapshot,
    ) {
        let was_scanning = self.scan.active();
        self.scan.tick(ant, now);
        if was_scanning || self.scan.active() {
            self.menu.set_message(self.scan.message());
        }
        let channels = ant.channels(now);
        for (index, channel) in channels.iter().enumerate() {
            let Some(channel) = channel else { continue };
            let Some(peer) = channel.selected else {
                continue;
            };
            if self.display_active
                && !self.scan.active()
                && !self.dropped_ant.contains(&Some(peer.device_type))
                && channel.link == device_api::ant::LinkState::Disconnected
                && now >= self.next_reconnect[index]
                && ant.request(device_api::ant::AntOperation::Connect(peer), now)
                    == Ok(device_api::ant::Admission::Accepted)
            {
                self.next_reconnect[index] = now.saturating_add(5000);
            }
        }
        for (index, kind) in [40, 120, 11].iter().enumerate() {
            let channel = ant.channel(*kind, now);
            if channel.is_none_or(|s| {
                s.link != device_api::ant::LinkState::Connected
                    || self.ant_epochs[index]
                        .is_some_and(|(generation, _)| generation != s.generation)
            }) {
                match index {
                    0 => self.radar.reset(),
                    1 => self.heart = None,
                    _ => self.power = None,
                }
                self.ant_epochs[index] = None;
            }
        }
        for _ in 0..device_api::ant::PACKET_CAPACITY * device_api::ant::CHANNEL_CAPACITY {
            let Some(packet) = ant.take_packet() else {
                break;
            };
            if matches!(packet.identity.device_type, 123 | 121) {
                self.speed.receive(packet);
                continue;
            }
            let Some(index) = [40, 120, 11]
                .iter()
                .position(|kind| *kind == packet.identity.device_type)
            else {
                continue;
            };
            let epoch = (packet.generation, packet.loss_count);
            if self.ant_epochs[index] != Some(epoch) {
                match index {
                    0 => self.radar.reset(),
                    1 => self.heart = None,
                    _ => self.power = None,
                }
                self.ant_epochs[index] = Some(epoch);
            }
            match index {
                0 => {
                    self.radar.receive(packet.data, packet.received_ms);
                }
                1 => {
                    self.heart = Some((
                        crate::sensors::ant::HeartRate::decode(packet.data),
                        packet.received_ms,
                    ))
                }
                _ => {
                    if let Some(power) = crate::sensors::ant::Power::decode(packet.data) {
                        self.power = Some((power, packet.received_ms));
                    }
                }
            }
        }
        let transport = ble.snapshot();
        self.sensors.update(transport);
        // Core queue has two packets. Never drain an unbounded producer here.
        for _ in 0..2 {
            let Some(packet) = ble.take_packet() else {
                break;
            };
            self.sensors.packet(packet);
        }
        let sensors = self.sensors.snapshot(now);
        let (heart_bpm, cadence_tenths) =
            crate::sensors::ble_profile::ride_fields(sensors, self.recorder.source());
        let location_e7 = position.snapshot(now).and_then(|snapshot| {
            (snapshot.gps.state == FixState::Fresh)
                .then(|| Some((snapshot.gps.latitude_e7?, snapshot.gps.longitude_e7?)))
                .flatten()
        });
        let clock = firmware_services::network_time::snapshot(now, 0, network_online);
        let utc_ms = clock.unix_seconds.and_then(|seconds| {
            seconds
                .checked_mul(1_000)?
                .checked_add(u64::from(clock.millis))
        });
        // Keep existing last-observed battery behavior. Core status still exposes
        // its age; these ride format bytes have no battery-age field.
        let battery_percent = input
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
        let speed_mm_s = [123, 121]
            .into_iter()
            .any(|kind| {
                ant.channel(kind, now)
                    .is_some_and(|s| s.link == device_api::ant::LinkState::Connected && !s.stale)
            })
            .then(|| self.speed.value(now))
            .flatten();
        if now >= self.metric_at.saturating_add(1000) {
            let elapsed = now.saturating_sub(self.metric_at).min(3000);
            self.metric_at = now;
            self.distance_mm = self
                .distance_mm
                .saturating_add(u64::from(speed_mm_s.unwrap_or(0)) * elapsed / 1000);
            if let Observation::Fresh { value, .. } = physical.pressure {
                if let Some((pressure, distance)) = self.pressure_base {
                    let travelled = self.distance_mm.saturating_sub(distance);
                    if travelled >= 30_000 {
                        // Local barometric derivative: dh = -8434 * dp / p, over >=30 m.
                        let grade = (i64::from(pressure) - i64::from(value.pressure_centi_pa))
                            * 8_434_000_000i64
                            / i64::from(pressure)
                            / travelled as i64;
                        self.live.gradient_tenths = (grade.abs() <= 400).then_some(grade as i16);
                        self.pressure_base = Some((value.pressure_centi_pa, self.distance_mm));
                    }
                } else {
                    self.pressure_base = Some((value.pressure_centi_pa, self.distance_mm));
                }
            } else {
                self.live.gradient_tenths = None;
                self.pressure_base = None;
            }
            if speed_mm_s.is_none_or(|speed| speed < 1000) {
                self.live.gradient_tenths = None;
                self.pressure_base = None;
            }
        }
        let sample = Sample {
            active_ms: 0,
            utc_ms,
            location_e7,
            demo_speed_mm_s: None,
            heart_bpm: self
                .heart
                .filter(|(_, at)| now.saturating_sub(*at) <= 3000)
                .and_then(|(heart, _)| heart.bpm)
                .map(u16::from)
                .or(heart_bpm),
            cadence_tenths,
            battery_percent,
            speed_mm_s,
            power_watts: self
                .power
                .filter(|(p, at)| now.saturating_sub(*at) <= 3000 && p.watts != u16::MAX)
                .map(|(p, _)| p.watts),
            gradient_tenths: self.live.gradient_tenths,
        };
        self.live = sample;
        let clock = self.clock;
        self.recorder
            .service(store, now, sample, &|| clock.map_or(now, |clock| clock()));
        if let Some(result) = self.recorder.take_result() {
            self.pending = None;
            self.completion = Some(result);
        }
    }
}
