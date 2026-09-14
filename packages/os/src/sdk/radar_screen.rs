//! Logical 240x320 RGB565 status screen for the outdoor ANT sensor capture test.
//! The caller supplies live connection and verified storage-commit state.

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SensorState {
    /// No sensor selected.
    #[default]
    Off,
    /// Selected, but no fresh packets.
    Wait,
    /// Selected sensor packets are fresh.
    On,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CaptureState {
    #[default]
    Idle,
    Preparing,
    Recording,
    Stopping,
    Stopped,
    Error,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Screen {
    pub state: CaptureState,
    pub remaining_secs: Option<u32>,
    pub free_slots: Option<u32>,
    pub saved_environment: u32,
    pub power_watts: Option<u16>,
    pub radar_age_secs: Option<u32>,
    pub power_age_secs: Option<u32>,
    pub sampled: bool,
    /// Independent packet freshness in RADAR, HEART, POWER order.
    pub sensors: [SensorState; 3],
    /// True only after the capture has verified a storage commit.
    pub logging: bool,
    pub saved_packets: u32,
    /// Fresh live position fix, independent of capture commits.
    pub gps_fix: bool,
    /// Verified GPS records, including records without a fix.
    pub saved_positions: u32,
    pub dropped: u32,
    pub elapsed_secs: u32,
    pub error: bool,
}

const BLACK: u16 = 0x0000;
const WHITE: u16 = 0xffff;
const GREEN: u16 = 0x07e0;
const RED: u16 = 0xf800;
const YELLOW: u16 = 0xffe0;

/// No allocation, text formatting or retained framebuffer is required.
pub fn pixel(x: usize, y: usize, state: Screen) -> u16 {
    if x >= 240 || y >= 320 {
        return BLACK;
    }
    if text(x, y, 39, 8, 3, b"RIDE TEST") {
        return WHITE;
    }
    let (label, color) = match if state.error {
        CaptureState::Error
    } else {
        state.state
    } {
        CaptureState::Idle => (b"IDLE".as_slice(), WHITE),
        CaptureState::Preparing => (b"PREPARING".as_slice(), YELLOW),
        CaptureState::Recording if !state.logging => (b"WAIT SAVE".as_slice(), YELLOW),
        CaptureState::Recording => (b"RECORDING".as_slice(), GREEN),
        CaptureState::Stopping => (b"STOPPING".as_slice(), YELLOW),
        CaptureState::Stopped => (b"STOPPED".as_slice(), WHITE),
        CaptureState::Error => (b"ERROR".as_slice(), RED),
    };
    if text(x, y, (240 - label.len() * 12) / 2, 36, 2, label) {
        return color;
    }
    if (y == 58 || y == 208) && (8..232).contains(&x) {
        return WHITE;
    }
    if text(x, y, 8, 68, 2, b"TIME LEFT")
        || optional_number(x, y, 144, 68, 2, state.remaining_secs, 5)
        || text(x, y, 208, 68, 2, b"S")
    {
        return WHITE;
    }
    if text(x, y, 8, 94, 2, b"GPS")
        || text(
            x,
            y,
            144,
            94,
            2,
            if state.gps_fix { b"FIX" } else { b"WAIT" },
        )
    {
        return if state.gps_fix { GREEN } else { YELLOW };
    }
    let (radar, radar_color) = sensor_label(state.sensors[0]);
    if text(x, y, 8, 122, 3, b"RADAR") || text(x, y, 144, 122, 3, radar) {
        return radar_color;
    }
    if text(x, y, 8, 149, 1, b"PACKET AGE")
        || optional_number(x, y, 80, 149, 1, state.radar_age_secs, 5)
        || text(x, y, 112, 149, 1, b"S")
    {
        return radar_color;
    }
    let (power, power_color) = sensor_label(state.sensors[2]);
    // A cached watt value must never look live after packets become stale.
    let watts = if state.sensors[2] == SensorState::On {
        state.power_watts.map(u32::from)
    } else {
        None
    };
    if text(x, y, 8, 167, 3, b"POWER")
        || optional_number(x, y, 120, 167, 3, watts, 5)
        || text(x, y, 216, 167, 3, b"W")
    {
        return power_color;
    }
    if text(x, y, 8, 195, 1, power)
        || text(x, y, 44, 195, 1, b"AGE")
        || optional_number(x, y, 80, 195, 1, state.power_age_secs, 5)
        || text(x, y, 112, 195, 1, b"S")
    {
        return power_color;
    }
    for (top, label, value) in [
        (218, b"ENV SAVED".as_slice(), Some(state.saved_environment)),
        (238, b"GPS SAVED".as_slice(), Some(state.saved_positions)),
        (258, b"SLOTS FREE".as_slice(), state.free_slots),
        (278, b"DROPS".as_slice(), Some(state.dropped)),
    ] {
        if text(x, y, 8, top, 2, label) || optional_number(x, y, 152, top, 2, value, 6) {
            return if top == 278 && state.dropped != 0 {
                YELLOW
            } else {
                WHITE
            };
        }
    }
    if state.sampled && text(x, y, 66, 302, 2, b"SAMPLE 2S") {
        return YELLOW;
    }
    BLACK
}

fn sensor_label(state: SensorState) -> (&'static [u8], u16) {
    match state {
        SensorState::Off => (b"OFF", WHITE),
        SensorState::Wait => (b"WAIT", YELLOW),
        SensorState::On => (b"ON", GREEN),
    }
}

fn optional_number(
    x: usize,
    y: usize,
    left: usize,
    top: usize,
    scale: usize,
    value: Option<u32>,
    digits: usize,
) -> bool {
    match value {
        Some(value) => number(x, y, left, top, scale, value, digits),
        None => text(x, y, left, top, scale, b"--"),
    }
}

pub(crate) fn text(
    x: usize,
    y: usize,
    left: usize,
    top: usize,
    scale: usize,
    label: &[u8],
) -> bool {
    if x < left || y < top || y >= top + 7 * scale {
        return false;
    }
    let column = (x - left) / scale;
    let Some(&character) = label.get(column / 6) else {
        return false;
    };
    dot(character, column % 6, (y - top) / scale)
}

/// Right-aligned in an explicit digit field; values saturate to its visible maximum.
pub(crate) fn number(
    x: usize,
    y: usize,
    left: usize,
    top: usize,
    scale: usize,
    value: u32,
    digits: usize,
) -> bool {
    if x < left || x >= left + digits * 6 * scale || y < top || y >= top + 7 * scale {
        return false;
    }
    let value = value.min(10_u32.pow(digits as u32) - 1);
    let column = (x - left) / scale;
    let divisor = 10_u32.pow((digits - 1 - column / 6) as u32);
    if divisor != 1 && value < divisor {
        return false;
    }
    dot(
        b'0' + ((value / divisor) % 10) as u8,
        column % 6,
        (y - top) / scale,
    )
}

fn dot(character: u8, x: usize, y: usize) -> bool {
    if x >= 5 {
        return false;
    }
    // Each row uses five low bits, with the leftmost dot in bit four.
    let rows = match character {
        b'.' => [0, 0, 0, 0, 0, 6, 6],
        b'Q' => [14, 17, 17, 17, 21, 18, 13],
        b'B' => [30, 17, 17, 30, 17, 17, 30],
        b'Y' => [17, 17, 10, 4, 4, 4, 4],
        b'>' => [16, 8, 4, 2, 4, 8, 16],
        b'-' => [0, 0, 0, 31, 0, 0, 0],
        b'M' => [17, 27, 21, 21, 17, 17, 17],
        b'U' => [17, 17, 17, 17, 17, 17, 14],
        b'A' => [14, 17, 17, 31, 17, 17, 17],
        b'C' => [14, 17, 16, 16, 16, 17, 14],
        b'D' => [30, 17, 17, 17, 17, 17, 30],
        b'E' => [31, 16, 16, 30, 16, 16, 31],
        b'F' => [31, 16, 16, 30, 16, 16, 16],
        b'H' => [17, 17, 17, 31, 17, 17, 17],
        b'G' => [14, 17, 16, 23, 17, 17, 15],
        b'I' => [31, 4, 4, 4, 4, 4, 31],
        b'K' => [17, 18, 20, 24, 20, 18, 17],
        b'L' => [16, 16, 16, 16, 16, 16, 31],
        b'N' => [17, 25, 25, 21, 19, 19, 17],
        b'O' => [14, 17, 17, 17, 17, 17, 14],
        b'P' => [30, 17, 17, 30, 16, 16, 16],
        b'R' => [30, 17, 17, 30, 20, 18, 17],
        b'S' => [15, 16, 16, 14, 1, 1, 30],
        b'T' => [31, 4, 4, 4, 4, 4, 4],
        b'V' => [17, 17, 17, 17, 17, 10, 4],
        b'W' => [17, 17, 17, 21, 21, 21, 10],
        b'X' => [17, 17, 10, 4, 10, 17, 17],
        b'0' => [14, 17, 19, 21, 25, 17, 14],
        b'1' => [4, 12, 4, 4, 4, 4, 14],
        b'2' => [14, 17, 1, 2, 4, 8, 31],
        b'3' => [30, 1, 1, 14, 1, 1, 30],
        b'4' => [2, 6, 10, 18, 31, 2, 2],
        b'5' => [31, 16, 16, 30, 1, 1, 30],
        b'6' => [14, 16, 16, 30, 17, 17, 14],
        b'7' => [31, 1, 2, 4, 8, 8, 8],
        b'8' => [14, 17, 17, 14, 17, 17, 14],
        b'9' => [14, 17, 17, 15, 1, 1, 14],
        _ => [0; 7],
    };
    rows[y] & (1 << (4 - x)) != 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_states_have_distinct_labels_and_expected_colors() {
        let states = [
            (CaptureState::Idle, WHITE),
            (CaptureState::Preparing, YELLOW),
            (CaptureState::Recording, GREEN),
            (CaptureState::Stopping, YELLOW),
            (CaptureState::Stopped, WHITE),
            (CaptureState::Error, RED),
        ];
        let mut masks = [[false; 240 * 14]; 6];
        for (index, (capture, expected)) in states.into_iter().enumerate() {
            let state = Screen {
                state: capture,
                logging: true,
                ..Screen::default()
            };
            let mut mask = [false; 240 * 14];
            for y in 36..50 {
                for x in 0..240 {
                    let color = pixel(x, y, state);
                    assert!(color == BLACK || color == expected);
                    mask[(y - 36) * 240 + x] = color != BLACK;
                }
            }
            assert!(mask.iter().any(|&lit| lit));
            assert!(!masks[..index].contains(&mask));
            masks[index] = mask;
        }
    }

    #[test]
    fn recording_waits_for_verified_save_and_error_overrides_it() {
        for (logging, error, label, expected) in [
            (false, false, b"WAIT SAVE".as_slice(), YELLOW),
            (true, false, b"RECORDING".as_slice(), GREEN),
            (false, true, b"ERROR".as_slice(), RED),
            (true, true, b"ERROR".as_slice(), RED),
        ] {
            let state = Screen {
                state: CaptureState::Recording,
                logging,
                error,
                ..Screen::default()
            };
            let mut lit = 0;
            for y in 36..50 {
                for x in 0..240 {
                    let expected_pixel = if text(x, y, (240 - label.len() * 12) / 2, 36, 2, label) {
                        lit += 1;
                        expected
                    } else {
                        BLACK
                    };
                    assert_eq!(pixel(x, y, state), expected_pixel);
                }
            }
            assert!(lit > 100);
        }
    }

    #[test]
    fn missing_or_stale_power_uses_placeholder_not_cached_watts() {
        let missing = Screen::default();
        let stale = Screen {
            power_watts: Some(250),
            ..missing
        };
        for y in 167..188 {
            for x in 120..210 {
                assert_eq!(pixel(x, y, missing), pixel(x, y, stale));
                assert_eq!(
                    pixel(x, y, missing) != BLACK,
                    text(x, y, 120, 167, 3, b"--")
                );
            }
        }
    }

    #[test]
    fn numeric_fields_keep_zero_and_saturated_maximum_inside_panel() {
        for (left, scale, digits) in [(144, 2, 5), (120, 3, 5), (152, 2, 6)] {
            assert!(left + digits * 6 * scale <= 240);
            for value in [0, 600, u32::MAX] {
                let mut lit = 0;
                for y in 0..14 * scale {
                    for x in 0..260 {
                        let visible = number(x, y, left, 0, scale, value, digits);
                        if visible {
                            assert!(x < 240);
                            lit += 1;
                        }
                        if value == u32::MAX {
                            assert_eq!(
                                visible,
                                number(x, y, left, 0, scale, 10_u32.pow(digits as u32) - 1, digits)
                            );
                        }
                    }
                }
                assert!(lit > 0);
            }
        }
        assert_eq!(pixel(usize::MAX, usize::MAX, Screen::default()), BLACK);
    }
}
