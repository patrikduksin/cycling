//! Bounded parsers and snapshots for standard Heart Rate and Cycling Speed and Cadence sensors.

pub const STALE_MS: u64 = 5_000;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Profile {
    #[default]
    Echo,
    HeartRate,
    Cadence,
}

impl Profile {
    pub const fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::HeartRate,
            2 => Self::Cadence,
            _ => Self::Echo,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Echo => "echo",
            Self::HeartRate => "heart",
            Self::Cadence => "cadence",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Link {
    #[default]
    Off,
    Scanning,
    Connecting,
    Connected,
    Retrying,
    Failed,
}

impl Link {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Scanning => "scanning",
            Self::Connecting => "connecting",
            Self::Connected => "connected",
            Self::Retrying => "retrying",
            Self::Failed => "failed",
        }
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub profile: Profile,
    pub link: Link,
    pub heart_bpm: Option<u16>,
    pub heart_age_ms: Option<u64>,
    pub cadence_tenths: Option<u16>,
    pub cadence_age_ms: Option<u64>,
    pub connections: u32,
    pub disconnections: u32,
    pub notifications: u32,
    pub invalid: u32,
    pub rr_dropped: u32,
}

pub const fn fresh_u16(value: u16, received_ms: u64, now_ms: u64) -> (Option<u16>, Option<u64>) {
    let age = now_ms.saturating_sub(received_ms);
    if age <= STALE_MS {
        (Some(value), Some(age))
    } else {
        (None, Some(age))
    }
}

