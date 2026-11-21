//! VANA workout rendering: the chrome around whichever screen is shown.
use super::Layout;
use super::canvas::Canvas;
use super::chrome::{BLACK, Battery, Gps, Look, Mark, Row, StatusBar, Toast};
#[cfg(test)]
use super::chrome::{CONTENT, HEIGHT, WIDTH};
use super::cue::{Badge, Cue};
use super::home::Home;
use super::ready::Readiness;
use super::ride::summary::{self, After};
use super::ride::{self, ClimbFrame, ClimbLent, Marks, RidePage};
use super::sign;
use crate::clock::Local;
use crate::kept::bests::Bests;
use crate::ride::course::nav::Sign;
use crate::ride::laps::Lap;
use crate::ride::log::{Sample, Status};
use crate::ride::rider::Rider;
use crate::ride::totals::Totals;
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::sensors::radar;
use core::fmt::Write;
use core::ops::Range;

/// Below this much recording time left, the ride page warns the rider.
pub const LOW_STORAGE_S: u32 = 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Boot,
    Format,
    Home,
    Sensors,
    /// Before a ride: what is ready, and ACT starts.
    Ready,
    Ride,
    /// "End ride?" over a paused ride.
    EndRide,
    /// "Discard ride?", which MOVE opens from "End ride?".
    DiscardRide,
    /// Saving, then the saved ride's summary.
    Summary,
    Settings,
    /// The rider's personal bests, opened from Settings.
    Bests,
    /// Fitness, fatigue and form, opened from Settings.
    Form,
    /// The workout picker.
    Workouts,
    /// Connect phone: rides to the phone over Wi-Fi, as FIT files.
    Phone,
    /// The saved rides, newest first.
    History,
    /// A saved ride read back, on the summary's pages.
    PastRide,
    /// First-run setup.
    Setup,
    /// LOSE n UNSAVED RIDES?, which a host CLEAR asks while the phone has
    /// not confirmed every ride of a volatile journal.
    LoseRides,
}

impl Page {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Format => "format",
            Self::Home => "home",
            Self::Sensors => "sensors",
            Self::Ready => "ready",
            Self::Ride => "ride",
            Self::EndRide => "end_ride",
            Self::DiscardRide => "discard_ride",
            Self::Summary => "summary",
            Self::Settings => "settings",
            Self::Bests => "bests",
            Self::Form => "form",
            Self::Workouts => "workouts",
            Self::Phone => "phone",
            Self::History => "history",
            Self::PastRide => "past_ride",
            Self::Setup => "setup",
            Self::LoseRides => "lose_rides",
        }
    }
}

/// Home cards, in MOVE order.
pub const HOME: [&[u8]; 5] = [b"RIDE", b"SENSORS", b"TRAIN", b"SETTINGS", b"RIDES"];

/// The one page body a frame paints. Only the variant for the shown page is
/// ever filled; asking for another paints blank, and trips a debug assertion.
// Exactly one body lives in View and its variants share one slot on purpose;
// boxing would allocate every frame.
#[allow(clippy::large_enum_variant)]
#[derive(Hash)]
pub(super) enum Body {
    /// Nothing prepared.
    Blank,
    /// Every page that prepares a Canvas, and the sheet over any page.
    Canvas(Canvas),
    Home(Home),
}

impl Body {
    fn canvas(&self) -> Option<&Canvas> {
        match self {
            Self::Canvas(canvas) => Some(canvas),
            _ => None,
        }
    }

    fn home(&self) -> Option<&Home> {
        match self {
            Self::Home(home) => Some(home),
            _ => None,
        }
    }
}

/// What the shown page prepares besides its body. A ride page's extras are
/// its chart, laps or map; a menu or lifecycle page's are its lines, picker,
/// readiness or summary. No page reads two: the FTP sheet over a page, the map
/// page's inset and the lap page's overlay are canvas shapes.
// One extra lives in View and its variants share one slot on purpose;
// boxing would allocate every frame.
#[allow(clippy::large_enum_variant)]
pub(super) enum Extra<'a> {
    None,
    /// The climbs, the trace, the forecast and the pinned climb are
    /// borrowed from `Ride` and the look-ahead (up to 1 KB each, which the
    /// main stack cannot spare a copy of).
    Climb {
        lent: ClimbLent<'a>,
        frame: ClimbFrame,
    },
    Workout(ride::WorkoutChart),
    /// The finished laps for the chart, oldest first, the open lap and the
    /// last finished one.
    Lap {
        bars: ride::LapBars,
        lap: Lap,
        previous: Option<Lap>,
    },
    /// The track, projected and kept beside the breadcrumb.
    Map(Option<ride::FilledMap<'a>>),
    /// Settings' lines, and the row under a resting finger.
    Settings {
        lines: SettingsLines,
        pressed: Option<u16>,
    },
    /// The Connect phone screen's lines and QR code.
    Phone {
        qr: Option<super::qr::Placed>,
        lines: Lines,
    },
    /// The first-run setup step and the note beside its cards.
    Setup {
        step: super::setup::Step,
        lines: Lines,
    },
    /// Ready: the checks before a ride.
    Ready(Readiness),
    /// The workout picker's rows.
    Workouts(super::workouts::Picker),
    /// The rider's personal bests, for the BESTS page.
    Bests {
        bests: Bests,
        mode: super::bests::Mode,
    },
    Form(super::form::Mode),
    /// The summary after a ride, or a saved ride read back (`past_title`).
    /// The elevation chart is borrowed from `Ride` (about 430 bytes, which
    /// the main stack cannot spare a copy of).
    Summary {
        after: After,
        past_title: Text<20>,
        elevation: Option<&'a ride::ElevationChart>,
    },
}

/// Lines of text prepared for a page: Settings' values, notes and status,
/// the phone's codes, the setup note.
pub(super) type Lines = [Text<38>; 9];
/// Settings' lines: the status, then a value and a note for each of a
/// section's rows.
pub(super) type SettingsLines = [Text<38>; 1 + 2 * super::settings::SECTION_ROWS];

/// `$name` is the variant's field, or none when `$pat` is another variant.
macro_rules! picks {
    ($($(#[$meta:meta])* $name:ident -> $ty:ty = $pat:pat => $found:expr;)*) => {
        $($(#[$meta])* pub(super) fn $name(&self) -> Option<$ty> {
            match self {
                $pat => Some($found),
                _ => None,
            }
        })*
    };
}

impl<'a> Extra<'a> {
    picks! {
        climb_lent -> ClimbLent<'a> = Self::Climb { lent, .. } => *lent;
        climb_frame -> &ClimbFrame = Self::Climb { frame, .. } => frame;
        #[cfg(test)]
        chart -> &ride::WorkoutChart = Self::Workout(chart) => chart;
        lap -> Lap = Self::Lap { lap, .. } => *lap;
        previous_lap -> Option<Lap> = Self::Lap { previous, .. } => *previous;
        lap_bars -> &ride::LapBars = Self::Lap { bars, .. } => bars;
        map -> Option<&'a ride::MapFrame> = Self::Map(map) => map.map(|map| map.frame());
        settings_lines -> &SettingsLines = Self::Settings { lines, .. } => lines;
        settings_pressed -> Option<u16> = Self::Settings { pressed, .. } => *pressed;
        lines -> &Lines = Self::Phone { lines, .. } | Self::Setup { lines, .. } => lines;
        qr -> Option<&super::qr::Placed> = Self::Phone { qr, .. } => qr.as_ref();
        setup_step -> super::setup::Step = Self::Setup { step, .. } => *step;
        readiness -> &Readiness = Self::Ready(readiness) => readiness;
        picker -> &super::workouts::Picker = Self::Workouts(picker) => picker;
        bests -> &Bests = Self::Bests { bests, .. } => bests;
        bests_mode -> super::bests::Mode = Self::Bests { mode, .. } => *mode;
        form -> super::form::Mode = Self::Form(mode) => *mode;
        after -> &After = Self::Summary { after, .. } => after;
        past_title -> &Text<20> = Self::Summary { past_title, .. } => past_title;
        elevation -> Option<&'a ride::ElevationChart> = Self::Summary { elevation, .. } => *elevation;
    }
}

#[derive(Hash)]
pub struct Scene<'v> {
    layout: &'static Layout,
    kind: Kind,
    look: Look,
    pub(super) bar: Option<&'v StatusBar>,
    /// The rows the main page's tree gives its status bar.
    main_bar: Range<usize>,
    body: &'v Body,
    painted: Painted<'v>,
    pub(super) radar: Option<ride::Radar>,
    cues: &'v [Cue],
    badge: Option<&'v Badge>,
    toast: Option<&'v Toast>,
    sign: Option<sign::Placed>,
}

#[derive(Clone, Copy, Debug, Hash)]
enum Kind {
    PageUnderBar,
    Scrolled,
    Home,
    Ride(RidePage),
    Phone,
    Summary {
        elevation: bool,
    },
    List {
        layout: &'static super::scroll::Layout,
        scroll: i32,
    },
}

#[derive(Clone, Copy, Hash)]
enum Painted<'v> {
    None,
    Climb(&'v ClimbFrame),
    Workout(&'v ride::WorkoutChart),
    Lap,
    Map(Option<&'v ride::FilledMap<'v>>),
    Settings,
    Phone(Option<&'v super::qr::Placed>),
    Setup,
    Ready,
    Workouts,
    Bests,
    Form,
    Summary(Option<&'v ride::ElevationChart>),
}

impl<'v> Painted<'v> {
    picks! {
        climb -> &'v ClimbFrame = Self::Climb(frame) => *frame;
        chart -> &'v ride::WorkoutChart = Self::Workout(chart) => *chart;
        map -> Option<&'v ride::FilledMap<'v>> = Self::Map(map) => *map;
        qr -> Option<&'v super::qr::Placed> = Self::Phone(qr) => *qr;
        elevation -> Option<&'v ride::ElevationChart> = Self::Summary(elevation) => *elevation;
    }
}

impl Extra<'_> {
    fn painted(&self) -> Painted<'_> {
        match self {
            Self::None => Painted::None,
            Self::Climb { lent: _, frame } => Painted::Climb(frame),
            Self::Workout(chart) => Painted::Workout(chart),
            Self::Lap {
                bars: _,
                lap: _,
                previous: _,
            } => Painted::Lap,
            Self::Map(map) => Painted::Map(map.as_ref()),
            Self::Settings {
                lines: _,
                pressed: _,
            } => Painted::Settings,
            Self::Phone { qr, lines: _ } => Painted::Phone(qr.as_ref()),
            Self::Setup { step: _, lines: _ } => Painted::Setup,
            Self::Ready(_) => Painted::Ready,
            Self::Workouts(_) => Painted::Workouts,
            Self::Bests { bests: _, mode: _ } => Painted::Bests,
            Self::Form(_) => Painted::Form,
            Self::Summary {
                after: _,
                past_title: _,
                elevation,
            } => Painted::Summary(*elevation),
        }
    }
}

#[derive(Hash)]
struct WholePage<'v> {
    kind: Kind,
    look: Look,
    radar: bool,
    cues: &'v [Cue],
    badge: Option<&'v Badge>,
}

impl firmware_shell::shell::Picture for Scene<'_> {
    fn paint(&self, row: &mut Row) {
        let row = &mut row.looked(self.look);
        row.fill(BLACK);
        self.paint_content(row);
        if let Some(sign) = &self.sign {
            sign.paint(&mut row.plain());
        }
    }

    fn parts(&self, parts: &mut firmware_shell::shell::Parts) {
        let height = self.layout.scaled.panel.height;
        let Kind::Ride(page) = self.kind else {
            parts.add(0..height, self);
            return;
        };
        parts.add(
            0..height,
            &WholePage {
                kind: self.kind,
                look: self.look,
                radar: self.radar.is_some(),
                cues: self.cues,
                badge: self.badge,
            },
        );
        let chrome = &self.layout.chrome;
        parts.add(self.bar_rows(), &self.bar);
        parts.add(chrome.toast.top..chrome.toast.bottom, &self.toast);
        if let Some(sign) = &self.sign {
            let rect = sign.rect();
            parts.add(rect.top..rect.bottom, sign);
        }
        ride::parts(self, page, parts);
    }
}

