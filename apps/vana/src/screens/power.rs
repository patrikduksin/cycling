//! Full-screen power sheets: confirm turning off, the idle warning and the
//! last frame before the device turns off. They only appear while a sheet
//! is shown, and like every screen they carry no bottom button labels: an
//! `ACCENT` corner marks ACT where it turns the device off.
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, AMBER, Align, BLACK, BODY, CAPTION, HERO, Rect, Row, SECONDARY, TITLE, WHITE,
};
use core::fmt::Write;
use firmware_shell::rendering::font::Face;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// "Turn off?" after BACK on Home: ACT turns off, anything else returns.
    Confirm,
    /// Idle auto-off countdown; any button keeps VANA on.
    Warning { seconds: u64 },
    /// Drawn once before the device takes over and the backlight goes dark.
    TurningOff,
}

const CENTRE: usize = 120;
/// Text keeps 10 px from the panel's edges.
const WIDTH: usize = 220;

impl Sheet {
    /// The sheet laid out for one frame.
    pub fn canvas(self) -> Canvas {
        let mut canvas = Canvas::default();
        match self {
            Self::Confirm => {
                centred(&mut canvas, &[&TITLE, &BODY], b"TURN OFF", 150, WHITE);
                centred(
                    &mut canvas,
                    &[&BODY, &CAPTION],
                    b"RIDES ARE SAVED",
                    182,
                    SECONDARY,
                );
                // BACK sits by its button, top left.
                canvas.caption(b"BACK", 60, 8, 20, Align::Left);
                // ACT, bottom right, turns off: its corner is lit.
                for rect in [
                    Rect {
                        left: 170,
                        top: 316,
                        right: 240,
                        bottom: 320,
                    },
                    Rect {
                        left: 236,
                        top: 270,
                        right: 240,
                        bottom: 320,
                    },
                ] {
                    canvas.shape(Shape::Bar {
                        rect,
                        radius: 0,
                        color: ACCENT,
                    });
                }
            }
            Self::Warning { seconds } => {
                centred(&mut canvas, &[&BODY], b"TURNING OFF IN", 96, AMBER);
                let mut count = super::workout::Text::<4>::new();
                let _ = write!(count, "{}", seconds.min(99));
                centred(&mut canvas, &[&HERO], count.as_bytes(), 176, WHITE);
                centred(&mut canvas, &[&CAPTION], b"SECONDS", 198, SECONDARY);
                centred(
                    &mut canvas,
                    &[&BODY, &CAPTION],
                    b"PRESS ANY BUTTON",
                    244,
                    SECONDARY,
                );
                centred(
                    &mut canvas,
                    &[&BODY, &CAPTION],
                    b"TO STAY ON",
                    268,
                    SECONDARY,
                );
            }
            Self::TurningOff => centred(&mut canvas, &[&BODY], b"TURNING OFF", 168, WHITE),
        }
        canvas
    }

    /// Paint one row of the 240 x 320 sheet from its `canvas`.
    pub fn paint(canvas: &Canvas, row: &mut Row) {
        row.fill(BLACK);
        canvas.paint(row);
    }
}

/// `text` centred on the sheet in the largest of `faces` that fits.
fn centred(canvas: &mut Canvas, faces: &[&'static Face], text: &[u8], base: usize, color: u16) {
    canvas.fit(faces, text, WIDTH, CENTRE, base, Align::Center, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{HEIGHT, Rect as Bounds};

    const SCREEN: Bounds = Bounds {
        left: 0,
        top: 0,
        right: 240,
        bottom: HEIGHT,
    };

    /// Every row the sheet paints equals the canvas's per-pixel reference.
    fn assert_paints_reference(sheet: Sheet) {
        let canvas = sheet.canvas();
        for y in 0..HEIGHT {
            let mut pixels = [0x5555; 240];
            Sheet::paint(&canvas, &mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                assert_eq!(pixel, canvas.pixel(x, y, BLACK), "{sheet:?} at {x},{y}");
            }
        }
    }

    #[test]
    fn sheets_lay_out_whole_and_paint_their_reference() {
        let sheets = [
            (Sheet::Confirm, &["TURN OFF", "RIDES ARE SAVED", "BACK"][..]),
            (
                Sheet::Warning { seconds: 9 },
                &["TURNING OFF IN", "9", "TO STAY ON"],
            ),
            (Sheet::Warning { seconds: 60 }, &["60"]),
            (Sheet::Warning { seconds: 1000 }, &["99"]),
            (Sheet::TurningOff, &["TURNING OFF"]),
        ];
        for (sheet, want) in sheets {
            sheet
                .canvas()
                .assert_laid_out(SCREEN, want, &std::format!("{sheet:?}"));
            assert_paints_reference(sheet);
        }
    }

    #[test]
    fn confirm_marks_act_and_turning_off_is_one_quiet_line() {
        let confirm = Sheet::Confirm.canvas();
        assert_eq!(confirm.pixel(239, 319, BLACK), ACCENT, "ACT corner marker");
        assert_eq!(confirm.find("TURN OFF").map(|run| run.color), Some(WHITE));
        let off = Sheet::TurningOff.canvas();
        assert_eq!(off.runs.len(), 1);
        assert!(off.shapes.is_empty());
    }
}
