//! Sensor data intake and the live ride sample built from it.
use super::{Observations, Runtime};
use crate::ride::auto::{Motion, Source};
use crate::ride::gps_speed::{GpsSpeed, Ground};
use crate::ride::log::Sample;
use crate::ride::metrics::{Gradient, Odometer, Power3s, Speed};
use crate::sensors::ant::{Epoch, HeartRate, Power, device_type};
use crate::sensors::ble_profile::{self, Profile};
use crate::sensors::radar::{Alert, Radar};
use device_api::ant::{Ant, CHANNEL_CAPACITY, LinkState, PACKET_CAPACITY, Packet};
use device_api::ble_transport::Ble;
use device_api::gps::FixState;
use device_api::input::InputObservation;
use device_api::observation::Observation;
use device_api::positioning::Positioning;
use firmware_services::network_time;

/// Decoded readings older than this no longer describe the rider.
const READING_FRESH_MS: u64 = 3000;
/// The BLE core queue holds two packets; never drain an unbounded producer here.
const BLE_PACKETS_PER_TICK: usize = 2;

/// A decoded value and when its packet arrived.
#[derive(Clone, Copy)]
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
    /// Ground speed from GPS, the fallback when no wheel sensor measures.
    gps: GpsSpeed,
    /// What the speed sources said at the last sample, for auto-pause.
    pub(super) motion: Motion,
    odometer: Odometer,
    gradient: Gradient,
    pub(super) heart: Option<Timed<HeartRate>>,
    pub(super) power: Option<Timed<Power>>,
    power_3s: Power3s,
    /// 3 s power at the last sample, for the ride page.
    pub(super) power_3s_watts: Option<u16>,
    pub(super) radar: Radar,
    pub(super) radar_alert: Alert,
    epochs: [Option<Epoch>; Decoded::ALL.len()],
    pub(super) battery: BatteryWatch,
}

/// Battery levels the rider is told about once per discharge, and whether
/// each is critical.
pub(super) const BATTERY_WARNINGS: [(u8, bool); 3] = [(20, false), (10, true), (5, true)];

/// News about the battery worth a toast.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BatteryNews {
    /// USB power arrived; the charge it started from.
    Charging(u8),
    /// The charge fell to this level.
    Low(u8),
    /// The charge fell to this level and a ride may not last.
    Critical(u8),
}

impl BatteryNews {
    /// A low or critical level, which also sounds.
    pub(super) const fn warning(self) -> bool {
        matches!(self, Self::Low(_) | Self::Critical(_))
    }

    pub(super) fn toast(self) -> crate::screens::chrome::Toast {
        use crate::screens::chrome::{Toast, Tone};
        use core::fmt::Write;
        let mut text = crate::screens::workout::Text::<20>::new();
        let tone = match self {
            Self::Charging(percent) => {
                let _ = write!(text, "CHARGING {percent}%");
                Tone::Good
            }
            Self::Low(percent) => {
                let _ = write!(text, "LOW BATTERY {percent}%");
                Tone::Warn
            }
            Self::Critical(percent) => {
                let _ = write!(text, "CHARGE SOON {percent}%");
                Tone::Alert
            }
        };
        Toast::new(text.as_str(), tone)
    }
}

/// The battery as the device last reported it, and which warnings this
/// discharge has given. Warnings fire once as the charge falls through each
/// level and are given again only after charging.
#[derive(Clone, Copy, Debug, Default)]
pub(super) struct BatteryWatch {
    percent: Option<u8>,
    charging: Option<bool>,
    /// The lowest warning level already given since the last charge.
    warned: Option<u8>,
}

impl BatteryWatch {
    /// USB power is connected: stock reads companion power status zero as
    /// charging.
    pub(super) fn charging(&self) -> bool {
        self.charging == Some(true)
    }

    /// Take a new report; returns news for the rider, if any. The first
    /// report only sets the scene: a device switched on low or on USB says
    /// so in its status bar, not with a toast.
    pub(super) fn observe(
        &mut self,
        percent: Option<u8>,
        charging: Option<bool>,
    ) -> Option<BatteryNews> {
        let first = self.percent.is_none();
        let plugged = charging == Some(true) && self.charging == Some(false);
        if charging.is_some() {
            self.charging = charging;
        }
        if percent.is_some() {
            self.percent = percent;
        }
        let percent = self.percent?;
        if self.charging() {
            self.warned = None;
            return plugged.then_some(BatteryNews::Charging(percent));
        }
        // Levels already below on the first report count as told.
        let due = BATTERY_WARNINGS
            .iter()
            .filter(|(level, _)| percent <= *level)
            .rfind(|(level, _)| self.warned.is_none_or(|warned| *level < warned))
            .copied();
        let (level, critical) = due?;
        self.warned = Some(level);
        let news = if critical {
            BatteryNews::Critical(percent)
        } else {
            BatteryNews::Low(percent)
        };
        (!first).then_some(news)
    }
}

