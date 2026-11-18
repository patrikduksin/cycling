//! The VANA mark and its two moments: the reveal when VANA starts and the
//! farewell before the device turns off. Both are the wordmark inside one
//! `ACCENT` ring; the reveal draws the ring closed around it, the farewell
//! unwinds it while the wordmark fades to black.
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
const RING_WIDTH: usize = 6;

/// The mark on one panel, a little above the middle.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The square the ring's centre line runs round.
    pub mark: Rect,
    /// VANA and LABS, their capitals ending on the rows' bottoms.
    pub vana: Rect,
    pub labs: Rect,
    /// The row under the ring, a line's capitals ending on its bottom.
    pub below: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, mark, below, _] = screen.rows([Fr(76), Unit(148), Unit(44), Fr(52)], Unit(0), at);
    let [_, square, _] = screen.cols([Fr(1), Unit(148), Fr(1)], Unit(0), at);
    let mark = mark.with_columns(square);
    let [_, words, _] = mark.cols([Unit(12), Fr(1), Unit(12)], Unit(0), at);
    let [vana, labs, _] = mark.rows([Unit(87), Unit(23), Fr(1)], Unit(0), at);
    Regions {
        mark,
        vana: vana.with_columns(words),
        labs: labs.with_columns(words),
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

fn wordmark(canvas: &mut Canvas, layout: &Layout, vana: u16, labs: u16) {
    let (at, regions) = (&layout.scaled, &layout.brand);
    let words: [(_, &[u8], _, _); 2] = [
        (FaceRole::Title, b"VANA", regions.vana, vana),
        (FaceRole::Caption, b"LABS", regions.labs, labs),
    ];
    for (role, text, row, color) in words {
        let faces = [at.face(role)];
        let (width, x) = (row.width(), row.center_x());
        canvas.fit(&faces, text, width, x, row.bottom, Align::Center, color);
    }
}

/// The start: VANA shows at once, so the first lit frame is never empty;
/// the ring then closes clockwise from 12 o'clock and LABS fades in.
pub fn reveal(canvas: &mut Canvas, layout: &Layout, progress: u16) {
    let ring = ring(
        &layout.scaled,
        layout.brand.mark,
        RING_WIDTH,
        0,
        turns(progress),
    );
    canvas.shape(Shape::ring(ring, ACCENT));
    let labs = fade(ACCENT, DONE - progress.min(DONE));
    wordmark(canvas, layout, WHITE, labs);
}

/// The goodbye: the ring's tail chases its head round until nothing is
/// left, and the wordmark fades with it. At `DONE` the frame is black.
pub fn farewell(canvas: &mut Canvas, layout: &Layout, progress: u16) {
    let gone = turns(progress);
    let ring = ring(
        &layout.scaled,
        layout.brand.mark,
        RING_WIDTH,
        gone,
        TURN - gone,
    );
    canvas.shape(Shape::ring(ring, ACCENT));
    wordmark(
        canvas,
        layout,
        fade(WHITE, progress),
        fade(ACCENT, progress),
    );
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
    fn the_reveal_closes_the_ring_round_the_wordmark() {
        for layout in panels::ALL {
            let mut first = Canvas::default();
            reveal(&mut first, layout, 0);
            assert_eq!(lit(layout, &first, ACCENT), 0, "no ring yet");
            assert!(
                lit(layout, &first, WHITE) > 50,
                "VANA shows on the first frame"
            );
            let mut half = Canvas::default();
            reveal(&mut half, layout, DONE / 2);
            let mut whole = Canvas::default();
            reveal(&mut whole, layout, DONE);
            let (halfway, closed) = (lit(layout, &half, ACCENT), lit(layout, &whole, ACCENT));
            assert!(halfway * 2 > closed * 9 / 10 && halfway * 2 < closed * 11 / 10);
            let what = std::format!("reveal on {:?}", layout.scaled.panel);
            whole.assert_laid_out(layout.scaled.screen(), &["VANA", "LABS"], &what);
        }
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
