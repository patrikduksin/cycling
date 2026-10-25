//! ANT+ bike radar interpretation. Feed only complete broadcast or acknowledged
//! payloads from a radar channel; channel identity and recovery belong to core.
//!
//! Original implementation referencing Garmin Bike Radar Profile revision 2.1,
//! sections 6.4–6.7 and 6.10.4. The profile document is not distributed here:
//! <https://devzone.nordicsemi.com/cfs-file/__key/support-attachments/beef5d1b77644c448dabff31668f3a47-40ef7acdda7a482aa67b000fa33e8a3f/D00001665_5F002D005F00_ANT_2B005F00_Device_5F00_Profile_5F002D005F00_Bike_5F00_Radar_5F00_2.1.pdf>.
//! This module does not implement every requirement of the device profile.

pub const STALE_MS: u64 = 2_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Threat {
    Approach,
    FastApproach,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Side {
    Behind,
    Right,
    Left,
    Reserved,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Target {
    pub threat: Threat,
    pub side: Side,
    pub range_mm: u32,
    pub closing_speed_cm_s: u16,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Status {
    #[default]
    Unavailable,
    Ready,
    Error,
    /// A fresh page contains an undefined threat value. Do not show all clear.
    ReservedThreat,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Snapshot {
    pub status: Status,
    /// Wire slots, not persistent identities or guaranteed distance ordering.
    pub targets: [Option<Target>; 8],
}

#[derive(Clone, Copy, Debug, Default)]
struct Page {
    received_ms: Option<u64>,
    targets: [Option<Target>; 4],
    reserved_threat: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Radar {
    pages: [Page; 2],
    error: bool,
}

impl Radar {
    pub const fn new() -> Self {
        Self {
            pages: [Page {
                received_ms: None,
                targets: [None; 4],
                reserved_threat: false,
            }; 2],
            error: false,
        }
    }

    /// Call on disconnect or when changing the selected sensor.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Returns whether this module recognized the page. Unknown pages neither
    /// refresh target ages nor clear an error. `now_ms` is monotonic time.
    pub fn receive(&mut self, data: [u8; 8], now_ms: u64) -> bool {
        match data[0] {
            0x30 | 0x31 => {
                let mut page = Page {
                    received_ms: Some(now_ms),
                    ..Page::default()
                };
                let ranges = u32::from_le_bytes([data[3], data[4], data[5], 0]);
                for (i, target) in page.targets.iter_mut().enumerate() {
                    let threat = match (data[1] >> (2 * i)) & 3 {
                        0 => continue,
                        1 => Threat::Approach,
                        2 => Threat::FastApproach,
                        _ => {
                            page.reserved_threat = true;
                            continue;
                        }
                    };
                    let side = match (data[2] >> (2 * i)) & 3 {
                        0 => Side::Behind,
                        1 => Side::Right,
                        2 => Side::Left,
                        _ => Side::Reserved,
                    };
                    *target = Some(Target {
                        threat,
                        side,
                        range_mm: ((ranges >> (6 * i)) & 63) * 3_125,
                        closing_speed_cm_s: u16::from((data[6 + i / 2] >> (4 * (i % 2))) & 15)
                            * 304,
                    });
                }
                self.pages[usize::from(data[0] - 0x30)] = page;
                self.error = false;
            }
            0x57 => {
                self.reset();
                self.error = true;
            }
            0x01 => {
                if data[1] & 1 != 0 || data[7] & 1 == 0 {
                    // Shutdown requested/forced, or explicit clear-targets bit.
                    self.pages = Self::new().pages;
                }
            }
            _ => return false,
        }
        true
    }

    /// When each half of the snapshot's slots (0..4, 4..8) was last
    /// reported, so a repeated identical report still counts as fresh.
    pub fn received_ms(&self) -> [Option<u64>; 2] {
        self.pages.map(|page| page.received_ms)
    }

    /// Each half expires independently, including when the sensor stops sending
    /// page B after its number of targets drops. Poll even when no data arrives.
    pub fn snapshot(&self, now_ms: u64) -> Snapshot {
        let mut snapshot = Snapshot::default();
        if self.error {
            snapshot.status = Status::Error;
            return snapshot;
        }
        for (index, page) in self.pages.iter().enumerate() {
            let fresh = page.received_ms.is_some_and(|received| {
                now_ms
                    .checked_sub(received)
                    .is_some_and(|age| age < STALE_MS)
            });
            if !fresh {
                continue;
            }
            if page.reserved_threat {
                snapshot.status = Status::ReservedThreat;
            } else if snapshot.status == Status::Unavailable {
                snapshot.status = Status::Ready;
            }
            snapshot.targets[index * 4..index * 4 + 4].copy_from_slice(&page.targets);
        }
        snapshot
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_fields_cross_byte_boundaries_and_use_exact_units() {
        // Four unequal ranges: 1, 2, 31, 63. Speeds: 1, 2, 14, 15.
        let mut radar = Radar::new();
        radar.receive([0x30, 0x99, 0xe4, 0x81, 0xf0, 0xfd, 0x21, 0xfe], 100);
        let snapshot = radar.snapshot(100);
        assert_eq!(snapshot.status, Status::Ready);
        for (index, (range, speed, side, threat)) in [
            (3_125, 304, Side::Behind, Threat::Approach),
            (6_250, 608, Side::Right, Threat::FastApproach),
            (96_875, 4_256, Side::Left, Threat::Approach),
            (196_875, 4_560, Side::Reserved, Threat::FastApproach),
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(
                snapshot.targets[index],
                Some(Target {
                    threat,
                    side,
                    range_mm: range,
                    closing_speed_cm_s: speed
                })
            );
        }
        assert_eq!(snapshot.targets[4..], [None; 4]);
    }

    #[test]
    fn no_threat_ignores_other_fields_and_reserved_is_not_all_clear() {
        let mut radar = Radar::new();
        radar.receive([0x30, 0, 255, 255, 255, 255, 255, 255], 0);
        assert_eq!(
            radar.snapshot(0),
            Snapshot {
                status: Status::Ready,
                targets: [None; 8]
            }
        );
        radar.receive([0x30, 3, 255, 255, 255, 255, 255, 255], 1);
        assert_eq!(radar.snapshot(1).status, Status::ReservedThreat);
        assert_eq!(radar.snapshot(1).targets, [None; 8]);
        radar.receive([0x31, 0, 0, 0, 0, 0, 0, 0], 2);
        assert_eq!(radar.snapshot(2).status, Status::ReservedThreat);
    }

    #[test]
    fn pages_expire_independently_and_background_does_not_refresh() {
        let mut radar = Radar::new();
        assert_eq!(radar.snapshot(0).status, Status::Unavailable);
        radar.receive([0x31, 1, 0, 1, 0, 0, 1, 0], 100);
        radar.receive([0x30, 1, 0, 2, 0, 0, 1, 0], 1_000);
        assert!(radar.snapshot(2_099).targets[4].is_some());
        let snapshot = radar.snapshot(2_100);
        assert_eq!(snapshot.status, Status::Ready);
        assert!(snapshot.targets[0].is_some());
        assert!(snapshot.targets[4].is_none());
        assert!(!radar.receive([0x50, 0, 0, 0, 0, 0, 0, 0], 2_999));
        assert_eq!(radar.snapshot(3_000).status, Status::Unavailable);
        assert_eq!(radar.snapshot(3_000).targets, [None; 8]);
        assert_eq!(radar.snapshot(99).status, Status::Unavailable);
    }

    #[test]
    fn errors_and_reset_drop_both_pages_until_new_targets() {
        let mut radar = Radar::new();
        radar.receive([0x30, 1, 0, 1, 0, 0, 1, 0], 0);
        radar.receive([0x31, 1, 0, 1, 0, 0, 1, 0], 0);
        radar.receive([0x57, 0, 0, 0, 0, 0, 0, 0], 1);
        radar.receive([0x01, 0, 0, 0, 0, 0, 0, 255], 2);
        assert_eq!(
            radar.snapshot(10_000),
            Snapshot {
                status: Status::Error,
                targets: [None; 8]
            }
        );
        radar.receive([0x30, 1, 0, 1, 0, 0, 1, 0], 10_001);
        let snapshot = radar.snapshot(10_001);
        assert_eq!(snapshot.status, Status::Ready);
        assert!(snapshot.targets[0].is_some());
        assert!(snapshot.targets[4].is_none());
        radar.reset();
        assert_eq!(radar.snapshot(10_001), Snapshot::default());
    }

    #[test]
    fn device_status_clears_targets_for_shutdown_or_clear_request() {
        for (state, last, clears) in [
            (0, 255, false),
            (2, 255, false),
            (1, 255, true),
            (3, 255, true),
            (0, 254, true),
        ] {
            let mut radar = Radar::new();
            radar.receive([0x30, 1, 0, 1, 0, 0, 1, 0], 0);
            radar.receive([0x01, state, 255, 255, 255, 255, 255, last], 1);
            assert_eq!(radar.snapshot(1).status == Status::Unavailable, clears);
        }
    }
}

pub fn close(target: Target) -> bool {
    target.closing_speed_cm_s > 0
        && (target.range_mm <= 40_000
            || target.range_mm <= u32::from(target.closing_speed_cm_s) * 60)
}

/// How near the nearest vehicle is, by distance or by time to reach the
/// rider, whichever is nearer.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd)]
pub enum Proximity {
    #[default]
    Clear,
    Far,
    Near,
    Close,
    Passing,
}

/// One beep the rider should hear now.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Beep {
    Appeared,
    Near,
    Close,
    Passing,
}

/// A level's bounds: the range under which a vehicle is at least that near,
/// and the seconds it needs to reach the rider.
const BANDS: [(Proximity, u32, u32); 3] = [
    (Proximity::Passing, 15_000, 2),
    (Proximity::Close, 30_000, 4),
    (Proximity::Near, 60_000, 7),
];

const MIN_GAP_MS: u64 = 500;
const NEAR_EVERY_MS: u64 = 1_500;
const CLOSE_EVERY_MS: u64 = 1_000;
const PASSING_EVERY_MS: u64 = 500;

impl Proximity {
    pub fn of(target: Target) -> Self {
        let speed_cm_s = u32::from(target.closing_speed_cm_s);
        if speed_cm_s == 0 {
            return Self::Far;
        }
        BANDS
            .into_iter()
            .find(|&(_, range_mm, secs)| {
                // cm/s times ten is mm/s.
                target.range_mm < range_mm || target.range_mm < speed_cm_s * 10 * secs
            })
            .map_or(Self::Far, |(level, ..)| level)
    }

    fn of_snapshot(snapshot: Snapshot) -> Self {
        snapshot
            .targets
            .into_iter()
            .flatten()
            .map(Self::of)
            .max()
            .unwrap_or(Self::Clear)
    }

    /// The beep this level repeats, and how often.
    const fn repeat(self) -> Option<(Beep, u64)> {
        match self {
            Self::Clear | Self::Far => None,
            Self::Near => Some((Beep::Near, NEAR_EVERY_MS)),
            Self::Close => Some((Beep::Close, CLOSE_EVERY_MS)),
            Self::Passing => Some((Beep::Passing, PASSING_EVERY_MS)),
        }
    }
}

/// Beeps for the nearest vehicle: an alarm when one first appears, then a
/// note that repeats faster and climbs as it closes in.
#[derive(Default)]
pub struct Warning {
    level: Proximity,
    last_ms: Option<u64>,
}

impl Warning {
    pub fn update(&mut self, snapshot: Snapshot, now: u64) -> Option<Beep> {
        let before = core::mem::replace(&mut self.level, Proximity::of_snapshot(snapshot));
        let level = self.level;
        if level == Proximity::Clear {
            self.last_ms = None;
            return None;
        }
        let beep = if before == Proximity::Clear {
            Some(Beep::Appeared)
        } else {
            let (beep, every_ms) = level.repeat()?;
            let since = now.saturating_sub(self.last_ms?);
            (since >= MIN_GAP_MS && (level > before || since >= every_ms)).then_some(beep)
        };
        if beep.is_some() {
            self.last_ms = Some(now);
        }
        beep
    }
}

#[cfg(test)]
mod warning_tests {
    use super::*;

    fn car(range_m: u32, closing_cm_s: u16) -> Snapshot {
        let mut snapshot = Snapshot {
            status: Status::Ready,
            targets: [None; 8],
        };
        snapshot.targets[0] = Some(Target {
            threat: Threat::Approach,
            side: Side::Behind,
            range_mm: range_m * 1000,
            closing_speed_cm_s: closing_cm_s,
        });
        snapshot
    }

    fn clear() -> Snapshot {
        Snapshot {
            status: Status::Ready,
            targets: [None; 8],
        }
    }

    #[test]
    fn a_closing_car_beeps_faster_each_time_it_gets_nearer() {
        let mut warning = Warning::default();
        let mut heard = std::vec::Vec::new();
        // 9 m/s from 150 m, sampled every 100 ms.
        for tick in 0..=160u64 {
            let range_m = 150u32.saturating_sub((tick * 9 / 10) as u32);
            if let Some(beep) = warning.update(car(range_m.max(1), 900), tick * 100) {
                heard.push((tick * 100, beep));
            }
        }
        let beeps: std::vec::Vec<_> = heard.iter().map(|&(_, beep)| beep).collect();
        assert_eq!(beeps[0], Beep::Appeared);
        let gaps = |kind: Beep| -> std::vec::Vec<u64> {
            let at: std::vec::Vec<_> = heard.iter().filter(|h| h.1 == kind).map(|h| h.0).collect();
            at.windows(2).map(|pair| pair[1] - pair[0]).collect()
        };
        assert!(beeps.iter().skip(1).any(|&b| b == Beep::Near));
        assert!(gaps(Beep::Near).iter().all(|&gap| gap == 1_500));
        assert!(gaps(Beep::Close).iter().all(|&gap| gap == 1_000));
        assert!(gaps(Beep::Passing).iter().all(|&gap| gap == 500));
        let order: std::vec::Vec<_> = {
            let mut order = beeps.clone();
            order.dedup();
            order
        };
        assert_eq!(
            order,
            [Beep::Appeared, Beep::Near, Beep::Close, Beep::Passing]
        );
        assert!(gaps(Beep::Passing).len() >= 2);
    }

    #[test]
    fn a_car_holding_the_riders_speed_beeps_only_once() {
        let mut warning = Warning::default();
        assert_eq!(warning.update(car(10, 0), 0), Some(Beep::Appeared));
        for tick in 1..100 {
            assert_eq!(warning.update(car(10, 0), tick * 100), None);
        }
    }

    #[test]
    fn time_to_reach_counts_as_well_as_range() {
        let level = |range_m, closing_cm_s| match car(range_m, closing_cm_s).targets[0] {
            Some(target) => Proximity::of(target),
            None => Proximity::Clear,
        };
        assert_eq!(level(100, 1500), Proximity::Near, "6.7 s away");
        assert_eq!(level(100, 2500), Proximity::Near, "4 s is not under 4");
        assert_eq!(level(100, 3000), Proximity::Close, "3.3 s away");
        assert_eq!(level(100, 5100), Proximity::Passing, "under 2 s");
        assert_eq!(level(100, 900), Proximity::Far);
        assert_eq!(level(14, 100), Proximity::Passing);
        assert_eq!(level(29, 100), Proximity::Close);
        assert_eq!(level(59, 100), Proximity::Near);
        assert_eq!(level(60, 100), Proximity::Far);
    }

    #[test]
    fn a_second_car_in_the_same_stretch_is_not_a_new_appearance() {
        let mut warning = Warning::default();
        assert_eq!(warning.update(car(20, 500), 0), Some(Beep::Appeared));
        // The first car passed and the next is still far: no beep, no alarm.
        assert_eq!(warning.update(car(200, 500), 100), None);
        assert_eq!(warning.update(car(190, 500), 200), None);
        assert_eq!(warning.update(car(40, 500), 2_000), Some(Beep::Near));
    }

    #[test]
    fn a_gap_to_clear_makes_the_next_car_appear_again() {
        let mut warning = Warning::default();
        assert_eq!(warning.update(car(100, 500), 0), Some(Beep::Appeared));
        assert_eq!(warning.update(clear(), 100), None);
        assert_eq!(warning.update(car(100, 500), 200), Some(Beep::Appeared));
    }

    #[test]
    fn a_falling_level_continues_at_the_slower_interval() {
        let mut warning = Warning::default();
        assert_eq!(warning.update(car(10, 500), 0), Some(Beep::Appeared));
        assert_eq!(warning.update(car(10, 500), 500), Some(Beep::Passing));
        assert_eq!(warning.update(car(45, 500), 600), None);
        assert_eq!(warning.update(car(45, 500), 1_900), None);
        assert_eq!(warning.update(car(45, 500), 2_000), Some(Beep::Near));
    }
}
