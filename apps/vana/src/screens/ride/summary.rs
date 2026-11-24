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
use super::elevation::Chart;
use super::stats::zone_bar;
use super::{elapsed, kilometres, speed, whole};
use crate::compose::tile::{Form, Forms, forms};
use crate::ride::recovery::Reading as Recovery;
use crate::ride::rider::Rider;
use crate::ride::totals::{EFFORT_SECONDS, Totals};
use crate::ride::zones::{HEART_ZONES, POWER_ZONES};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{
    AMBER, Align, GRADE, HEART, POWER, Rect, SECONDARY, SPEED, TERTIARY, WHITE,
};
use crate::screens::cue::{Badge, Icon};
use crate::screens::scaled::Scaled;
use crate::screens::workout::{Text, View};
use firmware_shell::rendering::font::{Face, FaceRole};

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
    /// The ride has an elevation page.
    pub elevation: bool,
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
        elevation: false,
        offer: None,
        recovery: None,
    };
}

pub fn total_pages(after: &After) -> usize {
    pages(after.elevation) + usize::from(after.offer.is_some())
}

/// Whether the FTP offer, the page after the others, is on screen.
pub fn offer_shown(after: &After) -> bool {
    after.page == pages(after.elevation) && after.offer.is_some()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shown {
    Message,
    Overview,
    Effort,
    Elevation,
    Best,
    Offer,
}

impl Shown {
    pub fn of(after: &After) -> Self {
        if after.saving == Saving::Writing {
            return Self::Message;
        }
        match after.page {
            0 => Self::Overview,
            1 => Self::Effort,
            _ if offer_shown(after) => Self::Offer,
            2 if after.elevation => Self::Elevation,
            _ => Self::Best,
        }
    }
}

/// Whether the page is a question with a back cue, shown without the bar.
pub fn question_shown(after: &After) -> bool {
    offer_shown(after)
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

/// What a summary tile reads: the ride's totals and what the summary shows
/// beyond them.
#[derive(Clone, Copy)]
pub struct Facts<'a> {
    pub(crate) after: &'a After,
    pub(crate) totals: &'a Totals,
    pub(crate) active_secs: u64,
    pub(crate) rider: Rider,
    pub(crate) power_time: Option<[usize; POWER_ZONES]>,
    pub(crate) heart_time: Option<[usize; HEART_ZONES]>,
    /// The lowest and highest altitude the profile draws, in whole metres.
    pub(crate) range_m: Option<(i32, i32)>,
}

/// The facts of the summary `view` shows.
pub(crate) fn facts<'v>(view: &'v View) -> Facts<'v> {
    let marks = &view.marks;
    Facts {
        after: view.after(),
        totals: &view.totals,
        active_secs: view.active_secs,
        rider: view.rider,
        power_time: marks.power_time,
        heart_time: marks.heart_time,
        range_m: view.elevation_chart().and_then(Chart::range_m),
    }
}

/// A number the summary shows in a cell: a caption over its value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stat {
    Time,
    Distance,
    AvgSpeed,
    Ascent,
    AvgPower,
    AvgHeart,
    Np,
    MaxPower,
    Work,
    Kcal,
    MaxHeart,
    Descent,
    Lowest,
    Highest,
    Climbs,
    Laps,
    /// The heart rate recovery after the finish, across the page in the
    /// body face; nothing until it is measured.
    Recovery,
}

impl Stat {
    pub const ALL: [Self; 17] = [
        Self::Time,
        Self::Distance,
        Self::AvgSpeed,
        Self::Ascent,
        Self::AvgPower,
        Self::AvgHeart,
        Self::Np,
        Self::MaxPower,
        Self::Work,
        Self::Kcal,
        Self::MaxHeart,
        Self::Descent,
        Self::Lowest,
        Self::Highest,
        Self::Climbs,
        Self::Laps,
        Self::Recovery,
    ];

