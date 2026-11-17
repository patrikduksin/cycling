//! End ride: the confirmation BACK opens from the paused screen.
//!
//! The question, then the ride it is about. ACT saves and ends it (green
//! check beside ACT); BACK keeps riding paused (back cue beside BACK). Like
//! turning off from Home, BACK opens the question and ACT answers it. With a
//! FILES volume to note it on, MOVE (red trash cue) asks DISCARD RIDE? in the
//! same layout: ACT (red trash cue beside ACT) discards the ride, BACK
//! returns to END RIDE?.
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, Rect, SECONDARY, WHITE};
use crate::screens::cue::{Corner, Cue, Icon};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::FaceRole;

const BACK: Cue = Cue::new(Corner::TopLeft, Icon::Back);

pub const CUES: [Cue; 2] = [BACK, Cue::new(Corner::BottomRight, Icon::Check)];
/// END RIDE? when the ride can be discarded.
pub const DISCARD_CUES: [Cue; 3] = [
    BACK,
    Cue::new(Corner::BottomRight, Icon::Check),
    Cue::new(Corner::BottomLeft, Icon::Trash),
];
/// A question whose ACT throws a ride away: DISCARD RIDE?, DELETE RIDE?.
pub const TRASH_CUES: [Cue; 2] = [BACK, Cue::new(Corner::BottomRight, Icon::Trash)];

/// A question about a ride on one panel, centred down it: rows of text whose
/// capitals end on their bottoms, between the cues.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub title: Rect,
    pub time: Rect,
    pub distance: Rect,
    /// What the answer does, wider than the rows above it.
    pub line: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, text, _] = screen.cols(crate::screens::cue::CLEAR_OF_CUES, Unit(0), at);
    let [_, wide, _] = screen.cols([Unit(10), Fr(1), Unit(10)], Unit(0), at);
    let [_, title, time, distance, line, _, _] = text.rows(
        [
            Fr(1),
            Unit(112),
            Unit(66),
            Unit(28),
            Unit(38),
            Unit(76),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    Regions {
        title,
        time,
        distance,
        line: line.with_columns(wide),
    }
}

pub fn prepare(view: &View) -> Canvas {
    question(view, b"END RIDE?", b"YOUR RIDE WILL BE SAVED")
}

pub fn discard(view: &View) -> Canvas {
    question(view, b"DISCARD RIDE?", b"IT WILL NOT BE SAVED")
}

/// A question about a ride: `title`, the ride time, the distance and a grey
/// `line` saying what the answer does.
pub fn question(view: &View, title: &[u8], line: &[u8]) -> Canvas {
    let layout = view.frame_layout();
    let (at, regions) = (&layout.scaled, &layout.end);
    let face = |role| at.face(role);
    let mut canvas = Canvas::default();
    let mut centred = |faces: &[_], text: &[u8], row: Rect, color| {
        canvas.fit(
            faces,
            text,
            row.width(),
            row.center_x(),
            row.bottom,
            Align::Center,
            color,
        );
    };
    let title_faces = [face(FaceRole::Title), face(FaceRole::Body)];
    centred(&title_faces, title, regions.title, WHITE);
    let mut time = Text::<12>::new();
    super::elapsed(&mut time, view.active_secs);
    let large = [face(FaceRole::Large), face(FaceRole::Value)];
    centred(&large, time.as_bytes(), regions.time, WHITE);
    let mut distance = Text::<12>::new();
    super::kilometres(&mut distance, view.totals.distance_mm());
    let _ = core::fmt::Write::write_str(&mut distance, " KM");
    centred(
        &[face(FaceRole::Body)],
        distance.as_bytes(),
        regions.distance,
        WHITE,
    );
    centred(&[face(FaceRole::Caption)], line, regions.line, SECONDARY);
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    #[test]
    fn the_question_and_the_longest_ride_lay_out_whole() {
        for layout in crate::screens::panels::ALL {
            for (page, words) in [
                (
                    Page::EndRide,
                    [
                        "END RIDE?",
                        "99:59:59",
                        "1000.0 KM",
                        "YOUR RIDE WILL BE SAVED",
                    ],
                ),
                (
                    Page::DiscardRide,
                    [
                        "DISCARD RIDE?",
                        "99:59:59",
                        "1000.0 KM",
                        "IT WILL NOT BE SAVED",
                    ],
                ),
            ] {
                let mut view = View::new(page, 0, None);
                view.layout = layout;
                view.active_secs = 359_999;
                let sample = crate::ride::log::Sample::default();
                view.totals.add(1000, 999_999_999, &sample, None);
                view.prepare().check_layout();
                let at = &layout.scaled;
                let screen = at.screen();
                let clear = Rect {
                    top: at.unit(50),
                    bottom: screen.bottom - at.unit(50),
                    ..screen
                };
                let what = std::format!("{} on {:?}", words[0], at.panel);
                view.canvas().assert_laid_out(clear, &words, &what);
            }
        }
    }
}
