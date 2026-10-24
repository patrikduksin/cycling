//! Arbitrary radio input never panics the ANT+ and BLE profile decoders.

use crate::sensors::ant::{Battery, HeartRate as AntHeart, Power};
use crate::sensors::ble::Client;
use crate::sensors::ble_profile::{CrankCadence, CscMeasurement, HeartRate, Profile};
use crate::sensors::radar::Radar;
use crate::testing::Rng;
use device_api::ble_transport::{Link, Packet, Snapshot};

fn page(rng: &mut Rng) -> [u8; 8] {
    let mut data = rng.next().to_le_bytes();
    // Mostly pages the decoders know, so their fields are what varies.
    if !rng.one_in(4) {
        data[0] = rng.pick(&[0x01, 0x07, 0x10, 0x30, 0x31, 0x52, 0x57]);
    }
    data
}

#[test]
fn arbitrary_ant_pages_decode_without_panic() {
    let mut rng = Rng::new(1);
    let mut radar = Radar::new();
    let mut now: u64 = 0;
    for _ in 0..20_000 {
        let data = page(&mut rng);
        if let Some(heart) = AntHeart::decode(data) {
            assert_ne!(heart.bpm, Some(0));
            assert_ne!(data[0], 0x52);
        }
        for kind in [11, 120, 121, 0, 255] {
            if let Some(battery) = Battery::decode(kind, data) {
                assert!(battery.millivolts.is_none_or(|mv| mv < 15_000));
            }
        }
        if let Some(power) = Power::decode(data) {
            assert_eq!(data[0], 0x10);
            assert_ne!(power.cadence_rpm, Some(0xff));
        }
        // Time may stall or step back across a sensor reset.
        now = match rng.below(8) {
            0 => now.saturating_sub(rng.below(10_000)),
            1 => rng.next(),
            _ => now + rng.below(2_000),
        };
        if rng.one_in(500) {
            radar.reset();
        }
        radar.receive(data, now);
        radar.snapshot(now.wrapping_add(rng.below(5_000)));
        radar.received_ms();
    }
}

#[test]
fn arbitrary_ble_notifications_decode_without_panic() {
    let mut rng = Rng::new(2);
    let mut cadence = CrankCadence::default();
    for _ in 0..20_000 {
        let value = rng.bytes(24);
        if let Some(heart) = HeartRate::parse(&value) {
            assert!(heart.rr_len <= 4);
            assert_eq!(heart.contact_detected.is_some(), heart.contact_supported);
        }
        if let Some(measurement) = CscMeasurement::parse(&value) {
            assert!(measurement.wheel.is_some() || measurement.crank.is_some());
        }
        let received = rng.next() >> rng.below(64);
        if let Some(tenths) = cadence.update(rng.next() as u16, rng.next() as u16, received) {
            assert!(tenths <= CrankCadence::MAX_TENTHS);
        }
    }
}

#[test]
fn a_ble_client_survives_any_link_and_packet_order() {
    for seed in 0..200 {
        let mut rng = Rng::new(seed);
        let profile = rng.pick(&[Profile::Echo, Profile::HeartRate, Profile::Cadence]);
        let mut client = Client::new(profile);
        let mut transport = Snapshot {
            link: Link::Connected,
            ..Snapshot::default()
        };
        let (mut sequence, mut now) = (0u32, 0u64);
        for _ in 0..200 {
            match rng.below(10) {
                0 => {
                    transport.link = rng.pick(&[
                        Link::Off,
                        Link::Advertising,
                        Link::Scanning,
                        Link::Connecting,
                        Link::Connected,
                        Link::Retrying,
                        Link::Failed,
                    ]);
                    transport.connections = transport.connections.wrapping_add(rng.below(2) as u32);
                    transport.dropped = transport.dropped.wrapping_add(rng.below(2) as u32);
                    client.update(transport);
                }
                1 => client.select_profile(profile, rng.next() as u32),
                _ => {
                    sequence = match rng.below(6) {
                        0 => rng.next() as u32,
                        1 => sequence.wrapping_sub(1),
                        _ => sequence.wrapping_add(1),
                    };
                    now += rng.below(3_000);
                    let mut bytes = [0; 60];
                    let value = rng.bytes(60);
                    bytes[..value.len()].copy_from_slice(&value);
                    client.packet(Packet {
                        connection: transport.connections,
                        sequence,
                        dropped: transport.dropped,
                        received_ms: now,
                        bytes,
                        // Lengths past the buffer are malformed, not truncated.
                        length: if rng.one_in(10) {
                            rng.byte()
                        } else {
                            value.len() as u8
                        },
                    });
                }
            }
            let snapshot = client.snapshot(now + rng.below(20_000));
            assert!(
                snapshot
                    .cadence_tenths
                    .is_none_or(|tenths| u32::from(tenths) <= CrankCadence::MAX_TENTHS)
            );
        }
    }
}