impl<'v> Scene<'v> {
    #[cfg(test)]
    pub(crate) fn cues(&self) -> &'v [Cue] {
        self.cues
    }

    fn paint_content(&self, row: &mut Row) {
        let y = row.y();
        let bar_rows = self.bar_rows();
        if let Some(bar) = self.bar
            && bar_rows.contains(&y)
        {
            bar.paint(&mut row.at(y - bar_rows.start));
            return;
        }
        match self.kind {
            Kind::PageUnderBar | Kind::Scrolled => self.canvas().paint(row),
            Kind::Home => {
                if let Some(home) = self.home() {
                    home.paint(row);
                }
            }
            Kind::Ride(page) => ride::paint(self, page, row),
            Kind::Phone => {
                self.canvas().paint(row);
                super::phone::paint(self, row);
            }
            Kind::Summary { elevation, .. } => {
                if let Some(chart) = self.elevation_chart().filter(|_| elevation) {
                    ride::elevation::paint(chart, row);
                }
                self.canvas().paint(row);
            }
            Kind::List { layout, scroll } => {
                if let Some(at) = layout.canvas_row(y, scroll) {
                    self.canvas().paint(&mut row.at(at));
                }
            }
        }
        if let Some(badge) = self.badge {
            badge.paint(row);
        }
        for cue in self.cues {
            cue.paint(row, self.layout);
        }
        if let Some(toast) = self.toast {
            toast.paint(row);
        }
    }

    #[cfg(test)]
    pub(super) fn look(&self) -> Look {
        self.look
    }

    fn bar_rows(&self) -> Range<usize> {
        match self.kind {
            Kind::Ride(RidePage::Map) => 0..self.layout.chrome.status_bar.bottom,
            Kind::Ride(RidePage::Main) => self.main_bar.clone(),
            _ => 0..self.layout.chrome.content.top,
        }
    }

    #[cfg(test)]
    pub(crate) fn pixel(&self, x: usize, y: usize) -> u16 {
        use firmware_shell::shell::Picture;
        let mut pixels = std::vec![0u16; self.layout.scaled.panel.width];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(0)
    }

    fn shown<T>(&self, found: Option<T>, what: &str, blank: T) -> T {
        debug_assert!(found.is_some(), "{:?} has no {what}", self.kind);
        found.unwrap_or(blank)
    }

    pub(super) fn canvas(&self) -> &'v Canvas {
        self.shown(self.body.canvas(), "canvas", &NO_CANVAS)
    }

    fn home(&self) -> Option<&'v Home> {
        let home = self.body.home();
        debug_assert!(home.is_some(), "{:?} has no home body", self.kind);
        home
    }

    pub(super) fn climb(&self) -> &'v ClimbFrame {
        self.shown(self.painted.climb(), "climb chart", &NO_CLIMB_FRAME)
    }

    pub(super) fn chart(&self) -> &'v ride::WorkoutChart {
        self.shown(self.painted.chart(), "workout chart", &NO_CHART)
    }

    pub(super) fn map(&self) -> Option<&'v ride::FilledMap<'v>> {
        self.shown(self.painted.map(), "map", None)
    }

    pub(super) fn qr(&self) -> Option<&'v super::qr::Placed> {
        self.shown(self.painted.qr(), "code", None)
    }

    fn elevation_chart(&self) -> Option<&'v ride::ElevationChart> {
        self.shown(self.painted.elevation(), "summary", None)
    }

    #[cfg(test)]
    pub(super) fn reference_pixel(&self, x: usize, y: usize) -> super::chrome::Shown {
        let color = self.reference_content(x, y);
        self.sign
            .as_ref()
            .and_then(|sign| sign.pixel(x, y, color))
            .unwrap_or(color)
    }

    #[cfg(test)]
    fn reference_content(&self, x: usize, y: usize) -> super::chrome::Shown {
        let look = self.look();
        let black = look.ink(BLACK);
        let bar_rows = self.bar_rows();
        if let Some(bar) = self.bar
            && bar_rows.contains(&y)
        {
            return bar.pixel(look, x, y - bar_rows.start);
        }
        if let Some(color) = self.toast.and_then(|toast| toast.pixel(look, x, y)) {
            return color;
        }
        let mut color = match self.kind {
            Kind::PageUnderBar | Kind::Scrolled => self.canvas().pixel(look, x, y, black),
            Kind::Home => self.home().map_or(black, |home| home.pixel(look, x, y)),
            Kind::Ride(page) => ride::reference_pixel(self, page, x, y),
            Kind::Phone => super::phone::pixel(self, x, y)
                .unwrap_or_else(|| self.canvas().pixel(look, x, y, black)),
            Kind::Summary { elevation, .. } => {
                let under = self
                    .elevation_chart()
                    .filter(|_| elevation)
                    .and_then(|chart| ride::elevation::pixel(chart, x, y))
                    .map_or(black, |color| look.ink(color));
                self.canvas().pixel(look, x, y, under)
            }
            Kind::List { layout, scroll } => layout
                .canvas_row(y, scroll)
                .map_or(black, |at| self.canvas().pixel(look, x, at, black)),
        };
        if let Some(badge) = self.badge {
            color = badge.pixel(look, x, y, color);
        }
        for cue in self.cues {
            color = cue.pixel(look, x, y, color, self.layout);
        }
        color
    }
}

#[cfg(any(test, feature = "invariants"))]
impl Scene<'_> {
    /// Check this frame's layout (`check`): fail a screen that breaks it.
    pub fn check_layout(&self) {
        use super::check;
        if !check::enabled() {
            return;
        }
        if let Some(canvas) = self.body.canvas() {
            canvas.assert_kept(format_args!("{:?} on {:?}", self.kind, self.layout.scaled));
        }
        if self.bar.is_some() {
            for cue in self.cues {
                assert!(
                    !cue.covers(self.layout.chrome.status_bar, self.layout),
                    "the {:?} cue on {:?} is under the status bar",
                    cue.icon,
                    self.layout.scaled
                );
            }
        }
        let panel = self.layout.scaled.screen();
        if let Kind::Ride(page) = self.kind {
            let breaks = self.canvas_breaks(page);
            assert!(
                breaks.is_empty(),
                "{} on {:?} breaks its layout:\n{}",
                page.name(),
                self.layout.scaled,
                breaks.join("\n")
            );
            return;
        }
        if let Kind::Summary { elevation } = self.kind {
            let breaks = self.summary_breaks(elevation);
            assert!(
                breaks.is_empty(),
                "the summary on {:?} breaks its layout:\n{}",
                self.layout.scaled,
                breaks.join("\n")
            );
            return;
        }
        if let Kind::Home = self.kind
            && let Some(home) = self.body.home()
        {
            let breaks = self.home_breaks(home);
            assert!(
                breaks.is_empty(),
                "Home on {:?} breaks its layout:\n{}",
                self.layout.scaled,
                breaks.join("\n")
            );
            return;
        }
        if let Kind::List { layout, scroll } = self.kind {
            let breaks = self.list_breaks(layout, scroll);
            assert!(
                breaks.is_empty(),
                "a list on {:?} breaks its layout:\n{}",
                self.layout.scaled,
                breaks.join("\n")
            );
            return;
        }
        if let Kind::PageUnderBar | Kind::Scrolled | Kind::Phone = self.kind {
            let regions = match self.bar {
                Some(_) if matches!(self.kind, Kind::Scrolled) => std::vec![("list", panel)],
                Some(_) => std::vec![
                    ("status bar", self.layout.chrome.status_bar),
                    ("page", self.layout.chrome.content),
                ],
                None => std::vec![("screen", panel)],
            };
            let page_first = regions.len() - 1;
            let mut breaks = self.breaks_within(regions, page_first);
            let canvas = self.canvas();
            let inks = canvas.runs.iter().filter_map(super::chrome::layout::text);
            let shapes = canvas
                .shapes
                .iter()
                .map(|shape| (std::format!("{shape:?}"), shape.bounds().clipped(panel)));
            breaks.extend(self.under_cues(inks.chain(shapes)));
            assert!(
                breaks.is_empty(),
                "a canvas page on {:?} breaks its layout:\n{}",
                self.layout.scaled,
                breaks.join("\n")
            );
        }
    }
}

