//! The power sheets' rings: the power symbol over TURN OFF, the idle
//! warning's countdown, the charge, and the farewell. Each is built round one
//! ring in the square the tree gives it, and draws what sits inside it.
use crate::compose::data::{Count, Data};
use crate::compose::solve::Placed;
use crate::compose::tile::{Arg, Form, Forms, Tile, forms};
use crate::screens::brand::{DONE, fade};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    ACCENT, AMBER, Align, BLACK, GREEN, Rect, Ring as Arc, SECONDARY, TURN, WHITE, mix,
};
use crate::screens::scaled::Scaled;
use crate::screens::workout::Text;
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole::{self, Body, Caption, Hero, Large, Title};

/// The idle warning's ring is full at this many seconds.
pub const WARNING_SECONDS: i32 = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Ring {
    /// A ring open at the top with a bar through the gap.
    Symbol,
    /// The seconds left inside an amber ring that empties with them.
    Countdown,
    /// The charge as a green ring with its percentage inside; while
    /// charging a lighter arc climbs from the charge to full.
    Charge,
    /// The ring's tail chases its head round until nothing is left, and the
    /// wordmark and TURNING OFF under it fade with it.
    Farewell,
}

impl Ring {
    /// The side of the square the ring's centre line runs round, and the
    /// rows under it the ring's tile keeps.
    const fn side_units(self) -> usize {
        match self {
            Self::Symbol => 64,
            Self::Countdown => 140,
            Self::Charge => 168,
            Self::Farewell => 148,
        }
    }

    const fn width_units(self) -> usize {
        match self {
            Self::Symbol => 7,
            Self::Countdown => 8,
            Self::Charge => 14,
            Self::Farewell => 6,
        }
    }

    /// The power symbol's bar reaches above its ring.
    const fn above_units(self) -> usize {
        match self {
            Self::Symbol => 10,
            _ => 0,
        }
    }

    /// Columns the ring's stroke and its smoothed edge reach past the
    /// square on each side.
    fn stroke(self, at: &Scaled) -> usize {
        at.unit(self.width_units()).div_ceil(2) + 1
    }

    /// Rows above the square: the bar or the stroke, whichever reaches
    /// further.
    fn above(self, at: &Scaled) -> usize {
        at.unit(self.above_units()).max(self.stroke(at))
    }

    /// TURNING OFF's row under the farewell's ring.
    const fn below_units(self) -> usize {
        match self {
            Self::Farewell => 44,
            _ => 0,
        }
    }
}

impl Arg for Ring {
    fn decode(raw: u8) -> Option<Self> {
        Some(match raw {
            0 => Self::Symbol,
            1 => Self::Countdown,
            2 => Self::Charge,
            3 => Self::Farewell,
            _ => return None,
        })
    }

    fn encode(self) -> u8 {
        self as u8
    }
}

pub struct PowerRing;

/// The arc of `sweep` from `start` round the square `rect`, `width` units
/// wide on the square's edge.
const fn arc(at: &Scaled, rect: Rect, width: usize, start: u16, sweep: u16) -> Arc {
    Arc {
        cx: 2 * (rect.left + rect.right) as i32,
        cy: 2 * (rect.top + rect.bottom) as i32,
        radius: 2 * rect.width() as i32,
        width: at.unit(4 * width) as i32,
        start,
        sweep,
    }
}

/// A share of a turn: `part` of `whole`, at most all of it.
fn share(part: i32, whole: i32) -> u16 {
    (part.clamp(0, whole) as u32 * u32::from(TURN) / whole as u32) as u16
}

