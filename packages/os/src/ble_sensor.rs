//! Bounded parsers for standard Heart Rate and Cycling Speed and Cadence measurements.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartRate {
    pub bpm: u16,
    pub contact_supported: bool,
    pub contact_detected: Option<bool>,
    pub energy: Option<u16>,
    pub rr: [u16; 4],
    pub rr_len: u8,
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
        let mut rr_len = 0;
        if flags & 0x10 != 0 {
            if value.len() == at
                || !(value.len() - at).is_multiple_of(2)
                || (value.len() - at) / 2 > rr.len()
            {
                return None;
            }
            while at < value.len() {
                rr[rr_len] = take_u16(value, &mut at)?;
                rr_len += 1;
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
        })
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
        Some(value.min(u64::from(u32::MAX)) as u32)
    }

    pub fn disconnected(&mut self) {
        self.previous = None;
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
            })
        );
        assert_eq!(HeartRate::parse(&[0x01, 0x2c, 0x01]).unwrap().bpm, 300);
        assert_eq!(
            HeartRate::parse(&[0x00, 60]).unwrap().contact_detected,
            None
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
        assert_eq!(cadence.update(u16::MAX, 12_024, 4000), Some(39_319_800));
        assert_eq!(cadence.update(0, 13_048, 15_001), None);
        cadence.disconnected();
        assert_eq!(cadence.update(3, 14_072, 16_000), None);
    }
}
