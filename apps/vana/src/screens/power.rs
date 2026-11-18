//! Full-screen power sheets: confirm turning off, the idle warning, the
//! farewell before the device turns off, and the charge shown while the
//! device rests on USB power with VANA off. Like every screen they carry no
//! bottom button labels: an `ACCENT` corner marks ACT where it turns the
//! device off.
//!
//! Each sheet is built round one ring (`chrome::Ring`): the power symbol,
//! the countdown, the VANA mark and the charge all use the same shape.
use super::Layout;
use super::brand::{self, ring};
use super::canvas::{Canvas, Shape};
use super::chrome::{
    ACCENT, AMBER, Align, BLACK, Battery, GREEN, Rect, Row, SECONDARY, TURN, WHITE, mix,
};
use super::scaled::{Scaled, Track};
use core::fmt::Write;
use firmware_shell::rendering::font::{Face, FaceRole};

/// The idle warning's ring is full at this many seconds.
pub const WARNING_SECONDS: u64 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sheet {
    /// "Turn off?" after BACK on Home: ACT turns off, anything else returns.
    /// With `unsaved` rides the phone has not confirmed it says to sync them
    /// first, and ACT does not turn off.
    Confirm { unsaved: u8 },
    /// Idle auto-off countdown; any button keeps VANA on.
    Warning { seconds: u64 },
    /// The farewell, `progress` thousandths of the way to black.
    TurningOff { progress: u16 },
    /// VANA is off and the device charges on USB power; `known` is
    /// whether the battery has reported its charge yet.
    Charging { battery: Battery, known: bool },
}

