//! ANT+ bicycle power calibration page (0x01): the manual zero request and
//! the power meter's answer.
//!
//! Layout as recalled from the ANT+ Bicycle Power profile and not checked
//! against a real power meter: byte 1 is the calibration ID, bytes 6 and 7 of
//! a response carry the offset, least significant byte first.

const PAGE: u8 = 0x01;
const MANUAL_ZERO: u8 = 0xaa;
const SUCCESS: u8 = 0xac;
const FAILURE: u8 = 0xaf;

/// What a power meter said about a manual zero request.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Response {
    /// Zeroed; the offset is in the power meter's own units.
    Zeroed(i16),
    Failed,
}

/// The acknowledged message that asks a power meter to zero itself.
pub const fn request() -> [u8; 8] {
    [PAGE, MANUAL_ZERO, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff]
}

/// Only a success or failure answer; every other page or ID is not one.
pub const fn decode(data: [u8; 8]) -> Option<Response> {
    if data[0] != PAGE {
        return None;
    }
    match data[1] {
        SUCCESS => Some(Response::Zeroed(i16::from_le_bytes([data[6], data[7]]))),
        FAILURE => Some(Response::Failed),
        _ => None,
    }
}

/// The page a simulated or test power meter sends back.
pub const fn answer(response: Response) -> [u8; 8] {
    match response {
        Response::Zeroed(offset) => {
            let [low, high] = offset.to_le_bytes();
            [PAGE, SUCCESS, 0xff, 0xff, 0xff, 0xff, low, high]
        }
        Response::Failed => [PAGE, FAILURE, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn answers_decode_to_what_they_say() {
        for offset in [i16::MIN, -1, 0, 1, 512, i16::MAX] {
            assert_eq!(
                decode(answer(Response::Zeroed(offset))),
                Some(Response::Zeroed(offset))
            );
        }
        assert_eq!(decode(answer(Response::Failed)), Some(Response::Failed));
    }

    #[test]
    fn only_calibration_answers_decode() {
        assert_eq!(decode(request()), None);
        for page in 0..=255u8 {
            for id in 0..=255u8 {
                let decoded = decode([page, id, 0, 0, 0, 0, 0x34, 0x12]);
                let wanted = page == PAGE && (id == SUCCESS || id == FAILURE);
                assert_eq!(decoded.is_some(), wanted, "{page:#x} {id:#x}");
            }
        }
    }

    #[test]
    fn the_offset_is_little_endian_and_signed() {
        let data = [0x01, 0xac, 0xff, 0xff, 0xff, 0xff, 0xfe, 0xff];
        assert_eq!(decode(data), Some(Response::Zeroed(-2)));
    }
}
