//! After the ride: a brief SAVING, then RIDE SAVED and what the ride added
//! up to, on three pages that MOVE or a swipe step through, or four for a
//! ride that recorded altitude.
//!
//! 1. Overview: time, distance, average speed, ascent, average power and
//!    heart rate under a green check.
//! 2. Effort: normalized and peak power, work and the calories it burned,
//!    heart rate, the time in each power and heart rate zone and, once
//!    the minute after the finish has passed, the heart rate recovery.
//! 3. Elevation, only for a ride that recorded altitude: ascent, descent,
//!    lowest and highest altitude over the ride's elevation profile.
//! 4. Best power (page 3 without an elevation page): the best 5 s, 1, 5 and
//!    20 min of the ride as bars, then the climbs and laps.
//! 5. Only when a new 20 min best suggests a higher FTP: `FTP 245 W?`. ACT
//!    (check cue) takes it and BACK (back cue) keeps the FTP in force; either
//!    one then finishes, as on every page.
//!
//! ACT (the green check cue) or BACK finishes and opens Home. A personal best
//! the ride set says `BEST` in its caption, in the reading's colour.
//!
//! A saved ride from the ride list shows on the same pages, read back from
//! its journal: its start replaces RIDE SAVED, BACK returns to the list and
//! ACT sends it to the phone. What the journal does not keep shows as --:
//! climbs, and the laps of a ride recorded before laps were kept. Ascent,
//! descent and the profile come from the recorded altitude; a ride recorded
//! without it keeps its pages as they were, with ascent estimated from the
//! gradient.
use super::elevation::Chart;
use super::stats::zone_bar;
use super::{elapsed, kilometres, speed, whole};
use crate::ride::bests::Found;
use crate::ride::recovery::Reading as Recovery;
use crate::ride::totals::EFFORT_SECONDS;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, BODY, CAPTION, CONTENT, GRADE, HEART, POWER, Rect, SECONDARY, SPEED, TERTIARY,
    TITLE, VALUE, WHITE, WIDTH,
};
use crate::screens::cue::{Badge, Corner, Cue, Icon};
use crate::screens::workout::{Text, View};

pub const CUE: Cue = Cue::new(Corner::BottomRight, Icon::Check);
/// A saved ride: ACT sends it to the phone. BACK keeps its fixed role,
/// back to the list, without a cue, as on the ride pages.
pub const PAST_CUES: [Cue; 1] = [Cue::new(Corner::BottomRight, Icon::Send)];
/// The FTP offer: ACT takes it, BACK keeps the FTP in force.
pub const OFFER_CUES: [Cue; 2] = [
    Cue::new(Corner::TopLeft, Icon::Back),
    Cue::new(Corner::BottomRight, Icon::Check),
];

/// A suggested FTP: 95% of a new 20 min best, well above the one in force.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FtpOffer {
    pub suggested: u16,
    pub current: u16,
    /// The 20 min best it comes from, in watts.
    pub best: u16,
}

/// Where the saved ride stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Saving {
    /// The last samples are being written.
    #[default]
    Writing,
    Saved,
    /// Storage failed while finishing: what was written before is kept.
    Failed,
    /// A saved ride is being read back.
    Reading,
    /// A saved ride could not be found or read.
    Unreadable,
}

/// What the summary shows beyond the ride totals.
#[derive(Clone, Copy, Debug, Default)]
pub struct After {
    pub page: usize,
    pub saving: Saving,
    /// Best average watts over `EFFORT_SECONDS`.
    pub best: [Option<u16>; 4],
    /// Unknown for a saved ride read back.
    pub climbs: Option<u16>,
    /// Unknown for a saved ride recorded without lap marks.
    pub laps: Option<u16>,
    /// A saved ride from the list, titled by `View::past_title`.
    pub past: bool,
    /// The ride has an elevation page.
    pub elevation: bool,
    /// The personal bests this ride set.
    pub found: Found,
    /// The FTP on offer as the last page.
    pub offer: Option<FtpOffer>,
    /// Heart-rate recovery after the finish; none for a saved ride, which
    /// does not keep it.
    pub recovery: Option<Recovery>,
}

