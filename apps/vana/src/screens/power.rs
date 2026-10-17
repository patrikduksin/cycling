//! Full-screen power sheets: confirm turning off, the idle warning, the
//! farewell before the device turns off, and the charge shown while the
//! device rests on USB power with VANA off. Like every screen they carry no
//! bottom button labels: an `ACCENT` corner marks ACT where it turns the
//! device off.
//!
//! Each sheet is built round one ring (`chrome::Ring`): the power symbol,
//! the countdown, the VANA mark and the charge all use the same shape.
use super::brand;
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, AMBER, Align, BLACK, BODY, Battery, CAPTION, GREEN, HERO, LARGE, Rect, Ring, Row,
    SECONDARY, TITLE, TURN, WHITE, blend,
};
use core::fmt::Write;
use firmware_shell::rendering::font::Face;

/// The idle warning's ring is full at this many seconds.
pub const WARNING_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// "Turn off?" after BACK on Home: ACT turns off, anything else returns.
    Confirm,
    /// Idle auto-off countdown; any button keeps VANA on.
    Warning { seconds: u64 },
    /// The farewell, `progress` thousandths of the way to black.
    TurningOff { progress: u16 },
    /// VANA is off and the device charges on USB power; `known` is
    /// whether the battery has reported its charge yet.
    Charging { battery: Battery, known: bool },
}

const CENTRE: usize = 120;
/// Text keeps 10 px from the panel's edges.
const WIDTH: usize = 220;

const fn ring(cy: usize, radius: i32, width: i32, start: u16, sweep: u16) -> Ring {
    Ring {
        cx: 4 * CENTRE as i32,
        cy: 4 * cy as i32,
        radius: 4 * radius,
        width: 4 * width,
        start,
        sweep,
    }
}

/// A share (0 to 100) of a turn.
const fn share(percent: u32) -> u16 {
    let percent = if percent > 100 { 100 } else { percent };
    (percent * TURN as u32 / 100) as u16
}

impl Sheet {
    /// Whether the sheet moves from frame to frame.
    pub const fn animated(self) -> bool {
        match self {
            Self::Confirm => false,
            Self::Warning { .. } | Self::TurningOff { .. } => true,
            Self::Charging { battery, .. } => battery.charging && battery.percent < 100,
        }
    }

    /// The sheet laid out for one frame.
    pub fn canvas(self) -> Canvas {
        let mut canvas = Canvas::default();
        match self {
            Self::Confirm => confirm(&mut canvas),
            Self::Warning { seconds } => warning(&mut canvas, seconds),
            Self::TurningOff { progress } => {
                brand::farewell(&mut canvas, progress);
                let quiet = blend(SECONDARY, BLACK, 15 - (progress.min(1000) / 67) as u8);
                centred(&mut canvas, &[&BODY], b"TURNING OFF", 268, quiet);
            }
            Self::Charging { battery, known } => charging(&mut canvas, battery, known),
        }
        canvas
    }

    /// Paint one row of the 240 x 320 sheet from its `canvas`.
    pub fn paint(canvas: &Canvas, row: &mut Row) {
        row.fill(BLACK);
        canvas.paint(row);
    }
}