#[cfg(any(test, feature = "invariants"))]
impl Scene<'_> {
    fn canvas_breaks(&self, page: RidePage) -> std::vec::Vec<std::string::String> {
        let mut regions = std::vec::Vec::new();
        regions.push(("status bar", self.layout.chrome.status_bar));
        let page_first = regions.len() + usize::from(self.radar.is_some());
        regions.extend(ride::canvas_regions(self, page));
        self.breaks_within(regions, page_first)
    }

    fn home_breaks(&self, home: &Home) -> std::vec::Vec<std::string::String> {
        use super::check::{self, Placed};
        use std::vec::Vec;
        let mut regions = std::vec![("status bar", self.layout.chrome.status_bar)];
        let mut placed = Vec::new();
        let mut inks = Vec::new();
        for (name, card, items) in home.cards() {
            regions.push((name, card));
            for (item, rect) in items {
                placed.push(Placed {
                    name: item.clone(),
                    rect,
                    region: regions.len() - 1,
                });
                inks.push((item, rect));
            }
        }
        let panel = self.layout.scaled.screen();
        let mut breaks = check::region_breaks(panel, &regions, &placed, &[]);
        breaks.extend(check::overlaps(&inks));
        breaks
    }

    fn breaks_within(
        &self,
        regions: std::vec::Vec<(&str, super::chrome::Rect)>,
        page_first: usize,
    ) -> std::vec::Vec<std::string::String> {
        self.breaks_in(regions, page_first, self.layout.scaled.screen())
    }

    fn breaks_in(
        &self,
        regions: std::vec::Vec<(&str, super::chrome::Rect)>,
        page_first: usize,
        bounds: super::chrome::Rect,
    ) -> std::vec::Vec<std::string::String> {
        let targets: std::vec::Vec<_> =
            self.sign.iter().map(|sign| ("sign", sign.rect())).collect();
        super::check::canvas_breaks(self.canvas(), bounds, &regions, page_first, &targets)
    }

    fn list_breaks(
        &self,
        list: &super::scroll::Layout,
        scroll: i32,
    ) -> std::vec::Vec<std::string::String> {
        use super::check;
        use super::chrome::Rect;
        let panel = self.layout.scaled.screen();
        let mut regions = std::vec::Vec::new();
        let page = if self.bar.is_some() {
            regions.push(("status bar", self.layout.chrome.status_bar));
            self.layout.chrome.content
        } else {
            panel
        };
        let page_first = regions.len();
        let band = list
            .pin
            .filter(|_| list.pinned(scroll))
            .map_or(0, |pin| pin.height);
        for (name, rect) in [
            (
                "above the list",
                Rect {
                    bottom: list.top,
                    ..page
                },
            ),
            (
                "pinned band",
                Rect {
                    top: list.top,
                    bottom: list.top + band,
                    ..page
                },
            ),
            (
                "below the list",
                Rect {
                    top: list.bottom,
                    ..page
                },
            ),
        ] {
            if rect.height() > 0 {
                regions.push((name, rect));
            }
        }
        let rows = Rect {
            top: list.stretched_top(),
            bottom: usize::MAX,
            ..panel
        };
        regions.push(("page", rows));
        self.breaks_in(regions, page_first, check::scrolling_bounds(panel))
    }

    fn summary_breaks(&self, elevation: bool) -> std::vec::Vec<std::string::String> {
        use super::check::{self, Placed};
        use super::chrome::Rect;
        use std::string::String;
        use std::vec::Vec;
        let panel = self.layout.scaled.screen();
        let mut regions = Vec::new();
        let page = if self.bar.is_some() {
            regions.push(("status bar", self.layout.chrome.status_bar));
            self.layout.chrome.content
        } else {
            panel
        };
        let region = regions.len();
        regions.push(("page", page));
        let canvas = self.canvas();
        let mut inks: Vec<(String, Rect)> = canvas
            .runs
            .iter()
            .filter_map(super::chrome::layout::text)
            .collect();
        if let Some(badge) = self.badge {
            let (cx, cy, radius) = (badge.cx, badge.cy, badge.radius);
            inks.push((
                String::from("badge"),
                Rect {
                    left: cx - radius,
                    top: cy - radius,
                    right: cx + radius + 1,
                    bottom: cy + radius + 1,
                },
            ));
        }
        if let Some(chart) = self.elevation_chart().filter(|_| elevation)
            && let Some(chart) = chart.rect()
        {
            let with_baseline = Rect {
                bottom: chart.bottom + 1,
                ..chart
            };
            inks.push((String::from("elevation chart"), with_baseline));
        }
        let shapes = canvas.shapes.iter().map(|shape| Placed {
            name: std::format!("{shape:?}"),
            rect: shape.bounds().clipped(panel),
            region,
        });
        let placed: Vec<Placed> = inks
            .iter()
            .map(|(name, rect)| Placed {
                name: name.clone(),
                rect: *rect,
                region,
            })
            .chain(shapes)
            .collect();
        let mut breaks = check::region_breaks(panel, &regions, &placed, &[]);
        breaks.extend(check::overlaps(&inks));
        breaks.extend(self.under_cues(placed.into_iter().map(|item| (item.name, item.rect))));
        breaks
    }

    fn under_cues(
        &self,
        items: impl Iterator<Item = (std::string::String, super::chrome::Rect)>,
    ) -> std::vec::Vec<std::string::String> {
        let mut breaks = std::vec::Vec::new();
        for (name, rect) in items {
            for cue in self.cues.iter().filter(|cue| cue.covers(rect, self.layout)) {
                breaks.push(std::format!(
                    "{name:?} {rect:?} is under the {:?} cue",
                    cue.icon
                ));
            }
        }
        breaks
    }
}

impl Scene<'_> {
    #[cfg(any(test, feature = "invariants"))]
    pub(super) fn layout(&self) -> &'static Layout {
        self.layout
    }

    pub(super) fn frame(&self) -> &ride::Frame {
        &self.layout.ride
    }
}

static NO_CANVAS: Canvas = Canvas::EMPTY;
static NO_CLIMB_FRAME: ClimbFrame = ClimbFrame::EMPTY;
static NO_CHART: ride::WorkoutChart = ride::WorkoutChart::EMPTY;
static NO_LINES: Lines = [const { Text::new() }; 9];
static NO_READINESS: Readiness = Readiness::EMPTY;
static NO_PICKER: super::workouts::Picker = super::workouts::Picker::EMPTY;
static NO_TITLE: Text<20> = Text::new();
static NO_BESTS: Bests = Bests::EMPTY;
static NO_AFTER: After = After::EMPTY;

/// What glides between sensor samples, for graphics only: the main page's
/// zone gauges and the radar markers. Numbers never glide; they read
/// `View::sample` and `View::power_3s`, which hold the readings the display
/// mode lets the numbers show: each as it came, or those of the second's start.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Graphics {
    /// The power gauge's 3 s power.
    pub power_3s: Option<u16>,
    pub heart_bpm: Option<u16>,
    /// The radar strip, its markers at their drawn ranges.
    pub radar: radar::Snapshot,
}

pub struct View<'a> {
    /// The panel's layout, for the pages that lay out from it.
    pub layout: &'static Layout,
    pub now_ms: u64,
    pub sample: Sample,
    /// Power over the last 3 s, which the ride page shows instead of the
    /// jumpy one-second reading.
    pub power_3s: Option<u16>,
    pub graphics: Graphics,
    pub active_secs: u64,
    pub state: Status,
    /// A radar is paired, so the ride screen keeps its strip.
    pub radar_paired: bool,
    /// The pause came from auto-pause, not the rider.
    pub auto_paused: bool,
    pub gps: Gps,
    pub battery: Option<Battery>,
    /// The brand reveal's progress, in thousandths, on the boot page.
    pub reveal: u16,
    pub toast: Option<Toast>,
    bar: StatusBar,
    pub totals: Totals,
    pub ride_page: RidePage,
    /// The rider's ride pages, with the map while a course is loaded, for
    /// the page dots.
    pub deck: ride::Deck,
    /// The readings on the main page's lower line.
    pub main_slots: ride::slots::Slots,
    /// The main page's and the shown screen's tiles, as the runtime solved them.
    pub laids: Option<&'a crate::compose::builtin::Laids>,
    /// What the shown page prepared for this frame.
    pub(super) body: Body,
    /// What the shown ride page prepared besides its body.
    pub(super) extra: Extra<'a>,
    pub zone_time: ZoneTime,
    pub profile: TrainingProfile,
    /// Weight, age and sex, for the summary's calories and W/kg.
    pub rider: Rider,
    /// Zone colours and gauges prepared for this frame.
    pub(super) marks: Marks,
    /// The workout chosen or running, for the workout page and the target
    /// band on the main page.
    pub training: Option<ride::Training>,
    /// A test's FTP waiting for the rider's answer, and the FTP in force.
    pub ftp_offer: Option<(u16, u16)>,
    pub format: super::format::Shows,
    pub page: Page,
    /// The focused Home card, or the selected Settings field.
    pub cursor: usize,
    pub sections: super::settings::Sections,
    /// Settings: the selected field is being changed.
    pub editing: bool,
    /// How far the page's list is scrolled, in pixels; negative past the top.
    pub scroll: i32,
    pub time: Text<8>,
    /// The light look is in force: the QR panel is painted inverted so the
    /// look turns it back to dark modules on light.
    pub light: bool,
    /// How long the ride has been paused.
    /// A u32 of seconds is 136 years; the room it leaves holds `scroll`.
    pub paused_secs: u32,
    /// The paused ride shows the paused screen rather than a ride page.
    pub pause_screen: bool,
    /// Whether END RIDE? offers DISCARD RIDE?, and whether that was answered.
    pub discard: Discard,
    /// Rides the phone has not confirmed, which LOSE n UNSAVED RIDES? counts.
    pub unsaved: u8,
    /// Button cues prepared for this frame.
    pub(super) cues: Cues,
    pub(super) badge: Option<Badge>,
    pub nav: Sign,
    /// A course is loaded, so the main page keeps room for its sign.
    pub course: bool,
}

