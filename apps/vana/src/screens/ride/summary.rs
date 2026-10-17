//! After the ride: a brief SAVING, then RIDE SAVED and what the ride added
//! up to, on three pages that MOVE or a swipe step through.
//!
//! 1. Overview: time, distance, average speed, ascent, average power and
//!    heart rate under a green check.
//! 2. Effort: normalized, average and peak power, work, heart rate and the
//!    time in each power and heart rate zone.
//! 3. Best power: the best 5 s, 1, 5 and 20 min of the ride as bars, then
//!    the climbs and laps.
//!
//! ACT (the green check cue) or BACK finishes and opens Home.
use super::stats::zone_bar;
use super::{elapsed, kilometres, speed, whole};
use crate::ride::totals::EFFORT_SECONDS;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, BODY, CAPTION, CONTENT, GRADE, HEART, POWER, Rect, SECONDARY, SPEED, TERTIARY,
    TITLE, VALUE, WHITE, WIDTH,
};
use crate::screens::cue::{Badge, Corner, Cue, Icon};
use crate::screens::workout::{Text, View};

pub const CUE: Cue = Cue::new(Corner::BottomRight, Icon::Check);
pub const PAGES: usize = 3;

/// Where the saved ride stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Saving {
    /// The last samples are being written.
    #[default]
    Writing,
    Saved,
    /// Storage failed while finishing: what was written before is kept.
    Failed,
}

/// What the summary shows beyond the ride totals.
#[derive(Clone, Copy, Debug, Default)]
pub struct After {
    pub page: usize,
    pub saving: Saving,
    /// Best average watts over `EFFORT_SECONDS`.
    pub best: [Option<u16>; 4],
    pub climbs: u16,
    pub laps: u16,
}

const LEFT: usize = 16;
const RIGHT: usize = WIDTH - 16;
const MIDDLE: usize = WIDTH / 2 + 4;
const COLUMN_GAP: usize = 8;
const CENTRE: usize = WIDTH / 2;
/// Zone bars start here; their captions sit at `LEFT`.
const BAR_LEFT: usize = RIGHT - 148;

/// The check over RIDE SAVED.
pub const BADGE: Badge = Badge {
    cx: CENTRE,
    cy: CONTENT.top + 34,
    radius: 20,
    icon: Icon::Check,
};

/// A caption over its value in a column of the band whose caption top is
/// `top`, as on the ride pages.
fn cell(canvas: &mut Canvas, column: usize, top: usize, caption: &[u8], value: &[u8], color: u16) {
    let (left, right) = if column == 0 {
        (LEFT, MIDDLE - COLUMN_GAP)
    } else {
        (MIDDLE, RIGHT)
    };
    canvas.caption(caption, right - left, left, top + 12, Align::Left);
    canvas.fit(
        &[&VALUE, &BODY],
        value,
        right - left,
        left,
        top + 41,
        Align::Left,
        color,
    );
}

fn cells(canvas: &mut Canvas, first: usize, cells: &[(&[u8], &Text<12>, u16)]) {
    for (index, (caption, value, color)) in cells.iter().enumerate() {
        let top = first + index / 2 * super::CELL_PITCH;
        cell(canvas, index % 2, top, caption, value.as_bytes(), *color);
    }
}

pub fn prepare(view: &View) -> Canvas {
    let mut canvas = Canvas::default();
    match view.after.saving {
        Saving::Writing => {
            centred(&mut canvas, &[&TITLE, &BODY], b"SAVING", 150, WHITE);
            centred(&mut canvas, &[&CAPTION], b"KEEP VANA ON", 178, SECONDARY);
            return canvas;
        }
        Saving::Failed if view.after.page == 0 => {
            centred(&mut canvas, &[&TITLE, &BODY], b"NOT ALL SAVED", 86, AMBER);
            centred(
                &mut canvas,
                &[&CAPTION],
                b"STORAGE FAILED AT THE END",
                108,
                SECONDARY,
            );
        }
        _ => {}
    }
    match view.after.page {
        0 => overview(view, &mut canvas),
        1 => effort(view, &mut canvas),
        _ => best_power(view, &mut canvas),
    }
    canvas
}

