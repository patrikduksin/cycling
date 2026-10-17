//! The rider's training profile, the zones it defines and time spent in them.
//!
//! Power uses Coggan's seven zones as a percentage of FTP; heart rate uses
//! Friel's five zones as a percentage of lactate threshold heart rate (LTHR).
//! Boundaries round to the nearest whole watt or beat.
use crate::ride::log::Sample;

pub const POWER_ZONES: usize = 7;
pub const HEART_ZONES: usize = 5;

/// Upper bound of power zones 1-6, in percent of FTP; zone 7 is open-ended.
const POWER_UPPER_PCT: [u32; POWER_ZONES - 1] = [55, 75, 90, 105, 120, 150];
/// Lower bound of heart zones 2-5, in percent of LTHR.
const HEART_LOWER_PCT: [u32; HEART_ZONES - 1] = [81, 90, 94, 100];
/// Where the open ends of the outer zones stop for a position gauge.
const POWER_GAUGE_TOP_PCT: u32 = 200;
const HEART_GAUGE_BOTTOM_PCT: u32 = 65;
const HEART_GAUGE_TOP_PCT: u32 = 110;

/// Thresholds the zones derive from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TrainingProfile {
    pub ftp_watts: u16,
    pub lthr_bpm: u16,
}

impl Default for TrainingProfile {
    /// FTP 215 W reproduces the fixed power table VANA used before profiles.
    fn default() -> Self {
        Self {
            ftp_watts: 215,
            lthr_bpm: 185,
        }
    }
}

impl TrainingProfile {
    pub const FTP_WATTS: core::ops::RangeInclusive<u16> = 50..=600;
    pub const LTHR_BPM: core::ops::RangeInclusive<u16> = 80..=220;

    /// A profile, if both thresholds are plausible.
    pub fn new(ftp_watts: u16, lthr_bpm: u16) -> Option<Self> {
        (Self::FTP_WATTS.contains(&ftp_watts) && Self::LTHR_BPM.contains(&lthr_bpm)).then_some(
            Self {
                ftp_watts,
                lthr_bpm,
            },
        )
    }

    /// Power zone 1-7.
    pub fn power_zone(&self, watts: u16) -> u8 {
        let above = POWER_UPPER_PCT
            .iter()
            .filter(|&&pct| u32::from(watts) > percent(self.ftp_watts, pct))
            .count();
        1 + above as u8
    }

    /// Heart rate zone 1-5.
    pub fn heart_zone(&self, bpm: u16) -> u8 {
        let reached = HEART_LOWER_PCT
            .iter()
            .filter(|&&pct| u32::from(bpm) >= percent(self.lthr_bpm, pct))
            .count();
        1 + reached as u8
    }

    /// Watts where `zone` begins and where the next one begins. The open top
    /// zone ends at twice FTP, for a gauge.
    pub fn power_range(&self, zone: u8) -> (u32, u32) {
        let upper = |index: usize| {
            POWER_UPPER_PCT
                .get(index)
                .map_or(percent(self.ftp_watts, POWER_GAUGE_TOP_PCT), |&pct| {
                    percent(self.ftp_watts, pct)
                })
                + 1
        };
        let index = usize::from(zone.clamp(1, POWER_ZONES as u8) - 1);
        let low = if index == 0 { 0 } else { upper(index - 1) };
        (low, upper(index).max(low + 1))
    }

    /// Beats where `zone` begins and where the next one begins. The open outer
    /// zones span 65 % to 110 % of LTHR, for a gauge.
    pub fn heart_range(&self, zone: u8) -> (u32, u32) {
        let lower = |index: usize| match index {
            0 => percent(self.lthr_bpm, HEART_GAUGE_BOTTOM_PCT),
            HEART_ZONES => percent(self.lthr_bpm, HEART_GAUGE_TOP_PCT),
            _ => percent(self.lthr_bpm, HEART_LOWER_PCT[index - 1]),
        };
        let index = usize::from(zone.clamp(1, HEART_ZONES as u8) - 1);
        let low = lower(index);
        (low, lower(index + 1).max(low + 1))
    }
}

/// `pct` percent of `base`, rounded half up.
fn percent(base: u16, pct: u32) -> u32 {
    (u32::from(base) * pct + 50) / 100
}

/// Recorded time in each power and heart rate zone, under the profile in
/// force at that second. Seconds without a reading count in no zone.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ZoneTime {
    pub power_ms: [u64; POWER_ZONES],
    pub heart_ms: [u64; HEART_ZONES],
}

