//! C606 companion sensor page decoding and observation state.
#[cfg(test)]
use device_api::observation::Observation;
use device_api::observation::observation;
use device_api::sensors::*;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    Pressure(Pressure),
    Motion { subtype: u8, value: Motion },
    Identity(Identity),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decode {
    Report(Report),
    Invalid,
    Unrelated,
}

/// Call only after envelope class and CRC validation. Reports use class four.
/// Length is exact: the identity page has ten payload bytes, sensor pages eight.
pub fn decode(group: u8, payload: &[u8]) -> Decode {
    match (group, payload.first().copied()) {
        (1, Some(1)) => {
            if payload.len() != 10 || payload[1] != 1 {
                return Decode::Invalid;
            }
            Decode::Report(Report::Identity(Identity {
                fields: [payload[7], payload[8], payload[9]],
            }))
        }
        (0x10, Some(0xf1)) => {
            let Some(&subtype) = payload.get(1) else {
                return Decode::Invalid;
            };
            if !matches!(subtype, 1..=3) {
                return Decode::Unrelated;
            }
            if payload.len() != 8 {
                return Decode::Invalid;
            }
            if subtype == 3 {
                let temperature_centi_c = i16::from_le_bytes([payload[2], payload[3]]);
                let pressure_centi_pa =
                    u32::from_le_bytes([payload[4], payload[5], payload[6], payload[7]]);
                // Broad decoder plausibility limits, not fitted-part specifications
                // or accuracy guarantees. Reject sentinel/overflow values.
                if !(-6_000..=10_000).contains(&temperature_centi_c)
                    || !(1_000_000..=13_000_000).contains(&pressure_centi_pa)
                {
                    return Decode::Invalid;
                }
                Decode::Report(Report::Pressure(Pressure {
                    pressure_centi_pa,
                    temperature_centi_c,
                }))
            } else {
                Decode::Report(Report::Motion {
                    subtype,
                    value: Motion {
                        axes: [
                            i16::from_le_bytes([payload[2], payload[3]]),
                            i16::from_le_bytes([payload[4], payload[5]]),
                            i16::from_le_bytes([payload[6], payload[7]]),
                        ],
                    },
                })
            }
        }
        _ => Decode::Unrelated,
    }
}

#[derive(Clone, Copy, Default)]
pub struct State {
    pressure: Option<(Pressure, u64)>,
    motion: [Option<(Motion, u64)>; 2],
    identity: Option<(Identity, u64)>,
    reports: u32,
    invalid_reports: u32,
    losses: u32,
}

impl State {
    /// Unknown traffic does not change sensor freshness or counters.
    pub fn receive(&mut self, group: u8, payload: &[u8], now_ms: u64) {
        match decode(group, payload) {
            Decode::Unrelated => {}
            Decode::Invalid => {
                self.invalid_reports = self.invalid_reports.saturating_add(1);
                // A malformed known page cannot refresh or preserve a ready value.
                match (group, payload.first(), payload.get(1)) {
                    (1, Some(1), _) => self.identity = None,
                    (0x10, Some(0xf1), Some(3)) => self.pressure = None,
                    (0x10, Some(0xf1), Some(subtype @ 1..=2)) => {
                        self.motion[usize::from(*subtype - 1)] = None;
                    }
                    _ => {}
                }
            }
            Decode::Report(report) => {
                self.reports = self.reports.saturating_add(1);
                match report {
                    Report::Pressure(value) => self.pressure = Some((value, now_ms)),
                    Report::Motion { subtype, value } => {
                        self.motion[usize::from(subtype - 1)] = Some((value, now_ms));
                    }
                    Report::Identity(value) => self.identity = Some((value, now_ms)),
                }
            }
        }
    }

    /// The single UART owner calls this before processing post-loss reports.
    pub fn loss(&mut self) {
        self.pressure = None;
        self.motion = [None; 2];
        self.identity = None;
        self.losses = self.losses.saturating_add(1);
    }