/// The badge shows over a saved ride's first page.
pub fn badge(view: &View) -> Option<Badge> {
    (view.after.saving == Saving::Saved && view.after.page == 0).then_some(BADGE)
}

fn overview(view: &View, canvas: &mut Canvas) {
    let totals = &view.totals;
    if view.after.saving == Saving::Saved {
        centred(canvas, &[&TITLE, &BODY], b"RIDE SAVED", 112, WHITE);
    }
    let mut values: [Text<12>; 6] = core::array::from_fn(|_| Text::new());
    let [time, distance, avg_speed, ascent, power, heart] = &mut values;
    elapsed(time, view.active_secs);
    kilometres(distance, totals.distance_mm());
    speed(avg_speed, totals.avg_speed_mm_s());
    whole(ascent, Some((totals.ascent_dm() + 5) / 10));
    whole(power, totals.avg_power());
    whole(heart, totals.avg_heart_bpm());
    cells(
        canvas,
        122,
        &[
            (b"TIME", time, WHITE),
            (b"DIST KM", distance, WHITE),
            (b"AVG KM/H", avg_speed, SPEED),
            (b"ASCENT M", ascent, GRADE),
            (b"AVG W", power, POWER),
            (b"AVG BPM", heart, HEART),
        ],
    );
}

fn effort(view: &View, canvas: &mut Canvas) {
    let totals = &view.totals;
    let mut values: [Text<12>; 6] = core::array::from_fn(|_| Text::new());
    let [np, avg, max, work, heart, max_heart] = &mut values;
    whole(np, totals.normalized_power());
    whole(avg, totals.avg_power());
    whole(max, totals.max_power());
    whole(work, totals.work_kj());
    whole(heart, totals.avg_heart_bpm());
    whole(max_heart, totals.max_heart_bpm());
    let first = CONTENT.top + 8;
    cells(
        canvas,
        first,
        &[
            (b"NP W", np, POWER),
            (b"AVG W", avg, POWER),
            (b"MAX W", max, POWER),
            (b"WORK KJ", work, WHITE),
            (b"AVG BPM", heart, HEART),
            (b"MAX BPM", max_heart, HEART),
        ],
    );
    let marks = &view.marks;
    let bars = first + 3 * super::CELL_PITCH + 12;
    zone_bar(canvas, b"PWR", (LEFT, BAR_LEFT), bars, marks.power_time);
    zone_bar(canvas, b"HR", (LEFT, BAR_LEFT), bars + 18, marks.heart_time);
}

/// The longest a best-power bar can be; the highest effort fills it.
const EFFORT_BAR: usize = 72;
const EFFORT_BAR_LEFT: usize = LEFT + 62;

fn best_power(view: &View, canvas: &mut Canvas) {
    let first = CONTENT.top + 8;
    canvas.caption(b"BEST POWER W", RIGHT - LEFT, LEFT, first + 12, Align::Left);
    let pitch = 38;
    let peak = view.after.best.iter().flatten().max().copied().unwrap_or(0);
    for (index, (seconds, best)) in EFFORT_SECONDS.iter().zip(view.after.best).enumerate() {
        let base = first + 48 + index * pitch;
        let mut label = Text::<12>::new();
        let _ = match seconds {
            s if *s < 60 => core::fmt::Write::write_fmt(&mut label, format_args!("{s} S")),
            s => core::fmt::Write::write_fmt(&mut label, format_args!("{} MIN", s / 60)),
        };
        canvas.caption(
            label.as_bytes(),
            EFFORT_BAR_LEFT - LEFT - 6,
            LEFT,
            base - 4,
            Align::Left,
        );
        let mut value = Text::<12>::new();
        whole(&mut value, best);
        canvas.fit(
            &[&VALUE, &BODY],
            value.as_bytes(),
            RIGHT - EFFORT_BAR_LEFT - EFFORT_BAR - 8,
            RIGHT,
            base,
            Align::Right,
            if best.is_some() { POWER } else { TERTIARY },
        );
        let length = match best {
            Some(watts) if peak > 0 => (usize::from(watts) * EFFORT_BAR / usize::from(peak)).max(4),
            _ => EFFORT_BAR,
        };
        canvas.shape(Shape::Bar {
            rect: Rect {
                left: EFFORT_BAR_LEFT,
                top: base - 12,
                right: EFFORT_BAR_LEFT + length,
                bottom: base - 4,
            },
            radius: 2,
            color: if best.is_some() { POWER } else { TERTIARY },
        });
    }
    let mut climbs = Text::<12>::new();
    let mut laps = Text::<12>::new();
    whole(&mut climbs, Some(view.after.climbs));
    whole(&mut laps, Some(view.after.laps));
    cells(
        canvas,
        first + 22 + 4 * pitch + 6,
        &[(b"CLIMBS", &climbs, GRADE), (b"LAPS", &laps, WHITE)],
    );
}