#[cfg(target_pointer_width = "32")]
const _: () = assert!(
    core::mem::size_of::<View>() <= 5_352,
    "View lives on the main stack every frame: shrink it, do not grow it"
);

impl<'a> View<'a> {
    pub fn new(page: Page, cursor: usize, local: Option<Local>) -> Self {
        let mut time = Text::new();
        if let Some(local) = local {
            let minutes = local.of_day() / 60;
            let hour = minutes / 60;
            let _ = write!(time, "{}:{:02}", (hour + 11) % 12 + 1, minutes % 60);
        } else {
            let _ = time.push_str("--:--");
        }
        Self {
            layout: &super::BASE,
            now_ms: 0,
            sample: Sample::default(),
            power_3s: None,
            graphics: Graphics::default(),
            active_secs: 0,
            state: Status::Ready,
            radar_paired: false,
            auto_paused: false,
            gps: Gps::Off,
            battery: None,
            reveal: 0,
            toast: None,
            bar: StatusBar::default(),
            totals: Totals::default(),
            ride_page: RidePage::Main,
            deck: ride::Deck::default(),
            main_slots: ride::slots::Slots::default(),
            laids: None,
            body: Body::Blank,
            extra: Extra::None,
            zone_time: ZoneTime::default(),
            profile: TrainingProfile::default(),
            rider: Rider::default(),
            marks: Marks::default(),
            training: None,
            ftp_offer: None,
            format: super::format::Shows::Wait,
            page,
            cursor,
            sections: super::settings::Sections::All,
            editing: false,
            scroll: 0,
            time,
            light: false,
            paused_secs: 0,
            pause_screen: false,
            discard: Discard::Unavailable,
            unsaved: 0,
            cues: Cues::new(),
            badge: None,
            nav: Sign::None,
            course: false,
        }
    }
    /// Solve the built-ins for this view, as the runtime's composition does.
    #[cfg(test)]
    pub(crate) fn compose_main(&mut self) {
        self.compose(self.main_slots);
    }

    /// Solve the built-ins for `slots` and what this view shows.
    #[cfg(test)]
    pub(crate) fn compose(&mut self, slots: ride::slots::Slots) {
        let choices = crate::compose::builtin::Choices { main_slots: slots };
        let built_in = crate::compose::Screens::EMPTY;
        let mut laids = crate::compose::builtin::Laids::new(
            self.layout,
            choices,
            ride::main_flags_of(self),
            &built_in,
        );
        laids.show(self.screen_id(), self.layout, &built_in);
        self.laids = Some(std::boxed::Box::leak(std::boxed::Box::new(laids)));
    }