    pub const fn caption(self) -> &'static [u8] {
        match self {
            Self::Time => b"TIME",
            Self::Distance => b"DIST KM",
            Self::AvgSpeed => b"AVG KM/H",
            Self::Ascent => b"ASCENT M",
            Self::AvgPower => b"AVG W",
            Self::AvgHeart => b"AVG BPM",
            Self::Np => b"NP W",
            Self::MaxPower => b"MAX W",
            Self::Work => b"WORK KJ",
            Self::Kcal => b"KCAL",
            Self::MaxHeart => b"MAX BPM",
            Self::Descent => b"DESCENT M",
            Self::Lowest => b"MIN M",
            Self::Highest => b"MAX M",
            Self::Climbs => b"CLIMBS",
            Self::Laps => b"LAPS",
            Self::Recovery => b"HR RECOVERY",
        }
    }

    const fn color(self) -> u16 {
        match self {
            Self::Time | Self::Distance | Self::Work | Self::Kcal | Self::Laps => WHITE,
            Self::AvgSpeed => SPEED,
            Self::Ascent | Self::Descent | Self::Lowest | Self::Highest | Self::Climbs => GRADE,
            Self::AvgPower | Self::Np | Self::MaxPower => POWER,
            Self::AvgHeart | Self::MaxHeart | Self::Recovery => HEART,
        }
    }

    /// The value, and its colour where the stat's own does not apply.
    fn value(self, facts: &Facts, value: &mut Text<16>) -> Option<u16> {
        let totals = facts.totals;
        let mut text = Text::<12>::new();
        let range = facts.range_m;
        match self {
            Self::Time => elapsed(&mut text, facts.active_secs),
            Self::Distance => kilometres(&mut text, totals.distance_mm()),
            Self::AvgSpeed => speed(&mut text, totals.avg_speed_mm_s()),
            Self::Ascent => whole(&mut text, Some((totals.ascent_dm() + 5) / 10)),
            Self::AvgPower => whole(&mut text, totals.avg_power()),
            Self::AvgHeart => whole(&mut text, totals.avg_heart_bpm()),
            Self::Np => whole(&mut text, totals.normalized_power()),
            Self::MaxPower => whole(&mut text, totals.max_power()),
            Self::Work => whole(&mut text, totals.work_kj()),
            Self::Kcal => whole(&mut text, totals.kcal_for(facts.rider)),
            Self::MaxHeart => whole(&mut text, totals.max_heart_bpm()),
            Self::Descent => whole(&mut text, Some((totals.descent_dm() + 5) / 10)),
            Self::Lowest => whole(&mut text, range.map(|(low, _)| low)),
            Self::Highest => whole(&mut text, range.map(|(_, high)| high)),
            Self::Climbs => whole(&mut text, facts.after.climbs),
            Self::Laps => whole(&mut text, facts.after.laps),
            Self::Recovery => {
                let reading = facts.after.recovery?;
                return Some(recovery(reading, value));
            }
        }
        let _ = value.push_str(text.as_str());
        None
    }
}

/// `MEASURING 42 S`, or the fall in beats a minute, and its colour.
fn recovery(reading: Recovery, text: &mut Text<16>) -> u16 {
    match reading {
        Recovery::Measuring(secs) => {
            let _ = core::fmt::Write::write_fmt(text, format_args!("MEASURING {secs} S"));
            SECONDARY
        }
        // `drop` counts the beats lost, so a fall prints as a minus.
        Recovery::Recovered(drop) => {
            let _ = core::fmt::Write::write_fmt(
                text,
                format_args!(
                    "{}{} BPM",
                    if drop > 0 { "-" } else { "+" },
                    drop.unsigned_abs()
                ),
            );
            HEART
        }
    }
}

/// A cell's caption baseline and value baseline, below its top.
const CAPTION_UNITS: usize = 12;
const VALUE_UNITS: usize = 41;
/// The widest value any cell shows, and the widest recovery.
const WIDEST: &[u8] = b"99:59:59";
const WIDEST_RECOVERY: &[u8] = b"MEASURING 59 S";

