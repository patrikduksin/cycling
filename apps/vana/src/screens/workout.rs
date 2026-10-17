//! VANA workout rendering: the chrome around whichever screen is shown.
use super::canvas::Canvas;
use super::chrome::{BLACK, Battery, CONTENT, Gps, Mark, Row, StatusBar, Toast, WIDTH};
use super::cue::{Badge, Cue};
use super::home::Home;
use super::ready::Readiness;
use super::ride::summary::{self, After};
use super::ride::{self, ClimbFrame, Marks, RidePage, Smooth};
use crate::ride::climb::Climbs;
use crate::ride::climb_ahead::Forecast;
use crate::ride::laps::{Lap, Laps};
use crate::ride::log::{Sample, Status};
use crate::ride::totals::Totals;
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::sensors::radar;
use core::fmt::Write;

/// Below this much recording time left, the ride page warns the rider.
pub const LOW_STORAGE_S: u32 = 60 * 60;
/// The clock's time zone: Santiago summer time, UTC-3, for this build.
/// Recordings keep UTC.
pub const UTC_OFFSET_S: u64 = 3 * 3600;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Boot,
    Home,
    Sensors,
    /// Before a ride: what is ready, and ACT starts.
    Ready,
    Ride,
    /// "End ride?" over a paused ride.
    EndRide,
    /// Saving, then the saved ride's summary.
    Summary,
    Settings,
    /// The workout picker.
    Workouts,
    /// Placeholder for connecting a phone (rides to the phone, FIT export).
    Phone,
    /// The saved rides, newest first.
    History,
    /// A saved ride read back, on the summary's pages.
    PastRide,
    /// First-run setup.
    Setup,
}

impl Page {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Home => "home",
            Self::Sensors => "sensors",
            Self::Ready => "ready",
            Self::Ride => "ride",
            Self::EndRide => "end_ride",
            Self::Summary => "summary",
            Self::Settings => "settings",
            Self::Workouts => "workouts",
            Self::Phone => "phone",
            Self::History => "history",
            Self::PastRide => "past_ride",
            Self::Setup => "setup",
        }
    }
}

/// Home cards, in MOVE order.
pub const HOME: [&[u8]; 5] = [b"RIDE", b"SENSORS", b"TRAIN", b"SETTINGS", b"RIDES"];

