//! VANA workout rendering: the chrome around whichever screen is shown.
use super::canvas::Canvas;
use super::chrome::{
    ACCENT, Align, BLACK, BODY, CAPTION, CONTENT, Gps, Mark, Row, SECONDARY, StatusBar, TITLE,
    Toast, WHITE, WIDTH,
};
use super::home::Home;
use super::ride::{self, ClimbFrame, Marks, RidePage, Smooth};
use crate::ride::climb::Climbs;
use crate::ride::laps::{Lap, Laps};
use crate::ride::log::{Sample, Status};
use crate::ride::totals::Totals;
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::sensors::radar;
use core::fmt::Write;

/// Below this much recording time left, the ride page warns the rider.
pub const LOW_STORAGE_S: u32 = 60 * 60;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Boot,
    Home,
    Sensors,
    Preflight,
    Ride,
    Settings,
}

impl Page {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Boot => "boot",
            Self::Home => "home",
            Self::Sensors => "sensors",
            Self::Preflight => "preflight",
            Self::Ride => "ride",
            Self::Settings => "settings",
        }
    }
}

/// Home cards, in MOVE order.
pub const HOME: [&[u8]; 3] = [b"RIDE", b"SENSORS", b"SETTINGS"];

pub struct View {
    pub now_ms: u64,
    pub sample: Sample,
    pub active_secs: u64,
    pub state: Status,
    pub radar: radar::Snapshot,
    /// A radar is paired, so the ride screen keeps its strip.
    pub radar_paired: bool,
    /// The pause came from auto-pause, not the rider.
    pub auto_paused: bool,
    pub gps: Gps,
    pub battery: Option<u8>,
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
    /// Finished laps for the lap page's chart, oldest first.
    pub lap_bars: ride::LapBars,
    /// What a canvas page (stats, lap, climb, Settings, Preflight, boot)
    /// prepared for this frame.
    pub(super) canvas: Canvas,
    pub(super) smooth: Smooth,
    pub zone_time: ZoneTime,
    pub profile: TrainingProfile,
    /// Zone colours and gauges prepared for this frame.
    pub(super) marks: Marks,
    /// The climb page's chart, prepared with `canvas`.
    pub(super) climb: ClimbFrame,
    pub page: Page,
    /// The focused Home card, or the selected Settings field.
    pub cursor: usize,
    /// Settings: the selected field is being changed.
    pub editing: bool,
    pub time: Text<16>,
    pub lines: [Text<38>; 9],
}
impl View {
    pub fn new(page: Page, cursor: usize, utc: Option<u64>) -> Self {
        let mut time = Text::new();
        // Santiago summer time for this workout build. UTC remains unchanged in recordings.
        if let Some(utc) = utc {
            let minutes = ((utc + 86400 - 3 * 3600) % 86400) / 60;
            let hour = minutes / 60;
            let _ = write!(time, "{}:{:02}", (hour + 11) % 12 + 1, minutes % 60);
        } else {
            let _ = time.push_str("--:--");
        }
        let mut view = Self {
            now_ms: 0,
            sample: Sample::default(),
            active_secs: 0,
            state: Status::Ready,
            radar: radar::Snapshot::default(),
            radar_paired: false,
            auto_paused: false,
            gps: Gps::Off,
            battery: None,
            toast: None,
            home: Home::new(cursor, false, &[], b"", Text::new()),
            bar: StatusBar::default(),
            totals: Totals::default(),
            lap: Laps::default().current(),
            previous_lap: None,
            ride_page: RidePage::Main,
            climbs: Climbs::default(),
            lap_bars: ride::LapBars::new(),
            canvas: Canvas::default(),
            smooth: Smooth::default(),
            zone_time: ZoneTime::default(),
            profile: TrainingProfile::default(),
            marks: Marks::default(),
            climb: ClimbFrame::default(),
            page,
            cursor,
            editing: false,
            time,
            lines: core::array::from_fn(|_| Text::new()),
        };
        // The brand flash is painted before anything prepares a frame, as
        // the preloaded startup frame: it needs no `prepare` to show.
        if page == Page::Boot {
            view.canvas = view.boot_canvas();
        }
        view
    }
    /// One pixel of the painted canvas; tests and previews only, as it paints a whole row.
    pub fn pixel(&self, x: usize, y: usize) -> u16 {
        let mut pixels = [0u16; WIDTH];
        self.paint(&mut Row::new(y, &mut pixels));
        pixels.get(x).copied().unwrap_or(0)
    }

    /// Paint one row of the 240 × 320 canvas: the status bar, the shown
    /// screen, then the toast over it.
    pub fn paint(&self, row: &mut Row) {
        let y = row.y();
        row.fill(BLACK);
        if self.page == Page::Boot {
            self.canvas.paint(row);
            return;
        }
        if y < CONTENT.top {
            self.bar.paint(row);
            return;
        }
        match self.page {
            Page::Ride => ride::paint(self, row),
            Page::Home => self.home.paint(row),
            _ => self.canvas.paint(row),
        }
        self.paint_toast(row);
    }

    /// The toast over the content, if one is shown.
    pub fn paint_toast(&self, row: &mut Row) {
        if let Some(toast) = &self.toast {
            toast.paint(row);
        }
    }