    /// The main ride page as the composition solved it.
    pub fn main(&self) -> Option<&'a crate::compose::builtin::Laid> {
        self.laids.map(crate::compose::builtin::Laids::main)
    }

    /// A canvas page the runtime prepares itself, such as the ride list,
    /// whose cards it makes on demand.
    pub fn set_canvas(&mut self, canvas: Canvas) {
        self.body = Body::Canvas(canvas);
    }

    /// Show the sensor page the menu drew, with its cues.
    pub fn show_sensors(&mut self, canvas: Canvas, cues: &'static [Cue]) {
        self.body = Body::Canvas(canvas);
        self.cues = cue_set(cues);
    }

    /// Show a prepared Home.
    pub fn set_home(&mut self, home: Home) {
        self.body = Body::Home(home);
    }

    /// What a page finds in its body or extra: the value, or for a wrong
    /// variant a blank, after a debug assertion. Release paints the blank.
    fn shown<T>(&self, found: Option<T>, what: &str, blank: T) -> T {
        debug_assert!(found.is_some(), "{:?} has no {what}", self.page);
        found.unwrap_or(blank)
    }

    #[cfg(test)]
    pub(crate) fn canvas(&self) -> &Canvas {
        self.shown(self.body.canvas(), "canvas", &NO_CANVAS)
    }

    /// What the climb page draws from.
    pub(super) fn climb_lent(&self) -> ClimbLent<'a> {
        self.shown(self.extra.climb_lent(), "climbs", ClimbLent::NONE)
    }

    /// The climb chart, on the climb page.
    pub(super) fn climb_frame(&self) -> Option<&ClimbFrame> {
        self.extra.climb_frame()
    }

    #[cfg(test)]
    pub(super) fn chart(&self) -> &ride::WorkoutChart {
        self.shown(self.extra.chart(), "workout chart", &NO_CHART)
    }

    pub(super) fn map(&self) -> Option<&'a ride::MapFrame> {
        self.shown(self.extra.map(), "map", None)
    }

    pub fn lines(&self) -> &[Text<38>] {
        if let Some(lines) = self.extra.settings_lines() {
            return lines;
        }
        self.shown(self.extra.lines(), "lines", &NO_LINES)
            .as_slice()
    }

    pub fn qr(&self) -> Option<&super::qr::Placed> {
        self.shown(self.extra.qr(), "code", None)
    }

    pub(super) fn setup_step(&self) -> super::setup::Step {
        self.shown(
            self.extra.setup_step(),
            "setup step",
            super::setup::Step::Welcome,
        )
    }

    pub(super) fn readiness(&self) -> &Readiness {
        self.shown(self.extra.readiness(), "readiness", &NO_READINESS)
    }

    pub(super) fn picker(&self) -> &super::workouts::Picker {
        self.shown(self.extra.picker(), "picker", &NO_PICKER)
    }

    pub(super) fn bests(&self) -> &Bests {
        self.shown(self.extra.bests(), "bests", &NO_BESTS)
    }

    pub(super) fn bests_mode(&self) -> super::bests::Mode {
        self.shown(self.extra.bests_mode(), "bests mode", Default::default())
    }

    pub(super) fn form(&self) -> super::form::Mode {
        self.shown(self.extra.form(), "form", Default::default())
    }

    pub(super) fn after(&self) -> &After {
        self.shown(self.extra.after(), "summary", &NO_AFTER)
    }

    pub(super) fn past_title(&self) -> &Text<20> {
        self.shown(self.extra.past_title(), "past title", &NO_TITLE)
    }

    pub(super) fn elevation_chart(&self) -> Option<&'a ride::ElevationChart> {
        self.shown(self.extra.elevation(), "summary", None)
    }

    /// The climb page's extras: what it draws from, lent, and a chart still
    /// to prepare.
    #[inline(never)]
    pub fn show_climb(&mut self, lent: ClimbLent<'a>) {
        self.extra = Extra::Climb {
            lent,
            frame: ClimbFrame::EMPTY,
        };
    }

    pub(super) fn set_climb_frame(&mut self, frame: ClimbFrame) {
        match &mut self.extra {
            Extra::Climb { frame: shown, .. } => *shown = frame,
            _ => debug_assert!(false, "{:?} has no climb chart", self.page),
        }
    }

    // Every `show_*` stays out of line: inlined, each would keep the Extra it
    // builds as a temporary in `present`'s frame.
    #[inline(never)]
    pub fn show_workout(&mut self, chart: ride::WorkoutChart) {
        self.extra = Extra::Workout(chart);
    }

    #[inline(never)]
    pub fn show_laps(&mut self, bars: ride::LapBars, lap: Lap, previous: Option<Lap>) {
        self.extra = Extra::Lap {
            bars,
            lap,
            previous,
        };
    }

    #[inline(never)]
    pub fn show_map(&mut self, map: Option<ride::FilledMap<'a>>) {
        self.extra = Extra::Map(map);
    }

    /// Settings' lines, blank, for the caller to write.
    #[inline(never)]
    pub fn show_settings(&mut self) -> &mut SettingsLines {
        self.extra = Extra::Settings {
            lines: Default::default(),
            pressed: None,
        };
        match &mut self.extra {
            Extra::Settings { lines, .. } => lines,
            _ => unreachable!(),
        }
    }

    /// Settings: the row of the section under a resting finger.
    pub fn press_setting(&mut self, row: Option<u16>) {
        if let Extra::Settings { pressed, .. } = &mut self.extra {
            *pressed = row;
        }
    }

    /// Settings: the row of the section shown as pressed.
    pub fn pressed_setting(&self) -> Option<u16> {
        self.extra.settings_pressed().flatten()
    }

    /// The phone page's code and lines, blank, for the caller to write.
    #[inline(never)]
    pub fn show_phone(&mut self) -> (&mut Option<super::qr::Placed>, &mut Lines) {
        self.extra = Extra::Phone {
            qr: None,
            lines: Lines::default(),
        };
        match &mut self.extra {
            Extra::Phone { qr, lines } => (qr, lines),
            _ => unreachable!(),
        }
    }

    /// The setup step's note lines, blank, for the caller to write.
    #[inline(never)]
    pub fn show_setup(&mut self, step: super::setup::Step) -> &mut Lines {
        self.extra = Extra::Setup {
            step,
            lines: Lines::default(),
        };
        match &mut self.extra {
            Extra::Setup { lines, .. } => lines,
            _ => unreachable!(),
        }
    }

    #[inline(never)]
    pub fn show_ready(&mut self, readiness: Readiness) {
        self.extra = Extra::Ready(readiness);
    }

    #[inline(never)]
    pub fn show_picker(&mut self, picker: super::workouts::Picker) {
        self.extra = Extra::Workouts(picker);
    }

    #[inline(never)]
    pub fn show_bests(&mut self, bests: Bests, mode: super::bests::Mode) {
        self.extra = Extra::Bests { bests, mode };
    }

    #[inline(never)]
    pub fn show_form(&mut self, mode: super::form::Mode) {
        self.extra = Extra::Form(mode);
    }

    #[inline(never)]
    pub fn show_summary(&mut self, after: After, past_title: Text<20>) {
        self.extra = Extra::Summary {
            after,
            past_title,
            elevation: None,
        };
    }

    /// Lend the summary the elevation chart its page draws.
    pub fn set_elevation_chart(&mut self, chart: &'a ride::ElevationChart) {
        match &mut self.extra {
            Extra::Summary { elevation, .. } => *elevation = Some(chart),
            _ => debug_assert!(false, "{:?} has no summary", self.page),
        }
    }

    #[cfg(test)]
    pub(super) fn after_mut(&mut self) -> &mut After {
        match &mut self.extra {
            Extra::Summary { after, .. } => after,
            _ => panic!("{:?} has no summary", self.page),
        }
    }

    fn full_screen(&self) -> bool {
        super::cue::any_at_top(&self.cues, self.layout)
            || match self.page {
                Page::Boot | Page::Format | Page::EndRide | Page::DiscardRide | Page::LoseRides => {
                    true
                }
                Page::Summary | Page::PastRide => summary::question_shown(self.after()),
                _ => self.paused_screen_shown(),
            }
    }

    /// The scrolling list this page shows, if any.
    pub fn layout(&self) -> Option<&'static super::scroll::Layout> {
        match self.page {
            Page::History => Some(&self.layout.history.list),
            Page::Workouts if self.ftp_offer.is_none() => Some(&self.layout.workouts.list),
            Page::Settings
                if !(self.editing
                    && self.sections.row(self.cursor).kind() == super::settings::Kind::Number) =>
            {
                Some(&self.layout.settings.list)
            }
            _ => None,
        }
    }

    #[cfg(test)]
    pub(crate) fn pixel(&mut self, x: usize, y: usize) -> u16 {
        self.prepare().pixel(x, y)
    }

    fn paused_screen_shown(&self) -> bool {
        self.page == Page::Ride && self.pause_screen
    }

    fn scene(&self) -> Scene<'_> {
        let sheet = self.page == Page::Boot || self.ftp_offer.is_some();
        let kind = match self.page {
            _ if sheet => self.canvas_kind(),
            Page::Home => Kind::Home,
            Page::Ride if !self.pause_screen => Kind::Ride(self.ride_page),
            Page::Phone => Kind::Phone,
            Page::Summary | Page::PastRide => Kind::Summary {
                elevation: summary::shows_elevation(self),
            },
            _ => match self.layout() {
                Some(layout) => Kind::List {
                    layout,
                    scroll: self.scroll,
                },
                None => self.canvas_kind(),
            },
        };
        let layout = self.layout;
        let frame = &layout.ride;
        let radar = match kind {
            Kind::Ride(_) => self
                .radar_paired
                .then(|| ride::radar(&self.graphics.radar, &frame.strip)),
            _ => None,
        };
        Scene {
            layout,
            kind,
            look: super::look::look(self.light),
            bar: (!sheet && !self.full_screen()).then_some(&self.bar),
            main_bar: self
                .main()
                .and_then(ride::main_status_bar)
                .map_or(0..0, |bar| bar.top..bar.bottom),
            body: &self.body,
            painted: self.extra.painted(),
            radar,
            cues: &self.cues,
            badge: self.badge.as_ref().filter(|_| !sheet),
            toast: self.toast.as_ref().filter(|_| !sheet),
            sign: match kind {
                Kind::Ride(page) => sign::on_page(
                    page,
                    self.nav,
                    self.radar_paired,
                    layout,
                    self.main().and_then(ride::main_chip),
                ),
                _ => None,
            },
        }
    }

    /// The brand reveal: VANA, then the ring closing round it
    /// (`brand::reveal`). Nothing technical shows here; Home says what is
    /// still starting.
    fn boot_canvas(&self) -> Canvas {
        let mut canvas = Canvas::default();
        super::brand::reveal(&mut canvas, self.layout, self.reveal);
        canvas
    }

    fn canvas_kind(&self) -> Kind {
        if self.page == Page::Sensors && !self.full_screen() {
            Kind::Scrolled
        } else {
            Kind::PageUnderBar
        }
    }

    pub fn prepare(&mut self) -> Scene<'_> {
        let mark = match self.state {
            Status::Recording => Mark::Recording,
            Status::Paused if self.auto_paused => Mark::AutoPaused,
            Status::Paused => Mark::Paused,
            _ => Mark::None,
        };
        let layout = self.layout;
        self.bar = StatusBar::placed(&self.time, self.gps, self.battery, mark, &layout.scaled);
        if layout.scaled != super::scaled::BASE {
            self.toast = self.toast.map(|toast| toast.on(layout));
        }
        let shown = core::mem::take(&mut self.cues);
        self.badge = None;
        self.cues = match self.composed() {
            Some(laid) => self.prepare_composed(laid),
            None if self.page == Page::Sensors => shown,
            None => cue_set(self.prepare_legacy()),
        };
        self.scene()
    }

    /// The built-in shown, once the composition lent its screens and the
    /// screen has a tree.
    fn composed(&self) -> Option<&'a crate::compose::builtin::Laid> {
        self.laids?.get(self.screen_id())
    }

    fn screen_id(&self) -> crate::compose::builtin::ScreenId {
        crate::compose::builtin::ScreenId::of(
            self.page,
            self.ride_page,
            self.pause_screen,
            self.ftp_offer.is_some(),
            self.asks(),
            match self.page {
                Page::Summary | Page::PastRide => summary::Shown::of(self.after()),
                _ => summary::Shown::Message,
            },
        )
    }

    fn asks(&self) -> crate::compose::builtin::Asks {
        use crate::compose::builtin::Asks;
        match self.page {
            Page::Bests => self.bests_mode().asks(),
            Page::Form => self.form().asks(),
            Page::Format => self.format.asks(),
            Page::DiscardRide if self.discard == Discard::Answered => Asks::Wait,
            _ => Asks::Own,
        }
    }

    /// Draw `laid` from its solved tiles; its cues are the ones its focus
    /// shows.
    fn prepare_composed(&mut self, laid: &crate::compose::builtin::Laid) -> Cues {
        let riding = self.page == Page::Ride && !self.pause_screen;
        if riding {
            let pages = self.deck.shown(self.training.is_some());
            self.bar
                .set_pages(pages.position(self.ride_page), pages.count());
            self.marks = ride::marks(self);
            ride::prepare_tiles(self, laid);
        }
        let after = matches!(self.page, Page::Summary | Page::PastRide).then(|| *self.after());
        if let Some(after) = after {
            self.bar.set_pages(after.page, summary::total_pages(&after));
            self.marks = ride::marks(self);
        }
        fill(self, |view| {
            let mut canvas = Canvas::default();
            laid.prepare(view, &mut canvas);
            canvas
        });
        if riding {
            ride::hide_banner(self, Some(laid));
        }
        let heading = laid.box_of(crate::compose::registry::WORDS);
        if let (Some(after), Some(heading)) = (after, heading)
            && laid.id() == crate::compose::builtin::ScreenId::SummaryOverview
        {
            self.badge = summary::badge(&after, heading, &self.layout.scaled);
        }
        let send_hidden = after.is_some_and(|after| summary::send_hidden(&after));
        laid.controls()
            .shown(laid.focus())
            .filter(|cue| !(send_hidden && cue.role == device_api::input::Role::Act))
            .filter(|cue| self.offers(cue))
            .collect()
    }

    fn offers(&self, cue: &Cue) -> bool {
        match self.page {
            Page::EndRide => {
                cue.role != device_api::input::Role::Move || self.discard == Discard::Offered
            }
            _ => true,
        }
    }

    /// The screens not yet on the engine: each page's own drawing and cues.
    fn prepare_legacy(&mut self) -> &'static [Cue] {
        match self.page {
            Page::Boot => {
                fill(self, |view| view.boot_canvas());
                &[]
            }
            Page::Settings => {
                fill(self, super::settings::prepare);
                let (section, _) = self.sections.section(self.cursor);
                self.bar.set_pages(section, self.sections.count());
                &[]
            }
            Page::Ready => {
                fill(self, |view| {
                    super::ready::prepare(view.layout, view.readiness())
                });
                if self.readiness().can_start() {
                    &super::ready::CUES
                } else {
                    &[]
                }
            }
            Page::Setup => {
                fill(self, |view| super::setup::prepare(view, view.setup_step()));
                let steps = super::setup::STEPS.len();
                self.bar
                    .set_pages(usize::from(self.setup_step().byte()) - 1, steps);
                &[]
            }
            Page::Phone => {
                fill(self, super::phone::prepare);
                &[]
            }
            Page::Workouts => {
                fill(self, super::workouts::prepare);
                &[]
            }
            // The runtime prepares the list with `set_canvas`.
            Page::History => &[],
            // Drawn from their trees.
            Page::Summary | Page::PastRide => &[],
            Page::Ride => {
                let training = self.training.is_some();
                let pages = self.deck.shown(training);
                self.bar
                    .set_pages(pages.position(self.ride_page), pages.count());
                self.marks = ride::marks(self);
                ride::prepare(self);
                &[]
            }
            // The menu's cues arrive with `show_sensors`.
            Page::Sensors => &[],
            // On the engine: `composed` drew them.
            Page::Format | Page::LoseRides | Page::DiscardRide | Page::Bests | Page::Form => &[],
            Page::Home => &[],
            Page::EndRide => &[],
        }
    }
}