/// Only durable live rides receive fresh standard-sensor values.
pub const fn ride_fields(
    snapshot: Snapshot,
    source: Option<crate::sdk::ride_log::Source>,
) -> (Option<u16>, Option<u16>) {
    if matches!(source, Some(crate::sdk::ride_log::Source::Live)) {
        (snapshot.heart_bpm, snapshot.cadence_tenths)
    } else {
        (None, None)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartRate {
    pub bpm: u16,
    pub contact_supported: bool,
    pub contact_detected: Option<bool>,
    pub energy: Option<u16>,
    pub rr: [u16; 4],
    pub rr_len: u8,
    /// Additional complete RR intervals that did not fit in `rr`.
    pub rr_dropped: u8,
}

impl HeartRate {
    pub fn parse(value: &[u8]) -> Option<Self> {
        let flags = *value.first()?;
        if flags & 0xe0 != 0 || flags & 0x02 != 0 && flags & 0x04 == 0 {
            return None;
        }
        let mut at = 1;
        let bpm = if flags & 0x01 != 0 {
            take_u16(value, &mut at)?
        } else {
            let bpm = u16::from(*value.get(at)?);
            at += 1;
            bpm
        };
        let energy = if flags & 0x08 != 0 {
            Some(take_u16(value, &mut at)?)
        } else {
            None
        };
        let mut rr = [0; 4];
        let mut rr_len = 0usize;
        let mut rr_dropped = 0u8;
        if flags & 0x10 != 0 {
            if value.len() == at || !(value.len() - at).is_multiple_of(2) {
                return None;
            }
            while at < value.len() {
                let interval = take_u16(value, &mut at)?;
                if rr_len < rr.len() {
                    rr[rr_len] = interval;
                    rr_len += 1;
                } else {
                    rr_dropped = rr_dropped.saturating_add(1);
                }
            }
        } else if at != value.len() {
            return None;
        }
        Some(Self {
            bpm,
            contact_supported: flags & 0x04 != 0,
            contact_detected: (flags & 0x04 != 0).then_some(flags & 0x02 != 0),
            energy,
            rr,
            rr_len: rr_len as u8,
            rr_dropped,
        })
    }
}

pub const fn usable_heart(value: HeartRate) -> Option<u16> {
    if matches!(value.contact_detected, Some(false)) {
        None
    } else {
        Some(value.bpm)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CscMeasurement {
    pub wheel: Option<(u32, u16)>,
    pub crank: Option<(u16, u16)>,
}

impl CscMeasurement {
    pub fn parse(value: &[u8]) -> Option<Self> {
        let flags = *value.first()?;
        if flags & !0x03 != 0 || flags == 0 {
            return None;
        }
        let mut at = 1;
        let wheel = if flags & 0x01 != 0 {
            Some((take_u32(value, &mut at)?, take_u16(value, &mut at)?))
        } else {
            None
        };
        let crank = if flags & 0x02 != 0 {
            Some((take_u16(value, &mut at)?, take_u16(value, &mut at)?))
        } else {
            None
        };
        (at == value.len()).then_some(Self { wheel, crank })
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CrankCadence {
    previous: Option<(u16, u16, u64)>,
}

impl CrankCadence {
    pub const MAX_TENTHS: u32 = 2_500;

    /// Returns tenths of an RPM. Duplicate times and gaps over ten seconds have no rate.
    pub fn update(&mut self, revolutions: u16, event_time: u16, received_ms: u64) -> Option<u32> {
        let previous = self
            .previous
            .replace((revolutions, event_time, received_ms))?;
        if received_ms.saturating_sub(previous.2) > 10_000 {
            return None;
        }
        let delta_time = event_time.wrapping_sub(previous.1);
        if delta_time == 0 || delta_time > 10 * 1024 {
            return None;
        }
        let delta_revolutions = revolutions.wrapping_sub(previous.0);
        let value = u64::from(delta_revolutions) * 60 * 1024 * 10 / u64::from(delta_time);
        (value <= u64::from(Self::MAX_TENTHS)).then_some(value as u32)
    }

    pub fn disconnected(&mut self) {
        self.previous = None;
    }
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CadenceReading {
    rate: CrankCadence,
    latest: Option<(u16, u64)>,
}

impl CadenceReading {
    pub fn update(&mut self, revolutions: u16, event_time: u16, received_ms: u64) -> Option<u16> {
        let value = self.rate.update(revolutions, event_time, received_ms)? as u16;
        self.latest = Some((value, received_ms));
        Some(value)
    }

    pub const fn snapshot(self, now_ms: u64) -> (Option<u16>, Option<u64>) {
        match self.latest {
            Some((value, at)) => fresh_u16(value, at, now_ms),
            None => (None, None),
        }
    }

    pub fn disconnected(&mut self) {
        self.rate.disconnected();
        self.latest = None;
    }
}

fn take_u16(value: &[u8], at: &mut usize) -> Option<u16> {
    let result = u16::from_le_bytes(value.get(*at..*at + 2)?.try_into().ok()?);
    *at += 2;
    Some(result)
}

fn take_u32(value: &[u8], at: &mut usize) -> Option<u32> {
    let result = u32::from_le_bytes(value.get(*at..*at + 4)?.try_into().ok()?);
    *at += 4;
    Some(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn readings_become_stale_at_a_bounded_age() {
        assert_eq!(fresh_u16(72, 100, 5_100), (Some(72), Some(5_000)));
        assert_eq!(fresh_u16(72, 100, 5_101), (None, Some(5_001)));
        assert_eq!(fresh_u16(72, 200, 100), (Some(72), Some(0)));
    }

    #[test]
    fn only_live_ride_samples_receive_fresh_sensor_values() {
        let snapshot = Snapshot {
            heart_bpm: Some(72),
            cadence_tenths: Some(600),
            ..Snapshot::default()
        };
        assert_eq!(
            ride_fields(snapshot, Some(crate::sdk::ride_log::Source::Live)),
            (Some(72), Some(600))
        );
        assert_eq!(
            ride_fields(snapshot, Some(crate::sdk::ride_log::Source::Demo)),
            (None, None)
        );
        assert_eq!(ride_fields(snapshot, None), (None, None));
    }

    #[test]
    fn heart_rate_flags_width_energy_rr_and_contact() {
        assert_eq!(
            HeartRate::parse(&[0x1e, 72, 0x34, 0x12, 0x00, 0x04, 0x80, 0x03]),
            Some(HeartRate {
                bpm: 72,
                contact_supported: true,
                contact_detected: Some(true),
                energy: Some(0x1234),
                rr: [1024, 896, 0, 0],
                rr_len: 2,
                rr_dropped: 0,
            })
        );
        assert_eq!(HeartRate::parse(&[0x01, 0x2c, 0x01]).unwrap().bpm, 300);
        assert_eq!(
            HeartRate::parse(&[0x00, 60]).unwrap().contact_detected,
            None
        );
    }

    #[test]
    fn heart_rate_retains_bpm_and_bounds_many_rr_intervals() {
        let parsed = HeartRate::parse(&[0x10, 77, 1, 0, 2, 0, 3, 0, 4, 0, 5, 0, 6, 0]).unwrap();
        assert_eq!(parsed.bpm, 77);
        assert_eq!(parsed.rr, [1, 2, 3, 4]);
        assert_eq!(parsed.rr_len, 4);
        assert_eq!(parsed.rr_dropped, 2);
    }

    #[test]
    fn contact_false_clears_heart_while_unsupported_contact_is_usable() {
        assert_eq!(usable_heart(HeartRate::parse(&[0x04, 72]).unwrap()), None);
        assert_eq!(
            usable_heart(HeartRate::parse(&[0x00, 72]).unwrap()),
            Some(72)
        );
    }

    #[test]
    fn heart_rate_rejects_reserved_contact_truncation_and_odd_rr() {
        for value in [
            &[0x20, 60][..],
            &[0x02, 60],
            &[0x01, 60],
            &[0x08, 60, 1],
            &[0x10, 60, 1],
            &[0x10, 60],
            &[0x00, 60, 1],
        ] {
            assert_eq!(HeartRate::parse(value), None, "{value:?}");
        }
    }

    #[test]
    fn csc_parses_both_fields_and_rejects_malformed_values() {
        assert_eq!(
            CscMeasurement::parse(&[0x03, 5, 0, 0, 0, 0, 4, 7, 0, 0, 2]),
            Some(CscMeasurement {
                wheel: Some((5, 1024)),
                crank: Some((7, 512)),
            })
        );
        for value in [&[0x00][..], &[0x04], &[0x01, 1, 2], &[0x02, 1, 0, 2]] {
            assert_eq!(CscMeasurement::parse(value), None, "{value:?}");
        }
    }

    #[test]
    fn crank_cadence_handles_wrap_duplicate_gap_and_disconnect() {
        let mut cadence = CrankCadence::default();
        assert_eq!(cadence.update(u16::MAX, 65_000, 0), None);
        assert_eq!(cadence.update(0, 488, 1000), Some(600));
        assert_eq!(cadence.update(1, 488, 2000), None);
        assert_eq!(cadence.update(2, 11_000, 3000), None);
        assert_eq!(cadence.update(u16::MAX, 12_024, 4000), None);
        assert_eq!(cadence.update(0, 13_048, 15_001), None);
        cadence.disconnected();
        assert_eq!(cadence.update(3, 14_072, 16_000), None);
    }

    #[test]
    fn implausible_reset_is_dropped_and_new_baseline_recovers() {
        let mut cadence = CrankCadence::default();
        assert_eq!(cadence.update(100, 1_024, 1_000), None);
        assert_eq!(cadence.update(50_000, 2_048, 2_000), None);
        assert_eq!(cadence.update(50_001, 3_072, 3_000), Some(600));
    }

    #[test]
    fn rejected_or_duplicate_cadence_does_not_refresh_freshness() {
        let mut cadence = CadenceReading::default();
        assert_eq!(cadence.update(100, 1_024, 1_000), None);
        assert_eq!(cadence.update(101, 2_048, 2_000), Some(600));
        assert_eq!(cadence.update(102, 2_048, 4_000), None);
        assert_eq!(cadence.snapshot(7_001), (None, Some(5_001)));
        cadence.disconnected();
        assert_eq!(cadence.snapshot(7_001), (None, None));
    }
}
