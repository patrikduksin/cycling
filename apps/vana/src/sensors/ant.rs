//! Small ANT+ heart-rate and standard power-only decoders for capture diagnostics.
//! Callers select the correct channel type and own freshness and session resets.
//! Raw capture retains all pages, including those not interpreted here.
//!
//! References: ANT+ Heart Rate rev 2.1 sections 6.3 and 6.6:
//! <https://homepages.laas.fr/rcayre/assets/ressources/ant_hrm_device_profile.pdf>
//! and Bicycle Power rev 5.1 section 8:
//! <https://forums.garmin.com/cfs-file/__key/communityserver-discussions-components-files/402/D00001086_5F00_ANT_2B005F00_Device_5F00_Profile_5F002D005F00_Bicycle_5F00_Power_5F00_Rev_5F00_5.1.pdf>.
//! These decoders do not implement the complete device profiles.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HeartRate {
    /// Last beat time, in 1/1024-second units, wrapping at 65536.
    pub beat_event_time: u16,
    pub beat_count: u8,
    /// Zero on the wire means invalid, not a measured zero heart rate.
    pub bpm: Option<u8>,
}

impl HeartRate {
    /// Every HRM page carries the common fields, regardless of page/toggle bits.
    pub fn decode(data: [u8; 8]) -> Self {
        Self {
            beat_event_time: u16::from_le_bytes([data[4], data[5]]),
            beat_count: data[6],
            bpm: (data[7] != 0).then_some(data[7]),
        }
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
                HeartRate {
                    beat_event_time: 0x1234,
                    beat_count: 255,
                    bpm: Some(72)
                }
            );
        }
        assert_eq!(HeartRate::decode([0; 8]).bpm, None);
        assert_eq!(HeartRate::decode([255; 8]).bpm, Some(255));
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