/// The power symbol, the question and the ACT corner.
fn confirm(canvas: &mut Canvas) {
    // A ring open at the top with a bar through the gap.
    let gap = TURN / 12;
    canvas.shape(Shape::ring(ring(118, 32, 7, gap, TURN - 2 * gap), WHITE));
    canvas.shape(Shape::Bar {
        rect: Rect {
            left: CENTRE - 3,
            top: 76,
            right: CENTRE + 4,
            bottom: 118,
        },
        radius: 3,
        color: WHITE,
    });
    centred(canvas, &[&TITLE, &BODY], b"TURN OFF", 208, WHITE);
    centred(
        canvas,
        &[&BODY, &CAPTION],
        b"RIDES ARE SAVED",
        240,
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

/// The seconds left inside an amber ring that empties with them.
fn warning(canvas: &mut Canvas, seconds: u64) {
    let cy = 168;
    let track = blend(AMBER, BLACK, 4);
    canvas.shape(Shape::ring(ring(cy, 70, 8, 0, TURN), track));
    let left = seconds.min(WARNING_SECONDS) as u32;
    let sweep = (left * u32::from(TURN) / WARNING_SECONDS as u32) as u16;
    canvas.shape(Shape::ring(ring(cy, 70, 8, TURN - sweep, sweep), AMBER));
    centred(canvas, &[&BODY], b"TURNING OFF IN", 72, AMBER);
    let mut count = super::workout::Text::<4>::new();
    let _ = write!(count, "{}", seconds.min(99));
    canvas.fit(
        &[&HERO, &LARGE],
        count.as_bytes(),
        110,
        CENTRE,
        cy + 22,
        Align::Center,
        WHITE,
    );
    centred(canvas, &[&CAPTION], b"SECONDS", cy + 44, SECONDARY);
    centred(
        canvas,
        &[&BODY, &CAPTION],
        b"PRESS ANY BUTTON",
        276,
        SECONDARY,
    );
    centred(canvas, &[&BODY, &CAPTION], b"TO STAY ON", 300, SECONDARY);
}

/// The charge as a green ring with its percentage inside. While charging,
/// a lighter arc climbs from the charge to full, as the status bar's icon
/// fills, so the screen shows it is alive.
fn charging(canvas: &mut Canvas, battery: Battery, known: bool) {
    let cy = 146;
    let (radius, width) = (84, 14);
    canvas.shape(Shape::ring(
        ring(cy, radius, width, 0, TURN),
        blend(GREEN, BLACK, 4),
    ));
    let level = if known {
        u32::from(battery.percent.min(100))
    } else {
        0
    };
    if known && battery.charging && level < 100 {
        let climbing = share(u32::from(battery.fill()));
        canvas.shape(Shape::ring(
            ring(cy, radius, width, 0, climbing),
            blend(GREEN, BLACK, 8),
        ));
    }
    canvas.shape(Shape::ring(ring(cy, radius, width, 0, share(level)), GREEN));
    let mut percent = super::workout::Text::<6>::new();
    if known {
        let _ = write!(percent, "{level}%");
    } else {
        let _ = percent.push_str("--");
    }
    canvas.fit(
        &[&HERO, &LARGE, &BODY],
        percent.as_bytes(),
        2 * (radius - width / 2) as usize - 12,
        CENTRE,
        cy + 20,
        Align::Center,
        WHITE,
    );
    let (state, color): (&[u8], u16) = match (battery.charging, level) {
        (_, 100) => (b"CHARGED", GREEN),
        (true, _) => (b"CHARGING", GREEN),
        (false, _) => (b"ON USB POWER", SECONDARY),
    };
    centred(canvas, &[&BODY], state, 280, color);
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

    fn charge(percent: u8, charging: bool, phase: u8) -> Sheet {
        Sheet::Charging {
            battery: Battery {
                percent,
                charging,
                phase,
            },
            known: true,
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
            (Sheet::TurningOff { progress: 0 }, &["TURNING OFF", "VANA"]),
            (Sheet::TurningOff { progress: 500 }, &["VANA"]),
            (charge(64, true, 2), &["64%", "CHARGING"]),
            (charge(100, true, 0), &["100%", "CHARGED"]),
            (charge(3, false, 0), &["3%", "ON USB POWER"]),
            (
                Sheet::Charging {
                    battery: Battery::default(),
                    known: false,
                },
                &["--", "ON USB POWER"],
            ),
        ];
        for (sheet, want) in sheets {
            sheet
                .canvas()
                .assert_laid_out(SCREEN, want, &std::format!("{sheet:?}"));
            assert_paints_reference(sheet);
        }
    }

    #[test]
    fn confirm_marks_act_and_draws_the_power_symbol() {
        let confirm = Sheet::Confirm.canvas();
        assert_eq!(confirm.pixel(239, 319, BLACK), ACCENT, "ACT corner marker");
        assert_eq!(confirm.find("TURN OFF").map(|run| run.color), Some(WHITE));
        // The ring's left side and the bar through its gap are white.
        assert_eq!(confirm.pixel(CENTRE - 32, 118, BLACK), WHITE);
        assert_eq!(confirm.pixel(CENTRE, 80, BLACK), WHITE);
        assert_eq!(confirm.pixel(CENTRE + 12, 87, BLACK), BLACK, "the gap");
    }

    #[test]
    fn the_warning_ring_empties_with_the_seconds() {
        let amber = |seconds| {
            let canvas = Sheet::Warning { seconds }.canvas();
            (0..HEIGHT)
                .flat_map(|y| (0..240).map(move |x| (x, y)))
                .filter(|&(x, y)| canvas.pixel(x, y, BLACK) == AMBER)
                .count()
        };
        let (full, half) = (amber(30), amber(15));
        assert!(
            half * 2 > full * 8 / 10 && half * 2 < full * 12 / 10,
            "{half} of {full}"
        );
    }

    #[test]
    fn charging_climbs_from_the_charge_to_full_and_rests_when_charged() {
        let green = |sheet: Sheet| {
            let canvas = sheet.canvas();
            (0..HEIGHT)
                .flat_map(|y| (0..240).map(move |x| (x, y)))
                .filter(|&(x, y)| canvas.pixel(x, y, BLACK) == blend(GREEN, BLACK, 8))
                .count()
        };
        assert!(
            green(charge(40, true, 0)) < 100,
            "the climb starts at the charge"
        );
        assert!(green(charge(40, true, 3)) > 1000, "and reaches full");
        assert!(charge(40, true, 1).animated());
        assert!(!charge(100, true, 1).animated());
        assert!(!charge(40, false, 1).animated());
        // The same arc on the right of the ring, 25% charge, is lit green.
        let canvas = charge(25, false, 0).canvas();
        assert_eq!(canvas.pixel(CENTRE + 84, 146, BLACK), GREEN);
        assert_ne!(canvas.pixel(CENTRE - 84, 146, BLACK), GREEN);
    }
}
