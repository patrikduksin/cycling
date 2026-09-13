// Exercise the device wire adapter on the host without linking the HAL.
#[path = "../src/device/ant_protocol.rs"]
mod ant_protocol;

#[test]
fn crc_loss_invalidates_channel_before_recovered_sensor_data() {
    use cycling_os::{
        ant::{Event, Identity, LinkState, State},
        companion::{self, Decoder},
    };
    let peer = Identity {
        device_type: 40,
        device_number: 0x1234,
        transmission_type: 5,
    };
    let mut state = State::new();
    state.connect(peer, 0).unwrap();
    state.receive(Event::Connected(peer), 1);
    let report = |payload| {
        let mut frame = companion::command(40, payload);
        frame[4] = 4;
        let crc = companion::crc16(&frame[..14]);
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
    companion::feed_frames(&mut decoder, 0, 0, &bytes, |frame| match frame {
        Err(()) => state.transport_loss(2),
        Ok(frame) => {
            recovered += 1;
            let (group, payload) = frame.report().unwrap();
            if let Some(event) = ant_protocol::decode(group, payload) {
                state.receive(event, 2);
            }
        }
    });
    assert_eq!(recovered, 1);
    assert_eq!(decoder.bad_crc, 1);
    assert_eq!(state.snapshot(2).link, LinkState::TransportLost);
    assert!(state.pop_packet().is_none());
}
