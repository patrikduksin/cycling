//! Allocation-free 5x7 bitmap text shared by shell and application screens.

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
#[cfg(feature = "cycling")]
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
        b':' => [0, 6, 6, 0, 6, 6, 0],
        b'/' => [1, 1, 2, 4, 8, 16, 16],
        b'+' => [0, 4, 4, 31, 4, 4, 0],
        b'%' => [25, 26, 2, 4, 8, 11, 19],
        b'Z' => [31, 1, 2, 4, 8, 16, 31],
        b'J' => [7, 2, 2, 2, 18, 18, 12],
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

#[cfg(all(test, feature = "cycling"))]
mod tests {
    use super::number;

    #[test]
    fn numeric_fields_keep_zero_and_saturated_maximum_inside_bounds() {
        for (left, scale, digits) in [(144, 2, 5), (120, 3, 5), (152, 2, 6)] {
            for value in [0, 600, u32::MAX] {
                let mut lit = 0;
                for y in 0..14 * scale {
                    for x in 0..260 {
                        let visible = number(x, y, left, 0, scale, value, digits);
                        if visible {
                            assert!((left..left + digits * 6 * scale).contains(&x));
                            assert!(y < 7 * scale);
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
    }
}