/// A cell's looks: every cell has the same, so a row of them splits evenly.
/// Its value takes the value face, or the body face where that is all
/// there is room for.
pub fn cell_forms(stat: Stat, at: &Scaled) -> Forms {
    let height = at.unit(VALUE_UNITS) as u16;
    let caption = at.face(FaceRole::Caption).fit_width(b"DESCENT M");
    let form = |role: FaceRole, widest: &[u8]| Form {
        width: at.face(role).fit_width(widest).max(caption) as u16,
        height,
        shapes: 0,
        runs: 2,
    };
    match stat {
        Stat::Recovery => forms(&[form(FaceRole::Body, WIDEST_RECOVERY)]),
        _ => forms(&[form(FaceRole::Value, WIDEST), form(FaceRole::Body, WIDEST)]),
    }
}

pub fn cell(stat: Stat, rect: Rect, at: &Scaled, facts: &Facts, out: &mut Canvas) {
    let mut value = Text::<16>::new();
    let color = match stat.value(facts, &mut value) {
        Some(color) => color,
        None if stat == Stat::Recovery => return,
        None => stat.color(),
    };
    let width = rect.width();
    let caption_face = at.face(FaceRole::Caption);
    let caption_base = rect.top + at.unit(CAPTION_UNITS);
    let base = rect.top + at.unit(VALUE_UNITS);
    out.fit(
        &[caption_face],
        stat.caption(),
        width,
        rect.left,
        caption_base,
        Align::Left,
        SECONDARY,
    );
    let faces = match stat {
        Stat::Recovery => &[at.face(FaceRole::Body)][..],
        _ => &[at.face(FaceRole::Value), at.face(FaceRole::Body)],
    };
    out.fit(
        faces,
        value.as_bytes(),
        width,
        rect.left,
        base,
        Align::Left,
        color,
    );
}

/// The caption beside a time-in-zone bar, and the bar's length and height.
const ZONE_CAPTION_UNITS: usize = 10;
const ZONE_BAR_UNITS: usize = 148;
const BAR_HEIGHT: usize = 8;

pub fn zone_time_forms(at: &Scaled) -> Forms {
    let caption = at.face(FaceRole::Caption).fit_width(b"PWR");
    forms(&[Form {
        width: (caption + at.unit(8 + ZONE_BAR_UNITS)) as u16,
        height: at.unit(ZONE_CAPTION_UNITS) as u16,
        shapes: POWER_ZONES as u8,
        runs: 1,
    }])
}

/// "PWR" or "HR" at the left of `rect` and the time in each zone as a bar
/// at its right.
pub fn zone_time(heart: bool, rect: Rect, at: &Scaled, facts: &Facts, out: &mut Canvas) {
    let bar = Rect {
        left: rect.right - at.unit(ZONE_BAR_UNITS),
        bottom: rect.top + at.unit(BAR_HEIGHT),
        ..rect
    };
    if heart {
        zone_bar(out, at, b"HR", rect.left, bar, facts.heart_time);
    } else {
        zone_bar(out, at, b"PWR", rect.left, bar, facts.power_time);
    }
}

const EFFORT_LABEL_WIDTH: usize = 56;
const EFFORT_BAR_UNITS: usize = 72;

pub fn effort_forms(at: &Scaled) -> Forms {
    let value = |role| at.face(role).fit_width(b"9999");
    let form = |role| Form {
        width: (at.unit(EFFORT_LABEL_WIDTH + 6 + EFFORT_BAR_UNITS + 8) + value(role)) as u16,
        height: at.face(FaceRole::Value).fit_height() as u16,
        shapes: 1,
        runs: 3,
    };
    forms(&[form(FaceRole::Value), form(FaceRole::Body)])
}

