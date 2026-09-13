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

#[derive(Clone, Copy, Debug, Default)]
pub struct Screen {
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
    if text(x, y, 39, 10, 3, b"RIDE TEST") {
        return WHITE;
    }
    for (index, label) in [b"RADAR", b"HEART", b"POWER"].iter().enumerate() {
        let top = 44 + index * 34;
        if (top..top + 28).contains(&y) {
            let (status, color) = match state.sensors[index] {
                SensorState::Off => (b"OFF".as_slice(), WHITE),
                SensorState::Wait => (b"WAIT".as_slice(), YELLOW),
                SensorState::On => (b"ON".as_slice(), GREEN),
            };
            if text(x, y, 12, top + 4, 3, *label) || text(x, y, 156, top + 4, 3, status) {
                return color;
            }
        }
    }
    if text(x, y, 12, 150, 3, b"GPS")
        || text(
            x,
            y,
            156,
            150,
            3,
            if state.gps_fix { b"FIX" } else { b"WAIT" },
        )
    {
        return if state.gps_fix { GREEN } else { YELLOW };
    }
    let (log_label, log_color) = if state.error {
        (b"LOG ERROR".as_slice(), RED)
    } else if state.logging {
        (b"LOG SAVING".as_slice(), GREEN)
    } else {
        (b"LOG WAIT".as_slice(), YELLOW)
    };
    if text(x, y, (240 - log_label.len() * 18) / 2, 184, 3, log_label) {
        return log_color;
    }
    if text(x, y, 6, 219, 2, b"ANT SAVED")
        || number(x, y, 114, 219, 2, state.saved_packets)
        || text(x, y, 6, 241, 2, b"GPS SAVED")
        || number(x, y, 114, 241, 2, state.saved_positions)
        || text(x, y, 6, 263, 2, b"SECONDS")
        || number(x, y, 114, 263, 2, state.elapsed_secs)
    {
        return WHITE;
    }
    if text(x, y, 6, 285, 2, b"DROPPED") || number(x, y, 114, 285, 2, state.dropped) {
        return if state.dropped == 0 { WHITE } else { YELLOW };
    }
    BLACK
}

fn text(x: usize, y: usize, left: usize, top: usize, scale: usize, label: &[u8]) -> bool {
    if x < left || y < top || y >= top + 7 * scale {
        return false;
    }
    let column = (x - left) / scale;
    let Some(&character) = label.get(column / 6) else {
        return false;
    };
    dot(character, column % 6, (y - top) / scale)
}

fn number(x: usize, y: usize, left: usize, top: usize, scale: usize, value: u32) -> bool {
    if x < left || x >= left + 60 * scale || y < top || y >= top + 7 * scale {
        return false;
    }
    const DIVISORS: [u32; 10] = [
        1_000_000_000,
        100_000_000,
        10_000_000,
        1_000_000,
        100_000,
        10_000,
        1_000,
        100,
        10,
        1,
    ];
    let column = (x - left) / scale;
    let divisor = DIVISORS[column / 6];
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
    fn each_sensor_row_and_logging_are_independent() {
        for index in 0..3 {
            for status in [SensorState::Off, SensorState::Wait, SensorState::On] {
                let mut state = Screen::default();
                state.sensors[index] = status;
                for row in 0..3 {
                    let expected = if row != index || status == SensorState::Off {
                        WHITE
                    } else if status == SensorState::Wait {
                        YELLOW
                    } else {
                        GREEN
                    };
                    let mut lit = 0;
                    for y in 44 + row * 34..72 + row * 34 {
                        for x in 0..240 {
                            let color = pixel(x, y, state);
                            if color != BLACK {
                                assert_eq!(color, expected);
                                lit += 1;
                            }
                        }
                    }
                    assert!(lit > 100);
                }
                for logging in [false, true] {
                    for error in [false, true] {
                        state.logging = logging;
                        state.error = error;
                        let expected = if error {
                            RED
                        } else if logging {
                            GREEN
                        } else {
                            YELLOW
                        };
                        let mut lit = 0;
                        for y in 184..205 {
                            for x in 0..240 {
                                let color = pixel(x, y, state);
                                if color != BLACK {
                                    assert_eq!(color, expected);
                                    lit += 1;
                                }
                            }
                        }
                        assert!(lit > 100);
                    }
                }
            }
        }
    }

    #[test]
    fn full_counter_range_renders_inside_panel() {
        for value in [0, 600, u32::MAX] {
            let state = Screen {
                saved_packets: value,
                saved_positions: value,
                elapsed_secs: value,
                dropped: value,
                ..Screen::default()
            };
            let mut lit = [0; 4];
            for (index, (top, bottom)) in [(219, 233), (241, 255), (263, 277), (285, 299)]
                .into_iter()
                .enumerate()
            {
                for y in top..bottom {
                    for x in 114..240 {
                        if pixel(x, y, state) != BLACK {
                            lit[index] += 1;
                        }
                    }
                }
            }
            assert!(lit.into_iter().all(|count| count > 0));
        }
        assert_eq!(pixel(usize::MAX, usize::MAX, Screen::default()), BLACK);
    }
}
