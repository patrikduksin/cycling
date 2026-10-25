//! The paused screen: while a ride is paused, the whole screen says so.
//!
//! Amber pause bars and PAUSED (or AUTO PAUSED), how long the pause has
//! lasted, then the ride so far. The green cue beside ACT resumes; the cue
//! beside BACK ends the ride. MOVE or a swipe shows the ride pages, and
//! paging past the last one comes back here.
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, BODY, CAPTION, HEIGHT, LARGE, Rect, SECONDARY, TITLE, VALUE, WHITE, WIDTH,
};
use crate::screens::cue::{self, Corner, Cue, Icon};
use crate::screens::workout::{Text, View};

pub const CUES: [Cue; 2] = [
    Cue::new(Corner::TopLeft, Icon::Stop),
    Cue::new(Corner::BottomRight, Icon::Play),
];

const CENTRE: usize = WIDTH / 2;
/// Text keeps clear of the cues on both sides.
const TEXT_WIDTH: usize = WIDTH - 2 * 24;
const BARS_TOP: usize = 54;
const BARS_HEIGHT: usize = 40;

pub fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    for left in [CENTRE - 20, CENTRE + 6] {
        canvas.shape(Shape::Bar {
            rect: Rect {
                left,
                top: BARS_TOP,
                right: left + 14,
                bottom: BARS_TOP + BARS_HEIGHT,
            },
            radius: 4,
            color: AMBER,
        });
    }
    let title: &[u8] = if view.auto_paused {
        b"AUTO PAUSED"
    } else {
        b"PAUSED"
    };
    centred(&mut canvas, &[&TITLE, &BODY], title, 136, AMBER);
    let mut paused = Text::<12>::new();
    let _ = core::fmt::Write::write_str(&mut paused, "FOR ");
    super::lap_time(&mut paused, u64::from(view.paused_secs));
    centred(&mut canvas, &[&BODY], paused.as_bytes(), 162, SECONDARY);
    centred(&mut canvas, &[&CAPTION], b"RIDE TIME", 198, SECONDARY);
    let mut time = Text::<12>::new();
    super::elapsed(&mut time, view.active_secs);
    centred(&mut canvas, &[&LARGE, &VALUE], time.as_bytes(), 244, WHITE);
    let mut distance = Text::<12>::new();
    super::kilometres(&mut distance, view.totals.distance_mm());
    let _ = core::fmt::Write::write_str(&mut distance, " KM");
    centred(&mut canvas, &[&BODY], distance.as_bytes(), 272, WHITE);
    if view.auto_paused {
        centred(
            &mut canvas,
            &[&CAPTION],
            b"RIDE ON TO RESUME",
            HEIGHT - cue::RADIUS + 8,
            SECONDARY,
        );
    }
    canvas
}

fn centred(
    canvas: &mut Canvas,
    faces: &[&'static firmware_shell::rendering::font::Face],
    text: &[u8],
    base: usize,
    color: u16,
) {
    canvas.fit(faces, text, TEXT_WIDTH, CENTRE, base, Align::Center, color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::workout::Page;

    #[test]
    fn longest_pause_lays_out_whole_between_the_cues() {
        for auto in [false, true] {
            let mut view = View::new(Page::Ride, 0, None);
            view.auto_paused = auto;
            view.paused_secs = 359_999;
            view.active_secs = 359_999;
            let sample = crate::ride::log::Sample::default();
            view.totals.add(1000, 999_999_999, &sample, None);
            let canvas = prepare(&view);
            let bounds = Rect {
                left: 0,
                top: 0,
                right: WIDTH,
                bottom: HEIGHT,
            };
            canvas.assert_laid_out(bounds, &["99:59:59", "1000.0 KM"], "paused");
            for run in &canvas.runs {
                // Nothing reaches under a cue.
                let (left, right) = (run.columns().start, run.columns().end);
                let (top, bottom) = (run.top, run.bottom());
                let near = |x: usize, y: usize| x * x + y * y < (cue::RADIUS + 2).pow(2);
                assert!(!near(left, top), "{:?}", run.line.text());
                assert!(
                    !near(WIDTH - right.min(WIDTH), HEIGHT - bottom.min(HEIGHT)),
                    "{:?}",
                    core::str::from_utf8(run.line.text())
                );
            }
        }
    }
}