pub fn effort(index: usize, rect: Rect, at: &Scaled, facts: &Facts, out: &mut Canvas) {
    let after = facts.after;
    let best = after.best[index];
    let peak = after.best.iter().flatten().max().copied().unwrap_or(0);
    let base = rect.top + at.face(FaceRole::Value).fit_height();
    let label = effort_label(index);
    let label_width = at.unit(EFFORT_LABEL_WIDTH);
    let below = base - at.unit(4);
    out.caption(
        at,
        label.as_bytes(),
        label_width,
        rect.left,
        below,
        Align::Left,
    );
    let bar_left = rect.left + label_width + at.unit(6);
    let longest = at.unit(EFFORT_BAR_UNITS);
    let value_left = bar_left + longest + at.unit(8);
    let mut value = Text::<12>::new();
    whole(&mut value, best);
    let color = if best.is_some() { POWER } else { TERTIARY };
    let faces = [at.face(FaceRole::Value), at.face(FaceRole::Body)];
    let width = rect.right - value_left;
    out.fit(
        &faces,
        value.as_bytes(),
        width,
        rect.right,
        base,
        Align::Right,
        color,
    );
    let length = match best {
        Some(watts) if peak > 0 => {
            (usize::from(watts) * longest / usize::from(peak)).max(at.unit(4))
        }
        _ => longest,
    };
    out.shape(Shape::Bar {
        rect: Rect {
            left: bar_left,
            top: base - at.unit(12),
            right: bar_left + length,
            bottom: below,
        },
        radius: at.unit(2),
        color,
    });
}

/// The summary's words: each a block of centred lines.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Words {
    Message,
    Heading,
    /// BEST POWER W, over the best efforts.
    BestPower,
    /// `FTP 245 W?`: what the new 20 min best suggests, and what it replaces.
    Offer,
}

impl Words {
    pub const ALL: [Self; 4] = [Self::Message, Self::Heading, Self::BestPower, Self::Offer];
}

const CAUSE_UNITS: usize = 22;
/// The badge over RIDE SAVED.
const BADGE_UNITS: usize = 40;
const HEADING_UNITS: usize = 74;
/// The heading's lines inside its row: the text margins less the row's.
const HEADING_INSET_UNITS: usize = 8;
/// A message's note below its title.
const NOTE_UNITS: usize = 28;
/// The FTP offer's lines, each below the one before.
const OFFER_FTP_UNITS: usize = 60;
const OFFER_NOW_UNITS: usize = 30;

pub fn words_forms(words: Words, at: &Scaled) -> Forms {
    let face = |role| at.face(role);
    let wide = |role: FaceRole, texts: &[&[u8]]| {
        texts
            .iter()
            .map(|text| face(role).fit_width(text))
            .max()
            .unwrap_or(0)
    };
    let (width, height) = match words {
        Words::Message => (
            wide(FaceRole::Body, &[b"SAVING"]).max(wide(FaceRole::Caption, &[b"KEEP VANA ON"])),
            face(FaceRole::Title).fit_height() + at.unit(NOTE_UNITS),
        ),
        Words::Heading => (
            wide(FaceRole::Body, &[b"NOT ALL SAVED"])
                .max(wide(FaceRole::Caption, &[b"STORAGE FAILED AT THE END"])),
            at.unit(HEADING_UNITS),
        ),
        Words::BestPower => (
            wide(FaceRole::Caption, &[b"BEST POWER W"]),
            face(FaceRole::Caption).fit_height(),
        ),
        Words::Offer => (
            wide(FaceRole::Caption, &[b"20 MIN BEST 9999 W"])
                .max(wide(FaceRole::Body, &[b"FTP 9999 W?"])),
            face(FaceRole::Body).fit_height() + at.unit(OFFER_FTP_UNITS + OFFER_NOW_UNITS),
        ),
    };
    forms(&[Form {
        width: width as u16,
        height: height as u16,
        shapes: 0,
        runs: 5,
    }])
}

/// Where the badge over RIDE SAVED goes in a heading placed at `rect`.
pub fn badge(after: &After, rect: Rect, at: &Scaled) -> Option<Badge> {
    let radius = at.unit(BADGE_UNITS) / 2;
    (after.saving == Saving::Saved).then_some(Badge {
        cx: (rect.left + rect.right) / 2,
        cy: rect.top + radius,
        radius,
        icon: Icon::Check,
    })
}

