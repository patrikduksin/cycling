//! The VANA mark and its two moments: the reveal when VANA starts and the
//! farewell before the device turns off. Both are the wordmark inside one
//! `ACCENT` ring; the reveal (the Brand screen's tree, `tiles::mark`) draws
//! the ring closed around it, the farewell unwinds it while the wordmark
//! fades to black.
//!
//! Progress is in thousandths. Each frame is a canvas of one ring and two
//! runs, so a frame costs about what a static sheet does.
use super::Layout;
use super::canvas::{Canvas, Shape};
use super::chrome::{ACCENT, Align, BLACK, Rect, Ring, TURN, WHITE, mix};
use super::scaled::{Scaled, Track};
use firmware_shell::rendering::font::FaceRole;

/// Whole progress.
pub const DONE: u16 = 1000;
/// The square the ring's centre line runs round, and the ring's width.
pub const SQUARE_UNITS: usize = 148;
pub const RING_UNITS: usize = 6;
/// VANA's and LABS's baselines below the square's top, and the columns the
/// words keep from its sides.
const VANA_UNITS: usize = 87;
const LABS_UNITS: usize = 110;
const WORDS_INSET_UNITS: usize = 12;

/// Where the power sheet's farewell draws the mark, a little above the
/// middle, and the row under it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub mark: Rect,
    pub below: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, mark, below, _] = screen.rows(
        [Fr(76), Unit(SQUARE_UNITS as u16), Unit(44), Fr(52)],
        Unit(0),
        at,
    );
    let [_, square, _] = screen.cols([Fr(1), Unit(SQUARE_UNITS as u16), Fr(1)], Unit(0), at);
    Regions {
        mark: mark.with_columns(square),
        below,
    }
}

pub(super) const fn ring(at: &Scaled, rect: Rect, width: usize, start: u16, sweep: u16) -> Ring {
    Ring {
        cx: 2 * (rect.left + rect.right) as i32,
        cy: 2 * (rect.top + rect.bottom) as i32,
        radius: 2 * rect.width() as i32,
        width: at.unit(4 * width) as i32,
        start,
        sweep,
    }
}

/// `progress` (thousandths) of a turn.
const fn turns(progress: u16) -> u16 {
    let progress = if progress > DONE { DONE } else { progress };
    (progress as u32 * TURN as u32 / DONE as u32) as u16
}

/// `color` faded to black by `progress`: whole at 0, gone at `DONE`.
pub fn fade(color: u16, progress: u16) -> u16 {
    let progress = if progress > DONE { DONE } else { progress };
    mix(
        color,
        BLACK,
        (15 * (DONE - progress) as u32 / DONE as u32) as u8,
    )
}

/// The ring from `start` round `sweep`, both in turns, and the wordmark in
/// `square`.
fn mark(
    canvas: &mut Canvas,
    at: &Scaled,
    square: Rect,
    (start, sweep): (u16, u16),
    [vana, labs]: [u16; 2],
) {
    canvas.shape(Shape::ring(
        ring(at, square, RING_UNITS, start, sweep),
        ACCENT,
    ));
    let width = square.width() - 2 * at.unit(WORDS_INSET_UNITS);
    let words: [(_, &[u8], _, _); 2] = [
        (FaceRole::Title, b"VANA", VANA_UNITS, vana),
        (FaceRole::Caption, b"LABS", LABS_UNITS, labs),
    ];
    for (role, text, base, color) in words {
        let base = square.top + at.unit(base);
        let faces = [at.face(role)];
        canvas.fit(
            &faces,
            text,
            width,
            square.center_x(),
            base,
            Align::Center,
            color,
        );
    }
}

/// The start: VANA shows at once, so the first lit frame is never empty;
/// the ring then closes clockwise from 12 o'clock and LABS fades in.
pub fn reveal(canvas: &mut Canvas, at: &Scaled, square: Rect, progress: u16) {
    let labs = fade(ACCENT, DONE - progress.min(DONE));
    mark(canvas, at, square, (0, turns(progress)), [WHITE, labs]);
}

/// The goodbye: the ring's tail chases its head round until nothing is
/// left, and the wordmark fades with it. At `DONE` the frame is black.
pub fn farewell(canvas: &mut Canvas, layout: &Layout, progress: u16) {
    let at = &layout.scaled;
    let gone = turns(progress);
    let colors = [fade(WHITE, progress), fade(ACCENT, progress)];
    mark(canvas, at, regions(at).mark, (gone, TURN - gone), colors);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{Look, Row, Shown};
    use crate::screens::panels;

    fn lit(layout: &Layout, canvas: &Canvas, color: u16) -> usize {
        let panel = layout.scaled.panel;
        (0..panel.height)
            .map(|y| {
                let mut pixels = std::vec![BLACK; panel.width];
                canvas.paint(&mut Row::new(y, &mut pixels));
                for (x, &pixel) in pixels.iter().enumerate() {
                    assert_eq!(
                        pixel,
                        canvas.pixel(Look::PLAIN, x, y, Shown(BLACK)).0,
                        "paint at {x},{y}"
                    );
                }
                pixels.iter().filter(|&&p| p == color).count()
            })
            .sum()
    }

    #[test]
    fn the_farewell_unwinds_to_black() {
        for layout in panels::ALL {
            let mut start = Canvas::default();
            farewell(&mut start, layout, 0);
            let mut end = Canvas::default();
            farewell(&mut end, layout, DONE);
            assert!(lit(layout, &start, ACCENT) > 500);
            assert!(lit(layout, &start, WHITE) > 50);
            let panel = layout.scaled.panel;
            let dark = (0..panel.height).all(|y| {
                (0..panel.width).all(|x| end.pixel(Look::PLAIN, x, y, Shown(BLACK)).0 == BLACK)
            });
            assert!(dark, "the last frame is black on {panel:?}");
        }
    }
}
