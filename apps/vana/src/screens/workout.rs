//! VANA workout rendering: the chrome around whichever screen is shown.
use super::Device;
use super::canvas::Canvas;
use super::chrome::{BLACK, Battery, Gps, Look, Mark, Row, StatusBar, Toast};
#[cfg(test)]
use super::chrome::{CONTENT, HEIGHT, WIDTH};
use super::cue::Cue;
use super::home::Home;
use super::ride::{self, ClimbFrame, ClimbLent, Marks, RidePage};
use super::sign;
use crate::clock::Local;
use crate::ride::course::nav::Sign;
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
    Ride,
    /// "End ride?" over a paused ride.
    EndRide,
    /// The saved ride as a treat, until a key or a tap.
    Done,
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
            Self::Ride => "ride",
            Self::EndRide => "end_ride",
            Self::Done => "ride_done",
            Self::LoseRides => "lose_rides",
        }
    }
}

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
}

impl Body {
    fn canvas(&self) -> Option<&Canvas> {
        match self {
            Self::Canvas(canvas) => Some(canvas),
            _ => None,
        }
    }
}

/// What the shown page prepares besides its body. A ride page's extras are
/// its chart or map; a menu or lifecycle page's are its lines. No page
/// reads two: the map page's inset is canvas shapes.
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
    /// The track, projected and kept beside the breadcrumb.
    Map(Option<ride::FilledMap<'a>>),
    /// What Home's cards show.
    Home(Home),
    /// The sensor pages.
    Sensors(&'a super::sensors::Menu),
    Done(&'a super::done::Done),
}

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
        map -> Option<&'a ride::MapFrame> = Self::Map(map) => map.map(|map| map.frame());
        home -> &Home = Self::Home(home) => home;
        sensors -> &'a super::sensors::Menu = Self::Sensors(menu) => *menu;
        done -> &'a super::done::Done = Self::Done(done) => *done;
    }
}

#[derive(Hash)]
pub struct Scene<'v> {
    device: &'static Device,
    kind: Kind,
    look: Look,
    /// The look a kit page paints in, unmapped; legacy pages and the
    /// overlays paint through `look`.
    theme: Option<&'static super::theme::Theme>,
    pub(super) bar: Option<&'v StatusBar>,
    /// The rows the main page's tree gives its status bar.
    main_bar: Range<usize>,
    body: &'v Body,
    painted: Painted<'v>,
    pub(super) radar: Option<ride::Radar>,
    cues: &'v [Cue],
    toast: Option<&'v Toast>,
    sign: Option<sign::Placed>,
    over: Option<super::sheet::Over<'v>>,
}

#[derive(Clone, Copy, Debug, Hash)]
enum Kind {
    PageUnderBar,
    Ride(RidePage),
}

#[derive(Clone, Copy, Hash)]
enum Painted<'v> {
    None,
    Climb(&'v ClimbFrame),
    Workout(&'v ride::WorkoutChart),
    Home(&'v super::home::Strip),
    Map(Option<&'v ride::FilledMap<'v>>),
    Done(&'v super::done::Done),
}

impl<'v> Painted<'v> {
    picks! {
        climb -> &'v ClimbFrame = Self::Climb(frame) => *frame;
        chart -> &'v ride::WorkoutChart = Self::Workout(chart) => *chart;
        map -> Option<&'v ride::FilledMap<'v>> = Self::Map(map) => *map;
    }
}

impl Extra<'_> {
    fn painted(&self) -> Painted<'_> {
        match self {
            Self::None => Painted::None,
            Self::Climb { lent: _, frame } => Painted::Climb(frame),
            Self::Workout(chart) => Painted::Workout(chart),
            Self::Map(map) => Painted::Map(map.as_ref()),
            Self::Home(home) => Painted::Home(&home.strip),
            Self::Sensors(_) => Painted::None,
            Self::Done(done) => Painted::Done(done),
        }
    }
}

#[derive(Hash)]
struct WholePage<'v> {
    kind: Kind,
    look: Look,
    radar: bool,
    cues: &'v [Cue],
    over: Option<super::sheet::Over<'v>>,
}