fn centred(
    canvas: &mut Canvas,
    faces: &[&'static firmware_shell::rendering::font::Face],
    text: &[u8],
    base: usize,
    color: u16,
) {
    canvas.fit(
        faces,
        text,
        RIGHT - LEFT,
        CENTRE,
        base,
        Align::Center,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::Sample;
    use crate::screens::chrome::HEIGHT;
    use crate::screens::cue::RADIUS;
    use crate::screens::workout::Page;

    fn extreme(page: usize, saving: Saving) -> View {
        let mut view = View::new(Page::Summary, 0, None);
        let sample = Sample {
            speed_mm_s: Some(33_333),
            power_watts: Some(1999),
            heart_bpm: Some(220),
            ..Sample::default()
        };
        for _ in 0..60 {
            view.totals.add(1000, 16_666_666, &sample, None);
            view.zone_time.add(1000, &sample, &view.profile);
        }
        view.active_secs = 359_999;
        view.after = After {
            page,
            saving,
            best: [Some(1999), Some(1500), None, None],
            climbs: 999,
            laps: 999,
        };
        view.prepare();
        view
    }

    #[test]
    fn every_page_lays_out_whole_above_the_done_cue() {
        // The cue's corner box; right-column text ends above it.
        let bounds = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT - RADIUS + 12,
        };
        for saving in [Saving::Saved, Saving::Failed] {
            for (page, want) in [
                (0, &["99:59:59", "1000.0", "1999", "220"][..]),
                (1, &["1999", "220", "PWR", "HR"]),
                (2, &["5 S", "20 MIN", "1999", "1500", "999"]),
            ] {
                let view = extreme(page, saving);
                let what = std::format!("summary {page} {saving:?}");
                view.canvas.assert_laid_out(bounds, want, &what);
                for run in &view.canvas.runs {
                    let dx = WIDTH - run.columns().end.min(WIDTH);
                    let dy = HEIGHT - run.bottom();
                    assert!(
                        dx * dx + dy * dy > (RADIUS + 2) * (RADIUS + 2),
                        "{what}: {:?} under the cue",
                        core::str::from_utf8(run.line.text())
                    );
                }
            }
        }
        let saved = extreme(0, Saving::Saved);
        assert!(saved.canvas.shows("RIDE SAVED"));
        assert!(badge(&saved).is_some());
        assert!(extreme(0, Saving::Failed).canvas.shows("NOT ALL SAVED"));
        let saving = extreme(0, Saving::Writing);
        assert!(saving.canvas.shows("SAVING") && badge(&saving).is_none());
    }

    #[test]
    fn best_power_bars_scale_to_the_highest_effort() {
        let view = extreme(2, Saving::Saved);
        let bars: std::vec::Vec<_> = view
            .canvas
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, color, .. } => Some((rect.width(), *color)),
                _ => None,
            })
            .collect();
        assert_eq!(bars[0], (EFFORT_BAR, POWER));
        assert_eq!(bars[1], (1500 * EFFORT_BAR / 1999, POWER));
        assert_eq!(bars[2], (EFFORT_BAR, TERTIARY), "no 5 min effort yet");
    }
}
