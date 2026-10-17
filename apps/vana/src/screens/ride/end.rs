//! End ride: the confirmation BACK opens from the paused screen.
//!
//! The question, then the ride it is about. ACT saves and ends it (green
//! check beside ACT); BACK keeps riding paused (back cue beside BACK). Like
//! turning off from Home, BACK opens the question and ACT answers it.
use crate::screens::canvas::Canvas;
use crate::screens::chrome::{Align, BODY, CAPTION, LARGE, SECONDARY, TITLE, VALUE, WHITE, WIDTH};
use crate::screens::cue::{Corner, Cue, Icon};
use crate::screens::workout::{Text, View};

pub const CUES: [Cue; 2] = [
    Cue::new(Corner::TopLeft, Icon::Back),
    Cue::new(Corner::BottomRight, Icon::Check),
];

const CENTRE: usize = WIDTH / 2;
const TEXT_WIDTH: usize = WIDTH - 2 * 24;

pub fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    canvas.fit(
        &[&TITLE, &BODY],
        b"END RIDE?",
        TEXT_WIDTH,
        CENTRE,
        112,
        Align::Center,
        WHITE,
    );
    let mut time = Text::<12>::new();
    super::elapsed(&mut time, view.active_secs);
    canvas.fit(
        &[&LARGE, &VALUE],
        time.as_bytes(),
        TEXT_WIDTH,
        CENTRE,
        178,
        Align::Center,
        WHITE,
    );
    let mut distance = Text::<12>::new();
    super::kilometres(&mut distance, view.totals.distance_mm());
    let _ = core::fmt::Write::write_str(&mut distance, " KM");
    canvas.fit(
        &[&BODY],
        distance.as_bytes(),
        TEXT_WIDTH,
        CENTRE,
        206,
        Align::Center,
        WHITE,
    );
    canvas.fit(
        &[&CAPTION],
        b"YOUR RIDE WILL BE SAVED",
        WIDTH - 20,
        CENTRE,
        244,
        Align::Center,
        SECONDARY,
    );
    canvas
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::chrome::{HEIGHT, Rect};
    use crate::screens::workout::Page;

    #[test]
    fn the_question_and_the_longest_ride_lay_out_whole() {
        let mut view = View::new(Page::EndRide, 0, None);
        view.active_secs = 359_999;
        let sample = crate::ride::log::Sample::default();
        view.totals.add(1000, 999_999_999, &sample, None);
        let screen = Rect {
            left: 0,
            top: 50,
            right: WIDTH,
            bottom: HEIGHT - 50,
        };
        prepare(&view).assert_laid_out(
            screen,
            &[
                "END RIDE?",
                "99:59:59",
                "1000.0 KM",
                "YOUR RIDE WILL BE SAVED",
            ],
            "end ride",
        );
    }
}