impl firmware_shell::shell::Picture for Scene<'_> {
    fn paint(&self, row: &mut Row) {
        let row = &mut row.looked(self.look);
        match self.theme {
            Some(theme) => row.plain().fill(theme.bg),
            None => row.fill(BLACK),
        }
        self.paint_content(row);
        if let Some(sign) = &self.sign {
            sign.paint(&mut row.plain());
        }
        if let Some(over) = &self.over {
            over.paint(row, self.canvas(), self.device);
        }
    }

    fn parts(&self, parts: &mut firmware_shell::shell::Parts) {
        let height = self.device.scaled.panel.height;
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
                over: self.over,
            },
        );
        let toast = super::chrome::toast_band(&self.device.scaled);
        parts.add(self.bar_rows(), &self.bar);
        parts.add(toast.top..toast.bottom, &self.toast);
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
            Kind::PageUnderBar => {
                if let (Painted::Done(done), Some(theme)) = (self.painted, self.theme) {
                    done.paint(&mut row.plain(), theme);
                }
                self.paint_canvas(row);
                if let (Painted::Home(strip), Some(theme)) = (self.painted, self.theme) {
                    strip.paint(theme, &mut row.plain());
                }
            }
            Kind::Ride(page) => ride::paint(self, page, row),
        }
        for cue in self.cues {
            cue.paint(row, self.device);
        }
        if let Some(toast) = self.toast {
            toast.paint(row);
        }
    }

    #[cfg(test)]
    pub(super) fn look(&self) -> Look {
        self.look
    }

    #[cfg(test)]
    pub(super) fn theme(&self) -> Option<&'static super::theme::Theme> {
        self.theme
    }

    fn bar_rows(&self) -> Range<usize> {
        match self.kind {
            Kind::Ride(_) if self.theme.is_some() => 0..0,
            Kind::Ride(RidePage::Main) => self.main_bar.clone(),
            _ => 0..crate::compose::tiles::status::rows(&self.device.scaled),
        }
    }

    #[cfg(test)]
    pub(crate) fn pixel(&self, x: usize, y: usize) -> u16 {
        use firmware_shell::shell::Picture;
        let mut pixels = std::vec![0u16; self.device.scaled.panel.width];
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

    /// The tiles' canvas, a kit page's in its own colours.
    pub(super) fn paint_canvas(&self, row: &mut Row) {
        match self.theme {
            Some(_) => self.canvas().paint(&mut row.plain()),
            None => self.canvas().paint(row),
        }
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

    #[cfg(test)]
    pub(super) fn reference_pixel(&self, x: usize, y: usize) -> super::chrome::Shown {
        let color = self.reference_content(x, y);
        let color = self
            .sign
            .as_ref()
            .and_then(|sign| sign.pixel(x, y, color))
            .unwrap_or(color);
        match &self.over {
            Some(over) => over.pixel(self.look(), x, y, color, self.canvas(), self.device),
            None => color,
        }
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
            Kind::PageUnderBar => match self.theme {
                Some(theme) => {
                    let under = match self.painted {
                        Painted::Home(strip) => strip.pixel(theme, x, y),
                        _ => None,
                    };
                    let bg = super::chrome::Shown(theme.bg);
                    let color = self.canvas().pixel(Look::PLAIN, x, y, bg);
                    under.map_or(color, super::chrome::Shown)
                }
                None => self.canvas().pixel(look, x, y, black),
            },
            Kind::Ride(page) => ride::reference_pixel(self, page, x, y),
        };
        for cue in self.cues {
            color = cue.pixel(look, x, y, color, self.device);
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
            canvas.assert_kept(format_args!("{:?} on {:?}", self.kind, self.device.scaled));
        }
        if self.bar.is_some() {
            for cue in self.cues {
                assert!(
                    !cue.covers(
                        crate::screens::chrome::status_bar(&self.device.scaled),
                        self.device
                    ),
                    "the {:?} cue on {:?} is under the status bar",
                    cue.icon,
                    self.device.scaled
                );
            }
        }
        let panel = self.device.scaled.screen();
        if let Kind::Ride(page) = self.kind {
            let breaks = self.canvas_breaks(page);
            assert!(
                breaks.is_empty(),
                "{} on {:?} breaks its layout:\n{}",
                page.name(),
                self.device.scaled,
                breaks.join("\n")
            );
            return;
        }
        if let Kind::PageUnderBar = self.kind {
            let regions = match self.bar {
                Some(_) => std::vec![
                    (
                        "status bar",
                        crate::screens::chrome::status_bar(&self.device.scaled)
                    ),
                    ("page", crate::screens::chrome::content(&self.device.scaled)),
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
                self.device.scaled,
                breaks.join("\n")
            );
        }
    }
}

#[cfg(any(test, feature = "invariants"))]
impl Scene<'_> {
    fn canvas_breaks(&self, page: RidePage) -> std::vec::Vec<std::string::String> {
        if self.theme.is_some() {
            return self.breaks_within(std::vec![("page", self.device.scaled.screen())], 0);
        }
        let mut regions = std::vec::Vec::new();
        regions.push((
            "status bar",
            crate::screens::chrome::status_bar(&self.device.scaled),
        ));
        let page_first = regions.len() + usize::from(self.radar.is_some());
        regions.extend(ride::canvas_regions(self, page));
        self.breaks_within(regions, page_first)
    }

    fn breaks_within(
        &self,
        regions: std::vec::Vec<(&str, super::chrome::Rect)>,
        page_first: usize,
    ) -> std::vec::Vec<std::string::String> {
        self.breaks_in(regions, page_first, self.device.scaled.screen())
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

    fn under_cues(
        &self,
        items: impl Iterator<Item = (std::string::String, super::chrome::Rect)>,
    ) -> std::vec::Vec<std::string::String> {
        let mut breaks = std::vec::Vec::new();
        for (name, rect) in items {
            for cue in self.cues.iter().filter(|cue| cue.covers(rect, self.device)) {
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
    pub(super) fn device(&self) -> &'static Device {
        self.device
    }
}

static NO_CANVAS: Canvas = Canvas::EMPTY;
static NO_CLIMB_FRAME: ClimbFrame = ClimbFrame::EMPTY;
static NO_CHART: ride::WorkoutChart = ride::WorkoutChart::EMPTY;

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
    pub device: &'static Device,
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
    /// The main page's and the shown screen's tiles, as the runtime solved them.
    pub laids: Option<&'a crate::compose::builtin::Laids>,
    /// What the shown page prepared for this frame.
    pub(super) body: Body,
    /// What the shown ride page prepared besides its body.
    pub(super) extra: Extra<'a>,
    pub zone_time: ZoneTime,
    pub profile: TrainingProfile,
    /// Weight, age and sex, for the calories.
    pub rider: Rider,
    /// Zone colours and gauges prepared for this frame.
    pub(super) marks: Marks,
    /// The workout chosen or running, for the workout page and the target
    /// band on the main page.
    pub training: Option<ride::Training>,
    pub format: super::format::Shows,
    pub page: Page,
    /// The focused Home card.
    pub cursor: usize,
    pub time: Text<8>,
    /// The light look is in force: the QR panel is painted inverted so the
    /// look turns it back to dark modules on light.
    pub light: bool,
    /// How long the ride has been paused.
    pub paused_secs: u32,
    /// The paused ride shows the paused screen rather than a ride page.
    pub pause_screen: bool,
    /// Rides the phone has not confirmed, which LOSE n UNSAVED RIDES? counts.
    pub unsaved: u8,
    /// Button cues prepared for this frame.
    pub(super) cues: Cues,
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
            device: &super::C606,
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
            laids: None,
            body: Body::Blank,
            extra: Extra::None,
            zone_time: ZoneTime::default(),
            profile: TrainingProfile::default(),
            rider: Rider::default(),
            marks: Marks::default(),
            training: None,
            format: super::format::Shows::Wait,
            page,
            cursor,
            time,
            light: false,
            paused_secs: 0,
            pause_screen: false,
            unsaved: 0,
            cues: Cues::new(),
            nav: Sign::None,
            course: false,
        }
    }
    /// Solve the built-ins for this view, as the runtime's composition does.
    #[cfg(test)]
    pub(crate) fn compose_main(&mut self) {
        self.compose_focused(None);
    }

    /// Solve the built-ins, with the focus on stop `stop` of the screen
    /// shown.
    #[cfg(test)]
    pub(crate) fn compose_focus(&mut self, stop: usize) {
        self.compose_focused(Some(stop));
    }

    #[cfg(test)]
    fn compose_focused(&mut self, stop: Option<usize>) {
        use crate::compose::builtin::Laids;
        let built_in = crate::compose::Screens::EMPTY;
        let mut laids = Laids::new(self.device, ride::main_flags_of(self), &built_in);
        let shown = self.screen_id();
        laids.show(shown, self.device, &built_in);
        if let Some(laid) = laids.get_mut(shown) {
            let node = stop.and_then(|stop| laid.solved().focus_order().nth(stop).map(|p| p.node));
            if let Some(node) = node {
                *laid.focus_mut() = crate::compose::input::Focus::Tile(node);
            }
        }
        self.laids = Some(std::boxed::Box::leak(std::boxed::Box::new(laids)));
    }

    #[cfg(test)]
    pub(crate) fn pull_sheet(&mut self, facts: super::sheet::Facts) {
        let mut laids = self.laids.expect("composed first").clone();
        laids.open_sheet(self.device);
        laids.set_sheet_facts(facts);
        self.laids = Some(std::boxed::Box::leak(std::boxed::Box::new(laids)));
    }

    /// The main ride page as the composition solved it.
    pub fn main(&self) -> Option<&'a crate::compose::builtin::Laid> {
        self.laids.map(crate::compose::builtin::Laids::main)
    }

    pub fn show_done(&mut self, done: &'a super::done::Done) {
        self.extra = Extra::Done(done);
    }

    pub(super) fn done(&self) -> Option<&'a super::done::Done> {
        self.extra.done()
    }
    /// Show the sensor page `menu` is on.
    pub fn show_sensors(&mut self, menu: &'a super::sensors::Menu) {
        self.extra = Extra::Sensors(menu);
    }

    /// The sensor pages, while one shows.
    pub(super) fn sensor_pages(&self) -> Option<&'a super::sensors::Menu> {
        self.extra.sensors()
    }

    /// Whether the tree shown keeps the status bar's band for the bar, so
    /// the bar covers none of its tiles.
    fn bar_has_room(&self) -> bool {
        let bar = crate::screens::chrome::status_bar(&self.device.scaled);
        let status = self
            .composed()
            .and_then(|laid| laid.box_of(crate::compose::registry::STATUS));
        status.is_some_and(|room| room.top <= bar.top && room.bottom >= bar.bottom)
    }

    /// Show what Home's cards say.
    pub fn set_home(&mut self, home: Home) {
        self.extra = Extra::Home(home);
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

    #[cfg(test)]
    pub(crate) fn home_mut(&mut self) -> &mut Home {
        match &mut self.extra {
            Extra::Home(home) => home,
            _ => panic!("Home's facts are set first"),
        }
    }
    /// What Home's cards show, once the runtime set it.
    pub(super) fn home(&self) -> Option<&Home> {
        self.extra.home()
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
    pub fn show_map(&mut self, map: Option<ride::FilledMap<'a>>) {
        self.extra = Extra::Map(map);
    }

    fn full_screen(&self) -> bool {
        match self.page {
            Page::Boot
            | Page::Format
            | Page::Home
            | Page::EndRide
            | Page::Done
            | Page::LoseRides => true,
            Page::Sensors => !self.bar_has_room(),
            _ => self.paused_screen_shown(),
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
        let over = self.pulled().map(|(_, facts, bottom)| super::sheet::Over {
            bottom,
            cues: &self.cues,
            theme: crate::compose::data::Data::theme(facts),
        });
        let sheet = self.page == Page::Boot;
        let kind = match self.page {
            _ if sheet => Kind::PageUnderBar,
            Page::Ride if !self.pause_screen => Kind::Ride(self.ride_page),
            _ => Kind::PageUnderBar,
        };
        let device = self.device;
        let radar = match kind {
            Kind::Ride(_) => self
                .radar_paired
                .then(|| ride::radar(&self.graphics.radar, &device.scaled)),
            _ => None,
        };
        let themed =
            self.screen_id().themed() && matches!(kind, Kind::Ride(_) | Kind::PageUnderBar);
        Scene {
            device,
            kind,
            look: super::look::look(self.light),
            theme: themed.then(|| crate::compose::data::Data::theme(self)),
            bar: (!sheet && !themed && !self.full_screen()).then_some(&self.bar),
            main_bar: self
                .main()
                .and_then(ride::main_status_bar)
                .map_or(0..0, |bar| bar.top..bar.bottom),
            body: &self.body,
            painted: self.extra.painted(),
            radar,
            cues: if over.is_some() { &[] } else { &self.cues },
            toast: self.toast.as_ref().filter(|_| !sheet),
            sign: match kind {
                Kind::Ride(page) => sign::on_page(
                    page,
                    self.nav,
                    self.radar_paired,
                    device,
                    self.main().and_then(ride::main_chip),
                ),
                _ => None,
            },
            over,
        }
    }

    fn pulled(
        &self,
    ) -> Option<(
        &'a crate::compose::builtin::Laid,
        &'a super::sheet::Facts,
        usize,
    )> {
        let (laid, facts) = self.laids?.sheet_with_facts()?;
        Some((laid, facts, super::sheet::bottom(laid, &self.device.scaled)))
    }

    pub fn prepare(&mut self) -> Scene<'_> {
        let mark = match self.state {
            Status::Recording => Mark::Recording,
            Status::Paused if self.auto_paused => Mark::AutoPaused,
            Status::Paused => Mark::Paused,
            _ => Mark::None,
        };
        let device = self.device;
        self.bar = StatusBar::placed(&self.time, self.gps, self.battery, mark, &device.scaled);
        if device.scaled != super::scaled::BASE {
            self.toast = self.toast.map(|toast| toast.on(device));
        }
        self.cues = match self.composed() {
            Some(laid) => self.prepare_composed(laid),
            None => Cues::new(),
        };
        if let Some((laid, facts, bottom)) = self.pulled() {
            self.cues = laid.controls().shown(laid.focus()).collect();
            if let Body::Canvas(canvas) = &mut self.body {
                super::sheet::prepare(laid, facts, bottom, canvas);
            }
        }
        self.scene()
    }

    /// The built-in shown, once the composition lent its screens and the
    /// screen has a tree.
    pub(crate) fn composed(&self) -> Option<&'a crate::compose::builtin::Laid> {
        self.laids?.get(self.screen_id())
    }

    fn screen_id(&self) -> crate::compose::builtin::ScreenId {
        let id = crate::compose::builtin::ScreenId::of(
            self.page,
            self.ride_page,
            self.pause_screen,
            self.asks(),
        );
        match self.sensor_pages() {
            Some(menu) if id == crate::compose::builtin::ScreenId::Sensors => menu.screen(),
            _ => id,
        }
    }

    fn asks(&self) -> crate::compose::builtin::Asks {
        use crate::compose::builtin::Asks;
        match self.page {
            Page::Format => self.format.asks(),
            Page::Home if self.home().is_some_and(|home| home.today.is_some()) => Asks::Plan,
            _ => Asks::Own,
        }
    }

    /// Draw `laid` from its solved tiles; its cues are the ones its focus
    /// shows.
    fn prepare_composed(&mut self, laid: &crate::compose::builtin::Laid) -> Cues {
        let riding = self.page == Page::Ride && !self.pause_screen;
        if riding {
            let pages = ride::Deck::new(self.training.is_some());
            self.bar
                .set_pages(pages.position(self.ride_page), pages.count());
            self.marks = ride::marks(self);
            ride::prepare_tiles(self, laid);
        }
        fill(self, |view| {
            let mut canvas = Canvas::default();
            laid.prepare(view, &mut canvas);
            canvas
        });
        if riding {
            ride::hide_banner(self, Some(laid));
        }
        laid.controls().shown(laid.focus()).collect()
    }
}

