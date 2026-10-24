//! Small ANT+ heart-rate and standard power-only decoders for capture diagnostics.
//! Callers select the correct channel type and own freshness and session resets.
//! Raw capture retains all pages, including those not interpreted here.
//!
//! References: ANT+ Heart Rate rev 2.1 sections 6.3 and 6.6:
//! <https://homepages.laas.fr/rcayre/assets/ressources/ant_hrm_device_profile.pdf>
//! and Bicycle Power rev 5.1 section 8:
//! <https://forums.garmin.com/cfs-file/__key/communityserver-discussions-components-files/402/D00001086_5F00_ANT_2B005F00_Device_5F00_Profile_5F002D005F00_Bicycle_5F00_Power_5F00_Rev_5F00_5.1.pdf>.
//! These decoders do not implement the complete device profiles.

/// ANT+ device types VANA recognizes.
pub mod device_type {
    pub const POWER: u8 = 11;
    pub const FITNESS_EQUIPMENT: u8 = 17;
    pub const SHIFTING: u8 = 34;
    pub const BIKE_LIGHTS: u8 = 35;
    pub const RADAR: u8 = 40;
    pub const HEART_RATE: u8 = 120;
    pub const SPEED_CADENCE: u8 = 121;
    pub const CADENCE: u8 = 122;
    pub const SPEED: u8 = 123;
    /// Shimano Di2 shifting.
    pub const SHIFTING_DI2: u8 = 128;
}

/// A channel session and its loss count. A packet from a different epoch is
/// not continuous with earlier packets, so derived state must restart.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Epoch {
    pub generation: u32,
    pub loss_count: u32,
}

impl Epoch {
    pub const fn of(packet: &device_api::ant::Packet) -> Self {
        Self {
            generation: packet.generation,
            loss_count: packet.loss_count,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartRate {
    /// Last beat time, in 1/1024-second units, wrapping at 65536.
    pub beat_event_time: u16,
    pub beat_count: u8,
    /// Zero on the wire means invalid, not a measured zero heart rate.
    pub bpm: Option<u8>,
}

impl HeartRate {
    /// Every HRM page carries the common fields, regardless of page/toggle
    /// bits, except the common Battery Status page, which reuses those bytes.
    pub fn decode(data: [u8; 8]) -> Option<Self> {
        (data[0] != COMMON_BATTERY_PAGE).then(|| Self {
            beat_event_time: u16::from_le_bytes([data[4], data[5]]),
            beat_count: data[6],
            bpm: (data[7] != 0).then_some(data[7]),
        })
    }
}

/// The ANT+ common Battery Status page.
pub const COMMON_BATTERY_PAGE: u8 = 0x52;
/// The heart-rate profile's own Battery Status page, without its toggle bit.
pub const HEART_RATE_BATTERY_PAGE: u8 = 0x07;

/// The descriptive battery status a sensor reports. The wire's reserved
/// and invalid values have no variant.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum BatteryStatus {
    New,
    Good,
    Ok,
    Low,
    Critical,
}

impl BatteryStatus {
    /// The 3-bit field: 1 to 5 are New to Critical; 0, 6 and 7 are reserved
    /// or invalid.
    pub const fn from_field(field: u8) -> Option<Self> {
        match field {
            1 => Some(Self::New),
            2 => Some(Self::Good),
            3 => Some(Self::Ok),
            4 => Some(Self::Low),
            5 => Some(Self::Critical),
            _ => None,
        }
    }

    /// Low or worse: the rider should change it.
    pub const fn is_low(self) -> bool {
        matches!(self, Self::Low | Self::Critical)
    }

    pub const fn text(self) -> &'static str {
        match self {
            Self::New => "NEW",
            Self::Good => "GOOD",
            Self::Ok => "OK",
            Self::Low => "LOW",
            Self::Critical => "CRITICAL",
        }
    }
}

/// A sensor's battery as it last reported.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Battery {
    pub status: BatteryStatus,
    /// Terminal voltage, when the sensor measures one.
    pub millivolts: Option<u16>,
}