pub fn words(words: Words, rect: Rect, at: &Scaled, facts: &Facts, out: &mut Canvas) {
    let after = facts.after;
    let face = |role| at.face(role);
    let (title, body, caption) = (
        face(FaceRole::Title),
        face(FaceRole::Body),
        face(FaceRole::Caption),
    );
    // The heading's row reaches past the text margins for its widest
    // caption; its lines keep to them, so its title takes the same face on
    // every panel.
    let width = match words {
        Words::Heading => rect.width() - 2 * at.unit(HEADING_INSET_UNITS),
        _ => rect.width(),
    };
    let centre = (rect.left + rect.right) / 2;
    let mut line = |faces: &[&'static Face], text: &[u8], base: usize, color: u16| {
        out.fit(faces, text, width, centre, base, Align::Center, color);
    };
    match words {
        Words::Message => {
            let base = rect.top + title.fit_height();
            line(&[title, body], b"SAVING", base, WHITE);
            line(
                &[caption],
                b"KEEP VANA ON",
                base + at.unit(NOTE_UNITS),
                SECONDARY,
            );
        }
        Words::Heading => {
            let base = rect.top + at.unit(HEADING_UNITS);
            match after.saving {
                Saving::Saved => line(&[title, body], b"RIDE SAVED", base, WHITE),
                Saving::Unsynced => line(&[title, body], b"SYNC TO SAVE", base, AMBER),
                _ => {
                    let failed = base - at.unit(CAUSE_UNITS);
                    line(&[title, body], b"NOT ALL SAVED", failed, AMBER);
                    line(&[caption], b"STORAGE FAILED AT THE END", base, SECONDARY);
                }
            }
        }
        Words::BestPower => {
            let base = rect.top + caption.fit_height();
            let width = rect.width();
            out.caption(at, b"BEST POWER W", width, rect.left, base, Align::Left);
        }
        Words::Offer => {
            let Some(offer) = after.offer else {
                return;
            };
            let base = rect.top + body.fit_height();
            let mut text = Text::<24>::new();
            let _ = core::fmt::Write::write_fmt(
                &mut text,
                format_args!("20 MIN BEST {} W", offer.best_watts),
            );
            line(&[body, caption], text.as_bytes(), base, SECONDARY);
            let base = base + at.unit(OFFER_FTP_UNITS);
            text = Text::new();
            let _ = core::fmt::Write::write_fmt(
                &mut text,
                format_args!("FTP {} W?", offer.suggested_watts),
            );
            line(&[title, body], text.as_bytes(), base, POWER);
            let base = base + at.unit(OFFER_NOW_UNITS);
            text = Text::new();
            let _ = core::fmt::Write::write_fmt(
                &mut text,
                format_args!("NOW {} W", offer.current_watts),
            );
            line(&[body, caption], text.as_bytes(), base, SECONDARY);
        }
    }
}

/// The profile's chart in a tile placed at `rect`: its fill ends a row
/// above the rect's bottom, on which the baseline runs.
pub const fn chart_rect(rect: Rect) -> Rect {
    Rect {
        bottom: rect.bottom - CHART_BASELINE_ROWS,
        ..rect
    }
}

const CHART_BASELINE_ROWS: usize = 1;

/// Where the elevation page's tree places the chart on `device`'s panel.
pub fn chart_on(device: &crate::screens::Device) -> Rect {
    use crate::compose::Flags;
    use crate::compose::builtin::{Choices, Laid, ScreenId};
    let laid = Laid::new(
        ScreenId::SummaryElevation,
        device,
        Choices::default(),
        Flags::NONE,
        None,
    );
    let profile = laid.and_then(|laid| laid.box_of(crate::compose::registry::PROFILE));
    chart_rect(profile.expect("the elevation page has a profile"))
}
/// The least room, across and down, a profile is worth drawing in.
const PROFILE_UNITS: usize = 48;

