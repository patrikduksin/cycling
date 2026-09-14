#[path = "../src/device/sound_protocol.rs"]
mod protocol;
#[test]
fn only_finite_patterns_and_stop_can_reach_transport() {
    for id in 0..=255u8 {
        assert_eq!(
            protocol::encode(Some(id)).is_some(),
            [0, 10, 21, 22].contains(&id)
        );
    }
    let stop = protocol::encode(None).unwrap();
    assert_eq!(&stop[6..14], &[0xe2, 1, 19, 0, 0, 0, 0, 0]);
    assert_eq!(
        u16::from_le_bytes(stop[14..].try_into().unwrap()),
        cycling_os::companion::crc16(&stop[..14])
    );
}