impl Battery {
    /// Page 0x52 from any sensor, or a heart-rate strap's own page 7. Other
    /// pages and reserved status values are not a battery report.
    pub fn decode(kind: u8, data: [u8; 8]) -> Option<Self> {
        let (fraction, coarse) = if data[0] == COMMON_BATTERY_PAGE {
            (data[6], data[7])
        } else if kind == device_type::HEART_RATE && data[0] & 0x7f == HEART_RATE_BATTERY_PAGE {
            (data[2], data[3])
        } else {
            return None;
        };
        let status = BatteryStatus::from_field(coarse >> 4 & 0b111)?;
        // 0xf is "invalid"; zero is a sensor that does not measure.
        let volts = u32::from(coarse & 0x0f);
        let millivolts = (volts != 0x0f)
            .then(|| volts * 1000 + u32::from(fraction) * 1000 / 256)
            .filter(|&millivolts| millivolts != 0)
            .map(|millivolts| millivolts as u16);
        Some(Self { status, millivolts })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Power {
    pub event_count: u8,
    pub cadence_rpm: Option<u8>,
    /// Page 0x10 instantaneous power, without averaging or torque conversion.
    pub watts: u16,
}

impl Power {
    /// Other pages contain different fields and must never refresh these values.
    pub fn decode(data: [u8; 8]) -> Option<Self> {
        (data[0] == 0x10).then(|| Self {
            event_count: data[1],
            cadence_rpm: (data[3] != 0xff).then_some(data[3]),
            watts: u16::from_le_bytes([data[6], data[7]]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heart_common_fields_ignore_page_and_toggle_and_preserve_wrapping_counters() {
        for page in [0, 4, 7, 0x80, 0x84, 0xff] {
            assert_eq!(
                HeartRate::decode([page, 1, 2, 3, 0x34, 0x12, 255, 72]),
                Some(HeartRate {
                    beat_event_time: 0x1234,
                    beat_count: 255,
                    bpm: Some(72)
                })
            );
        }
        assert_eq!(HeartRate::decode([0; 8]).unwrap().bpm, None);
        assert_eq!(HeartRate::decode([255; 8]).unwrap().bpm, Some(255));
    }

    #[test]
    fn battery_page_never_refreshes_heart_rate_or_power() {
        let page = [0x52, 0xff, 1, 0, 0, 0, 0x80, 0x42];
        assert_eq!(HeartRate::decode(page), None);
        assert_eq!(Power::decode(page), None);
    }

    #[test]
    fn every_status_value_decodes_or_is_rejected() {
        let expected = [
            None,
            Some(BatteryStatus::New),
            Some(BatteryStatus::Good),
            Some(BatteryStatus::Ok),
            Some(BatteryStatus::Low),
            Some(BatteryStatus::Critical),
            None,
            None,
        ];
        for (field, want) in expected.into_iter().enumerate() {
            // Bit 7 (operating-time resolution) and the coarse voltage never
            // change the status.
            for extra in [0x00, 0x80, 0x03, 0x8f] {
                let byte = (field as u8) << 4 | extra;
                let common = Battery::decode(device_type::POWER, [0x52, 255, 1, 0, 0, 0, 0, byte]);
                assert_eq!(common.map(|b| b.status), want, "{field} {extra:#x}");
                let strap = Battery::decode(device_type::HEART_RATE, [7, 100, 0, byte, 0, 0, 0, 0]);
                assert_eq!(strap.map(|b| b.status), want, "{field} {extra:#x}");
            }
        }
        assert!(BatteryStatus::Low.is_low() && BatteryStatus::Critical.is_low());
        assert!(!BatteryStatus::Ok.is_low() && !BatteryStatus::New.is_low());
    }

    #[test]
    fn battery_voltage_is_coarse_plus_fraction_or_absent() {
        let voltage = |fraction: u8, coarse: u8| {
            Battery::decode(
                device_type::POWER,
                [0x52, 255, 1, 0, 0, 0, fraction, 0x20 | coarse],
            )
            .unwrap()
            .millivolts
        };
        assert_eq!(voltage(0x80, 2), Some(2500));
        assert_eq!(voltage(0xff, 14), Some(14_996));
        assert_eq!(voltage(0, 0), None);
        assert_eq!(voltage(0x80, 0x0f), None);
    }

    #[test]
    fn only_battery_pages_report_a_battery() {
        for page in 0..=255u8 {
            let data = [page, 0xff, 0x40, 0x40, 0xff, 0xff, 0x40, 0x40];
            for kind in [device_type::POWER, device_type::HEART_RATE, 0, 255] {
                let wanted = page == 0x52 || (kind == device_type::HEART_RATE && page & 0x7f == 7);
                assert_eq!(
                    Battery::decode(kind, data).is_some(),
                    wanted,
                    "{page:#x} {kind}"
                );
            }
        }
    }

    #[test]
    fn power_only_fields_and_invalid_cadence() {
        assert_eq!(
            Power::decode([0x10, 255, 0, 90, 0, 0, 0x34, 0x12]),
            Some(Power {
                event_count: 255,
                cadence_rpm: Some(90),
                watts: 0x1234
            })
        );
        assert_eq!(
            Power::decode([0x10, 0, 0, 255, 0, 0, 255, 255]).unwrap(),
            Power {
                event_count: 0,
                cadence_rpm: None,
                watts: 65535
            }
        );
        assert_eq!(Power::decode([0x10, 0, 0, 0, 0, 0, 0, 0]).unwrap().watts, 0);
        for page in [0x01, 0x11, 0x12, 0x20, 0x50, 0x90] {
            assert_eq!(
                Power::decode([page, 255, 255, 255, 255, 255, 255, 255]),
                None
            );
        }
    }
}