    /// The brand flash: VANA and LABS, and what is starting under them.
    fn boot_canvas(&self) -> Canvas {
        let mut canvas = Canvas::default();
        let width = WIDTH - 20;
        let centre = WIDTH / 2;
        canvas.fit(&[&TITLE], b"VANA", width, centre, 160, Align::Center, WHITE);
        canvas.fit(
            &[&CAPTION],
            b"LABS",
            width,
            centre,
            184,
            Align::Center,
            ACCENT,
        );
        let status = self.lines[0].as_bytes();
        canvas.fit(
            &[&CAPTION],
            status,
            width,
            centre,
            296,
            Align::Center,
            SECONDARY,
        );
        canvas
    }

    /// Preflight: a title, then one row per check, its name in a caption
    /// over its state.
    fn lines_canvas(&self) -> Canvas {
        const LEFT: usize = 16;
        const RIGHT: usize = WIDTH - 16;
        let mut canvas = Canvas::default();
        canvas.caption(
            self.lines[0].as_bytes(),
            RIGHT - LEFT,
            LEFT,
            40,
            Align::Left,
        );
        let rows = self.lines[1..]
            .iter()
            .filter(|line| !line.as_bytes().is_empty());
        for (row, line) in rows.enumerate() {
            let base = 64 + row * 34;
            let text = line.as_bytes();
            let split = text.iter().position(|&c| c == b' ').unwrap_or(text.len());
            let (name, state) = (&text[..split], text.get(split + 1..).unwrap_or(&[]));
            canvas.caption(name, RIGHT - LEFT, LEFT, base, Align::Left);
            canvas.fit(
                &[&BODY, &CAPTION],
                state,
                RIGHT - LEFT,
                LEFT,
                base + 19,
                Align::Left,
                WHITE,
            );
        }
        canvas
    }

    /// The per-pixel original of `paint`, kept as the reference it must match.
    #[cfg(test)]
    pub(super) fn reference_pixel(&self, x: usize, y: usize) -> u16 {
        if self.page == Page::Boot {
            return self.canvas.pixel(x, y, BLACK);
        }
        if y < CONTENT.top {
            return self.bar.pixel(x, y);
        }
        if let Some(color) = self.toast.as_ref().and_then(|toast| toast.pixel(x, y)) {
            return color;
        }
        match self.page {
            Page::Ride => ride::reference_pixel(self, x, y),
            Page::Home => self.home.pixel(x, y),
            _ => self.canvas.pixel(x, y, BLACK),
        }
    }

    pub fn prepare(&mut self) {
        let mark = match self.state {
            Status::Recording => Mark::Recording,
            Status::Paused if self.auto_paused => Mark::AutoPaused,
            Status::Paused => Mark::Paused,
            _ => Mark::None,
        };
        self.bar = StatusBar::new(&self.time, self.gps, self.battery, mark);
        match self.page {
            Page::Boot => self.canvas = self.boot_canvas(),
            Page::Settings => self.canvas = super::settings::prepare(self),
            Page::Preflight => self.canvas = self.lines_canvas(),
            Page::Home | Page::Sensors | Page::Ride => {}
        }
        if self.page == Page::Ride {
            self.bar
                .set_pages(self.ride_page.index(), RidePage::ALL.len());
            self.marks = ride::marks(self);
            ride::prepare(self);
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
        Tone, zone_color,
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
        view.totals.add(1000, 12_345, &view.sample, None);
        view.active_secs = 3725;
        view.state = Status::Recording;
        view.prepare();
        assert!(painted(&view, 0..STATUS_BAR.bottom, 0..60, RED), "REC");
        assert_eq!(label_color(&view, b"1:02:05"), Some(WHITE));
        assert_eq!(label_color(&view, b"-3.5%"), Some(GRADE));
        assert_eq!(label_color(&view, b"29.9"), Some(SPEED));
        assert_eq!(label_color(&view, b"KM/H"), Some(SECONDARY));
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
        view.home = Home::new(1, false, &[], b"", Text::new());
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
        for cursor in 0..3 {
            let mut view = View::new(Page::Home, cursor, Some(1_000_000));
            view.battery = Some(14);
            view.gps = Gps::Fixed;
            let last = last_ride(4_999_800, 599);
            view.home = Home::new(cursor, cursor == 1, &glance[..cursor], b"FTP 215 W", last);
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
                view.battery = Some(80);
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
            (Page::Preflight, false),
            (Page::Sensors, false),
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

    /// Preflight and the brand flash with the longest lines they can get.
    #[test]
    fn text_pages_lay_out_whole_with_long_lines() {
        let mut view = View::new(Page::Preflight, 0, None);
        for (line, text) in view.lines.iter_mut().zip([
            "CONNECTION CHECK",
            "",
            "SPEED 65535 WAITING FOR RADIO",
            "HEART 65535 CONNECTED",
            "POWER --",
            "RADAR 65535 SEARCHING",
            "GPS SEARCHING",
            "STORAGE 99:59 LEFT",
            "RADIO WAITING FOR RADIO",
        ]) {
            let _ = line.push_str(text);
        }
        view.prepare();
        let content = Rect {
            left: 0,
            top: CONTENT.top,
            right: WIDTH,
            bottom: 320,
        };
        let want = [
            "CONNECTION CHECK",
            "65535 WAITING FOR RADIO",
            "65535 CONNECTED",
            "WAITING FOR RADIO",
        ];
        view.canvas.assert_laid_out(content, &want, "preflight");

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
