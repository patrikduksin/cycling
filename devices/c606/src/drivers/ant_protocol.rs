//! ANT messages of the installed C606 companion firmware.
//!
//! Timeout fields are inferred to be seconds from stock callers. Data reports
//! carry only the device type. The bridge has ten fixed sensor slots, one per type.

use device_api::ant::Event;
use device_api::ant::Identity;
use device_api::ant::Request;

/// Categories accepted by the installed bridge, not implemented ANT+ profiles.
pub fn supports_type(device_type: u8) -> bool {
    matches!(
        device_type,
        40 | 120 | 11 | 122 | 123 | 121 | 34 | 17 | 128 | 35
    )
}

/// Encode one acknowledged-message request. Its reply cannot prove radio delivery.
pub fn encode_send(device_type: u8, data: [u8; 8]) -> Option<[u8; 16]> {
    supports_type(device_type).then(|| crate::drivers::companion::command(device_type, data))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SendReply {
    pub device_type: u8,
    pub echoed: [u8; 2],
    /// Only the bridge wrapper's result. Radio acceptance and delivery are unknown.
    pub accepted: bool,
}

/// Decode only complete class-five payloads validated by the companion decoder.
pub fn decode_send_reply(group: u8, data: [u8; 8]) -> Option<SendReply> {
    if !supports_type(group) || data[2] > 1 {
        return None;
    }
    Some(SendReply {
        device_type: group,
        echoed: [data[0], data[1]],
        accepted: data[2] == 1,
    })
}

/// Decode only complete class-4 reports validated by the companion decoder.
pub fn decode(group: u8, data: [u8; 8]) -> Option<Event> {
    match group {
        16 if data[..2] == [0xf3, 3] => match data[7] {
            0 => Some(Event::ScanEnded),
            1..=15 => Some(Event::Discovery {
                identity: Identity {
                    device_type: data[2],
                    device_number: u16::from_le_bytes([data[3], data[4]]),
                    transmission_type: data[5],
                },
                rssi: data[6] as i8,
            }),
            _ => None,
        },
        1 if data[0] == 0x17 => {
            let identity = Identity {
                device_type: data[1],
                device_number: u16::from_le_bytes([data[2], data[3]]),
                transmission_type: data[4],
            };
            match data[5] {
                3 => Some(Event::Connected(identity)),
                4 => Some(Event::Disconnected(identity)),
                5 => Some(Event::Timeout(identity)),
                _ => None,
            }
        }
        // These groups contain local companion control and input reports.
        0 | 1 | 16 => None,
        device_type => Some(Event::Data { device_type, data }),
    }
}

/// Restore the selected device number in C606 control acknowledgments.
///
/// The installed companion firmware writes a left-shifted high byte into an
/// eight-bit field, so connected/disconnected/timeout reports always lose that
/// byte. Discovery retains the full number. Only the selected channel's exact
/// type, low byte and transmission type can supply the missing high byte.
/// An acknowledgment therefore cannot independently confirm all 16 identity bits.
pub fn normalize(event: Event, selected: Option<Identity>) -> Event {
    let Some(selected) = selected else {
        return event;
    };
    let reported = match event {
        Event::Connected(identity) | Event::Disconnected(identity) | Event::Timeout(identity) => {
            identity
        }
        _ => return event,
    };
    if reported.device_number > u16::from(u8::MAX)
        || reported.device_number as u8 != selected.device_number as u8
        || reported.device_type != selected.device_type
        || reported.transmission_type != selected.transmission_type
    {
        return event;
    }
    match event {
        Event::Connected(_) => Event::Connected(selected),
        Event::Disconnected(_) => Event::Disconnected(selected),
        Event::Timeout(_) => Event::Timeout(selected),
        _ => event,
    }
}

pub fn encode(request: Request) -> [u8; 16] {
    let (group, data) = match request {
        Request::Scan { duration_ms } => {
            let seconds = duration_ms.div_ceil(1000).min(u32::from(u16::MAX)) as u16;
            let [lo, hi] = seconds.to_le_bytes();
            (16, [0xe1, 2, 0, lo, hi, 0xff, 0, 0])
        }
        Request::StopScan => (16, [0xe1, 2, 0, 0, 0, 0xff, 0, 0]),
        Request::Connect { identity } => {
            let [lo, hi] = identity.device_number.to_le_bytes();
            (
                1,
                [
                    0x17,
                    identity.device_type,
                    lo,
                    hi,
                    identity.transmission_type,
                    0,
                    10,
                    0,
                ],
            )
        }
        Request::Disconnect { identity } => (1, [0x17, identity.device_type, 0, 0, 0, 1, 0, 0]),
    };
    crate::drivers::companion::command(group, data)
}

#[cfg(test)]
mod tests {
    use super::*;
    const PEER: Identity = Identity {
        device_type: 40,
        device_number: 0x1234,
        transmission_type: 5,
    };

    #[test]
    fn decodes_discovery_and_control_without_leaking_local_reports() {
        assert_eq!(
            decode(16, [0xf3, 3, 40, 0x34, 0x12, 5, 200, 1]),
            Some(Event::Discovery {
                identity: PEER,
                rssi: -56
            })
        );
        assert_eq!(
            decode(16, [0xf3, 3, 0, 0, 0, 0, 0, 0]),
            Some(Event::ScanEnded)
        );
        assert_eq!(decode(16, [0xf3, 3, 40, 0x34, 0x12, 5, 200, 16]), None);
        for (code, event) in [
            (3, Event::Connected(PEER)),
            (4, Event::Disconnected(PEER)),
            (5, Event::Timeout(PEER)),
        ] {
            assert_eq!(
                decode(1, [0x17, 40, 0x34, 0x12, 5, code, 0, 0]),
                Some(event)
            );
        }
        assert_eq!(decode(1, [0x17, 40, 0x34, 0x12, 5, 2, 0, 0]), None);
        for group in [0, 1, 16] {
            assert_eq!(decode(group, [0; 8]), None);
        }
        assert_eq!(
            decode(40, [9; 8]),
            Some(Event::Data {
                device_type: 40,
                data: [9; 8]
            })
        );
    }

    #[test]
    fn normalizes_only_selected_zero_high_byte_control_identity() {
        let truncated = Identity {
            device_number: 0x34,
            ..PEER
        };
        for constructor in [Event::Connected, Event::Disconnected, Event::Timeout] {
            assert_eq!(
                normalize(constructor(truncated), Some(PEER)),
                constructor(PEER)
            );
            assert_eq!(normalize(constructor(PEER), Some(PEER)), constructor(PEER));
            assert_eq!(
                normalize(constructor(truncated), None),
                constructor(truncated)
            );
            for mismatch in [
                Identity {
                    device_number: 0x35,
                    ..truncated
                },
                Identity {
                    device_type: 41,
                    ..truncated
                },
                Identity {
                    transmission_type: 6,
                    ..truncated
                },
                Identity {
                    device_number: 0x1334,
                    ..truncated
                },
            ] {
                assert_eq!(
                    normalize(constructor(mismatch), Some(PEER)),
                    constructor(mismatch)
                );
            }
        }
        for event in [
            Event::Discovery {
                identity: truncated,
                rssi: -56,
            },
            Event::Discovery {
                identity: PEER,
                rssi: -56,
            },
            Event::ScanEnded,
            Event::Data {
                device_type: PEER.device_type,
                data: [0; 8],
            },
        ] {
            assert_eq!(normalize(event, Some(PEER)), event);
        }
    }

    #[test]
    fn command_bytes_and_checksum_match_fixed_envelope() {
        for (request, group, data) in [
            (
                Request::Scan { duration_ms: 1001 },
                16,
                [0xe1, 2, 0, 2, 0, 0xff, 0, 0],
            ),
            (Request::StopScan, 16, [0xe1, 2, 0, 0, 0, 0xff, 0, 0]),
            (
                Request::Connect { identity: PEER },
                1,
                [0x17, 40, 0x34, 0x12, 5, 0, 10, 0],
            ),
            (
                Request::Disconnect { identity: PEER },
                1,
                [0x17, 40, 0, 0, 0, 1, 0, 0],
            ),
        ] {
            let frame = encode(request);
            assert_eq!(frame[..6], [0xa5, 12, 0x6f, 0xf1, 2, group]);
            assert_eq!(frame[6..14], data);
            let mut decoder = crate::drivers::companion::Decoder::default();
            let mut validated = false;
            for byte in frame {
                if decoder.push_frame(byte).is_some() {
                    validated = true;
                }
            }
            assert!(validated);
            assert_eq!(decoder.bad_crc, 0);
        }
    }
}