/// The button cues a frame shows.
pub type Cues = heapless::Vec<Cue, 3, u8>;

fn cue_set(cues: &[Cue]) -> Cues {
    cues.iter().copied().take(3).collect()
}

/// Discarding the ride END RIDE? asks about.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Discard {
    /// No FILES store to note it on: END RIDE? has no trash cue.
    #[default]
    Unavailable,
    Offered,
    /// DISCARD RIDE? was answered: the ride is being saved, to be hidden.
    Answered,
}

#[derive(Clone)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: usize,
}
impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self::new()
    }
}
impl<const N: usize> Text<N> {
    pub const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
        }
    }
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..self.len]
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
    pub fn push_str(&mut self, value: &str) -> core::fmt::Result {
        self.write_str(value)
    }
}
impl<const N: usize> core::fmt::Write for Text<N> {
    fn write_str(&mut self, value: &str) -> core::fmt::Result {
        if self.len + value.len() > N {
            return Err(core::fmt::Error);
        }
        for byte in value.bytes() {
            self.bytes[self.len] = byte.to_ascii_uppercase();
            self.len += 1;
        }
        Ok(())
    }
}

/// Prepare a page's canvas in this frame, not inlined into `View::prepare`:
/// a canvas is about 4 KB, and inlined, each page's would take its own slot
/// in one frame under every frame VANA prepares (#167).
#[inline(never)]
pub(super) fn fill(view: &mut View, make: impl FnOnce(&View) -> Canvas) {
    let canvas = make(view);
    view.body = Body::Canvas(canvas);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::laps::Laps;

    use crate::screens::chrome::{
        AMBER, CADENCE, GRADE, GREEN, HEART, POWER, RAISED, RED, Rect, SPEED, STATUS_BAR, TOAST,
        Tone, WHITE, zone_color,
    };

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "has no canvas")]
    fn a_page_asking_for_another_pages_body_trips_the_assertion() {
        let _ = View::new(Page::Home, 0, None).canvas();
    }

    /// What release paints for a wrong variant, which `View`'s accessors
    /// reach only after their debug assertion: nothing found, and blank
    /// statics.
    #[test]
    fn a_wrong_variant_has_nothing_and_the_blanks_are_empty() {
        assert!(Body::Blank.canvas().is_none());
        assert!(Body::Blank.home().is_none());
        let none = Extra::None;
        assert!(none.climb_lent().is_none());
        assert!(none.climb_frame().is_none() && none.chart().is_none());
        assert!(none.lap().is_none() && none.previous_lap().is_none());
        assert!(none.lap_bars().is_none() && none.map().is_none());
        assert!(none.lines().is_none() && none.qr().is_none());
        assert!(none.setup_step().is_none() && none.readiness().is_none());
        assert!(none.picker().is_none() && none.bests().is_none());
        assert!(none.bests_mode().is_none() && none.form().is_none());
        assert!(none.after().is_none() && none.past_title().is_none());
        assert!(none.elevation().is_none());
        assert!(NO_CANVAS.runs.is_empty() && NO_CANVAS.shapes.is_empty());
        assert!(ClimbLent::NONE.climbs.climbing().is_none());
        assert!(NO_LINES.iter().all(|line| line.as_bytes().is_empty()));
        assert!(NO_READINESS.riding.as_bytes().is_empty() && NO_TITLE.as_bytes().is_empty());
        assert!(NO_PICKER.cursor == 0 && NO_PICKER.chosen == 0);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "has no map")]
    fn a_page_asking_for_another_pages_extra_trips_the_assertion() {
        let _ = View::new(Page::Ride, 0, None).map();
    }

    /// The colour of the prepared label that reads `text`.
    fn label_color(view: &View, text: &[u8]) -> Option<u16> {
        view.canvas()
            .runs
            .iter()
            .find(|run| run.line.text() == text)
            .map(|run| run.color)
    }

    /// Whether any pixel in the rows and columns has `color`.
    fn painted(
        view: &mut View,
        ys: core::ops::Range<usize>,
        xs: core::ops::Range<usize>,
        color: u16,
    ) -> bool {
        let scene = view.prepare();
        ys.into_iter()
            .any(|y| xs.clone().any(|x| scene.pixel(x, y) == color))
    }

    #[test]
    fn clock_shows_twelve_hour_local_time_without_a_suffix() {
        let clock = |local: Option<u64>| {
            View::new(Page::Home, 0, local.map(Local))
                .time
                .as_bytes()
                .to_vec()
        };
        assert_eq!(clock(Some(21 * 3600)), b"9:00");
        assert_eq!(clock(Some(12 * 3600)), b"12:00");
        assert_eq!(clock(Some(0)), b"12:00");
        assert_eq!(clock(Some(5 * 86400 + 59 * 60 + 59)), b"12:59");
        assert_eq!(clock(None), b"--:--");
    }

    #[test]
    fn ride_page_shows_prepared_metrics_in_their_colours() {
        let mut view = View::new(Page::Ride, 0, None);
        view.radar_paired = true;
        view.sample = Sample {
            speed_mm_s: Some(8333),
            power_watts: Some(250),
            cadence_tenths: Some(904),
            gradient_tenths: Some(-35),
            ..Sample::default()
        };
        view.power_3s = Some(250);
        view.graphics.power_3s = Some(250);
        view.totals.add(1000, 12_345, &view.sample, None);
        view.active_secs = 3725;
        view.state = Status::Recording;
        view.compose_main();
        view.prepare();
        assert!(painted(&mut view, 0..STATUS_BAR.bottom, 0..60, RED), "REC");
        assert_eq!(label_color(&view, b"1:02:05"), Some(WHITE));
        assert_eq!(label_color(&view, b"-3.5%"), Some(GRADE));
        assert_eq!(label_color(&view, b"29.9"), Some(SPEED));
        assert_eq!(
            label_color(&view, b"KM/H"),
            Some(crate::screens::chrome::SECONDARY)
        );
        assert_eq!(label_color(&view, b"250"), Some(POWER));
        assert_eq!(label_color(&view, b"Z5"), Some(zone_color(5)));
        assert_eq!(label_color(&view, b"--"), Some(HEART));
        assert_eq!(label_color(&view, b"Z-"), Some(zone_color(0)));
        assert_eq!(label_color(&view, b"90"), Some(CADENCE));
        assert_eq!(label_color(&view, b"0.01"), Some(WHITE));
        for color in [WHITE, SPEED, POWER, GRADE, CADENCE] {
            assert!(painted(&mut view, 24..280, 28..240, color), "{color:#x}");
        }
        // The power gauge lights five of seven zones; Z5 is amber.
        let watts = view.canvas().find("250").unwrap();
        let power = watts.bottom()..watts.bottom() + 30;
        assert!(painted(&mut view, power.clone(), 120..200, AMBER));
        // No button labels: a toast may cover the lower line for a moment.
        assert!(view.prepare().cues().is_empty());
        // The same watts sit in a lower zone for a stronger rider.
        view.profile = crate::ride::zones::TrainingProfile::new(300, 185).unwrap();
        view.prepare();
        assert_eq!(label_color(&view, b"Z3"), Some(zone_color(3)));
        assert!(!painted(&mut view, power, 120..200, AMBER));
    }

    #[test]
    fn the_main_page_retiles_beside_a_radar_and_toasts_overlay_the_bottom() {
        let mut view = View::new(Page::Ride, 0, None);
        view.sample.speed_mm_s = Some(8333);
        view.compose_main();
        view.prepare();
        let leftmost = |view: &mut View| {
            view.prepare();
            view.canvas().find("29.9").map(|run| run.left)
        };
        let alone = leftmost(&mut view).unwrap();
        view.radar_paired = true;
        view.compose_main();
        assert_eq!(leftmost(&mut view), Some(alone + ride::RADAR_WIDTH));
        view.toast = Some(Toast::new("RIDE SAVED", Tone::Good));
        assert!(painted(&mut view, TOAST.top..320, 0..120, GREEN));
        assert!(painted(&mut view, TOAST.top..320, 0..240, RAISED));
        // Two pages: two dots, the first lit.
        assert!(painted(&mut view, 0..STATUS_BAR.bottom, 0..40, WHITE));
    }

    /// A first lap of five minutes at 200 W and 30 km/h, then `seconds` of
    /// the second at `watts` and `speed_mm_s`.
    fn two_laps(view: &mut View, seconds: u64, watts: u16, speed_mm_s: u32) {
        let mut laps = Laps::default();
        let steady = |watts, speed_mm_s| Sample {
            speed_mm_s: Some(speed_mm_s),
            power_watts: Some(watts),
            heart_bpm: Some(150),
            ..Sample::default()
        };
        for second in 0..300 {
            laps.add(1000, 8334, &steady(200, 8334), second * 1000);
        }
        let _ = laps.lap(300_000);
        for second in 0..seconds {
            let sample = steady(watts, speed_mm_s);
            laps.add(
                1000,
                u64::from(speed_mm_s),
                &sample,
                300_000 + second * 1000,
            );
        }
        view.show_laps(ride::LapBars::new(), laps.current(), laps.previous());
    }

    #[track_caller]
    fn assert_parts_cover(before: &View, after: &View, what: &str) {
        use firmware_shell::shell::{Parts, Picture};
        let (before, after) = (before.scene(), after.scene());
        let sent = Parts::of(&before).changed(&Parts::of(&after));
        let mut changed = 0;
        for y in 0..HEIGHT {
            let (mut was, mut now) = ([0; WIDTH], [0; WIDTH]);
            before.paint(&mut Row::new(y, &mut was));
            after.paint(&mut Row::new(y, &mut now));
            if was != now {
                assert!(
                    sent.contains(y),
                    "{what}: row {y} changed, its band's key did not"
                );
                changed += 1;
            }
        }
        assert!(changed > 0, "{what} changes no pixel");
    }

    #[test]
    fn under_a_banner_only_its_digits_send() {
        use crate::ride::course::nav::{Distance, Glyph};
        use firmware_shell::shell::Parts;
        let view = |page, metres: u32, secs| {
            let mut view = View::new(Page::Ride, 0, None);
            view.ride_page = page;
            view.course = true;
            view.nav = Sign::Turn {
                glyph: Glyph::Left,
                distance: Distance::from_mm(metres * 1000),
            };
            view.active_secs = secs;
            if page == RidePage::Climb {
                view.show_climb(ClimbLent::NONE);
            }
            view.prepare();
            view
        };
        let sent = |before: &View, after: &View| {
            let sent = Parts::of(&before.scene()).changed(&Parts::of(&after.scene()));
            (0..HEIGHT)
                .filter(|&y| sent.contains(y))
                .collect::<std::vec::Vec<_>>()
        };
        for page in [RidePage::Main, RidePage::Stats, RidePage::Climb] {
            let rows = sent(&view(page, 120, 60), &view(page, 110, 60));
            assert!(!rows.is_empty(), "{page:?}");
            assert!(
                rows.iter().all(|y| (20..104).contains(y)),
                "{page:?}: {rows:?}"
            );
        }
        let ticked = sent(
            &view(RidePage::Main, 120, 60),
            &view(RidePage::Main, 120, 61),
        );
        assert_eq!(ticked, std::vec::Vec::<usize>::new());
    }

    fn rows(view: &View) -> std::vec::Vec<[u16; WIDTH]> {
        let scene = view.scene();
        (0..HEIGHT)
            .map(|y| {
                let mut pixels = [0; WIDTH];
                firmware_shell::shell::Picture::paint(&scene, &mut Row::new(y, &mut pixels));
                pixels
            })
            .collect()
    }

    #[test]
    fn a_banner_hides_what_it_partly_covers_and_moves_nothing_else() {
        use crate::ride::course::nav::{Distance, Glyph};
        use firmware_shell::rendering::font::SPILL;
        use firmware_shell::shell::Parts;
        let turn = Sign::Turn {
            glyph: Glyph::Left,
            distance: Distance::from_mm(120_000),
        };
        let view = |page, nav, radar_paired| {
            let mut view = View::new(Page::Ride, 0, None);
            view.ride_page = page;
            view.radar_paired = radar_paired;
            view.course = true;
            view.nav = nav;
            if page == RidePage::Climb {
                view.show_climb(ClimbLent::NONE);
            }
            view.compose_main();
            view.prepare();
            view
        };
        for page in [RidePage::Stats, RidePage::Climb] {
            for radar_paired in [false, true] {
                let what = std::format!("{page:?} radar {radar_paired}");
                let bare = view(page, Sign::None, radar_paired);
                let under = view(page, turn, radar_paired);
                let banner = sign::on_page(page, turn, radar_paired, under.layout, None)
                    .unwrap()
                    .rect();
                let canvas = bare.canvas();
                let shapes = canvas.shapes.iter().map(|shape| shape.bounds());
                let runs = canvas.runs.iter().map(|run| Rect {
                    left: run.left.saturating_sub(SPILL),
                    top: run.top,
                    right: run.left + run.line.width() + SPILL,
                    bottom: run.bottom(),
                });
                let straddling = shapes
                    .chain(runs)
                    .filter(|rect| rect.top < banner.bottom && banner.bottom < rect.bottom)
                    .collect::<std::vec::Vec<_>>();
                if page == RidePage::Stats {
                    assert!(
                        !straddling.is_empty(),
                        "{what}: a reading straddles the banner's edge"
                    );
                }
                let hidden = |x: usize, y| {
                    straddling
                        .iter()
                        .any(|r| (r.left..r.right).contains(&x) && (r.top..r.bottom).contains(&y))
                };
                let sent = Parts::of(&bare.scene()).changed(&Parts::of(&under.scene()));
                let (bare, under) = (rows(&bare), rows(&under));
                for y in 0..HEIGHT {
                    if bare[y] != under[y] {
                        assert!(sent.contains(y), "{what}: row {y} changes but is not sent");
                    }
                }
                for y in (0..banner.top).chain(banner.bottom..HEIGHT) {
                    for x in 0..WIDTH {
                        let pixel = under[y][x];
                        if hidden(x, y) {
                            assert_eq!(pixel, BLACK, "{what}: a covered reading shows at {x},{y}");
                        } else {
                            // A tile the banner touches goes whole.
                            assert!(
                                pixel == bare[y][x] || pixel == BLACK,
                                "{what}: {x},{y} changed"
                            );
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn the_sign_keeps_its_fill_and_white_ink_in_the_light_look() {
        use crate::ride::course::nav::{Distance, Glyph};
        let turn = Sign::Turn {
            glyph: Glyph::Left,
            distance: Distance::from_mm(120_000),
        };
        let page = |light| {
            let mut view = View::new(Page::Ride, 0, None);
            view.ride_page = RidePage::Stats;
            view.course = true;
            view.nav = turn;
            view.light = light;
            view.prepare();
            rows(&view)
        };
        let (dark, light) = (page(false), page(true));
        let rect = sign::on_page(RidePage::Stats, turn, false, &crate::screens::BASE, None)
            .unwrap()
            .rect();
        let banner = rect.top..rect.bottom;
        let pixels = |rows: &[[u16; WIDTH]]| {
            rows[banner.clone()]
                .iter()
                .flatten()
                .copied()
                .collect::<std::vec::Vec<_>>()
        };
        assert_eq!(
            pixels(&light),
            pixels(&dark),
            "the banner paints the same in both looks"
        );
        assert!(pixels(&light).contains(&sign::TURN_FILL), "turn fill");
        assert!(pixels(&light).contains(&WHITE), "white ink");
    }

    #[test]
    fn each_ride_page_input_alone_keys_the_rows_it_changes() {
        let target = |closing_speed_cm_s| radar::Target {
            threat: radar::Threat::Approach,
            side: radar::Side::Behind,
            range_mm: 30_000,
            closing_speed_cm_s,
        };
        let ride = |change: &dyn Fn(&mut View)| {
            let mut view = View::new(Page::Ride, 0, None);
            view.radar_paired = true;
            view.sample.speed_mm_s = Some(8333);
            view.graphics.radar.status = radar::Status::Ready;
            view.graphics.radar.targets[0] = Some(target(0));
            view.compose_main();
            view.prepare();
            change(&mut view);
            view
        };
        let base = ride(&|_| {});
        let band = |view: &mut View| {
            if let Body::Canvas(canvas) = &mut view.body {
                canvas.shape(super::super::canvas::Shape::Bar {
                    rect: Rect {
                        left: 100,
                        top: 200,
                        right: 160,
                        bottom: 202,
                    },
                    radius: 0,
                    color: WHITE,
                });
            }
        };
        type Change<'c> = dyn for<'v> Fn(&mut View<'v>) + 'c;
        let cases: [(&str, &Change<'_>); 7] = [
            ("the light look", &|view| view.light = true),
            ("radar pairing", &|view| {
                view.radar_paired = false;
                view.prepare();
            }),
            ("the radar track's status", &|view| {
                view.graphics.radar.status = radar::Status::Error;
            }),
            ("a marker turning close on its row", &|view| {
                view.graphics.radar.targets[0] = Some(target(500));
            }),
            ("the workout target band", &band),
            ("button cues", &|view| {
                view.cues = cue_set(&[
                    Cue::new(device_api::input::Role::Back, super::super::cue::Icon::Stop),
                    Cue::new(device_api::input::Role::Act, super::super::cue::Icon::Play),
                ])
            }),
            ("a badge", &|view| {
                view.badge = Some(Badge {
                    cx: 120,
                    cy: 160,
                    radius: 12,
                    icon: super::super::cue::Icon::Check,
                })
            }),
        ];
        for (what, change) in cases {
            assert_parts_cover(&base, &ride(change), what);
        }
    }

    #[test]
    fn the_phone_code_alone_keys_the_page() {
        let phone = |text: &str| {
            let mut view = View::new(Page::Phone, 0, None);
            let (qr, _) = view.show_phone();
            *qr = super::super::qr::Qr::encode(text)
                .map(|code| super::super::qr::Placed::centred(code, 120, 100, 160));
            view.prepare();
            view
        };
        assert_parts_cover(&phone("WIFI:A"), &phone("WIFI:B"), "the code");
    }

    #[test]
    fn home_highlights_the_selected_tile() {
        let mut view = View::new(Page::Home, 1, None);
        view.set_home(Home::new(&crate::screens::BASE, 1, false, &[], Text::new()));
        view.prepare();
        assert_eq!(view.pixel(200, 215), RAISED, "Sensors is raised");
        assert_ne!(view.pixel(200, 120), RAISED, "Ride is not");
    }

    /// Every row the view paints equals the per-pixel reference.
    #[track_caller]
    fn assert_paints_reference(view: &View, what: &str) {
        for y in 0..320 {
            let mut pixels = [0x5555; WIDTH];
            let scene = view.scene();
            firmware_shell::shell::Picture::paint(&scene, &mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                let want = scene.reference_pixel(x, y).0;
                assert_eq!(
                    pixel, want,
                    "{what} at {x},{y}: {pixel:#06x} != {want:#06x}"
                );
            }
        }
    }

    #[test]
    fn painted_rows_match_the_per_pixel_reference() {
        use crate::screens::chrome::Tone;
        use crate::screens::home::last_ride;
        use crate::screens::sensors::Glance;
        let sample = Sample {
            speed_mm_s: Some(8333),
            power_watts: Some(262),
            heart_bpm: Some(151),
            cadence_tenths: Some(904),
            gradient_tenths: Some(-35),
            ..Sample::default()
        };
        let glance = [
            Glance {
                kind: 120,
                live: true,
                color: GREEN,
                battery: None,
            },
            Glance {
                kind: 11,
                live: false,
                color: AMBER,
                battery: None,
            },
        ];
        for cursor in 0..5 {
            let mut view = View::new(Page::Home, cursor, Some(Local(1_000_000)));
            view.battery = Some(Battery::level(14));
            view.gps = Gps::Fixed;
            let last = last_ride(4_999_800);
            view.set_home(Home::new(
                &crate::screens::BASE,
                cursor,
                cursor == 1,
                &glance[..cursor.min(2)],
                last,
            ));
            view.toast = (cursor == 2).then(|| Toast::new("RIDE SAVED", Tone::Good));
            view.prepare();
            assert_paints_reference(&view, "home");
        }
        use crate::ride::course::nav::{Distance, Glyph};
        let distance = Distance::from_mm(120_000);
        let signs = [
            (false, Sign::None, false),
            (true, Sign::None, false),
            (
                true,
                Sign::Chip {
                    glyph: Glyph::Right,
                    distance,
                },
                false,
            ),
            (
                true,
                Sign::Turn {
                    glyph: Glyph::Left,
                    distance,
                },
                false,
            ),
            (
                true,
                Sign::Turn {
                    glyph: Glyph::Left,
                    distance,
                },
                true,
            ),
            (true, Sign::OffCourse { back: distance }, false),
            (true, Sign::Arrived, true),
        ];
        for page in RidePage::ALL {
            for (radar_paired, (course, nav, light)) in [false, true]
                .into_iter()
                .flat_map(|radar| signs.map(|sign| (radar, sign)))
            {
                let mut view = View::new(Page::Ride, 0, Some(Local(1_000_000)));
                view.ride_page = page;
                view.radar_paired = radar_paired;
                view.course = course;
                view.nav = nav;
                view.light = light;
                view.sample = sample;
                // Gliding behind the numbers, so the gauges are drawn too.
                view.graphics.power_3s = Some(255);
                view.graphics.heart_bpm = Some(148);
                view.state = Status::Recording;
                view.active_secs = 3725;
                view.battery = Some(Battery {
                    percent: 80,
                    charging: radar_paired,
                    phase: 2,
                });
                view.gps = Gps::Searching;
                for _ in 0..600 {
                    view.totals.add(1000, 8333, &sample, None);
                    view.zone_time.add(1000, &sample, &view.profile);
                }
                for (slot, (range_mm, threat)) in [
                    (40_000, radar::Threat::Approach),
                    (41_500, radar::Threat::FastApproach),
                    (150_000, radar::Threat::Approach),
                ]
                .into_iter()
                .enumerate()
                {
                    view.graphics.radar.targets[slot] = Some(radar::Target {
                        threat,
                        side: radar::Side::Behind,
                        range_mm,
                        closing_speed_cm_s: 900,
                    });
                }
                view.graphics.radar.status = radar::Status::Ready;
                view.toast = radar_paired.then(|| Toast::new("LAP 1 05:00 200W", Tone::Info));
                match page {
                    RidePage::Lap => two_laps(&mut view, 125, 250, 9000),
                    RidePage::Workout => view.show_workout(ride::WorkoutChart::default()),
                    RidePage::Climb => view.show_climb(ClimbLent::NONE),
                    RidePage::Map => view.show_map(None),
                    RidePage::Main | RidePage::Stats => {}
                }
                view.prepare();
                assert_paints_reference(
                    &view,
                    &std::format!("{page:?} radar {radar_paired} {nav:?} light {light}"),
                );
            }
        }
        for ((page, editing), light) in [
            (Page::Settings, false),
            (Page::Settings, true),
            (Page::Sensors, false),
            (Page::Phone, false),
            (Page::Bests, false),
            (Page::Form, false),
        ]
        .into_iter()
        .flat_map(|page| [(page, false), (page, true)])
        {
            let mut view = View::new(page, 1, None);
            view.editing = editing;
            view.light = light;
            let lines = match page {
                Page::Settings => Some(view.show_settings().as_mut_slice()),
                Page::Phone => Some(view.show_phone().1.as_mut_slice()),
                Page::Bests => {
                    view.show_bests(Bests::default(), crate::screens::bests::Mode::List);
                    None
                }
                Page::Form => {
                    view.show_form(crate::screens::form::Mode::Reading);
                    None
                }
                Page::Sensors => {
                    let mut menu = crate::screens::sensors::Menu::new(&crate::screens::BASE);
                    menu.open();
                    view.show_sensors(menu.canvas(), menu.cues());
                    None
                }
                _ => None,
            };
            for (i, line) in lines.into_iter().flatten().enumerate() {
                let _ = write!(line, "LINE {i} OK");
            }
            view.compose_main();
            view.prepare();
            assert_paints_reference(&view, "text page");
        }
        for light in [false, true] {
            let mut view = View::new(Page::Boot, 0, None);
            view.light = light;
            view.prepare();
            assert_paints_reference(&view, "boot");
        }
    }

    /// The ride lifecycle screens: Ready, paused, end ride and the summary
    /// pages, with their cues, badge and a toast.
    #[test]
    fn lifecycle_screens_paint_their_reference() {
        use crate::screens::ready::{Check, Level};
        use crate::screens::ride::summary::Saving;
        let sample = Sample {
            speed_mm_s: Some(8333),
            power_watts: Some(262),
            heart_bpm: Some(151),
            ..Sample::default()
        };
        let ride = |page: Page| {
            let mut view = View::new(page, 0, Some(Local(1_000_000)));
            view.battery = Some(Battery {
                percent: 80,
                ..Default::default()
            });
            view.gps = Gps::Fixed;
            view.active_secs = 3725;
            view.paused_secs = 42;
            for _ in 0..600 {
                view.totals.add(1000, 8333, &sample, None);
                view.zone_time.add(1000, &sample, &view.profile);
            }
            view
        };
        let mut ready = ride(Page::Ready);
        let mut readiness = Readiness::default();
        for (index, level) in [Level::Good, Level::Wait, Level::Off, Level::Problem]
            .into_iter()
            .enumerate()
        {
            readiness.checks[index] = Check::new(level, "GPS LOCKED", "YOU CAN START");
        }
        ready.show_ready(readiness.clone());
        ready.prepare();
        assert!(
            ready.cues.is_empty(),
            "no start cue while storage is a problem"
        );
        assert_paints_reference(&ready, "ready, blocked");
        readiness.checks[3] = Check::new(Level::Good, "ROOM FOR 99 H", "");
        ready.show_ready(readiness);
        ready.prepare();
        assert_eq!(ready.cues.len(), 1);
        assert_paints_reference(&ready, "ready");
        for auto in [false, true] {
            let mut paused = ride(Page::Ride);
            paused.state = Status::Paused;
            paused.auto_paused = auto;
            paused.pause_screen = true;
            paused.toast = Some(Toast::new("WRITE FAILED", Tone::Alert));
            paused.compose_main();
            paused.prepare();
            assert_eq!(paused.cues.len(), 2);
            assert_paints_reference(&paused, "paused");
        }
        let mut end = ride(Page::EndRide);
        end.state = Status::Paused;
        end.compose_main();
        end.prepare();
        assert_paints_reference(&end, "end ride");
        let mut trace = crate::ride::elevation::Trace::default();
        for metres in (0..4000).step_by(5) {
            trace.add(metres * 1000, Some(6000 + (metres as i32 % 1500) / 2));
        }
        let mut chart = ride::ElevationChart::default();
        chart.refresh(&trace, ride::summary::chart_on(&crate::screens::BASE));
        for elevation in [false, true] {
            for saving in [Saving::Writing, Saving::Saved, Saving::Failed] {
                for page in 0..crate::screens::ride::summary::pages(elevation) {
                    let mut summary = ride(Page::Summary);
                    summary.state = Status::Saved;
                    summary.show_summary(
                        summary::After {
                            saving,
                            page,
                            best: [Some(900), Some(400), Some(300), None],
                            elevation,
                            ..summary::After::default()
                        },
                        Text::new(),
                    );
                    summary.set_elevation_chart(&chart);
                    summary.compose_main();
                    summary.prepare();
                    let what = std::format!("summary {page} {saving:?}");
                    assert_paints_reference(&summary, &what);
                }
            }
        }
    }

    #[test]
    fn a_boot_view_paints_the_brand() {
        let mut view = View::new(Page::Boot, 0, None);
        let scene = view.prepare();
        assert!(scene.canvas().shows("VANA") && scene.canvas().shows("LABS"));
        let lit = (0..320)
            .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
            .filter(|&(x, y)| scene.pixel(x, y) != BLACK)
            .count();
        assert!(lit > 200, "{lit} lit pixels");
    }

    /// The brand flash with the longest line it can get.
    #[test]
    fn the_brand_lays_out_whole_with_a_long_line() {
        let content = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: 320,
        };
        let mut view = View::new(Page::Boot, 0, None);
        view.prepare();
        let screen = Rect { top: 0, ..content };
        view.canvas()
            .assert_laid_out(screen, &["VANA", "LABS"], "boot");
    }

    #[test]
    fn text_is_uppercase_and_bounded() {
        let mut line = Text::<4>::new();
        assert!(line.push_str("ok").is_ok());
        assert!(line.push_str("abc").is_err());
        assert_eq!(line.as_bytes(), b"OK");
    }
}