/// The button cues a frame shows.
pub type Cues = heapless::Vec<Cue, 3, u8>;

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
        self.bytes[self.len..][..value.len()].copy_from_slice(value.as_bytes());
        self.len += value.len();
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

    use crate::screens::chrome::{AMBER, GREEN, RAISED, Rect, STATUS_BAR, TOAST, Tone, WHITE};

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
        assert!(Extra::None.home().is_none());
        let none = Extra::None;
        assert!(none.climb_lent().is_none());
        assert!(none.climb_frame().is_none() && none.chart().is_none());
        assert!(none.map().is_none());
        assert!(NO_CANVAS.runs.is_empty() && NO_CANVAS.shapes.is_empty());
        assert!(ClimbLent::NONE.climbs.climbing().is_none());
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "has no map")]
    fn a_page_asking_for_another_pages_extra_trips_the_assertion() {
        let _ = View::new(Page::Ride, 0, None).map();
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
            view.compose_main();
            view.prepare();
            view
        };
        let sent = |before: &View, after: &View| {
            let sent = Parts::of(&before.scene()).changed(&Parts::of(&after.scene()));
            (0..HEIGHT)
                .filter(|&y| sent.contains(y))
                .collect::<std::vec::Vec<_>>()
        };
        for page in [RidePage::Main, RidePage::Climb] {
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
        assert!(
            !ticked.is_empty()
                && ticked
                    .iter()
                    .all(|&y| y < usize::from(crate::screens::theme::LAYOUT.space.top_line)),
            "only the top line's time sends above the banner: {ticked:?}"
        );
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
        for page in [RidePage::Main, RidePage::Climb] {
            for radar_paired in [false, true] {
                let what = std::format!("{page:?} radar {radar_paired}");
                let bare = view(page, Sign::None, radar_paired);
                let under = view(page, turn, radar_paired);
                let banner = sign::on_page(page, turn, radar_paired, under.device, None)
                    .unwrap()
                    .rect();
                let canvas = bare.canvas();
                // Stats hides only words the banner crosses; its rules and
                // fills show on past the banner's edge.
                let kit = page == RidePage::Main;
                let shapes = canvas
                    .shapes
                    .iter()
                    .filter(|_| !kit)
                    .map(|shape| shape.bounds());
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
                if page == RidePage::Climb {
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
            view.ride_page = RidePage::Main;
            view.course = true;
            view.nav = turn;
            view.light = light;
            view.compose_main();
            view.prepare();
            rows(&view)
        };
        let (dark, light) = (page(false), page(true));
        let rect = sign::on_page(RidePage::Main, turn, false, &crate::screens::C606, None)
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
        let cases: [(&str, &Change<'_>); 6] = [
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
                view.cues = [
                    Cue::new(device_api::input::Role::Back, super::super::cue::Icon::Stop),
                    Cue::new(device_api::input::Role::Act, super::super::cue::Icon::Play),
                ]
                .into_iter()
                .collect()
            }),
        ];
        for (what, change) in cases {
            assert_parts_cover(&base, &ride(change), what);
        }
    }

    #[test]
    fn home_opens_on_its_start_and_rings_what_has_the_focus() {
        use crate::compose::builtin::ScreenId;
        for today in [false, true] {
            let mut view = home_view(today, false);
            view.compose_main();
            view.prepare();
            let id = if today {
                ScreenId::HomeToday
            } else {
                ScreenId::Home
            };
            let laid = view.laids.and_then(|laids| laids.get(id));
            let solved = laid.unwrap().solved();
            let start = solved
                .focus_order()
                .find(|p| Some(p.node) == solved.opening());
            let start = start.unwrap().rect.rect();
            let ring = (start.center_x(), start.top - 4);
            assert_eq!(view.pixel(ring.0, ring.1), super::super::theme::DARK.focus);
            let widget = solved.focus_order().next().unwrap().rect.rect();
            let above = (widget.center_x(), widget.top - 4);
            assert_eq!(
                view.pixel(above.0, above.1),
                0,
                "{today}: the widget has none"
            );
        }
    }

    const DAY_7_10_PM: u64 = 20_781 * 86_400 + 19 * 3_600 + 10 * 60;

    fn home_view(today: bool, light: bool) -> View<'static> {
        use crate::compose::builtin::ScreenId;
        use crate::compose::registry::TODAY;
        use crate::screens::home::{Strip, Today};
        use crate::workout::fixtures::THRESHOLD;
        use crate::workout::shape::Shape;
        let mut view = View::new(Page::Home, 0, Some(Local(1_000_000)));
        view.light = light;
        view.set_home(Home {
            local: Some(Local(DAY_7_10_PM)),
            today: today.then(|| Today {
                name: THRESHOLD.name,
                planned_secs: THRESHOLD.planned_secs(),
            }),
            ..Home::default()
        });
        view.compose_main();
        let card = view
            .laids
            .and_then(|laids| laids.get(ScreenId::HomeToday)?.box_of(TODAY));
        if let Some(card) = card {
            let rect = crate::compose::tiles::home::strip(card, &view.device.scaled);
            let shape = Shape::of(&THRESHOLD);
            view.home_mut().strip = Strip::lay(shape.pieces(), rect);
        }
        view
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
        for (cursor, (today, light)) in [(false, false), (true, false), (true, true), (false, true)]
            .into_iter()
            .enumerate()
        {
            let mut view = home_view(today, light);
            view.home_mut().sensors = glance[..cursor.min(2)].iter().copied().collect();
            view.compose_focus(cursor % 2);
            view.battery = Some(Battery::level(14));
            view.gps = [Gps::Fixed, Gps::Searching][cursor % 2];
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
                    RidePage::Workout => view.show_workout(ride::WorkoutChart::default()),
                    RidePage::Climb => view.show_climb(ClimbLent::NONE),
                    RidePage::Map => view.show_map(None),
                    RidePage::Main => {}
                }
                view.compose_main();
                view.prepare();
                assert_paints_reference(
                    &view,
                    &std::format!("{page:?} radar {radar_paired} {nav:?} light {light}"),
                );
            }
        }
        for light in [false, true] {
            let mut view = View::new(Page::Sensors, 1, None);
            view.light = light;
            let menu = std::boxed::Box::leak(std::boxed::Box::default());
            crate::screens::sensors::Menu::open(menu);
            view.show_sensors(menu);
            view.compose_main();
            view.prepare();
            assert_paints_reference(&view, "text page");
        }
        for light in [false, true] {
            let mut view = View::new(Page::Boot, 0, None);
            view.light = light;
            view.compose_main();
            view.prepare();
            assert_paints_reference(&view, "boot");
        }
    }

    #[test]
    fn lifecycle_screens_paint_their_reference() {
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
    }

    #[test]
    fn a_boot_view_paints_the_brand() {
        let mut view = View::new(Page::Boot, 0, None);
        view.compose_main();
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
        view.compose_main();
        view.prepare();
        let screen = Rect { top: 0, ..content };
        view.canvas()
            .assert_laid_out(screen, &["VANA", "LABS"], "boot");
    }

    #[test]
    fn text_keeps_its_case_and_is_bounded() {
        let mut line = Text::<4>::new();
        assert!(line.push_str("Ok").is_ok());
        assert!(line.push_str("abc").is_err());
        assert_eq!(line.as_bytes(), b"Ok");
    }
}