impl Default for Readings {
    fn default() -> Self {
        Self {
            live: Sample::default(),
            speed: Speed::default(),
            gps: GpsSpeed::default(),
            motion: Motion::Lost,
            odometer: Odometer::default(),
            gradient: Gradient::default(),
            heart: None,
            power: None,
            power_3s: Power3s::default(),
            power_3s_watts: None,
            radar: Radar::new(),
            radar_alert: Alert::default(),
            epochs: [None; Decoded::ALL.len()],
            battery: BatteryWatch::default(),
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

/// A measuring wheel sensor first, then GPS; a live wheel without a speed
/// yet only when GPS cannot measure either.
fn motion(wheel_fresh: bool, wheel: Option<u32>, ground: Ground) -> Motion {
    match (wheel, ground) {
        (Some(speed), _) => Motion::Measured(Source::Wheel, speed),
        (None, Ground::Measured(speed)) => Motion::Measured(Source::Gps, speed),
        (None, _) if wheel_fresh => Motion::Unknown(Source::Wheel),
        (None, Ground::Unknown) => Motion::Unknown(Source::Gps),
        (None, Ground::Lost) => Motion::Lost,
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
            // A removed or replaced sensor's pages never reach the ride.
            if !self.links.rejects(packet.identity) {
                self.readings.receive(packet);
            }
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

    /// The live heart rate from ANT+ or BLE, whether or not a ride is open,
    /// which `sample` withholds from a BLE strap outside one.
    pub(super) fn live_heart_bpm(&mut self, now: u64) -> Option<u16> {
        self.readings
            .heart
            .filter(|heart| now.saturating_sub(heart.received_ms) <= READING_FRESH_MS)
            .and_then(|heart| heart.value.bpm)
            .map(u16::from)
            .or(self.sensors.snapshot(now).heart_bpm)
    }

    pub(super) fn sample(
        &mut self,
        now: u64,
        ant: &impl Ant,
        observations: &Observations<'_, impl Positioning, impl InputObservation>,
    ) -> Sample {
        let ble = self.sensors.snapshot(now);
        let (ble_heart_bpm, cadence_tenths) =
            ble_profile::ride_fields(ble, self.ride.active().source());
        let position = observations.position.snapshot(now);
        let location_e7 = position.and_then(|snapshot| {
            (snapshot.gps.state == FixState::Fresh)
                .then(|| Some((snapshot.gps.latitude_e7?, snapshot.gps.longitude_e7?)))
                .flatten()
        });
        let network = network_time::snapshot(now, 0, observations.network_online);
        let utc_ms = utc_ms(network, position.and_then(|snapshot| snapshot.gps.utc_ms));
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
        let wheel_fresh = [device_type::SPEED, device_type::SPEED_CADENCE]
            .into_iter()
            .any(|kind| {
                ant.channel(kind, now)
                    .is_some_and(|s| s.link == LinkState::Connected && !s.stale)
            });
        let wheel = wheel_fresh
            .then(|| self.readings.speed.value(now))
            .flatten();
        self.readings
            .gps
            .observe(now, position.as_ref().map(|snapshot| &snapshot.gps));
        let motion = motion(wheel_fresh, wheel, self.readings.gps.reading(now));
        self.readings.motion = motion;
        let speed_mm_s = match motion {
            Motion::Measured(_, speed) => Some(speed),
            _ => None,
        };
        let pressure = match observations.physical.pressure {
            Observation::Fresh { value, .. } => Some(value.pressure_centi_pa),
            _ => None,
        };
        let readings = &mut self.readings;
        let step = readings.odometer.step(now, speed_mm_s);
        if step.is_some() {
            let distance_mm = readings.odometer.distance_mm();
            readings.gradient.update(pressure, distance_mm, speed_mm_s);
        }
        let fresh = |received_ms: u64| now.saturating_sub(received_ms) <= READING_FRESH_MS;
        let power = readings
            .power
            .filter(|power| fresh(power.received_ms))
            .map(|power| power.value);
        let mut sample = Sample {
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
            // A power meter's crank cadence stands in when no BLE sensor reports one.
            cadence_tenths: cadence_tenths.or_else(|| {
                power
                    .and_then(|power| power.cadence_rpm)
                    .map(|rpm| u16::from(rpm) * 10)
            }),
            battery_percent,
            speed_mm_s,
            power_watts: power
                .filter(|power| power.watts != u16::MAX)
                .map(|power| power.watts),
            gradient_tenths: readings.gradient.tenths(),
            altitude: None,
            lap: None,
        };
        if let Some(step) = step {
            let profile = self.preferences.profile;
            if let Some(event) = self.ride.count(now, step, &sample, pressure, &profile) {
                self.ride_climb_event(event);
            }
            if !self.ride.workout_events.is_empty() {
                self.workout_events(now);
            }
        }
        sample.altitude = self.ride.recorded_altitude(pressure);
        self.readings.power_3s_watts = self.readings.power_3s.update(now, sample.power_watts);
        self.readings.live = sample;
        sample
    }
}

/// UTC now: Wi-Fi SNTP when it has synced since boot or sleep, else the
/// receiver clock. SNTP goes first because it is there from a ride's start or
/// not at all, while GNSS time shows up in nearly every outdoor ride. The
/// source then changes mid-ride only if Wi-Fi appears mid-ride.
fn utc_ms(network: network_time::Snapshot, receiver: Option<u64>) -> Option<u64> {
    network
        .unix_seconds
        .and_then(|seconds| {
            seconds
                .checked_mul(1_000)?
                .checked_add(u64::from(network.millis))
        })
        .or(receiver)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_low_level_warns_once_per_discharge_and_charging_rearms_them() {
        let mut watch = BatteryWatch::default();
        let mut feed = |percent, charging| watch.observe(Some(percent), Some(charging));
        // Switched on low: the status bar says so, no toast.
        assert_eq!(feed(18, false), None);
        assert_eq!(feed(15, false), None, "20% was already passed");
        assert_eq!(feed(10, false), Some(BatteryNews::Critical(10)));
        assert_eq!(feed(9, false), None);
        assert_eq!(feed(4, false), Some(BatteryNews::Critical(4)));
        assert_eq!(feed(3, false), None);
        assert_eq!(feed(3, true), Some(BatteryNews::Charging(3)));
        assert_eq!(feed(30, true), None);
        assert_eq!(feed(30, false), None, "unplugging is quiet");
        assert_eq!(feed(20, false), Some(BatteryNews::Low(20)));
        // A sudden drop past two levels gives only the more urgent one.
        assert_eq!(feed(9, false), Some(BatteryNews::Critical(9)));
        assert_eq!(feed(5, false), Some(BatteryNews::Critical(5)));
    }

    #[test]
    fn missing_reports_keep_the_last_known_state() {
        let mut watch = BatteryWatch::default();
        assert_eq!(watch.observe(None, None), None);
        assert!(!watch.charging());
        assert_eq!(watch.observe(Some(50), Some(false)), None);
        assert_eq!(
            watch.observe(None, Some(true)),
            Some(BatteryNews::Charging(50))
        );
        assert!(watch.charging());
        assert_eq!(watch.observe(Some(51), None), None);
        assert!(watch.charging());
    }

    #[test]
    fn battery_toasts_read_as_news() {
        assert_eq!(BatteryNews::Charging(64).toast().text(), b"CHARGING 64%");
        assert_eq!(BatteryNews::Low(20).toast().text(), b"LOW BATTERY 20%");
        assert_eq!(BatteryNews::Critical(5).toast().text(), b"CHARGE SOON 5%");
    }

    #[test]
    fn sntp_wins_and_the_receiver_fills_in() {
        let synced = network_time::Snapshot {
            unix_seconds: Some(1_792_000_000),
            millis: 250,
            ..Default::default()
        };
        let receiver = Some(1_792_000_009_000);
        assert_eq!(utc_ms(synced, receiver), Some(1_792_000_000_250));
        assert_eq!(utc_ms(synced, None), Some(1_792_000_000_250));
        let offline = network_time::Snapshot::default();
        assert_eq!(utc_ms(offline, receiver), receiver);
        assert_eq!(utc_ms(offline, None), None);
    }

    #[test]
    fn a_measuring_wheel_wins_then_gps_then_a_live_wheel() {
        use Ground::{Lost, Measured, Unknown};
        let wheel = Motion::Measured(Source::Wheel, 5_000);
        let gps = Motion::Measured(Source::Gps, 6_000);
        let rows = [
            (true, Some(5_000), Measured(6_000), wheel),
            (true, Some(5_000), Lost, wheel),
            (true, None, Measured(6_000), gps),
            (false, None, Measured(6_000), gps),
            (true, None, Unknown, Motion::Unknown(Source::Wheel)),
            (true, None, Lost, Motion::Unknown(Source::Wheel)),
            (false, None, Unknown, Motion::Unknown(Source::Gps)),
            (false, None, Lost, Motion::Lost),
        ];
        for (fresh, speed, ground, expected) in rows {
            assert_eq!(
                motion(fresh, speed, ground),
                expected,
                "{fresh} {speed:?} {ground:?}"
            );
        }
    }
}
