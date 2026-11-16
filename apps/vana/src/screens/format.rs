//! FORMAT STORAGE?: what a boot asks when stored data is of another version.
//!
//! The question in the layout of END RIDE?, white on black. ACT (trash cue)
//! formats, BACK keeps everything as it is, MOVE does nothing. While the
//! storage is formatted the same page says so, with no cues.
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, BODY, CAPTION, SECONDARY, TITLE, WHITE, WIDTH};

const CENTRE: usize = WIDTH / 2;
const TEXT_WIDTH: usize = WIDTH - 2 * 24;
pub const TITLE_TEXT: &str = "FORMAT STORAGE?";
pub const LINES: [&str; 2] = ["MADE BY ANOTHER VERSION", "RIDES AND SETTINGS GO"];
pub const WAIT: &str = "CHECKING STORAGE";

pub fn prepare(formatting: bool) -> Canvas {
    let mut canvas = Canvas::default();
    let mut line = |faces: &[_], text: &str, width, y, color| {
        canvas.fit(
            faces,
            text.as_bytes(),
            width,
            CENTRE,
            y,
            Align::Center,
            color,
        );
    };
    if formatting {
        line(&[&BODY, &CAPTION], WAIT, TEXT_WIDTH, 178, WHITE);
        return canvas;
    }
    line(&[&TITLE, &BODY], TITLE_TEXT, TEXT_WIDTH, 112, WHITE);
    line(&[&BODY, &CAPTION], LINES[0], WIDTH - 20, 178, WHITE);
    line(&[&CAPTION], LINES[1], WIDTH - 20, 244, SECONDARY);
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{HEIGHT, Rect};

    #[test]
    fn the_question_and_the_wait_lay_out_whole() {
        let screen = Rect {
            left: 0,
            top: 50,
            right: WIDTH,
            bottom: HEIGHT - 50,
        };
        prepare(false).assert_laid_out(screen, &[TITLE_TEXT, LINES[0], LINES[1]], "format");
        prepare(true).assert_laid_out(screen, &[WAIT], "formatting");
    }
}