pub fn profile_forms(at: &Scaled) -> Forms {
    forms(&[Form {
        width: at.unit(PROFILE_UNITS) as u16,
        height: at.unit(PROFILE_UNITS) as u16,
        shapes: 0,
        runs: 0,
    }])
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::log::Sample;
    use crate::screens::chrome::{CONTENT, HEIGHT, WIDTH};
    const RADIUS: usize = 42;
    use crate::screens::workout::Page;

    use crate::screens::Device;

    fn extreme(page: usize, saving: Saving) -> View<'static> {
        extreme_on(&crate::screens::C606, page, saving)
    }

    fn extreme_on(device: &'static Device, page: usize, saving: Saving) -> View<'static> {
        let mut view = View::new(Page::Summary, 0, None);
        view.device = device;
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
        view.show_summary(After {
            page,
            saving,
            best: [Some(1999), Some(1500), None, None],
            climbs: Some(999),
            laps: Some(999),
            elevation: false,
            offer: None,
            recovery: Some(Recovery::Recovered(120)),
        });
        view.compose_main();
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
        assert!(saved.badge.is_some());
        let unsynced = extreme(0, Saving::Unsynced);
        let sync = unsynced.canvas().find("SYNC TO SAVE").map(|run| run.color);
        assert_eq!(sync, Some(AMBER), "a protected ride is not called saved");
        assert!(!unsynced.canvas().shows("RIDE SAVED") && unsynced.badge.is_none());
        assert!(extreme(0, Saving::Failed).canvas().shows("NOT ALL SAVED"));
        let saving = extreme(0, Saving::Writing);
        assert!(saving.canvas().shows("SAVING") && saving.badge.is_none());
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
        let rect = chart_on(&crate::screens::C606);
        chart.refresh(&trace, rect);
        let with_altitude = |page| {
            let mut view = extreme(page, Saving::Saved);
            view.set_elevation_chart(&chart);
            view.after_mut().elevation = true;
            view.compose_main();
            view.prepare();
            view
        };
        let mut view = with_altitude(2);
        let want = ["ASCENT M", "DESCENT M", "MIN M", "MAX M", "-12", "186"];
        view.canvas().assert_laid_out(bounds, &want, "elevation");
        assert!(shows_elevation(&view));
        view.compose_main();
        view.prepare().check_layout();
        let fill = crate::screens::ride::elevation::pixel(&chart, WIDTH / 2, rect.bottom - 1);
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
            view.compose_main();
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
        let longest = EFFORT_BAR_UNITS;
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
        let cases: [Case; 10] = [
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
                2,
                Saving::Unsynced,
                |after| after.elevation = true,
                &["ASCENT M", "MAX M"],
            ),
            (0, Saving::Failed, |_| {}, &["NOT ALL SAVED", "TIME"]),
            (0, Saving::Writing, |_| {}, &["SAVING", "KEEP VANA ON"]),
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
                |after| after.elevation = true,
                &["ASCENT M", "MAX M"],
            ),
            (
                3,
                Saving::Saved,
                |_| {},
                &["20 MIN BEST 1999 W", "FTP 1999 W?"],
            ),
        ];
        for device in crate::screens::panels::ALL {
            let mut chart = Chart::default();
            chart.refresh(&trace, chart_on(device));
            for (page, saving, change, want) in cases {
                let mut view = extreme_on(device, page, saving);
                let after = view.after_mut();
                if page == 3 {
                    after.offer = Some(offer);
                }
                change(after);
                if view.after().elevation {
                    view.set_elevation_chart(&chart);
                }
                let what = std::format!("page {page} {saving:?} on {:?}", device.scaled.panel);
                view.compose_main();
                view.prepare().check_layout();
                let canvas = view.canvas();
                for text in want {
                    assert!(canvas.shows(text), "{what}: {text}");
                }
            }
        }
    }
}
