//! Full-screen power sheets: confirm turning off, the idle warning and the
//! last frame before the device turns off. Hints sit next to the physical
//! button they describe and only appear while a sheet is shown.
use firmware_shell::rendering::row::Row;
#[cfg(test)]
use firmware_shell::rendering::text::text;

const WHITE: u16 = 0xffff;
const CYAN: u16 = 0x07ff;
const DIM: u16 = 0x8410;
const AMBER: u16 = 0xfd20;
const RULE: u16 = 0x39e7;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// "Turn off?" after BACK on Home: ACT turns off, anything else returns.
    Confirm,
    /// Idle auto-off countdown; any button keeps VANA on.
    Warning { seconds: u64 },
    /// Drawn once before the device takes over and the backlight goes dark.
    TurningOff,
}

/// Left edge that centres `chars` glyphs of `scale` on the 240-wide canvas.
const fn centre(chars: usize, scale: usize) -> usize {
    (240 - chars * 6 * scale) / 2
}

impl Sheet {
    /// One pixel of the sheet; tests only, as it paints a whole row.
    pub fn pixel(self, x: usize, y: usize) -> u16 {
        let mut pixels = [0u16; 240];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(0)
    }

    /// Paint one row of the 240 x 320 sheet.
    pub fn paint(self, row: &mut Row) {
        let y = row.y();
        row.fill(0);
        match self {
            Self::Confirm => {
                row.text(centre(8, 4), 104, 4, b"TURN OFF", WHITE);
                if y == 150 {
                    row.span(40..200, RULE);
                }
                row.text(centre(15, 1), 166, 1, b"RIDES ARE SAVED", DIM);
                // Hints beside the buttons: BACK top left, OFF bottom right.
                row.text(8, 8, 2, b"BACK", DIM);
                row.text(240 - 3 * 6 * 3 - 6, 290, 3, b"OFF", CYAN);
                if y >= 316 {
                    row.span(170..240, CYAN);
                } else if y >= 270 {
                    row.span(236..240, CYAN);
                }
            }
            Self::Warning { seconds } => {
                row.text(centre(11, 3), 84, 3, b"TURNING OFF", AMBER);
                // "IN nn S", with the count right-aligned in two digits.
                let left = centre(7, 4);
                let seconds = seconds.min(99) as u8;
                let tens = if seconds >= 10 {
                    b'0' + seconds / 10
                } else {
                    b' '
                };
                row.text(left, 130, 4, b"IN", WHITE);
                row.text(left + 3 * 24, 130, 4, &[tens, b'0' + seconds % 10], WHITE);
                row.text(left + 6 * 24, 130, 4, b"S", WHITE);
                row.text(centre(16, 2), 206, 2, b"PRESS ANY BUTTON", DIM);
                row.text(centre(10, 2), 230, 2, b"TO STAY ON", DIM);
            }
            Self::TurningOff => row.text(centre(11, 3), 150, 3, b"TURNING OFF", WHITE),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shows(sheet: Sheet, left: usize, top: usize, scale: usize, label: &[u8]) -> bool {
        (top..top + 7 * scale).all(|y| {
            (left..left + label.len() * 6 * scale)
                .all(|x| (sheet.pixel(x, y) != 0) == text(x, y, left, top, scale, label))
        })
    }

    #[test]
    fn confirm_names_the_action_and_its_button() {
        assert!(shows(Sheet::Confirm, centre(8, 4), 104, 4, b"TURN OFF"));
        assert!(shows(Sheet::Confirm, 8, 8, 2, b"BACK"));
        assert_eq!(Sheet::Confirm.pixel(239, 319), CYAN, "ACT corner marker");
    }

    #[test]
    fn warning_counts_down_in_whole_seconds() {
        let sheet = Sheet::Warning { seconds: 9 };
        let left = centre(7, 4);
        // A single digit leaves the tens column dark.
        assert!((left + 72..left + 96).all(|x| (130..158).all(|y| sheet.pixel(x, y) == 0)));
        assert!(shows(sheet, left + 96, 130, 4, b"9"));
    }

    #[test]
    fn turning_off_is_a_single_quiet_line() {
        let lit = (0..320)
            .flat_map(|y| (0..240).map(move |x| (x, y)))
            .filter(|&(x, y)| Sheet::TurningOff.pixel(x, y) != 0)
            .all(|(_, y)| (150..171).contains(&y));
        assert!(lit);
    }
}
