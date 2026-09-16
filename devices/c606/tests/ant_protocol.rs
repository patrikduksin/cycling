// Exercise the device wire adapter on the host without linking the HAL.
use c606_firmware::drivers::ant_protocol;

#[test]
fn crc_loss_invalidates_channel_before_recovered_sensor_data() {
    use device_api::ant::Event;
    use device_api::ant::Identity;
    use device_api::ant::LinkState;
    use firmware_services::ant::State;

    use c606_firmware::drivers::companion::Decoder;
    let peer = Identity {
        device_type: 40,
        device_number: 0x1234,
        transmission_type: 5,
    };
    let mut state = State::new();
    state.connect(peer, 0).unwrap();
    state.receive(Event::Connected(peer), 1);
    let report = |payload| {
        let mut frame = c606_firmware::drivers::companion::command(40, payload);
        frame[4] = 4;
        let crc = c606_firmware::drivers::companion::crc16(&frame[..14]);
        frame[14..].copy_from_slice(&crc.to_le_bytes());
        frame
    };
    let mut corrupt = report([0x30, 1, 0, 4, 0, 0, 2, 0]);
    corrupt[8] ^= 1;
    let good = report([0x31, 1, 0, 4, 0, 0, 2, 0]);
    let mut bytes = [0; 32];
    bytes[..16].copy_from_slice(&corrupt);
    bytes[16..].copy_from_slice(&good);
    let mut decoder = Decoder::default();
    let mut recovered = 0;
    c606_firmware::drivers::companion::feed_frames(
        &mut decoder,
        0,
        0,
        &bytes,
        |frame| match frame {
            Err(()) => state.transport_loss(2),
            Ok(frame) => {
                recovered += 1;
                let (group, payload) = frame.report().unwrap();
                if let Some(event) = ant_protocol::decode(group, payload) {
                    state.receive(event, 2);
                }
            }
        },
    );
    assert_eq!(recovered, 1);
    assert_eq!(decoder.bad_crc, 1);
    assert_eq!(state.snapshot(2).link, LinkState::TransportLost);
    assert!(state.pop_packet().is_none());
}

#[test]
fn sending_uses_supported_sensor_group_and_exact_eight_byte_payload() {
    let payload = [0x46, 0xff, 0xff, 0xff, 0xff, 1, 0x50, 1];
    for device_type in [40, 120, 11, 122, 123, 121, 34, 17, 128, 35] {
        let frame = ant_protocol::encode_send(device_type, payload).unwrap();
        assert_eq!(frame[..6], [0xa5, 12, 0x6f, 0xf1, 2, device_type]);
        assert_eq!(frame[6..14], payload);
        let mut decoder = c606_firmware::drivers::companion::Decoder::default();
        assert!(
            frame
                .into_iter()
                .find_map(|byte| decoder.push_frame(byte))
                .is_some()
        );
    }
    for unsupported in [0, 1, 16, 41, 255] {
        assert!(ant_protocol::encode_send(unsupported, payload).is_none());
    }
}

#[test]
fn bridge_replies_are_separate_from_sensor_reports_and_local_input() {
    use c606_firmware::drivers::companion::{Decoder, command, crc16};
    for (class, group, flag, expected) in [
        (5, 120, 1, Some(true)),
        (5, 120, 0, Some(false)),
        (5, 120, 2, None),
        (5, 1, 1, None),
        (5, 41, 1, None),
        (4, 120, 1, None),
    ] {
        let mut bytes = command(group, [0x46, 0xff, flag, 0, 0, 0, 0, 0]);
        bytes[4] = class;
        let checksum = crc16(&bytes[..14]);
        bytes[14..].copy_from_slice(&checksum.to_le_bytes());
        let mut decoder = Decoder::default();
        let frame = bytes
            .into_iter()
            .find_map(|byte| decoder.push_frame(byte))
            .unwrap();
        let reply = frame
            .reply()
            .and_then(|(group, data)| ant_protocol::decode_send_reply(group, data));
        assert_eq!(reply.map(|reply| reply.accepted), expected);
        if let Some(reply) = reply {
            assert_eq!(reply.device_type, 120);
            assert_eq!(reply.echoed, [0x46, 0xff]);
        }
        assert_eq!(frame.report().is_some(), class == 4);
        assert!(frame.identity_reply().is_none());
        assert!(frame.input().is_none());
    }
}
