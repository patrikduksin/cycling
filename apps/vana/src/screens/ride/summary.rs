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
//! ACT sends it to the phone. With a FILES volume to note it on, one more
//! page follows them, reached only going forward: DELETE RIDE?, where ACT
//! (red trash cue) deletes the ride. Climbs and the heart rate recovery come from
//! the ride's facts on FILES; without them climbs show as --, as do the laps
//! of a ride recorded before laps were kept, and there is no recovery. Ascent,
//! descent and the profile come from the recorded altitude; a ride recorded
//! without it keeps its pages as they were, with ascent estimated from the
//! gradient.
use super::elevation::Chart;
use super::stats::zone_bar;
use super::{Cells, elapsed, kilometres, speed, whole};
use crate::kept::bests::Found;
use crate::ride::recovery::Reading as Recovery;
use crate::ride::totals::EFFORT_SECONDS;
use crate::screens::Layout;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, GRADE, HEART, POWER, Rect, SECONDARY, SPEED, TERTIARY, WHITE,
};
use crate::screens::cue::{Badge, Corner, Cue, Icon};
use crate::screens::scaled::{Scaled, Track};
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::{Face, FaceRole};

pub const CUE: Cue = Cue::new(Corner::BottomRight, Icon::Check);
/// A saved ride: ACT sends it to the phone. BACK keeps its fixed role,
/// back to the list, without a cue, as on the ride pages.
pub const PAST_CUES: [Cue; 1] = [Cue::new(Corner::BottomRight, Icon::Send)];
/// The FTP offer: ACT takes it, BACK keeps the FTP in force.
pub const OFFER_CUES: [Cue; 2] = [
    Cue::new(Corner::TopLeft, Icon::Back),
    Cue::new(Corner::BottomRight, Icon::Check),
];

/// Where the saved ride stands.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Saving {
    /// The last samples are being written.
    #[default]
    Writing,
    Saved,
    /// Written to a journal that loses it at power-off, and the phone has
    /// not confirmed a copy: only syncing saves it.
    Unsynced,
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
    /// Unknown for a saved ride read back without its facts.
    pub climbs: Option<u16>,
    /// Unknown for a saved ride recorded without lap marks.
    pub laps: Option<u16>,
    /// A saved ride from the list, titled by `View::past_title`.
    pub past: bool,
    /// ACT sends the saved ride to the phone.
    pub send: bool,
    /// The saved ride's pages end with DELETE RIDE?.
    pub delete: bool,
    /// The ride has an elevation page.
    pub elevation: bool,
    /// The personal bests this ride set.
    pub found: Found,
    /// The FTP on offer as the last page.
    pub offer: Option<crate::workout::ftp::Offer>,
    /// Heart-rate recovery after the finish; for a saved ride, from its
    /// facts.
    pub recovery: Option<Recovery>,
}

impl After {
    pub const EMPTY: Self = Self {
        page: 0,
        saving: Saving::Writing,
        best: [None; 4],
        climbs: None,
        laps: None,
        past: false,
        send: false,
        delete: false,
        elevation: false,
        found: Found::EMPTY,
        offer: None,
        recovery: None,
    };
}

/// How many pages the summary has, the FTP offer or DELETE RIDE? included.
pub fn total_pages(after: &After) -> usize {
    let last = if after.past {
        after.delete
    } else {
        after.offer.is_some()
    };
    pages(after.elevation) + usize::from(last)
}

/// Whether the FTP offer, the page after the others, is on screen.
pub fn offer_shown(after: &After) -> bool {
    after.page == pages(after.elevation) && after.offer.is_some() && !after.past
}

/// Whether DELETE RIDE?, the page after a saved ride's others, is on screen.
pub fn delete_shown(after: &After) -> bool {
    after.page == pages(after.elevation) && after.delete && after.past
}