    /// The consumer chooses freshness; transport timestamps are receive uptime.
    pub fn snapshot(&self, now_ms: u64, stale_ms: u64) -> Snapshot {
        Snapshot {
            pressure: observation(self.pressure, now_ms, stale_ms),
            motion: self
                .motion
                .map(|value| observation(value, now_ms, stale_ms)),
            identity: observation(self.identity, now_ms, stale_ms),
            reports: self.reports,
            invalid_reports: self.invalid_reports,
            losses: self.losses,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::drivers::companion::{Decoder, crc16, feed_frames};

    fn pressure(temperature: i16, pressure: u32) -> [u8; 8] {
        let mut payload = [0xf1, 3, 0, 0, 0, 0, 0, 0];
        payload[2..4].copy_from_slice(&temperature.to_le_bytes());
        payload[4..8].copy_from_slice(&pressure.to_le_bytes());
        payload
    }

    #[test]
    fn pressure_preserves_signed_temperature_and_fractional_pa() {
        assert_eq!(
            decode(16, &pressure(-1234, 10_132_501)),
            Decode::Report(Report::Pressure(Pressure {
                temperature_centi_c: -1234,
                pressure_centi_pa: 10_132_501
            }))
        );
        for payload in [
            pressure(-6001, 10_000_000),
            pressure(10001, 10_000_000),
            pressure(0, 999_999),
            pressure(0, 13_000_001),
            pressure(0, u32::MAX),
        ] {
            assert_eq!(decode(16, &payload), Decode::Invalid);
        }
    }

    #[test]
    fn motion_retains_signed_extremes_and_subtypes_independently() {
        let mut state = State::default();
        for subtype in 1..=2 {
            state.receive(
                16,
                &[0xf1, subtype, 0, 0x80, 0xff, 0x7f, 0xff, 0xff],
                u64::from(subtype),
            );
        }
        for value in state.snapshot(2, 5).motion {
            assert!(matches!(
                value,
                Observation::Fresh {
                    value: Motion {
                        axes: [i16::MIN, i16::MAX, -1]
                    },
                    ..
                }
            ));
        }
        state.receive(16, &[0xf1, 1], 3);
        let snapshot = state.snapshot(3, 5);
        assert!(matches!(snapshot.motion[0], Observation::Unavailable));
        assert!(matches!(snapshot.motion[1], Observation::Fresh { .. }));
    }

    #[test]
    fn unknown_traffic_does_not_refresh_and_loss_requires_new_reports() {
        let mut state = State::default();
        state.receive(16, &pressure(2500, 10_000_000), 10);
        state.receive(16, &[0xf1, 4, 0, 0, 0, 0, 0, 0], 99);
        state.receive(120, &[0; 8], 99);
        assert!(matches!(
            state.snapshot(100, 50).pressure,
            Observation::Stale {
                received_ms: 10,
                ..
            }
        ));
        state.loss();
        assert!(matches!(
            state.snapshot(100, 50).pressure,
            Observation::Unavailable
        ));
        state.receive(16, &pressure(2500, 10_000_000), 101);
        let snapshot = state.snapshot(101, 50);
        assert_eq!(
            (snapshot.reports, snapshot.invalid_reports, snapshot.losses),
            (2, 0, 1)
        );
        assert!(matches!(
            snapshot.pressure,
            Observation::Fresh {
                received_ms: 101,
                ..
            }
        ));
    }

    #[test]
    fn identity_is_exact_length_and_stays_opaque() {
        let payload = [1, 1, 0, 0, 0, 0, 0, 0x42, 0x23, 0x19];
        assert_eq!(
            decode(1, &payload),
            Decode::Report(Report::Identity(Identity {
                fields: [0x42, 0x23, 0x19]
            }))
        );
        assert_eq!(decode(1, &payload[..8]), Decode::Invalid);
        assert_eq!(decode(16, &payload), Decode::Unrelated);
    }

    #[test]
    fn fragmented_and_corrupt_envelopes_preserve_other_acquisition() {
        let mut frame = [0xa5, 12, 0x6f, 0xf1, 4, 16, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0];
        frame[6..14].copy_from_slice(&pressure(1234, 10_132_500));
        let crc = crc16(&frame[..14]);
        frame[14..].copy_from_slice(&crc.to_le_bytes());
        let mut decoder = Decoder::default();
        let mut state = State::default();
        for (now, bytes) in [(1, &frame[..7]), (2, &frame[7..])] {
            feed_frames(&mut decoder, 0, 0, bytes, |result| match result {
                Ok(frame) => {
                    if let Some((group, payload)) = frame.report() {
                        state.receive(group, &payload, now);
                    }
                }
                Err(()) => state.loss(),
            });
        }
        assert_eq!(state.snapshot(2, 10).reports, 1);
        let mut corrupt = frame;
        corrupt[8] ^= 1;
        feed_frames(&mut decoder, 0, 0, &corrupt, |result| {
            if result.is_err() {
                state.loss();
            }
        });
        assert!(matches!(
            state.snapshot(3, 10).pressure,
            Observation::Unavailable
        ));
        let mut recovered = false;
        feed_frames(&mut decoder, 0, 0, &frame, |result| {
            recovered |= result.is_ok();
        });
        assert!(recovered);
        assert_eq!(decoder.bad_crc, 1);
    }
}