/// How many pages the summary has, the FTP offer included.
pub fn total_pages(after: &After) -> usize {
    pages(after.elevation) + usize::from(after.offer.is_some() && !after.past)
}

/// Whether the FTP offer, the page after the others, is on screen.
pub fn offer_shown(after: &After) -> bool {
    after.page == pages(after.elevation) && after.offer.is_some() && !after.past
}

/// "5 S", "1 MIN", "5 MIN" or "20 MIN": the length of best effort `index`.
pub fn effort_label(index: usize) -> Text<12> {
    let mut label = Text::new();
    let _ = match EFFORT_SECONDS[index] {
        s if s < 60 => core::fmt::Write::write_fmt(&mut label, format_args!("{s} S")),
        s => core::fmt::Write::write_fmt(&mut label, format_args!("{} MIN", s / 60)),
    };
    label
}
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
/// `top`, as on the ride pages. A new personal best says `BEST` in its
/// caption, in the reading's colour.
fn cell(canvas: &mut Canvas, column: usize, top: usize, reading: &Reading) {
    let (left, right) = if column == 0 {
        (LEFT, MIDDLE - COLUMN_GAP)
    } else {
        (MIDDLE, RIGHT)
    };
    let (caption, caption_color) = match reading.best {
        Some(caption) => (caption, reading.color),
        None => (reading.caption, SECONDARY),
    };
    canvas.fit(
        &[&CAPTION],
        caption,
        right - left,
        left,
        top + 12,
        Align::Left,
        caption_color,
    );
    canvas.fit(
        &[&VALUE, &BODY],
        reading.value.as_bytes(),
        right - left,
        left,
        top + 41,
        Align::Left,
        reading.color,
    );
}

/// A caption over a value, and the caption of a personal best in its place.
#[derive(Clone, Copy)]
struct Reading<'a> {
    caption: &'a [u8],
    value: &'a Text<12>,
    color: u16,
    best: Option<&'a [u8]>,
}

/// The room a `BEST` caption has under a best-power bar.
const BEST_WIDTH: usize = 44;

fn reading<'a>(caption: &'a [u8], value: &'a Text<12>, color: u16) -> Reading<'a> {
    Reading {
        caption,
        value,
        color,
        best: None,
    }
}

impl<'a> Reading<'a> {
    /// A new personal best: `caption` replaces the usual one, and the value
    /// takes `color` where it would be white.
    fn best(self, set: bool, caption: &'a [u8], color: u16) -> Self {
        if !set {
            return self;
        }
        Self {
            best: Some(caption),
            color,
            ..self
        }
    }
}

