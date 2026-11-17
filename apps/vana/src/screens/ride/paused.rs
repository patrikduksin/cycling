//! The paused screen: while a ride is paused, the whole screen says so.
//!
//! Amber pause bars and PAUSED (or AUTO PAUSED), how long the pause has
//! lasted, then the ride so far. The green cue beside ACT resumes; the cue
//! beside BACK ends the ride. MOVE or a swipe shows the ride pages, and
//! paging past the last one comes back here.
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{AMBER, Align, Rect, SECONDARY, WHITE};
use crate::screens::cue::{Corner, Cue, Icon};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::{Face, FaceRole};

pub const CUES: [Cue; 2] = [
    Cue::new(Corner::TopLeft, Icon::Stop),
    Cue::new(Corner::BottomRight, Icon::Play),
];

/// The paused screen on one panel: the pause bars, then rows of text
/// centred between the cues, each row's capitals ending on its bottom. A
/// taller panel parts the pause from the ride so far, and the ride from the
/// hint at the bottom.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    pub bars: [Rect; 2],
    /// PAUSED or AUTO PAUSED.
    pub title: Rect,
    /// How long the pause has lasted.
    pub lasted: Rect,
    /// RIDE TIME over the time.
    pub caption: Rect,
    pub time: Rect,
    pub distance: Rect,
    /// RIDE ON TO RESUME, while auto-paused, just above the cue beside ACT.
    pub resume: Rect,
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let screen = at.screen();
    let [_, text, _] = screen.cols(crate::screens::cue::CLEAR_OF_CUES, Unit(0), at);
    let [
        _,
        _,
        bars,
        title,
        lasted,
        _,
        caption,
        time,
        distance,
        _,
        resume,
        _,
    ] = text.rows(
        [
            Fr(1),
            Unit(54),
            Unit(40),
            Unit(42),
            Unit(26),
            Fr(1),
            Unit(36),
            Unit(46),
            Unit(28),
            Fr(1),
            Unit(14),
            Unit(34),
        ],
        Unit(0),
        at,
    );
    let [_, left, _, right, _] =
        screen.cols([Fr(1), Unit(14), Unit(12), Unit(14), Fr(1)], Unit(0), at);
    Regions {
        bars: [bars.with_columns(left), bars.with_columns(right)],
        title,
        lasted,
        caption,
        time,
        distance,
        resume,
    }
}

/// Which of the trace's shown readings the paused screen draws.
pub const READINGS: [&str; 2] = ["time", "dist"];

pub fn prepare(view: &View) -> Canvas {
    let layout = view.frame_layout();
    let (at, regions) = (&layout.scaled, &layout.paused);
    let face = |role| at.face(role);
    let mut canvas = Canvas::default();
    for rect in regions.bars {
        canvas.shape(Shape::Bar {
            rect,
            radius: at.unit(4),
            color: AMBER,
        });
    }
    let title: &[u8] = if view.auto_paused {
        b"AUTO PAUSED"
    } else {
        b"PAUSED"
    };
    let title_faces = [face(FaceRole::Title), face(FaceRole::Body)];
    centred(&mut canvas, &title_faces, title, regions.title, AMBER);
    let mut paused = Text::<12>::new();
    let _ = core::fmt::Write::write_str(&mut paused, "FOR ");
    super::lap_time(&mut paused, u64::from(view.paused_secs));
    let body = [face(FaceRole::Body)];
    centred(
        &mut canvas,
        &body,
        paused.as_bytes(),
        regions.lasted,
        SECONDARY,
    );
    let caption = [face(FaceRole::Caption)];
    centred(
        &mut canvas,
        &caption,
        b"RIDE TIME",
        regions.caption,
        SECONDARY,
    );
    let mut time = Text::<12>::new();
    super::elapsed(&mut time, view.active_secs);
    let large = [face(FaceRole::Large), face(FaceRole::Value)];
    centred(&mut canvas, &large, time.as_bytes(), regions.time, WHITE);
    let mut distance = Text::<12>::new();
    super::kilometres(&mut distance, view.totals.distance_mm());
    let _ = core::fmt::Write::write_str(&mut distance, " KM");
    centred(
        &mut canvas,
        &body,
        distance.as_bytes(),
        regions.distance,
        WHITE,
    );
    if view.auto_paused {
        centred(
            &mut canvas,
            &caption,
            b"RIDE ON TO RESUME",
            regions.resume,
            SECONDARY,
        );
    }
    canvas
}

fn centred(canvas: &mut Canvas, faces: &[&'static Face], text: &[u8], row: Rect, color: u16) {
    canvas.fit(
        faces,
        text,
        row.width(),
        row.center_x(),
        row.bottom,
        Align::Center,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::screens::cue;
    use crate::screens::workout::Page;

    #[test]
    fn longest_pause_lays_out_whole_between_the_cues() {
        for layout in crate::screens::panels::ALL {
            for auto in [false, true] {
                let mut view = View::new(Page::Ride, 0, None);
                view.layout = layout;
                view.pause_screen = true;
                view.auto_paused = auto;
                view.paused_secs = 359_999;
                view.active_secs = 359_999;
                let sample = crate::ride::log::Sample::default();
                view.totals.add(1000, 999_999_999, &sample, None);
                view.prepare().check_layout();
                let canvas = view.canvas();
                let screen = layout.scaled.screen();
                let what = std::format!("paused on {:?}", layout.scaled.panel);
                canvas.assert_laid_out(screen, &["99:59:59", "1000.0 KM"], &what);
                let (width, height) = (screen.right, screen.bottom);
                let reach = layout.scaled.unit(cue::RADIUS + 2);
                for run in &canvas.runs {
                    let (left, right) = (run.columns().start, run.columns().end);
                    let (top, bottom) = (run.top, run.bottom());
                    let near = |x: usize, y: usize| x * x + y * y < reach.pow(2);
                    let text = core::str::from_utf8(run.line.text());
                    assert!(!near(left, top), "{what}: {text:?}");
                    assert!(
                        !near(width - right.min(width), height - bottom.min(height)),
                        "{what}: {text:?}"
                    );
                }
            }
        }
    }
}
