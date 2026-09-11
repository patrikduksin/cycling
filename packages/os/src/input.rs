//! Single-finger reports used by the C606's stock 0x5a touch driver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Report {
    Press(Point),
    Release,
    Invalid,
}

/// Seven bytes from D000. Reject malformed, multi-finger and out-of-panel reports.
/// The controller family is a candidate; this wire format was recovered from stock.
pub fn decode(data: [u8; 7]) -> Report {
    if data[6] != 0xab || data[5] & 0x80 != 0 || data[5] > 1 {
        return Report::Invalid;
    }
    if data[5] == 0 || data[0] & 0x0f == 0 {
        return Report::Release;
    }
    if data[0] & 0x0f != 6 {
        return Report::Invalid;
    }
    let x = (u16::from(data[1]) << 4) | u16::from(data[3] >> 4);
    let y = (u16::from(data[2]) << 4) | u16::from(data[3] & 0x0f);
    if x >= 240 || y >= 320 {
        return Report::Invalid;
    }
    Report::Press(Point { x, y })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_packed_coordinates_and_release() {
        assert_eq!(
            decode([6, 14, 19, 0xff, 50, 1, 0xab]),
            Report::Press(Point { x: 239, y: 319 })
        );
        assert_eq!(decode([0, 14, 19, 0xff, 0, 1, 0xab]), Report::Release);
        assert_eq!(decode([6, 0, 0, 0, 0, 0, 0xab]), Report::Release);
    }

    #[test]
    fn rejects_bad_reports() {
        for data in [
            [6, 15, 0, 0, 0, 1, 0xab],
            [6, 0, 20, 0, 0, 1, 0xab],
            [6, 0, 0, 0, 0, 2, 0xab],
            [6, 0, 0, 0, 0, 1, 0],
            [6, 0, 0, 0, 0, 0x80, 0xab],
            [3, 0, 0, 0, 0, 1, 0xab],
        ] {
            assert_eq!(decode(data), Report::Invalid);
        }
    }
}