fn cells(canvas: &mut Canvas, first: usize, cells: &[Reading]) {
    for (index, reading) in cells.iter().enumerate() {
        let top = first + index / 2 * super::CELL_PITCH;
        cell(canvas, index % 2, top, reading);
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
        Saving::Reading => {
            centred(&mut canvas, &[&TITLE, &BODY], b"READING", 150, WHITE);
            centred(&mut canvas, &[&CAPTION], b"ONE MOMENT", 178, SECONDARY);
            return canvas;
        }
        Saving::Unreadable => {
            centred(&mut canvas, &[&BODY], b"RIDE UNREADABLE", 150, AMBER);
            centred(
                &mut canvas,
                &[&CAPTION],
                b"ITS RECORD IS DAMAGED",
                174,
                SECONDARY,
            );
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
        _ if offer_shown(&view.after) => ftp_offer(view, &mut canvas),
        2 if view.after.elevation => elevation_page(view, &mut canvas),
        _ => best_power(view, &mut canvas),
    }
    canvas
}

/// The badge shows over a saved ride's first page.
pub fn badge(view: &View) -> Option<Badge> {
    (view.after.saving == Saving::Saved && view.after.page == 0 && !view.after.past)
        .then_some(BADGE)
}

fn overview(view: &View, canvas: &mut Canvas) {
    let totals = &view.totals;
    if view.after.past {
        canvas.caption(b"SAVED RIDE", RIGHT - LEFT, CENTRE, 70, Align::Center);
        centred(
            canvas,
            &[&TITLE, &BODY],
            view.past_title.as_bytes(),
            112,
            WHITE,
        );
    } else if view.after.saving == Saving::Saved {
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
    let found = view.after.found;
    cells(
        canvas,
        122,
        &[
            reading(b"TIME", time, WHITE).best(found.time, b"BEST TIME", SPEED),
            reading(b"DIST KM", distance, WHITE).best(found.distance, b"BEST KM", SPEED),
            reading(b"AVG KM/H", avg_speed, SPEED),
            reading(b"ASCENT M", ascent, GRADE).best(found.ascent, b"BEST ASC M", GRADE),
            reading(b"AVG W", power, POWER),
            reading(b"AVG BPM", heart, HEART),
        ],
    );
}

fn effort(view: &View, canvas: &mut Canvas) {
    let totals = &view.totals;
    let mut values: [Text<12>; 6] = core::array::from_fn(|_| Text::new());
    let [np, max, work, kcal, heart, max_heart] = &mut values;
    whole(np, totals.normalized_power());
    whole(max, totals.max_power());
    whole(work, totals.work_kj());
    whole(kcal, totals.kcal());
    whole(heart, totals.avg_heart_bpm());
    whole(max_heart, totals.max_heart_bpm());
    let first = CONTENT.top + 8;
    cells(
        canvas,
        first,
        &[
            reading(b"NP W", np, POWER),
            reading(b"MAX W", max, POWER),
            reading(b"WORK KJ", work, WHITE),
            reading(b"KCAL", kcal, WHITE),
            reading(b"AVG BPM", heart, HEART),
            reading(b"MAX BPM", max_heart, HEART),
        ],
    );
    let marks = &view.marks;
    let bars = first + 3 * super::CELL_PITCH + 12;
    zone_bar(canvas, b"PWR", (LEFT, BAR_LEFT), bars, marks.power_time);
    zone_bar(canvas, b"HR", (LEFT, BAR_LEFT), bars + 18, marks.heart_time);
    if let Some(reading) = view.after.recovery {
        recovery(canvas, reading, bars + 40);
    }
}

/// The recovery cell, whose caption top is `top`.
fn recovery(canvas: &mut Canvas, reading: Recovery, top: usize) {
    let mut text = Text::<16>::new();
    let color = match reading {
        Recovery::Measuring(secs) => {
            let _ = core::fmt::Write::write_fmt(&mut text, format_args!("MEASURING {secs} S"));
            SECONDARY
        }
        // A fall is the good direction; a rise is shown plainly.
        Recovery::Recovered(drop) => {
            let _ = core::fmt::Write::write_fmt(
                &mut text,
                format_args!(
                    "{}{} BPM",
                    if drop > 0 { "-" } else { "+" },
                    drop.unsigned_abs()
                ),
            );
            HEART
        }
    };
    // A cell as above, but across the page and in BODY: VALUE has no
    // letters, and the measuring text is long.
    canvas.caption(b"HR RECOVERY", RIGHT - LEFT, LEFT, top + 12, Align::Left);
    canvas.fit(
        &[&BODY],
        text.as_bytes(),
        RIGHT - LEFT,
        LEFT,
        top + 41,
        Align::Left,
        color,
    );
}

/// The pages of the summary. A ride that recorded altitude has an elevation
/// page between effort and best power.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SummaryPage {
    Overview,
    Effort,
    Elevation,
    BestPower,
}

impl SummaryPage {
    /// The page at `index` of a summary with or without an elevation page.
    pub const fn at(index: usize, elevation: bool) -> Self {
        match (index, elevation) {
            (0, _) => Self::Overview,
            (1, _) => Self::Effort,
            (2, true) => Self::Elevation,
            _ => Self::BestPower,
        }
    }
}

/// How many pages the summary has.
pub const fn pages(elevation: bool) -> usize {
    if elevation { 4 } else { 3 }
}

/// The elevation profile shows once the ride is saved.
pub fn shows_elevation(view: &View) -> bool {
    view.after.elevation
        && SummaryPage::at(view.after.page, true) == SummaryPage::Elevation
        && matches!(view.after.saving, Saving::Saved | Saving::Failed)
}

/// Where the elevation page's cells start, and its chart below them.
const ELEVATION_CELLS: usize = CONTENT.top + 8;

fn elevation_page(view: &View, canvas: &mut Canvas) {
    let totals = &view.totals;
    let mut values: [Text<12>; 4] = core::array::from_fn(|_| Text::new());
    let [ascent, descent, low, high] = &mut values;
    whole(ascent, Some((totals.ascent_dm() + 5) / 10));
    whole(descent, Some((totals.descent_dm() + 5) / 10));
    let range = view.elevation_chart.and_then(Chart::range_m);
    whole(low, range.map(|(low, _)| low));
    whole(high, range.map(|(_, high)| high));
    cells(
        canvas,
        ELEVATION_CELLS,
        &[
            reading(b"ASCENT M", ascent, GRADE),
            reading(b"DESCENT M", descent, GRADE),
            reading(b"MIN M", low, GRADE),
            reading(b"MAX M", high, GRADE),
        ],
    );
}

/// The longest a best-power bar can be; the highest effort fills it.
const EFFORT_BAR: usize = 72;
const EFFORT_BAR_LEFT: usize = LEFT + 62;

fn best_power(view: &View, canvas: &mut Canvas) {
    let first = CONTENT.top + 8;
    canvas.caption(b"BEST POWER W", RIGHT - LEFT, LEFT, first + 12, Align::Left);
    let pitch = 38;
    let peak = view.after.best.iter().flatten().max().copied().unwrap_or(0);
    for (index, best) in view.after.best.into_iter().enumerate() {
        let base = first + 48 + index * pitch;
        let label = effort_label(index);
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
        // A new personal best: its own caption under the bar.
        if view.after.found.power[index] {
            canvas.fit(
                &[&CAPTION],
                b"BEST",
                BEST_WIDTH,
                EFFORT_BAR_LEFT,
                base + 11,
                Align::Left,
                POWER,
            );
        }
    }
    let mut climbs = Text::<12>::new();
    let mut laps = Text::<12>::new();
    whole(&mut climbs, view.after.climbs);
    whole(&mut laps, view.after.laps);
    cells(
        canvas,
        first + 22 + 4 * pitch + 6,
        &[
            reading(b"CLIMBS", &climbs, GRADE),
            reading(b"LAPS", &laps, WHITE),
        ],
    );
}

/// `FTP 245 W?`: what the new 20 min best suggests, and what it replaces.
fn ftp_offer(view: &View, canvas: &mut Canvas) {
    let Some(offer) = view.after.offer else {
        return;
    };
    let mut best = Text::<24>::new();
    let _ = core::fmt::Write::write_fmt(&mut best, format_args!("20 MIN BEST {} W", offer.best));
    centred(canvas, &[&BODY, &CAPTION], best.as_bytes(), 104, SECONDARY);
    let mut ftp = Text::<12>::new();
    let _ = core::fmt::Write::write_fmt(&mut ftp, format_args!("FTP {} W?", offer.suggested));
    centred(canvas, &[&TITLE, &BODY], ftp.as_bytes(), 164, POWER);
    let mut now = Text::<24>::new();
    let _ = core::fmt::Write::write_fmt(&mut now, format_args!("NOW {} W", offer.current));
    centred(canvas, &[&BODY, &CAPTION], now.as_bytes(), 194, SECONDARY);
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

    fn extreme(page: usize, saving: Saving) -> View<'static> {
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
            climbs: Some(999),
            laps: Some(999),
            past: false,
            elevation: false,
            found: Found::default(),
            offer: None,
            recovery: Some(Recovery::Recovered(120)),
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
                (
                    1,
                    &[
                        "1999",
                        "220",
                        "KCAL",
                        "PWR",
                        "HR",
                        "HR RECOVERY",
                        "-120 BPM",
                    ],
                ),
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
    fn a_ride_with_altitude_has_an_elevation_page_before_best_power() {
        let bounds = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT - RADIUS + 12,
        };
        let mut trace = crate::ride::elevation::Trace::default();
        for metres in (0..4000).step_by(5) {
            trace.add(metres * 1000, Some(-120 + (metres as i32 % 2000)));
        }
        let mut chart = Chart::default();
        chart.refresh(&trace);
        let with_altitude = |page| {
            let mut view = extreme(page, Saving::Saved);
            view.elevation_chart = Some(&chart);
            view.after.elevation = true;
            view.prepare();
            view
        };
        let view = with_altitude(2);
        let want = ["ASCENT M", "DESCENT M", "MIN M", "MAX M", "-12", "186"];
        view.canvas.assert_laid_out(bounds, &want, "elevation");
        assert!(shows_elevation(&view));
        let fill = crate::screens::ride::elevation::pixel(
            &chart,
            WIDTH / 2,
            crate::screens::ride::elevation::BOTTOM - 1,
        );
        assert!(fill.is_some(), "the chart fills above its baseline");
        assert!(with_altitude(3).canvas.shows("BEST POWER W"));
        assert!(!with_altitude(1).canvas.shows("ASCENT M"));
        assert!(!shows_elevation(&with_altitude(1)));
        let mut writing = extreme(2, Saving::Writing);
        writing.after.elevation = true;
        assert!(!shows_elevation(&writing));
        let plain = extreme(2, Saving::Saved);
        assert!(plain.canvas.shows("BEST POWER W") && !shows_elevation(&plain));
        assert_eq!((pages(false), pages(true)), (3, 4));
    }

    #[test]
    fn the_effort_page_shows_calories_and_the_recovery_states() {
        let bounds = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT - RADIUS + 12,
        };
        let with = |recovery| {
            let mut view = extreme(1, Saving::Saved);
            view.after.recovery = recovery;
            view.prepare();
            view
        };
        // 1999 W for a minute is 120 kJ, so 120 kcal.
        let measuring = with(Some(Recovery::Measuring(42)));
        let want = ["KCAL", "120", "HR RECOVERY", "MEASURING 42 S"];
        measuring.canvas.assert_laid_out(bounds, &want, "measuring");
        let fell = with(Some(Recovery::Recovered(28)));
        fell.canvas.assert_laid_out(bounds, &["-28 BPM"], "fell");
        assert!(with(Some(Recovery::Recovered(-3))).canvas.shows("+3 BPM"));
        assert!(!with(None).canvas.shows("HR RECOVERY"));
    }

    #[test]
    fn a_saved_ride_shows_its_start_and_what_is_unknown() {
        let bounds = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: HEIGHT - RADIUS + 12,
        };
        let past = |page, saving| {
            let mut view = extreme(page, saving);
            view.page = Page::PastRide;
            view.after.past = true;
            view.after.climbs = None;
            view.after.laps = None;
            let _ = view.past_title.push_str("DEC 31  12:59");
            view.prepare();
            view
        };
        let first = past(0, Saving::Saved);
        let want = ["SAVED RIDE", "DEC 31  12:59", "99:59:59"];
        first.canvas.assert_laid_out(bounds, &want, "past ride");
        assert!(!first.canvas.shows("RIDE SAVED") && badge(&first).is_none());
        assert!(past(2, Saving::Saved).canvas.shows("--"));
        let reading = past(0, Saving::Reading);
        assert!(reading.canvas.shows("READING"));
        let missing = past(0, Saving::Unreadable);
        missing.canvas.assert_laid_out(
            bounds,
            &["RIDE UNREADABLE", "ITS RECORD IS DAMAGED"],
            "unreadable",
        );
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
