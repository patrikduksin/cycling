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

/// Alert on a close/rapidly closing target, at most once every five seconds.
#[derive(Default)]
pub struct Alert {
    last: Option<u64>,
}
pub fn close(target: Target) -> bool {
    target.closing_speed_cm_s > 0
        && (target.range_mm <= 40_000
            || target.range_mm <= u32::from(target.closing_speed_cm_s) * 60)
}
impl Alert {
    pub fn update(&mut self, snapshot: Snapshot, now: u64) -> bool {
        if snapshot.targets.into_iter().flatten().any(close)
            && self.last.is_none_or(|at| now.saturating_sub(at) >= 5000)
        {
            self.last = Some(now);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod alert_tests {
    use super::*;
    #[test]
    fn approaching_targets_alert_without_stale_or_repeated_beeps() {
        let mut radar = Radar::new();
        let mut alert = Alert::default();
        radar.receive([0x30, 1, 0, 30, 0, 0, 1, 0], 0);
        assert!(!alert.update(radar.snapshot(0), 0));
        radar.receive([0x30, 1, 0, 10, 0, 0, 1, 0], 1000);
        assert!(alert.update(radar.snapshot(1000), 1000));
        assert!(!alert.update(radar.snapshot(1100), 1100));
        assert!(!alert.update(radar.snapshot(7000), 7000));
        radar.receive([0x30, 2, 0, 30, 0, 0, 10, 0], 7001);
        assert!(alert.update(radar.snapshot(7001), 7001));
    }
}
