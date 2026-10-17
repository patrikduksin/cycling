//! The VANA mark and its two moments: the reveal when VANA starts and the
//! farewell before the device turns off. Both are the wordmark inside one
//! `ACCENT` ring; the reveal draws the ring closed around it, the farewell
//! unwinds it while the wordmark fades to black.
//!
//! Progress is in thousandths. Each frame is a canvas of one ring and two
//! runs, so a frame costs about what a static sheet does.
use super::canvas::{Canvas, Shape};
use super::chrome::{ACCENT, Align, BLACK, CAPTION, Ring, TITLE, TURN, WHITE, WIDTH, blend};

/// Where the mark sits: a little above the middle of the screen.
pub const CENTRE: (usize, usize) = (WIDTH / 2, 150);
const RADIUS: i32 = 74;
const RING_WIDTH: i32 = 6;
/// Whole progress.
pub const DONE: u16 = 1000;

fn ring(start: u16, sweep: u16) -> Ring {
    Ring {
        cx: 4 * CENTRE.0 as i32,
        cy: 4 * CENTRE.1 as i32,
        radius: 4 * RADIUS,
        width: 4 * RING_WIDTH,
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
fn fade(color: u16, progress: u16) -> u16 {
    let progress = if progress > DONE { DONE } else { progress };
    blend(
        color,
        BLACK,
        (15 * (DONE - progress) as u32 / DONE as u32) as u8,
    )
}

fn wordmark(canvas: &mut Canvas, vana: u16, labs: u16) {
    let (x, y) = CENTRE;
    let width = 2 * RADIUS as usize - 24;
    canvas.fit(&[&TITLE], b"VANA", width, x, y + 13, Align::Center, vana);
    canvas.fit(&[&CAPTION], b"LABS", width, x, y + 36, Align::Center, labs);
}

/// The start: VANA shows at once, so the first lit frame is never empty;
/// the ring then closes clockwise from 12 o'clock and LABS fades in.
pub fn reveal(canvas: &mut Canvas, progress: u16) {
    canvas.shape(Shape::ring(ring(0, turns(progress)), ACCENT));
    let labs = fade(ACCENT, DONE - progress.min(DONE));
    wordmark(canvas, WHITE, labs);
}

/// The goodbye: the ring's tail chases its head round until nothing is
/// left, and the wordmark fades with it. At `DONE` the frame is black.
pub fn farewell(canvas: &mut Canvas, progress: u16) {
    let gone = turns(progress);
    canvas.shape(Shape::ring(ring(gone, TURN - gone), ACCENT));
    wordmark(canvas, fade(WHITE, progress), fade(ACCENT, progress));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{HEIGHT, Rect, Row};

    const SCREEN: Rect = Rect {
        left: 0,
        top: 0,
        right: WIDTH,
        bottom: HEIGHT,
    };

    fn lit(canvas: &Canvas, color: u16) -> usize {
        (0..HEIGHT)
            .map(|y| {
                let mut pixels = [BLACK; WIDTH];
                canvas.paint(&mut Row::new(y, &mut pixels));
                for (x, &pixel) in pixels.iter().enumerate() {
                    assert_eq!(pixel, canvas.pixel(x, y, BLACK), "paint at {x},{y}");
                }
                pixels.iter().filter(|&&p| p == color).count()
            })
            .sum()
    }

    #[test]
    fn the_reveal_closes_the_ring_round_the_wordmark() {
        let mut first = Canvas::default();
        reveal(&mut first, 0);
        assert_eq!(lit(&first, ACCENT), 0, "no ring yet");
        assert!(lit(&first, WHITE) > 100, "VANA shows on the first frame");
        let mut half = Canvas::default();
        reveal(&mut half, DONE / 2);
        let mut whole = Canvas::default();
        reveal(&mut whole, DONE);
        let (half, whole) = (lit(&half, ACCENT), lit(&whole, ACCENT));
        assert!(half * 2 > whole * 9 / 10 && half * 2 < whole * 11 / 10);
        let mut done = Canvas::default();
        reveal(&mut done, DONE);
        done.assert_laid_out(SCREEN, &["VANA", "LABS"], "reveal");
    }

    #[test]
    fn the_farewell_unwinds_to_black() {
        let mut start = Canvas::default();
        farewell(&mut start, 0);
        let mut end = Canvas::default();
        farewell(&mut end, DONE);
        assert!(lit(&start, ACCENT) > 1000);
        assert!(lit(&start, WHITE) > 100);
        let dark = (0..HEIGHT).all(|y| (0..WIDTH).all(|x| end.pixel(x, y, BLACK) == BLACK));
        assert!(dark, "the last frame is black");
    }
}
