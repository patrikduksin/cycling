//! LOSE n UNSAVED RIDES?: a host CLEAR would discard rides the phone has not
//! confirmed. ACT (red trash cue) discards them with the journal; BACK keeps
//! them, as on DISCARD RIDE?.
use super::canvas::Canvas;
use super::chrome::{Align, Rect, SECONDARY, WHITE};
use super::scaled::{Scaled, Track};
use super::workout::{Text, View};
use core::fmt::Write;
use firmware_shell::rendering::font::FaceRole;

/// The question on one panel, centred down it: rows of text whose capitals
/// end on their bottoms.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// LOSE n UNSAVED.
    pub count: Rect,
    /// RIDE? or RIDES?.
    pub rides: Rect,
    /// What the answer does.
    pub line: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, text, _] = screen.cols([Unit(12), Fr(1), Unit(12)], Unit(0), at);
    let [_, count, rides, line, _, _] = text.rows(
        [Fr(1), Unit(136), Unit(26), Unit(28), Unit(130), Fr(1)],
        Unit(0),
        at,
    );
    Regions { count, rides, line }
}

pub fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.unsaved);
    let mut canvas = Canvas::default();
    let one = view.unsaved == 1;
    let mut count = Text::<16>::new();
    let _ = write!(count, "LOSE {} UNSAVED", view.unsaved);
    let rides: &[u8] = if one { b"RIDE?" } else { b"RIDES?" };
    let question = [at.face(FaceRole::Title), at.face(FaceRole::Body)];
    for (line, row) in [(count.as_bytes(), regions.count), (rides, regions.rides)] {
        canvas.fit(
            &question,
            line,
            row.width(),
            row.center_x(),
            row.bottom,
            Align::Center,
            WHITE,
        );
    }
    let line: &[u8] = if one {
        b"IT CANNOT COME BACK"
    } else {
        b"THEY CANNOT COME BACK"
    };
    let row = regions.line;
    canvas.fit(
        &[at.face(FaceRole::Caption)],
        line,
        row.width(),
        row.center_x(),
        row.bottom,
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
    fn the_question_counts_the_rides_it_would_lose_on_every_panel() {
        for layout in crate::screens::panels::ALL {
            for (unsaved, question, line) in [
                (1, ["LOSE 1 UNSAVED", "RIDE?"], "IT CANNOT COME BACK"),
                (2, ["LOSE 2 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
                (16, ["LOSE 16 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
                (255, ["LOSE 255 UNSAVED", "RIDES?"], "THEY CANNOT COME BACK"),
            ] {
                let mut view = View::new(Page::LoseRides, 0, None);
                view.layout = layout;
                view.unsaved = unsaved;
                view.prepare().check_layout();
                let at = &layout.scaled;
                let screen = at.screen();
                let clear = Rect {
                    top: at.unit(50),
                    bottom: screen.bottom - at.unit(50),
                    ..screen
                };
                let want = [question[0], question[1], line];
                let what = std::format!("{} on {:?}", question[0], at.panel);
                view.canvas().assert_laid_out(clear, &want, &what);
            }
        }
    }
}