pub struct View<'a> {
    pub now_ms: u64,
    pub sample: Sample,
    /// Power over the last 3 s, which the ride page shows instead of the
    /// jumpy one-second reading.
    pub power_3s: Option<u16>,
    pub active_secs: u64,
    pub state: Status,
    pub radar: radar::Snapshot,
    /// A radar is paired, so the ride screen keeps its strip.
    pub radar_paired: bool,
    /// The pause came from auto-pause, not the rider.
    pub auto_paused: bool,
    pub gps: Gps,
    pub battery: Option<Battery>,
    /// The brand reveal's progress, in thousandths, on the boot page.
    pub reveal: u16,
    /// The first-run setup page shown on `Page::Setup`.
    pub setup: super::setup::Step,
    pub toast: Option<Toast>,
    pub home: Home,
    bar: StatusBar,
    pub totals: Totals,
    /// The open lap and the last finished one.
    pub lap: Lap,
    pub previous_lap: Option<Lap>,
    pub ride_page: RidePage,
    /// Climbs and the elevation profile; only the climb page reads them.
    pub climbs: Climbs,
    /// The road ahead from the map, borrowed: it is 1 KB and main's stack
    /// cannot spare a copy. Only the climb page reads it.
    pub forecast: &'a Forecast,
    /// Finished laps for the lap page's chart, oldest first.
    pub lap_bars: ride::LapBars,
    /// What a canvas page (stats, lap, climb, Settings, Ready, paused, end
    /// ride, summary, boot) prepared for this frame.
    pub(super) canvas: Canvas,
    pub(super) smooth: Smooth,
    pub zone_time: ZoneTime,
    pub profile: TrainingProfile,
    /// Zone colours and gauges prepared for this frame.
    pub(super) marks: Marks,
    /// The climb page's chart, prepared with `canvas`.
    pub(super) climb: ClimbFrame,
    /// The workout chosen or running, for the workout page and the target
    /// band on the main page.
    pub training: Option<ride::Training>,
    /// The workout page's chart, prepared with `canvas`.
    pub chart: ride::WorkoutChart,
    /// The workout picker's rows.
    pub picker: super::workouts::Picker,
    /// A test's FTP waiting for the rider's answer, and the FTP in force.
    pub ftp_offer: Option<(u16, u16)>,
    pub page: Page,
    /// The focused Home card, or the selected Settings field.
    pub cursor: usize,
    /// Settings: the selected field is being changed.
    pub editing: bool,
    pub time: Text<16>,
    pub lines: [Text<38>; 9],
    /// Ready: the checks before a ride.
    pub readiness: Readiness,
    /// How long the ride has been paused.
    pub paused_secs: u64,
    /// The paused ride shows the paused screen rather than a ride page.
    pub pause_screen: bool,
    /// The summary after a ride.
    pub after: After,
    /// A saved ride read back: when it started, over its summary.
    pub past_title: Text<20>,
    /// The phone screen was opened for one saved ride.
    pub phone_ride: bool,
    /// Button cues prepared for this frame.
    pub(super) cues: heapless::Vec<Cue, 2>,
    pub(super) badge: Option<Badge>,
}
impl View<'_> {
    pub fn new(page: Page, cursor: usize, utc: Option<u64>) -> Self {
        let mut time = Text::new();
        // Santiago summer time for this workout build. UTC remains unchanged in recordings.
        if let Some(utc) = utc {
            let minutes = ((utc + 86400 - UTC_OFFSET_S) % 86400) / 60;
            let hour = minutes / 60;
            let _ = write!(time, "{}:{:02}", (hour + 11) % 12 + 1, minutes % 60);
        } else {
            let _ = time.push_str("--:--");
        }
        let mut view = Self {
            now_ms: 0,
            sample: Sample::default(),
            power_3s: None,
            active_secs: 0,
            state: Status::Ready,
            radar: radar::Snapshot::default(),
            radar_paired: false,
            auto_paused: false,
            gps: Gps::Off,
            battery: None,
            reveal: 0,
            setup: super::setup::Step::Welcome,
            toast: None,
            home: Home::new(cursor, false, &[], Text::new()),
            bar: StatusBar::default(),
            totals: Totals::default(),
            lap: Laps::default().current(),
            previous_lap: None,
            ride_page: RidePage::Main,
            climbs: Climbs::default(),
            forecast: &Forecast::NONE,
            lap_bars: ride::LapBars::new(),
            canvas: Canvas::default(),
            smooth: Smooth::default(),
            zone_time: ZoneTime::default(),
            profile: TrainingProfile::default(),
            marks: Marks::default(),
            climb: ClimbFrame::default(),
            training: None,
            chart: ride::WorkoutChart::default(),
            picker: super::workouts::Picker::default(),
            ftp_offer: None,
            page,
            cursor,
            editing: false,
            time,
            lines: core::array::from_fn(|_| Text::new()),
            readiness: Readiness::default(),
            paused_secs: 0,
            pause_screen: false,
            after: After::default(),
            past_title: Text::new(),
            phone_ride: false,
            cues: heapless::Vec::new(),
            badge: None,
        };
        // The brand flash is painted before anything prepares a frame, as
        // the preloaded startup frame: it needs no `prepare` to show.
        if page == Page::Boot {
            view.canvas = view.boot_canvas();
        }
        view
    }
    /// A canvas page the runtime prepares itself, such as the ride list,
    /// whose cards it makes on demand.
    pub fn set_canvas(&mut self, canvas: Canvas) {
        self.canvas = canvas;
    }

    /// One pixel of the painted canvas; tests and previews only, as it paints a whole row.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let mut pixels = [0u16; WIDTH];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(0)
    }

    /// The screen takes the whole panel, without the status bar: the brand,
    /// the paused screen and the end-ride question.
    fn full_screen(&self) -> bool {
        matches!(self.page, Page::Boot | Page::EndRide) || self.paused_screen_shown()
    }

    fn paused_screen_shown(&self) -> bool {
        self.page == Page::Ride && self.pause_screen
    }

    /// Paint one row of the 240 × 320 canvas: the status bar, the shown
    /// screen, its button cues, then the toast over it.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        row.fill(BLACK);
        if self.page == Page::Boot || self.ftp_offer.is_some() {
            self.canvas.paint(row);
            return;
        }
        if y < CONTENT.top && !self.full_screen() {
            self.bar.paint(row);
            return;
        }
        match self.page {
            Page::Ride if !self.pause_screen => ride::paint(self, row),
            Page::Home => self.home.paint(row),
            _ => self.canvas.paint(row),
        }
        if let Some(badge) = &self.badge {
            badge.paint(row);
        }
        for cue in &self.cues {
            cue.paint(row);
        }
        self.paint_toast(row);
    }

    /// The toast over the content, if one is shown.
    pub fn paint_toast(&self, row: &mut Row) {
        if let Some(toast) = &self.toast {
            toast.paint(row);
        }
    }

    /// The brand reveal: VANA, then the ring closing round it
    /// (`brand::reveal`). Nothing technical shows here; Home says what is
    /// still starting.
    fn boot_canvas(&self) -> Canvas {
        let mut canvas = Canvas::default();
        super::brand::reveal(&mut canvas, self.reveal);
        canvas
    }

    /// The per-pixel original of `paint`, kept as the reference it must match.
    #[cfg(test)]
    pub(super) fn reference_pixel(&self, x: usize, y: usize) -> u16 {
        if self.page == Page::Boot || self.ftp_offer.is_some() {
            return self.canvas.pixel(x, y, BLACK);
        }
        if y < CONTENT.top && !self.full_screen() {
            return self.bar.pixel(x, y);
        }
        if let Some(color) = self.toast.as_ref().and_then(|toast| toast.pixel(x, y)) {
            return color;
        }
        let mut color = match self.page {
            Page::Ride if !self.pause_screen => ride::reference_pixel(self, x, y),
            Page::Home => self.home.pixel(x, y),
            _ => self.canvas.pixel(x, y, BLACK),
        };
        if let Some(badge) = &self.badge {
            color = badge.pixel(x, y, color);
        }
        for cue in &self.cues {
            color = cue.pixel(x, y, color);
        }
        color
    }

    pub fn prepare(&mut self) {
        let mark = match self.state {
            Status::Recording => Mark::Recording,
            Status::Paused if self.auto_paused => Mark::AutoPaused,
            Status::Paused => Mark::Paused,
            _ => Mark::None,
        };
        self.bar = StatusBar::with_battery(&self.time, self.gps, self.battery, mark);
        self.cues.clear();
        self.badge = None;
        let cues: &[Cue] = match self.page {
            Page::Boot => {
                self.canvas = self.boot_canvas();
                &[]
            }
            Page::Settings => {
                self.canvas = super::settings::prepare(self);
                let (section, _) = super::settings::section(self.cursor);
                self.bar.set_pages(section, super::settings::SECTIONS.len());
                &[]
            }
            Page::Ready => {
                self.canvas = super::ready::prepare(&self.readiness);
                if self.readiness.can_start() {
                    &super::ready::CUES
                } else {
                    &[]
                }
            }
            Page::EndRide => {
                self.canvas = ride::end::prepare(self);
                &ride::end::CUES
            }
            Page::Summary => {
                self.bar.set_pages(self.after.page, summary::PAGES);
                self.marks = ride::marks(self);
                self.canvas = summary::prepare(self);
                self.badge = summary::badge(self);
                if self.after.saving == summary::Saving::Writing {
                    &[]
                } else {
                    core::slice::from_ref(&summary::CUE)
                }
            }
            Page::Setup => {
                self.canvas = super::setup::prepare(self, self.setup);
                let steps = super::setup::STEPS.len();
                self.bar
                    .set_pages(usize::from(self.setup.byte()) - 1, steps);
                &[]
            }
            Page::Phone => {
                self.canvas = super::phone::prepare(self.phone_ride);
                &[]
            }
            Page::Workouts => {
                self.canvas = super::workouts::prepare(self);
                &[]
            }
            // The runtime prepares the list with `set_canvas`.
            Page::History => &[],
            Page::PastRide => {
                self.bar.set_pages(self.after.page, summary::PAGES);
                self.marks = ride::marks(self);
                self.canvas = summary::prepare(self);
                if self.after.saving == summary::Saving::Reading {
                    &[]
                } else {
                    &summary::PAST_CUES
                }
            }
            Page::Ride if self.pause_screen => {
                self.canvas = ride::paused::prepare(self);
                &ride::paused::CUES
            }
            Page::Ride => {
                let training = self.training.is_some();
                self.bar.set_pages(
                    self.ride_page.position(training),
                    RidePage::pages(training).len(),
                );
                self.marks = ride::marks(self);
                ride::prepare(self);
                &[]
            }
            Page::Home | Page::Sensors => &[],
        };
        self.cues = cues.iter().copied().collect();
        // The FTP sheet covers whatever page it shows over.
        if let Some((new, old)) = self.ftp_offer {
            self.canvas = super::workouts::ftp_sheet(new, old);
            self.cues.clear();
            self.badge = None;
        }
    }
}

