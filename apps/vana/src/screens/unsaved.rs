//! LOSE n UNSAVED RIDES?: a host CLEAR would discard rides the phone has not
//! confirmed. ACT (red trash cue) discards them with the journal; BACK keeps
//! them, as on DISCARD RIDE?.
use super::canvas::Canvas;
use super::chrome::{Align, BODY, CAPTION, SECONDARY, TITLE, WHITE, WIDTH};
use super::workout::{Text, View};
use core::fmt::Write;

const CENTRE: usize = WIDTH / 2;
const TEXT_WIDTH: usize = WIDTH - 2 * 12;

pub fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    let one = view.unsaved == 1;
    let mut count = Text::<16>::new();
    let _ = write!(count, "LOSE {} UNSAVED", view.unsaved);
    let rides: &[u8] = if one { b"RIDE?" } else { b"RIDES?" };
    for (line, base) in [(count.as_bytes(), 136), (rides, 162)] {
        canvas.fit(
            &[&TITLE, &BODY],
            line,
            TEXT_WIDTH,
            CENTRE,
            base,
            Align::Center,
            WHITE,
        );
    }
    let line: &[u8] = if one {
        b"IT CANNOT COME BACK"
    } else {
        b"THEY CANNOT COME BACK"
    };
    canvas.fit(
        &[&CAPTION],
        line,
        TEXT_WIDTH,
        CENTRE,
        190,
        Align::Center,
        SECONDARY,
    );
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    #[test]
    fn the_question_counts_the_rides_it_would_lose() {
        for (unsaved, question, line) in [
            (1, ["LOSE 1 UNSAVED", "RIDE?"], "IT CANNOT COME BACK"),
            (2, ["LOSE 2 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
            (16, ["LOSE 16 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
        ] {
            let mut view = View::new(Page::LoseRides, 0, None);
            view.unsaved = unsaved;
            let canvas = prepare(&view);
            let want = [question[0], question[1], line];
            canvas.assert_laid_out(crate::screens::chrome::CONTENT, &want, question[0]);
        }
    }
}