fn centred(
    out: &mut Canvas,
    faces: &[&'static firmware_shell::rendering::font::Face],
    text: &[u8],
    row: Rect,
    color: u16,
) {
    out.fit(
        faces,
        text,
        row.width(),
        row.center_x(),
        row.bottom,
        Align::Center,
        color,
    );
}

/// Rows `[top, top + height)` of `rect`, inset `side` units from each side.
fn band(at: &Scaled, rect: Rect, top: usize, height: usize, side: usize) -> Rect {
    Rect {
        left: rect.left + at.unit(side),
        right: rect.right - at.unit(side),
        top: rect.top + at.unit(top),
        bottom: rect.top + at.unit(top + height),
    }
}

impl Tile for PowerRing {
    type Arg = Ring;
    const NAME: &'static str = "power ring";

    fn forms(ring: Ring, at: &Scaled) -> Forms {
        let (side, stroke) = (at.unit(ring.side_units()), ring.stroke(at));
        let (above, below) = (ring.above(at), at.unit(ring.below_units()).max(stroke));
        forms(&[Form {
            width: (side + 2 * stroke) as u16,
            height: (above + side + below) as u16,
            shapes: 3,
            runs: 3,
        }])
    }

    fn prepare(ring: Ring, placed: &Placed, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
        let rect = placed.rect.rect();
        let side = at.unit(ring.side_units());
        let left = rect.left + rect.width().saturating_sub(side) / 2;
        let top = rect.top + ring.above(at);
        let square = Rect {
            left,
            right: left + side,
            top,
            bottom: top + side,
        };
        let width = ring.width_units();
        let face = |role: FaceRole| at.face(role);
        match ring {
            Ring::Symbol => {
                let gap = TURN / 12;
                out.shape(Shape::ring(
                    arc(at, square, width, gap, TURN - 2 * gap),
                    WHITE,
                ));
                let stem = Rect {
                    left: square.left + at.unit(29),
                    right: square.left + at.unit(36),
                    top: rect.top,
                    bottom: square.top + at.unit(32),
                };
                out.shape(Shape::Bar {
                    rect: stem,
                    radius: at.unit(3),
                    color: WHITE,
                });
            }
            Ring::Countdown => {
                let seconds = data.count(Count::Seconds).unwrap_or(0).max(0);
                let track = mix(AMBER, BLACK, 4);
                out.shape(Shape::ring(arc(at, square, width, 0, TURN), track));
                let sweep = share(seconds, WARNING_SECONDS);
                out.shape(Shape::ring(
                    arc(at, square, width, TURN - sweep, sweep),
                    AMBER,
                ));
                let mut count = Text::<4>::new();
                let _ = write!(count, "{}", seconds.min(99));
                let number = band(at, square, 0, 92, 15);
                centred(
                    out,
                    &[face(Hero), face(Large)],
                    count.as_bytes(),
                    number,
                    WHITE,
                );
                let under = band(at, square, 92, 22, 0);
                centred(out, &[face(Caption)], b"SECONDS", under, SECONDARY);
            }
            Ring::Charge => {
                let level = data.count(Count::Charge);
                let charging = data.count(Count::Charging) == Some(1);
                out.shape(Shape::ring(
                    arc(at, square, width, 0, TURN),
                    mix(GREEN, BLACK, 4),
                ));
                let percent = level.unwrap_or(0).clamp(0, 100);
                if level.is_some() && charging && percent < 100 {
                    let fill = data.count(Count::ChargeFill).unwrap_or(percent);
                    let climb = mix(GREEN, BLACK, 8);
                    out.shape(Shape::ring(
                        arc(at, square, width, 0, share(fill, 100)),
                        climb,
                    ));
                }
                out.shape(Shape::ring(
                    arc(at, square, width, 0, share(percent, 100)),
                    GREEN,
                ));
                let mut text = Text::<6>::new();
                match level {
                    Some(_) => {
                        let _ = write!(text, "{percent}%");
                    }
                    None => {
                        let _ = text.push_str("--");
                    }
                }
                let inside = band(at, square, 0, 104, 13);
                let faces = [face(Hero), face(Large), face(Body)];
                centred(out, &faces, text.as_bytes(), inside, WHITE);
            }
            Ring::Farewell => {
                let progress = data
                    .count(Count::Farewell)
                    .unwrap_or(0)
                    .clamp(0, i32::from(DONE));
                let gone = share(progress, i32::from(DONE));
                out.shape(Shape::ring(
                    arc(at, square, width, gone, TURN - gone),
                    ACCENT,
                ));
                let progress = progress as u16;
                let vana = band(at, square, 0, 87, 12);
                centred(out, &[face(Title)], b"VANA", vana, fade(WHITE, progress));
                let labs = band(at, square, 87, 23, 12);
                centred(out, &[face(Caption)], b"LABS", labs, fade(ACCENT, progress));
                let below = Rect {
                    top: square.bottom,
                    bottom: rect.bottom,
                    ..rect
                };
                let quiet = fade(SECONDARY, progress);
                centred(out, &[face(Body)], b"TURNING OFF", below, quiet);
            }
        }
    }
}