/// The sheets on one panel. Text rows end on their capitals' baseline and
/// keep 10 units from the panel's edges; each ring runs round a square.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub confirm: Confirm,
    pub warning: Warning,
    /// TURNING OFF under the farewell's mark.
    pub farewell: Rect,
    pub charge: Charge,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Confirm {
    /// The power symbol's ring, and the bar from above it to its centre.
    pub ring: Rect,
    pub stem: Rect,
    pub title: Rect,
    pub line: Rect,
    /// BACK, by its button.
    pub back: Rect,
    /// The lit corner by ACT: along the bottom edge and up the right one.
    pub act: [Rect; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Warning {
    pub heading: Rect,
    pub ring: Rect,
    pub count: Rect,
    pub seconds: Rect,
    pub press: Rect,
    pub stay: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Charge {
    pub ring: Rect,
    pub percent: Rect,
    pub state: Rect,
}

const fn square(rows: Rect, side: u16, at: &Scaled) -> Rect {
    use Track::{Fr, Unit};
    let [_, columns, _] = rows.cols([Fr(1), Unit(side), Fr(1)], Unit(0), at);
    rows.with_columns(columns)
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, text, _] = screen.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [_, _, above, upper, lower, title, line, _, _] = screen.rows(
        [
            Unit(76),
            Fr(1),
            Unit(10),
            Unit(32),
            Unit(32),
            Unit(58),
            Unit(32),
            Fr(1),
            Unit(80),
        ],
        Unit(0),
        at,
    );
    let symbol = square(
        Rect {
            bottom: lower.bottom,
            ..upper
        },
        64,
        at,
    );
    let [_, stem, _] = symbol.cols([Unit(29), Unit(7), Fr(1)], Unit(0), at);
    let [back, _] = screen.rows([Unit(20), Fr(1)], Unit(0), at);
    let [_, back, _] = back.cols([Unit(8), Unit(60), Fr(1)], Unit(0), at);
    let [_, bottom_edge] = screen.rows([Fr(1), Unit(4)], Unit(0), at);
    let [_, along] = bottom_edge.cols([Fr(1), Unit(70)], Unit(0), at);
    let [_, right_edge] = screen.cols([Fr(1), Unit(4)], Unit(0), at);
    let [_, up] = right_edge.rows([Fr(1), Unit(50)], Unit(0), at);
    let confirm = Confirm {
        ring: symbol,
        stem: Rect {
            top: above.top,
            bottom: upper.bottom,
            ..stem
        },
        title: title.with_columns(text),
        line: line.with_columns(text),
        back,
        act: [along, up],
    };

    let [heading, _, _, ring, press, stay, _, _] = screen.rows(
        [
            Unit(72),
            Fr(1),
            Unit(26),
            Unit(140),
            Unit(38),
            Unit(24),
            Fr(1),
            Unit(20),
        ],
        Unit(0),
        at,
    );
    let ring = square(ring, 140, at);
    let [count, seconds, _] = ring.rows([Unit(92), Unit(22), Fr(1)], Unit(0), at);
    let [_, inside, _] = ring.cols([Unit(15), Fr(1), Unit(15)], Unit(0), at);
    let warning = Warning {
        heading: heading.with_columns(text),
        ring,
        count: count.with_columns(inside),
        seconds: seconds.with_columns(text),
        press: press.with_columns(text),
        stay: stay.with_columns(text),
    };

    let [_, _, ring, state, _, _] = screen.rows(
        [Unit(62), Fr(1), Unit(168), Unit(50), Fr(1), Unit(40)],
        Unit(0),
        at,
    );
    let ring = square(ring, 168, at);
    let [percent, _] = ring.rows([Unit(104), Fr(1)], Unit(0), at);
    let [_, inside, _] = ring.cols([Unit(13), Fr(1), Unit(13)], Unit(0), at);
    let charge = Charge {
        ring,
        percent: percent.with_columns(inside),
        state: state.with_columns(text),
    };

    Regions {
        confirm,
        warning,
        farewell: brand::regions(at).below.with_columns(text),
        charge,
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
            Self::Confirm { .. } => false,
            Self::Warning { .. } | Self::TurningOff { .. } => true,
            Self::Charging { battery, .. } => battery.charging && battery.percent < 100,
        }
    }

    /// The sheet laid out for one frame on `layout`'s panel.
    pub fn canvas(self, layout: &Layout) -> Canvas {
        let mut canvas = Canvas::default();
        match self {
            Self::Confirm { unsaved } => confirm(&mut canvas, layout, unsaved),
            Self::Warning { seconds } => warning(&mut canvas, layout, seconds),
            Self::TurningOff { progress } => {
                brand::farewell(&mut canvas, layout, progress);
                let quiet = brand::fade(SECONDARY, progress);
                let body = layout.scaled.face(FaceRole::Body);
                let row = layout.power.farewell;
                centred(&mut canvas, &[body], b"TURNING OFF", row, quiet);
            }
            Self::Charging { battery, known } => charging(&mut canvas, layout, battery, known),
        }
        canvas
    }

    /// Paint one row of the sheet from its `canvas`.
    pub fn paint(canvas: &Canvas, row: &mut Row) {
        row.fill(BLACK);
        canvas.paint(row);
    }
}

/// The power symbol, the question and the ACT corner, which unsaved rides
/// take away.
fn confirm(canvas: &mut Canvas, layout: &Layout, unsaved: u8) {
    let (at, regions) = (&layout.scaled, &layout.power.confirm);
    // A ring open at the top with a bar through the gap.
    let gap = TURN / 12;
    let symbol = ring(at, regions.ring, 7, gap, TURN - 2 * gap);
    canvas.shape(Shape::ring(symbol, WHITE));
    canvas.shape(Shape::Bar {
        rect: regions.stem,
        radius: at.unit(3),
        color: WHITE,
    });
    let [title, body, caption] =
        [FaceRole::Title, FaceRole::Body, FaceRole::Caption].map(|role| at.face(role));
    centred(canvas, &[title, body], b"TURN OFF", regions.title, WHITE);
    let (line, color): (&[u8], u16) = match unsaved {
        0 => (b"RIDES ARE SAVED", SECONDARY),
        1 => (b"SYNC RIDE FIRST", AMBER),
        _ => (b"SYNC RIDES FIRST", AMBER),
    };
    centred(canvas, &[body, caption], line, regions.line, color);
    // BACK sits by its button, top left.
    let back = regions.back;
    canvas.caption(
        at,
        b"BACK",
        back.width(),
        back.left,
        back.bottom,
        Align::Left,
    );
    if unsaved > 0 {
        return;
    }
    // ACT, bottom right, turns off: its corner is lit.
    for rect in regions.act {
        canvas.shape(Shape::Bar {
            rect,
            radius: 0,
            color: ACCENT,
        });
    }
}

/// The seconds left inside an amber ring that empties with them.
fn warning(canvas: &mut Canvas, layout: &Layout, seconds: u64) {
    let (at, regions) = (&layout.scaled, &layout.power.warning);
    let track = mix(AMBER, BLACK, 4);
    canvas.shape(Shape::ring(ring(at, regions.ring, 8, 0, TURN), track));
    let left = seconds.min(WARNING_SECONDS) as u32;
    let sweep = (left * u32::from(TURN) / WARNING_SECONDS as u32) as u16;
    let arc = ring(at, regions.ring, 8, TURN - sweep, sweep);
    canvas.shape(Shape::ring(arc, AMBER));
    let [hero, large, body, caption] = [
        FaceRole::Hero,
        FaceRole::Large,
        FaceRole::Body,
        FaceRole::Caption,
    ]
    .map(|role| at.face(role));
    centred(canvas, &[body], b"TURNING OFF IN", regions.heading, AMBER);
    let mut count = super::workout::Text::<4>::new();
    let _ = write!(count, "{}", seconds.min(99));
    centred(
        canvas,
        &[hero, large],
        count.as_bytes(),
        regions.count,
        WHITE,
    );
    centred(canvas, &[caption], b"SECONDS", regions.seconds, SECONDARY);
    let press = regions.press;
    centred(
        canvas,
        &[body, caption],
        b"PRESS ANY BUTTON",
        press,
        SECONDARY,
    );
    centred(
        canvas,
        &[body, caption],
        b"TO STAY ON",
        regions.stay,
        SECONDARY,
    );
}

/// The charge as a green ring with its percentage inside. While charging,
/// a lighter arc climbs from the charge to full, as the status bar's icon
/// fills, so the screen shows it is alive.
fn charging(canvas: &mut Canvas, layout: &Layout, battery: Battery, known: bool) {
    let (at, regions) = (&layout.scaled, &layout.power.charge);
    let arc = |start, sweep| ring(at, regions.ring, 14, start, sweep);
    canvas.shape(Shape::ring(arc(0, TURN), mix(GREEN, BLACK, 4)));
    let level = if known {
        u32::from(battery.percent.min(100))
    } else {
        0
    };
    if known && battery.charging && level < 100 {
        let climbing = share(u32::from(battery.fill()));
        canvas.shape(Shape::ring(arc(0, climbing), mix(GREEN, BLACK, 8)));
    }
    canvas.shape(Shape::ring(arc(0, share(level)), GREEN));
    let mut percent = super::workout::Text::<6>::new();
    if known {
        let _ = write!(percent, "{level}%");
    } else {
        let _ = percent.push_str("--");
    }
    let [hero, large, body] =
        [FaceRole::Hero, FaceRole::Large, FaceRole::Body].map(|role| at.face(role));
    centred(
        canvas,
        &[hero, large, body],
        percent.as_bytes(),
        regions.percent,
        WHITE,
    );
    let (state, color): (&[u8], u16) = match (battery.charging, level) {
        (_, 100) => (b"CHARGED", GREEN),
        (true, _) => (b"CHARGING", GREEN),
        (false, _) => (b"ON USB POWER", SECONDARY),
    };
    centred(canvas, &[body], state, regions.state, color);
}

fn centred(canvas: &mut Canvas, faces: &[&'static Face], text: &[u8], row: Rect, color: u16) {
    let (width, x) = (row.width(), row.center_x());
    canvas.fit(faces, text, width, x, row.bottom, Align::Center, color);
}

#[cfg(any(test, feature = "invariants"))]
impl Sheet {
    /// Fail a sheet whose runs or shapes leave the panel, or whose runs
    /// overlap.
    pub fn check_layout(canvas: &Canvas, layout: &Layout) {
        if !super::check::enabled() {
            return;
        }
        canvas.assert_kept(format_args!("a power sheet on {:?}", layout.scaled));
        let panel = layout.scaled.screen();
        let breaks = super::check::canvas_breaks(canvas, panel, &[("screen", panel)], 0, &[]);
        assert!(
            breaks.is_empty(),
            "a power sheet on {:?} breaks its layout:\n{}",
            layout.scaled,
            breaks.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{Look, Shown};
    use crate::screens::{BASE, panels};

    fn assert_paints_reference(sheet: Sheet, layout: &Layout) {
        let canvas = sheet.canvas(layout);
        let panel = layout.scaled.panel;
        for look in [false, true].map(crate::screens::look::look) {
            for y in 0..panel.height {
                let mut pixels = std::vec![0x5555; panel.width];
                Sheet::paint(&canvas, &mut Row::new(y, &mut pixels).looked(look));
                for (x, &pixel) in pixels.iter().enumerate() {
                    let want = canvas.pixel(look, x, y, look.ink(BLACK));
                    assert_eq!(Shown(pixel), want, "{sheet:?} at {x},{y}");
                }
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

    fn count(sheet: Sheet, color: u16) -> usize {
        let canvas = sheet.canvas(&BASE);
        (0..320)
            .flat_map(|y| (0..240).map(move |x| (x, y)))
            .filter(|&(x, y)| canvas.pixel(Look::PLAIN, x, y, Shown(BLACK)).0 == color)
            .count()
    }

    #[test]
    fn sheets_lay_out_whole_and_paint_their_reference() {
        let sheets = [
            (
                Sheet::Confirm { unsaved: 0 },
                &["TURN OFF", "RIDES ARE SAVED", "BACK"][..],
            ),
            (
                Sheet::Confirm { unsaved: 1 },
                &["TURN OFF", "SYNC RIDE FIRST", "BACK"][..],
            ),
            (
                Sheet::Confirm { unsaved: 3 },
                &["TURN OFF", "SYNC RIDES FIRST", "BACK"][..],
            ),
            (
                Sheet::Warning { seconds: 9 },
                &[
                    "TURNING OFF IN",
                    "9",
                    "SECONDS",
                    "PRESS ANY BUTTON",
                    "TO STAY ON",
                ],
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
        super::super::check::enable(true);
        for layout in panels::ALL {
            for (sheet, want) in sheets {
                let canvas = sheet.canvas(layout);
                let what = std::format!("{sheet:?} on {:?}", layout.scaled.panel);
                canvas.assert_laid_out(layout.scaled.screen(), want, &what);
                Sheet::check_layout(&canvas, layout);
                assert_paints_reference(sheet, layout);
            }
        }
    }

    /// Shutdown follows the last farewell frame, so it must be black.
    #[test]
    fn the_last_farewell_frame_is_black() {
        for layout in panels::ALL {
            let canvas = Sheet::TurningOff {
                progress: brand::DONE,
            }
            .canvas(layout);
            let panel = layout.scaled.panel;
            let dark = (0..panel.height).all(|y| {
                (0..panel.width).all(|x| canvas.pixel(Look::PLAIN, x, y, Shown(BLACK)).0 == BLACK)
            });
            assert!(dark, "{panel:?}");
        }
    }

    #[test]
    fn confirm_marks_act_and_draws_the_power_symbol() {
        for layout in panels::ALL {
            let confirm = Sheet::Confirm { unsaved: 0 }.canvas(layout);
            let panel = layout.scaled.panel;
            let (right, bottom) = (panel.width - 1, panel.height - 1);
            assert_eq!(
                confirm.pixel(Look::PLAIN, right, bottom, Shown(BLACK)).0,
                ACCENT,
                "ACT corner marker on {panel:?}"
            );
            assert_eq!(confirm.find("TURN OFF").map(|run| run.color), Some(WHITE));
        }
        let confirm = Sheet::Confirm { unsaved: 0 }.canvas(&BASE);
        // The ring's left side and the bar through its gap are white.
        assert_eq!(confirm.pixel(Look::PLAIN, 88, 118, Shown(BLACK)).0, WHITE);
        assert_eq!(confirm.pixel(Look::PLAIN, 120, 80, Shown(BLACK)).0, WHITE);
        assert_eq!(
            confirm.pixel(Look::PLAIN, 132, 87, Shown(BLACK)).0,
            BLACK,
            "the gap"
        );
    }

    #[test]
    fn the_warning_ring_empties_with_the_seconds() {
        let amber = |seconds| count(Sheet::Warning { seconds }, AMBER);
        let (full, half) = (amber(30), amber(15));
        assert!(
            half * 2 > full * 8 / 10 && half * 2 < full * 12 / 10,
            "{half} of {full}"
        );
    }

    #[test]
    fn charging_climbs_from_the_charge_to_full_and_rests_when_charged() {
        let climb = mix(GREEN, BLACK, 8);
        assert!(
            count(charge(40, true, 0), climb) < 100,
            "the climb starts at the charge"
        );
        assert!(count(charge(40, true, 3), climb) > 1000, "and reaches full");
        assert!(charge(40, true, 1).animated());
        assert!(!charge(100, true, 1).animated());
        assert!(!charge(40, false, 1).animated());
        // The same arc on the right of the ring, 25% charge, is lit green.
        let canvas = charge(25, false, 0).canvas(&BASE);
        assert_eq!(canvas.pixel(Look::PLAIN, 204, 146, Shown(BLACK)).0, GREEN);
        assert_ne!(canvas.pixel(Look::PLAIN, 36, 146, Shown(BLACK)).0, GREEN);
    }
}