impl ZoneTime {
    pub fn add(&mut self, elapsed_ms: u64, sample: &Sample, profile: &TrainingProfile) {
        if let Some(watts) = sample.power_watts {
            let zone = &mut self.power_ms[usize::from(profile.power_zone(watts) - 1)];
            *zone = zone.saturating_add(elapsed_ms);
        }
        if let Some(bpm) = sample.heart_bpm {
            let zone = &mut self.heart_ms[usize::from(profile.heart_zone(bpm) - 1)];
            *zone = zone.saturating_add(elapsed_ms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_power_zones_match_the_former_fixed_table() {
        let profile = TrainingProfile::default();
        for (watts, zone) in [
            (0, 1),
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
            (u16::MAX, 7),
        ] {
            assert_eq!(profile.power_zone(watts), zone, "{watts} W");
        }
    }

    #[test]
    fn default_heart_zones_follow_friel_from_lthr() {
        let profile = TrainingProfile::default();
        for (bpm, zone) in [
            (0, 1),
            (149, 1),
            (150, 2),
            (166, 2),
            (167, 3),
            (173, 3),
            (174, 4),
            (184, 4),
            (185, 5),
            (220, 5),
        ] {
            assert_eq!(profile.heart_zone(bpm), zone, "{bpm} bpm");
        }
    }

    #[test]
    fn zones_scale_with_the_profile() {
        let profile = TrainingProfile::new(300, 170).unwrap();
        // 76 % of 300 W is the first tempo watt; 106 % the first VO2 watt.
        assert_eq!(profile.power_zone(225), 2);
        assert_eq!(profile.power_zone(226), 3);
        assert_eq!(profile.power_zone(315), 4);
        assert_eq!(profile.power_zone(316), 5);
        assert_eq!(profile.heart_zone(169), 4);
        assert_eq!(profile.heart_zone(170), 5);
        // The same effort sits lower for a stronger rider.
        assert_eq!(TrainingProfile::default().power_zone(250), 5);
        assert_eq!(profile.power_zone(250), 3);
    }

    #[test]
    fn profiles_reject_implausible_thresholds() {
        assert!(TrainingProfile::new(49, 185).is_none());
        assert!(TrainingProfile::new(601, 185).is_none());
        assert!(TrainingProfile::new(215, 79).is_none());
        assert!(TrainingProfile::new(215, 221).is_none());
        assert_eq!(
            TrainingProfile::new(215, 185),
            Some(TrainingProfile::default())
        );
    }

    #[test]
    fn ranges_tile_the_zones_and_bound_the_open_ends() {
        for profile in [
            TrainingProfile::default(),
            TrainingProfile::new(50, 80).unwrap(),
            TrainingProfile::new(600, 220).unwrap(),
        ] {
            assert_eq!(profile.power_range(1).0, 0);
            for zone in 1..POWER_ZONES as u8 {
                assert_eq!(profile.power_range(zone).1, profile.power_range(zone + 1).0);
            }
            for zone in 1..=POWER_ZONES as u8 {
                let (low, high) = profile.power_range(zone);
                assert!(low < high);
                assert_eq!(profile.power_zone(low as u16), zone);
                if zone < POWER_ZONES as u8 {
                    assert_eq!(profile.power_zone(high as u16 - 1), zone);
                }
            }
            for zone in 1..HEART_ZONES as u8 {
                assert_eq!(profile.heart_range(zone).1, profile.heart_range(zone + 1).0);
            }
            for zone in 2..=HEART_ZONES as u8 {
                assert_eq!(profile.heart_zone(profile.heart_range(zone).0 as u16), zone);
            }
        }
        let profile = TrainingProfile::default();
        assert_eq!(profile.power_range(4), (195, 227));
        assert_eq!(profile.power_range(7), (324, 431));
        assert_eq!(profile.heart_range(1), (120, 150));
        assert_eq!(profile.heart_range(5), (185, 204));
    }

    #[test]
    fn zone_time_counts_each_second_in_its_zone() {
        let profile = TrainingProfile::default();
        let mut time = ZoneTime::default();
        let sample = |power_watts, heart_bpm| Sample {
            power_watts,
            heart_bpm,
            ..Sample::default()
        };
        time.add(1000, &sample(Some(150), Some(140)), &profile);
        time.add(1000, &sample(Some(250), Some(180)), &profile);
        time.add(1500, &sample(Some(250), None), &profile);
        time.add(1000, &sample(None, None), &profile);
        assert_eq!(time.power_ms, [0, 1000, 0, 0, 2500, 0, 0]);
        assert_eq!(time.heart_ms, [1000, 0, 0, 1000, 0]);
    }
}