/// Whether the page is a question with a back cue, shown without the bar.
pub fn question_shown(after: &After) -> bool {
    offer_shown(after) || delete_shown(after)
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

/// The summary on one panel. A text row's bottom is its baseline. On a
/// taller panel the elevation chart takes the spare rows, a lone message
/// and the FTP offer stay in the middle, and the other pages keep the top,
/// clear of the done cue's corner.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Regions {
    /// The text margins across the content.
    pub text: Rect,
    /// Two columns of cells, as on the ride pages.
    pub cells: Cells,
    /// The check over RIDE SAVED.
    pub badge: Badge,
    pub message: Message,
    pub overview: Overview,
    pub effort: Effort,
    pub elevation: Elevation,
    pub best: BestPower,
    /// `20 MIN BEST`, `FTP 245 W?` and `NOW`.
    pub offer: [Rect; 3],
}

/// SAVING, READING or RIDE UNREADABLE, alone on the page.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Message {
    pub title: Rect,
    /// Why a ride is unreadable.
    pub reason: Rect,
    /// What to do while the ride is written or read.
    pub note: Rect,
}

/// The first page: SAVED RIDE over a saved ride's start, or RIDE SAVED; NOT
/// ALL SAVED over its cause; then the ride's totals.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Overview {
    pub saved: Rect,
    pub failed: Rect,
    pub cause: Rect,
    pub title: Rect,
    pub cells: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Effort {
    pub cells: Rect,
    /// The time-in-zone bars; "PWR" and "HR" sit left of them.
    pub power_bar: Rect,
    pub heart_bar: Rect,
    /// The heart rate recovery: a cell across the page.
    pub recovery: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Elevation {
    pub cells: Rect,
    /// The profile's fill; its baseline is the row under it.
    pub chart: Rect,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct BestPower {
    /// BEST POWER W.
    pub header: Rect,
    /// One row per effort, each across the text margins: its label, its bar
    /// and its watts lie in the columns below.
    pub efforts: [Rect; 4],
    pub label: Rect,
    /// The longest a bar can be; the highest effort fills it.
    pub bar: Rect,
    pub value: Rect,
    /// Climbs and laps.
    pub cells: Rect,
}

/// Inside the summary's text margins, which the elevation chart spans.
pub(super) const fn text(at: &Scaled) -> Rect {
    use Track::{Fr, Unit};
    let content = crate::screens::chrome::regions(at).content;
    let [_, text, _] = content.cols([Unit(16), Fr(1), Unit(16)], Unit(0), at);
    text
}

pub const fn regions(at: &Scaled) -> Regions {
    use Track::{Fr, Unit};
    let content = crate::screens::chrome::regions(at).content;
    let text = text(at);
    let [_, left, _, right, _] = content.cols(
        [Unit(16), Fr(1), Unit(COLUMN_GAP), Fr(1), Unit(16)],
        Unit(0),
        at,
    );
    let pitch = super::CELL_PITCH as u16;
    let [_, badge, _] = content.rows([Unit(14), Unit(40), Fr(1)], Unit(0), at);
    let [_, title, reason, note, _, _] = content.rows(
        [Fr(1), Unit(126), Unit(24), Unit(4), Unit(142), Fr(1)],
        Unit(0),
        at,
    );
    let [saved, failed, cause, overview_title, _, overview_cells, _] = content.rows(
        [
            Unit(46),
            Unit(16),
            Unit(22),
            Unit(4),
            Unit(10),
            Unit(3 * pitch),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, effort_cells, _, power_bar, _, heart_bar, _, recovery, _] = content.rows(
        [
            Unit(8),
            Unit(3 * pitch),
            Unit(12),
            Unit(BAR_HEIGHT),
            Unit(10),
            Unit(BAR_HEIGHT),
            Unit(14),
            Unit(pitch),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [_, bars] = text.cols([Fr(1), Unit(148)], Unit(0), at);
    let [_, elevation_cells, _, chart, _] = content.rows(
        [Unit(8), Unit(2 * pitch), Unit(16), Fr(1), Unit(48)],
        Unit(0),
        at,
    );
    let chart = chart.with_columns(text);
    // The baseline, the row under the fill, ends 2 px above the done cue's disc.
    let clear = at.panel.height - 3 - crate::screens::cue::reach(at.panel.width - chart.right);
    let chart = Rect {
        bottom: if clear < chart.bottom {
            clear
        } else {
            chart.bottom
        },
        ..chart
    };
    assert!(
        chart.width() <= super::elevation::COLUMNS
            && chart.height() <= super::elevation::MAX_HEIGHT,
        "the elevation chart has more columns or rows than it keeps"
    );
    let [_, header, first, second, third, fourth, _, best_cells, _] = content.rows(
        [
            Unit(8),
            Unit(12),
            Unit(36),
            Unit(38),
            Unit(38),
            Unit(38),
            Unit(18),
            Unit(pitch),
            Fr(1),
        ],
        Unit(0),
        at,
    );
    let [label, _] = text.cols([Unit(EFFORT_LABEL_WIDTH), Fr(1)], Unit(0), at);
    let after_label = Rect {
        left: label.right,
        ..text
    };
    let [_, bar, _, value] = after_label.cols([Unit(6), Unit(72), Unit(8), Fr(1)], Unit(0), at);
    let [_, best, ftp, now, _, _] = content.rows(
        [Fr(1), Unit(80), Unit(60), Unit(30), Unit(126), Fr(1)],
        Unit(0),
        at,
    );
    let efforts = Rect {
        top: first.top,
        bottom: fourth.bottom,
        ..text
    };
    Regions {
        text,
        cells: Cells {
            columns: [(left.left, left.right), (right.left, right.right)],
            caption: at.unit(12),
            value: at.unit(41),
            pitch: at.unit(super::CELL_PITCH),
        },
        badge: Badge {
            cx: (text.left + text.right) / 2,
            cy: (badge.top + badge.bottom) / 2,
            radius: badge.height() / 2,
            icon: Icon::Check,
        },
        message: Message {
            title: title.with_columns(text),
            reason: reason.with_columns(text),
            note: note.with_columns(text),
        },
        overview: Overview {
            saved: saved.with_columns(text),
            failed: failed.with_columns(text),
            cause: cause.with_columns(text),
            title: overview_title.with_columns(text),
            cells: overview_cells.with_columns(text),
        },
        effort: Effort {
            cells: effort_cells.with_columns(text),
            power_bar: power_bar.with_columns(bars),
            heart_bar: heart_bar.with_columns(bars),
            recovery: recovery.with_columns(text),
        },
        elevation: Elevation {
            cells: elevation_cells.with_columns(text),
            chart,
        },
        best: BestPower {
            header: header.with_columns(text),
            efforts: [
                first.with_columns(text),
                second.with_columns(text),
                third.with_columns(text),
                fourth.with_columns(text),
            ],
            label: efforts.with_columns(label),
            bar: efforts.with_columns(bar),
            value: efforts.with_columns(value),
            cells: best_cells.with_columns(text),
        },
        offer: [
            best.with_columns(text),
            ftp.with_columns(text),
            now.with_columns(text),
        ],
    }
}

/// The best efforts' label column, before it widens for a step's faces.
const EFFORT_LABEL_WIDTH: u16 = 56;
const COLUMN_GAP: u16 = 8;
const BAR_HEIGHT: u16 = 8;

/// A caption over its value in a column of the band whose caption top is
/// `top`, as on the ride pages. A new personal best says `BEST` in its
/// caption, in the reading's colour.
fn cell(canvas: &mut Canvas, layout: &Layout, column: usize, top: usize, reading: &Reading) {
    let (at, cells) = (&layout.scaled, &layout.summary.cells);
    let (left, right) = cells.columns[column];
    let (caption, caption_color) = match reading.best {
        Some(caption) => (caption, reading.color),
        None => (reading.caption, SECONDARY),
    };
    canvas.fit(
        &[at.face(FaceRole::Caption)],
        caption,
        right - left,
        left,
        top + cells.caption,
        Align::Left,
        caption_color,
    );
    canvas.fit(
        &[at.face(FaceRole::Value), at.face(FaceRole::Body)],
        reading.value.as_bytes(),
        right - left,
        left,
        top + cells.value,
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

fn cells(canvas: &mut Canvas, layout: &Layout, rows: Rect, cells: &[Reading]) {
    let pitch = layout.summary.cells.pitch;
    for (index, reading) in cells.iter().enumerate() {
        let top = rows.top + index / 2 * pitch;
        cell(canvas, layout, index % 2, top, reading);
    }
}

pub fn prepare(view: &View) -> Canvas {
    let layout = view.layout;
    let (at, regions) = (&layout.scaled, &layout.summary);
    let after = view.after();
    if delete_shown(after) {
        let mut canvas = super::end::question(view, b"DELETE RIDE?", b"IT GOES FROM YOUR RIDES");
        let title = view.past_title().as_bytes();
        let row = layout.end.name;
        let centre = (row.left + row.right) / 2;
        canvas.caption(at, title, row.width(), centre, row.bottom, Align::Center);
        return canvas;
    }
    let mut canvas = Canvas::default();
    let (title, body, caption) = (
        at.face(FaceRole::Title),
        at.face(FaceRole::Body),
        at.face(FaceRole::Caption),
    );
    let message = &regions.message;
    match after.saving {
        Saving::Writing => {
            centred(&mut canvas, &[title, body], b"SAVING", message.title, WHITE);
            let note = b"KEEP VANA ON";
            centred(&mut canvas, &[caption], note, message.note, SECONDARY);
            return canvas;
        }
        Saving::Reading => {
            centred(
                &mut canvas,
                &[title, body],
                b"READING",
                message.title,
                WHITE,
            );
            let note = b"ONE MOMENT";
            centred(&mut canvas, &[caption], note, message.note, SECONDARY);
            return canvas;
        }
        Saving::Unreadable => {
            let unreadable = b"RIDE UNREADABLE";
            centred(&mut canvas, &[body], unreadable, message.title, AMBER);
            let reason = b"ITS RECORD IS DAMAGED";
            centred(&mut canvas, &[caption], reason, message.reason, SECONDARY);
            return canvas;
        }
        Saving::Failed if after.page == 0 => {
            let overview = &regions.overview;
            let failed = b"NOT ALL SAVED";
            centred(&mut canvas, &[title, body], failed, overview.failed, AMBER);
            let cause = b"STORAGE FAILED AT THE END";
            centred(&mut canvas, &[caption], cause, overview.cause, SECONDARY);
        }
        _ => {}
    }
    match after.page {
        0 => overview(view, layout, &mut canvas),
        1 => effort(view, layout, &mut canvas),
        _ if offer_shown(after) => ftp_offer(view, layout, &mut canvas),
        2 if after.elevation => elevation_page(view, layout, &mut canvas),
        _ => best_power(view, layout, &mut canvas),
    }
    canvas
}

/// The badge shows over a saved ride's first page.
pub fn badge(view: &View) -> Option<Badge> {
    let after = view.after();
    (after.saving == Saving::Saved && after.page == 0 && !after.past)
        .then_some(view.layout.summary.badge)
}

fn overview(view: &View, layout: &Layout, canvas: &mut Canvas) {
    let (at, regions) = (&layout.scaled, &layout.summary.overview);
    let totals = &view.totals;
    let after = view.after();
    let faces = [at.face(FaceRole::Title), at.face(FaceRole::Body)];
    if after.past {
        let saved = regions.saved;
        if after.saving == Saving::Unsynced {
            let caption = [at.face(FaceRole::Caption)];
            centred(canvas, &caption, b"SYNC TO SAVE", saved, AMBER);
        } else {
            let centre = (saved.left + saved.right) / 2;
            let text = b"SAVED RIDE";
            canvas.caption(at, text, saved.width(), centre, saved.bottom, Align::Center);
        }
        let title = view.past_title().as_bytes();
        centred(canvas, &faces, title, regions.title, WHITE);
    } else if after.saving == Saving::Saved {
        centred(canvas, &faces, b"RIDE SAVED", regions.title, WHITE);
    } else if after.saving == Saving::Unsynced {
        centred(canvas, &faces, b"SYNC TO SAVE", regions.title, AMBER);
    }
    let mut values: [Text<12>; 6] = core::array::from_fn(|_| Text::new());
    let [time, distance, avg_speed, ascent, power, heart] = &mut values;
    elapsed(time, view.active_secs);
    kilometres(distance, totals.distance_mm());
    speed(avg_speed, totals.avg_speed_mm_s());
    whole(ascent, Some((totals.ascent_dm() + 5) / 10));
    whole(power, totals.avg_power());
    whole(heart, totals.avg_heart_bpm());
    let found = view.after().found;
    cells(
        canvas,
        layout,
        regions.cells,
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

fn effort(view: &View, layout: &Layout, canvas: &mut Canvas) {
    let regions = &layout.summary.effort;
    let totals = &view.totals;
    let mut values: [Text<12>; 6] = core::array::from_fn(|_| Text::new());
    let [np, max, work, kcal, heart, max_heart] = &mut values;
    whole(np, totals.normalized_power());
    whole(max, totals.max_power());
    whole(work, totals.work_kj());
    whole(kcal, totals.kcal_for(view.rider));
    whole(heart, totals.avg_heart_bpm());
    whole(max_heart, totals.max_heart_bpm());
    cells(
        canvas,
        layout,
        regions.cells,
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
    let left = layout.summary.text.left;
    zone_bar(
        canvas,
        layout,
        b"PWR",
        left,
        regions.power_bar,
        marks.power_time,
    );
    zone_bar(
        canvas,
        layout,
        b"HR",
        left,
        regions.heart_bar,
        marks.heart_time,
    );
    if let Some(reading) = view.after().recovery {
        recovery(canvas, layout, reading);
    }
}

fn recovery(canvas: &mut Canvas, layout: &Layout, reading: Recovery) {
    let (at, cells) = (&layout.scaled, &layout.summary.cells);
    let row = layout.summary.effort.recovery;
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
    canvas.caption(
        at,
        b"HR RECOVERY",
        row.width(),
        row.left,
        row.top + cells.caption,
        Align::Left,
    );
    canvas.fit(
        &[at.face(FaceRole::Body)],
        text.as_bytes(),
        row.width(),
        row.left,
        row.top + cells.value,
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
    let after = view.after();
    after.elevation
        && SummaryPage::at(after.page, true) == SummaryPage::Elevation
        && matches!(
            after.saving,
            Saving::Saved | Saving::Unsynced | Saving::Failed
        )
}

fn elevation_page(view: &View, layout: &Layout, canvas: &mut Canvas) {
    let totals = &view.totals;
    let mut values: [Text<12>; 4] = core::array::from_fn(|_| Text::new());
    let [ascent, descent, low, high] = &mut values;
    whole(ascent, Some((totals.ascent_dm() + 5) / 10));
    whole(descent, Some((totals.descent_dm() + 5) / 10));
    let range = view.elevation_chart().and_then(Chart::range_m);
    whole(low, range.map(|(low, _)| low));
    whole(high, range.map(|(_, high)| high));
    cells(
        canvas,
        layout,
        layout.summary.elevation.cells,
        &[
            reading(b"ASCENT M", ascent, GRADE),
            reading(b"DESCENT M", descent, GRADE),
            reading(b"MIN M", low, GRADE),
            reading(b"MAX M", high, GRADE),
        ],
    );
}

fn best_power(view: &View, layout: &Layout, canvas: &mut Canvas) {
    let (at, regions) = (&layout.scaled, &layout.summary.best);
    let header = regions.header;
    let text = b"BEST POWER W";
    canvas.caption(
        at,
        text,
        header.width(),
        header.left,
        header.bottom,
        Align::Left,
    );
    let after = view.after();
    let peak = after.best.iter().flatten().max().copied().unwrap_or(0);
    let (label_column, bar_column, value_column) = (regions.label, regions.bar, regions.value);
    let longest = bar_column.width();
    for ((index, best), row) in after.best.into_iter().enumerate().zip(regions.efforts) {
        let base = row.bottom;
        let label = effort_label(index);
        canvas.caption(
            at,
            label.as_bytes(),
            label_column.width(),
            label_column.left,
            base - at.unit(4),
            Align::Left,
        );
        let mut value = Text::<12>::new();
        whole(&mut value, best);
        canvas.fit(
            &[at.face(FaceRole::Value), at.face(FaceRole::Body)],
            value.as_bytes(),
            value_column.width(),
            value_column.right,
            base,
            Align::Right,
            if best.is_some() { POWER } else { TERTIARY },
        );
        let length = match best {
            Some(watts) if peak > 0 => {
                (usize::from(watts) * longest / usize::from(peak)).max(at.unit(4))
            }
            _ => longest,
        };
        canvas.shape(Shape::Bar {
            rect: Rect {
                top: base - at.unit(12),
                right: bar_column.left + length,
                bottom: base - at.unit(4),
                ..bar_column
            },
            radius: at.unit(2),
            color: if best.is_some() { POWER } else { TERTIARY },
        });
        // A new personal best: its own caption under the bar.
        if after.found.power[index] {
            canvas.fit(
                &[at.face(FaceRole::Caption)],
                b"BEST",
                at.unit(BEST_WIDTH),
                bar_column.left,
                base + at.unit(11),
                Align::Left,
                POWER,
            );
        }
    }
    let mut climbs = Text::<12>::new();
    let mut laps = Text::<12>::new();
    whole(&mut climbs, after.climbs);
    whole(&mut laps, after.laps);
    cells(
        canvas,
        layout,
        regions.cells,
        &[
            reading(b"CLIMBS", &climbs, GRADE),
            reading(b"LAPS", &laps, WHITE),
        ],
    );
}

/// `FTP 245 W?`: what the new 20 min best suggests, and what it replaces.
fn ftp_offer(view: &View, layout: &Layout, canvas: &mut Canvas) {
    let Some(offer) = view.after().offer else {
        return;
    };
    let at = &layout.scaled;
    let [best_row, ftp_row, now_row] = layout.summary.offer;
    let (body, caption) = (at.face(FaceRole::Body), at.face(FaceRole::Caption));
    let mut best = Text::<24>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut best,
        format_args!("20 MIN BEST {} W", offer.best_watts),
    );
    centred(
        canvas,
        &[body, caption],
        best.as_bytes(),
        best_row,
        SECONDARY,
    );
    let mut ftp = Text::<12>::new();
    let _ = core::fmt::Write::write_fmt(&mut ftp, format_args!("FTP {} W?", offer.suggested_watts));
    let title = at.face(FaceRole::Title);
    centred(canvas, &[title, body], ftp.as_bytes(), ftp_row, POWER);
    let mut now = Text::<24>::new();
    let _ = core::fmt::Write::write_fmt(&mut now, format_args!("NOW {} W", offer.current_watts));
    centred(canvas, &[body, caption], now.as_bytes(), now_row, SECONDARY);
}

fn centred(canvas: &mut Canvas, faces: &[&'static Face], text: &[u8], row: Rect, color: u16) {
    let centre = (row.left + row.right) / 2;
    canvas.fit(
        faces,
        text,
        row.width(),
        centre,
        row.bottom,
        Align::Center,
        color,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::Sample;
    use crate::screens::chrome::{CONTENT, HEIGHT, WIDTH};
    use crate::screens::cue::RADIUS;
    use crate::screens::workout::Page;

    const REGIONS: Regions = crate::screens::BASE.summary;

    fn extreme(page: usize, saving: Saving) -> View<'static> {
        extreme_on(&crate::screens::BASE, page, saving)
    }

    fn extreme_on(layout: &'static Layout, page: usize, saving: Saving) -> View<'static> {
        let mut view = View::new(Page::Summary, 0, None);
        view.layout = layout;
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
        view.show_summary(
            After {
                page,
                saving,
                best: [Some(1999), Some(1500), None, None],
                climbs: Some(999),
                laps: Some(999),
                past: false,
                send: false,
                delete: false,
                elevation: false,
                found: Found::default(),
                offer: None,
                recovery: Some(Recovery::Recovered(120)),
            },
            Text::new(),
        );
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
                view.canvas().assert_laid_out(bounds, want, &what);
                for run in &view.canvas().runs {
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
        assert!(saved.canvas().shows("RIDE SAVED"));
        assert!(badge(&saved).is_some());
        let unsynced = extreme(0, Saving::Unsynced);
        let sync = unsynced.canvas().find("SYNC TO SAVE").map(|run| run.color);
        assert_eq!(sync, Some(AMBER), "a protected ride is not called saved");
        assert!(!unsynced.canvas().shows("RIDE SAVED") && badge(&unsynced).is_none());
        assert!(extreme(0, Saving::Failed).canvas().shows("NOT ALL SAVED"));
        let saving = extreme(0, Saving::Writing);
        assert!(saving.canvas().shows("SAVING") && badge(&saving).is_none());
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
        let rect = REGIONS.elevation.chart;
        chart.refresh(&trace, rect);
        let with_altitude = |page| {
            let mut view = extreme(page, Saving::Saved);
            view.set_elevation_chart(&chart);
            view.after_mut().elevation = true;
            view.prepare();
            view
        };
        let mut view = with_altitude(2);
        let want = ["ASCENT M", "DESCENT M", "MIN M", "MAX M", "-12", "186"];
        view.canvas().assert_laid_out(bounds, &want, "elevation");
        assert!(shows_elevation(&view));
        view.prepare().check_layout();
        let fill = crate::screens::ride::elevation::pixel(&chart, rect, WIDTH / 2, rect.bottom - 1);
        assert!(fill.is_some(), "the chart fills above its baseline");
        assert!(with_altitude(3).canvas().shows("BEST POWER W"));
        assert!(!with_altitude(1).canvas().shows("ASCENT M"));
        assert!(!shows_elevation(&with_altitude(1)));
        let mut writing = extreme(2, Saving::Writing);
        writing.after_mut().elevation = true;
        assert!(!shows_elevation(&writing));
        let plain = extreme(2, Saving::Saved);
        assert!(plain.canvas().shows("BEST POWER W") && !shows_elevation(&plain));
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
            view.after_mut().recovery = recovery;
            view.prepare();
            view
        };
        // 1999 W for a minute is 120 kJ, so 120 kcal.
        let measuring = with(Some(Recovery::Measuring(42)));
        let want = ["KCAL", "120", "HR RECOVERY", "MEASURING 42 S"];
        measuring
            .canvas()
            .assert_laid_out(bounds, &want, "measuring");
        let fell = with(Some(Recovery::Recovered(28)));
        fell.canvas().assert_laid_out(bounds, &["-28 BPM"], "fell");
        assert!(with(Some(Recovery::Recovered(-3))).canvas().shows("+3 BPM"));
        assert!(!with(None).canvas().shows("HR RECOVERY"));
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
            let after = *view.after();
            let mut title = Text::new();
            let _ = title.push_str("DEC 31  12:59");
            view.show_summary(
                After {
                    past: true,
                    climbs: None,
                    laps: None,
                    ..after
                },
                title,
            );
            view.prepare();
            view
        };
        let protected = past(0, Saving::Unsynced);
        assert!(protected.canvas().shows("SYNC TO SAVE"));
        assert!(!protected.canvas().shows("SAVED RIDE"));
        let first = past(0, Saving::Saved);
        let want = ["SAVED RIDE", "DEC 31  12:59", "99:59:59"];
        first.canvas().assert_laid_out(bounds, &want, "past ride");
        assert!(!first.canvas().shows("RIDE SAVED") && badge(&first).is_none());
        assert!(past(2, Saving::Saved).canvas().shows("--"));
        let reading = past(0, Saving::Reading);
        assert!(reading.canvas().shows("READING"));
        let missing = past(0, Saving::Unreadable);
        missing.canvas().assert_laid_out(
            bounds,
            &["RIDE UNREADABLE", "ITS RECORD IS DAMAGED"],
            "unreadable",
        );
    }

    #[test]
    fn best_power_bars_scale_to_the_highest_effort() {
        let view = extreme(2, Saving::Saved);
        let bars: std::vec::Vec<_> = view
            .canvas()
            .shapes
            .iter()
            .filter_map(|shape| match shape {
                Shape::Bar { rect, color, .. } => Some((rect.width(), *color)),
                _ => None,
            })
            .collect();
        let longest = REGIONS.best.bar.width();
        assert_eq!(bars[0], (longest, POWER));
        assert_eq!(bars[1], (1500 * longest / 1999, POWER));
        assert_eq!(bars[2], (longest, TERTIARY), "no 5 min effort yet");
    }

    #[test]
    fn every_page_lays_out_on_every_panel() {
        let mut trace = crate::ride::elevation::Trace::default();
        for metres in (0..4000).step_by(5) {
            trace.add(metres * 1000, Some(-120 + (metres as i32 % 2000)));
        }
        let offer = crate::workout::ftp::Offer {
            suggested_watts: 1999,
            current_watts: 1999,
            best_watts: 1999,
        };
        type Case = (usize, Saving, fn(&mut After), &'static [&'static str]);
        let cases: [Case; 16] = [
            (
                0,
                Saving::Saved,
                |_| {},
                &["RIDE SAVED", "99:59:59", "1000.0", "220"],
            ),
            (
                0,
                Saving::Unsynced,
                |_| {},
                &["SYNC TO SAVE", "99:59:59", "1000.0", "220"],
            ),
            (
                0,
                Saving::Unsynced,
                |after| {
                    after.past = true;
                    after.page = 0;
                },
                &["SYNC TO SAVE", "DEC 31  12:59", "99:59:59"],
            ),
            (
                2,
                Saving::Unsynced,
                |after| after.elevation = true,
                &["ASCENT M", "MAX M"],
            ),
            (0, Saving::Failed, |_| {}, &["NOT ALL SAVED", "TIME"]),
            (0, Saving::Writing, |_| {}, &["SAVING", "KEEP VANA ON"]),
            (0, Saving::Reading, |_| {}, &["READING", "ONE MOMENT"]),
            (0, Saving::Unreadable, |_| {}, &["RIDE UNREADABLE"]),
            (1, Saving::Saved, |_| {}, &["NP W", "PWR", "HR RECOVERY"]),
            (
                1,
                Saving::Saved,
                |after| after.recovery = Some(Recovery::Measuring(59)),
                &["MEASURING 59 S"],
            ),
            (2, Saving::Saved, |_| {}, &["20 MIN", "1999", "1500", "999"]),
            (
                2,
                Saving::Saved,
                |after| after.found.power = [true; 4],
                &["BEST", "CLIMBS"],
            ),
            (
                2,
                Saving::Saved,
                |after| after.elevation = true,
                &["ASCENT M", "MAX M"],
            ),
            (
                3,
                Saving::Saved,
                |after| {
                    after.found.time = true;
                    after.found.distance = true;
                    after.found.ascent = true;
                    after.past = true;
                    after.page = 0;
                },
                &["SAVED RIDE", "BEST TIME", "BEST KM"],
            ),
            (
                3,
                Saving::Saved,
                |_| {},
                &["20 MIN BEST 1999 W", "FTP 1999 W?"],
            ),
            (
                0,
                Saving::Saved,
                |after| {
                    after.past = true;
                    after.delete = true;
                    after.page = pages(after.elevation);
                },
                &[
                    "DELETE RIDE?",
                    "DEC 31  12:59",
                    "99:59:59",
                    "1000.0 KM",
                    "IT GOES FROM YOUR RIDES",
                ],
            ),
        ];
        for layout in crate::screens::panels::ALL {
            let mut chart = Chart::default();
            chart.refresh(&trace, layout.summary.elevation.chart);
            for (page, saving, change, want) in cases {
                let mut view = extreme_on(layout, page, saving);
                let after = view.after_mut();
                if page == 3 && !after.past {
                    after.offer = Some(offer);
                }
                change(after);
                if after.past {
                    view.page = Page::PastRide;
                    let mut title = Text::new();
                    let _ = title.push_str("DEC 31  12:59");
                    let after = *view.after();
                    view.show_summary(after, title);
                }
                if view.after().elevation {
                    view.set_elevation_chart(&chart);
                }
                let what = std::format!("page {page} {saving:?} on {:?}", layout.scaled.panel);
                view.prepare().check_layout();
                let canvas = view.canvas();
                for text in want {
                    assert!(canvas.shows(text), "{what}: {text}");
                }
            }
        }
    }
}