#[derive(Clone)]
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
    pub fn new() -> Self {
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

#[cfg(test)]
mod tests {
    use super::*;

    use crate::screens::chrome::{
        AMBER, CADENCE, GRADE, GREEN, HEART, POWER, RAISED, RED, Rect, SPEED, STATUS_BAR, TOAST,
        Tone, WHITE, zone_color,
    };

    /// The colour of the prepared label that reads `text`.
    fn label_color(view: &View, text: &[u8]) -> Option<u16> {
        view.smooth
            .labels
            .iter()
            .find(|label| label.line.text() == text)
            .map(|label| label.color)
    }

    /// Whether any pixel in the rows and columns has `color`.
    fn painted(
        view: &View,
        ys: core::ops::Range<usize>,
        xs: core::ops::Range<usize>,
        color: u16,
    ) -> bool {
        ys.into_iter()
            .any(|y| xs.clone().any(|x| view.pixel(x, y) == color))
    }

    #[test]
    fn clock_shows_twelve_hour_local_time_without_a_suffix() {
        let clock = |utc| View::new(Page::Home, 0, utc).time.as_bytes().to_vec();
        assert_eq!(clock(Some(0)), b"9:00");
        assert_eq!(clock(Some(15 * 3600)), b"12:00");
        assert_eq!(clock(Some(3 * 3600)), b"12:00");
        assert_eq!(clock(Some(5 * 86400 + 3 * 3600 + 59 * 60 + 59)), b"12:59");
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
        view.totals.add(1000, 12_345, &view.sample, None);
        view.active_secs = 3725;
        view.state = Status::Recording;
        view.prepare();
        assert!(painted(&view, 0..STATUS_BAR.bottom, 0..60, RED), "REC");
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
            assert!(painted(&view, 24..280, 28..240, color), "{color:#x}");
        }
        // The power gauge lights five of seven zones; Z5 is amber.
        assert!(painted(&view, 170..190, 120..200, AMBER));
        // No button labels: the bottom band is free for toasts.
        assert!(!painted(&view, TOAST.top + 2..320, 28..240, WHITE));
        // The same watts sit in a lower zone for a stronger rider.
        view.profile = crate::ride::zones::TrainingProfile::new(300, 185).unwrap();
        view.prepare();
        assert_eq!(label_color(&view, b"Z3"), Some(zone_color(3)));
        assert!(!painted(&view, 170..190, 120..200, AMBER));
    }

    #[test]
    fn ride_page_centres_without_a_radar_and_toasts_overlay_the_bottom() {
        let mut view = View::new(Page::Ride, 0, None);
        view.sample.speed_mm_s = Some(8333);
        view.prepare();
        let leftmost = |view: &View| {
            (60..130)
                .flat_map(|y| (0..240).map(move |x| (x, y)))
                .filter(|&(x, y)| view.pixel(x, y) == SPEED)
                .map(|(x, _)| x)
                .min()
        };
        let centred = leftmost(&view).unwrap();
        view.radar_paired = true;
        assert_eq!(leftmost(&view), Some(centred + 14));
        view.toast = Some(Toast::new("RIDE SAVED", Tone::Good));
        assert!(painted(&view, TOAST.top..320, 0..120, GREEN));
        assert!(painted(&view, TOAST.top..320, 0..240, RAISED));
        // Two pages: two dots, the first lit.
        assert!(painted(&view, 0..STATUS_BAR.bottom, 0..40, WHITE));
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
        view.lap = laps.current();
        view.previous_lap = laps.previous();
    }

    #[test]
    fn home_highlights_the_selected_tile() {
        let mut view = View::new(Page::Home, 1, None);
        view.home = Home::new(1, false, &[], Text::new());
        view.prepare();
        assert_eq!(view.pixel(200, 215), RAISED, "Sensors is raised");
        assert_ne!(view.pixel(200, 120), RAISED, "Ride is not");
    }

    /// Every row the view paints equals the per-pixel reference.
    #[track_caller]
    fn assert_paints_reference(view: &View, what: &str) {
        for y in 0..320 {
            let mut pixels = [0x5555; WIDTH];
            view.paint(&mut Row::new(y, &mut pixels));
            for (x, &pixel) in pixels.iter().enumerate() {
                let want = view.reference_pixel(x, y);
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
            },
            Glance {
                kind: 11,
                live: false,
                color: AMBER,
            },
        ];
        for cursor in 0..5 {
            let mut view = View::new(Page::Home, cursor, Some(1_000_000));
            view.battery = Some(Battery::level(14));
            view.gps = Gps::Fixed;
            let last = last_ride(4_999_800);
            view.home = Home::new(cursor, cursor == 1, &glance[..cursor.min(2)], last);
            view.toast = (cursor == 2).then(|| Toast::new("RIDE SAVED", Tone::Good));
            view.prepare();
            assert_paints_reference(&view, "home");
        }
        for page in RidePage::ALL {
            for radar_paired in [false, true] {
                let mut view = View::new(Page::Ride, 0, Some(1_000_000));
                view.ride_page = page;
                view.radar_paired = radar_paired;
                view.sample = sample;
                view.state = Status::Recording;
                view.active_secs = 3725;
                view.battery = Some(Battery {
                    percent: 80,
                    charging: radar_paired,
                    phase: 2,
                });
                view.gps = Gps::Searching;
                two_laps(&mut view, 125, 250, 9000);
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
                    view.radar.targets[slot] = Some(radar::Target {
                        threat,
                        side: radar::Side::Behind,
                        range_mm,
                        closing_speed_cm_s: 900,
                    });
                }
                view.radar.status = radar::Status::Ready;
                view.toast = radar_paired.then(|| Toast::new("LAP 1 05:00 200W", Tone::Info));
                view.prepare();
                assert_paints_reference(&view, &std::format!("{page:?} radar {radar_paired}"));
            }
        }
        for (page, editing) in [
            (Page::Settings, false),
            (Page::Settings, true),
            (Page::Sensors, false),
            (Page::Phone, false),
        ] {
            let mut view = View::new(page, 1, None);
            view.editing = editing;
            for (i, line) in view.lines.iter_mut().enumerate() {
                let _ = write!(line, "LINE {i} OK");
            }
            view.prepare();
            assert_paints_reference(&view, "text page");
        }
        let mut view = View::new(Page::Boot, 0, None);
        let _ = view.lines[0].push_str("COMPANION READY");
        view.prepare();
        assert_paints_reference(&view, "boot");
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
            let mut view = View::new(page, 0, Some(1_000_000));
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
        for (index, level) in [Level::Good, Level::Wait, Level::Off, Level::Problem]
            .into_iter()
            .enumerate()
        {
            ready.readiness.checks[index] = Check::new(level, "GPS LOCKED", "YOU CAN START");
        }
        ready.prepare();
        assert!(
            ready.cues.is_empty(),
            "no start cue while storage is a problem"
        );
        assert_paints_reference(&ready, "ready, blocked");
        ready.readiness.checks[3] = Check::new(Level::Good, "ROOM FOR 99 H", "");
        ready.prepare();
        assert_eq!(ready.cues.len(), 1);
        assert_paints_reference(&ready, "ready");
        for auto in [false, true] {
            let mut paused = ride(Page::Ride);
            paused.state = Status::Paused;
            paused.auto_paused = auto;
            paused.pause_screen = true;
            paused.toast = Some(Toast::new("WRITE FAILED", Tone::Alert));
            paused.prepare();
            assert_eq!(paused.cues.len(), 2);
            assert_paints_reference(&paused, "paused");
        }
        let mut end = ride(Page::EndRide);
        end.state = Status::Paused;
        end.prepare();
        assert_paints_reference(&end, "end ride");
        for saving in [Saving::Writing, Saving::Saved, Saving::Failed] {
            for page in 0..crate::screens::ride::summary::PAGES {
                let mut summary = ride(Page::Summary);
                summary.state = Status::Saved;
                summary.after.saving = saving;
                summary.after.page = page;
                summary.after.best = [Some(900), Some(400), Some(300), None];
                summary.prepare();
                let what = std::format!("summary {page} {saving:?}");
                assert_paints_reference(&summary, &what);
            }
        }
    }

    /// A Boot view straight from `new`, as the startup frame is built,
    /// paints VANA and LABS without `prepare`.
    #[test]
    fn a_new_boot_view_paints_the_brand_without_prepare() {
        let view = View::new(Page::Boot, 0, None);
        assert!(view.canvas.shows("VANA") && view.canvas.shows("LABS"));
        let lit = (0..320)
            .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
            .filter(|&(x, y)| view.pixel(x, y) != BLACK)
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
        let _ = view.lines[0].push_str("RADIO WAITING FOR RADIO / GPS SEARCHING");
        view.prepare();
        let screen = Rect { top: 0, ..content };
        view.canvas
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
