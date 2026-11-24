//! The screens the firmware ships: one file per screen, each naming its
//! tree and buttons in `BUILTIN`, and the table that finds them by
//! [`ScreenId`].
use super::data::Data;
use super::input::{self, Buttons, Controls, Focus, Stop};
use super::node::{Flags, MAX_NODES, Node, What};
use super::parse::{Screen, Screens};
use super::registry;
use super::solve::{MAX_LEAVES, Placed, Rect16, Scroll, Solved};
use crate::screens::Device;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Rect;
use crate::screens::cue::Keys;
use crate::screens::ride::RidePage;
use crate::screens::ride::slots::Slots;
use crate::screens::ride::summary::Shown;
use crate::screens::workout::Page;

mod bests;
mod brand;
mod control_sheet;
mod discard;
mod form;
mod format;
mod history;
mod home;
mod lose_rides;
mod past_best;
mod past_delete;
mod past_effort;
mod past_elevation;
mod past_message;
mod past_overview;
mod phone;
mod power;
mod question;
mod ready;
mod ride_climb;
mod ride_end;
mod ride_lap;
mod ride_main;
mod ride_map;
mod ride_paused;
mod ride_workout;
mod sensors;
mod settings;
mod setup;
mod sheets;
mod summary_best;
mod summary_effort;
mod summary_elevation;
mod summary_message;
mod summary_offer;
mod summary_overview;
#[cfg(test)]
mod tests;
mod workouts;

pub use ride_main::{GLIDES, tree as main_tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenId {
    RideMain,
    RideLap,
    RideClimb,
    RideWorkout,
    RideMap,
    RidePaused,
    RideEnd,
    SummaryMessage,
    SummaryOverview,
    SummaryEffort,
    SummaryElevation,
    SummaryBest,
    SummaryOffer,
    PastMessage,
    PastOverview,
    PastEffort,
    PastElevation,
    PastBest,
    PastDelete,
    Home,
    Ready,
    Setup,
    Phone,
    Bests,
    Form,
    Sheets,
    Brand,
    Format,
    LoseRides,
    Discard,
    Workouts,
    History,
    Settings,
    Sensors,
    ClearBests,
    BestsNote,
    FormNote,
    FormatWait,
    Discarding,
    TurnOff,
    SyncFirst,
    AutoOff,
    TurningOff,
    Charging,
    SensorsNearby,
    SensorDetail,
    PowerMeter,
    ForgetSensor,
    ControlSheet,
}

/// Which of its screens a page shows where it has more than one: its own,
/// the question it asks over itself, a note in place of what it lists, or
/// its owner's work under way, when its buttons rest.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Asks {
    #[default]
    Own,
    Question,
    Note,
    Wait,
}

impl ScreenId {
    pub const COUNT: usize = 50;
    pub const ALL: [Self; Self::COUNT] = [
        Self::RideMain,
        Self::RideLap,
        Self::RideClimb,
        Self::RideWorkout,
        Self::RideMap,
        Self::RidePaused,
        Self::RideEnd,
        Self::SummaryMessage,
        Self::SummaryOverview,
        Self::SummaryEffort,
        Self::SummaryElevation,
        Self::SummaryBest,
        Self::SummaryOffer,
        Self::PastMessage,
        Self::PastOverview,
        Self::PastEffort,
        Self::PastElevation,
        Self::PastBest,
        Self::PastDelete,
        Self::Home,
        Self::Ready,
        Self::Setup,
        Self::Phone,
        Self::Bests,
        Self::Form,
        Self::Sheets,
        Self::Brand,
        Self::Format,
        Self::LoseRides,
        Self::Discard,
        Self::Workouts,
        Self::History,
        Self::Settings,
        Self::Sensors,
        Self::ClearBests,
        Self::BestsNote,
        Self::FormNote,
        Self::FormatWait,
        Self::Discarding,
        Self::TurnOff,
        Self::SyncFirst,
        Self::AutoOff,
        Self::TurningOff,
        Self::Charging,
        Self::SensorsNearby,
        Self::SensorDetail,
        Self::PowerMeter,
        Self::ForgetSensor,
        Self::ControlSheet,
    ];

    pub const fn of(
        page: Page,
        ride: RidePage,
        paused: bool,
        sheet: bool,
        asks: Asks,
        summary: Shown,
    ) -> Self {
        if sheet {
            return Self::Sheets;
        }
        match page {
            Page::Boot => Self::Brand,
            Page::Format if matches!(asks, Asks::Wait) => Self::FormatWait,
            Page::Format => Self::Format,
            Page::Home => Self::Home,
            Page::Sensors => Self::Sensors,
            Page::Ready => Self::Ready,
            Page::Ride if paused => Self::RidePaused,
            Page::Ride => match ride {
                RidePage::Main => Self::RideMain,
                RidePage::Lap => Self::RideLap,
                RidePage::Climb => Self::RideClimb,
                RidePage::Workout => Self::RideWorkout,
                RidePage::Map => Self::RideMap,
            },
            Page::EndRide => Self::RideEnd,
            Page::DiscardRide if matches!(asks, Asks::Wait) => Self::Discarding,
            Page::DiscardRide => Self::Discard,
            Page::Summary => match summary {
                Shown::Message | Shown::Delete => Self::SummaryMessage,
                Shown::Overview => Self::SummaryOverview,
                Shown::Effort => Self::SummaryEffort,
                Shown::Elevation => Self::SummaryElevation,
                Shown::Best => Self::SummaryBest,
                Shown::Offer => Self::SummaryOffer,
            },
            Page::PastRide => match summary {
                Shown::Message | Shown::Offer => Self::PastMessage,
                Shown::Overview => Self::PastOverview,
                Shown::Effort => Self::PastEffort,
                Shown::Elevation => Self::PastElevation,
                Shown::Best => Self::PastBest,
                Shown::Delete => Self::PastDelete,
            },
            Page::Settings => Self::Settings,
            Page::Bests => match asks {
                Asks::Question => Self::ClearBests,
                Asks::Note => Self::BestsNote,
                Asks::Own | Asks::Wait => Self::Bests,
            },
            Page::Form if matches!(asks, Asks::Note) => Self::FormNote,
            Page::Form => Self::Form,
            Page::Workouts => Self::Workouts,
            Page::Phone => Self::Phone,
            Page::History => Self::History,
            Page::Setup => Self::Setup,
            Page::LoseRides => Self::LoseRides,
        }
    }

    /// The key a config names the screen by.
    pub const fn key(self) -> u8 {
        self as u8
    }

    /// The screen a config names by `key`.
    pub fn of_key(key: u8) -> Option<Self> {
        Self::ALL.get(usize::from(key)).copied()
    }

    /// The buttons a changed screen `key` keeps, its built-in's, and the
    /// tabs they show beside `keys`; none for a key that names no screen.
    pub fn controls(key: u8, keys: Keys) -> Controls {
        match Self::of_key(key) {
            Some(id) => Controls::new(id.builtin().buttons, keys),
            None => Controls::FREE,
        }
    }

    /// The screens drawn from the kit, which paint in the theme's colours
    /// rather than through the light transform.
    pub const fn themed(self) -> bool {
        matches!(self, Self::RideMain)
    }

    /// The pages whose built-in must set MOVE to `Go::Next`.
    pub const fn deck(self) -> bool {
        matches!(
            self,
            Self::RideMain
                | Self::RideLap
                | Self::RideClimb
                | Self::RideWorkout
                | Self::RideMap
                | Self::SummaryMessage
                | Self::SummaryOverview
                | Self::SummaryEffort
                | Self::SummaryElevation
                | Self::SummaryBest
                | Self::SummaryOffer
                | Self::PastMessage
                | Self::PastOverview
                | Self::PastEffort
                | Self::PastElevation
                | Self::PastBest
                | Self::PastDelete
        )
    }

    pub const fn builtin(self) -> &'static Builtin {
        match self {
            Self::RideMain => ride_main::BUILTIN,

            Self::RideLap => ride_lap::BUILTIN,

            Self::RideClimb => ride_climb::BUILTIN,

            Self::RideWorkout => ride_workout::BUILTIN,

            Self::RideMap => ride_map::BUILTIN,

            Self::RidePaused => ride_paused::BUILTIN,

            Self::RideEnd => ride_end::BUILTIN,

            Self::SummaryMessage => summary_message::BUILTIN,

            Self::SummaryOverview => summary_overview::BUILTIN,

            Self::SummaryEffort => summary_effort::BUILTIN,

            Self::SummaryElevation => summary_elevation::BUILTIN,

            Self::SummaryBest => summary_best::BUILTIN,

            Self::SummaryOffer => summary_offer::BUILTIN,

            Self::PastMessage => past_message::BUILTIN,

            Self::PastOverview => past_overview::BUILTIN,

            Self::PastEffort => past_effort::BUILTIN,

            Self::PastElevation => past_elevation::BUILTIN,

            Self::PastBest => past_best::BUILTIN,

            Self::PastDelete => past_delete::BUILTIN,

            Self::Home => home::BUILTIN,

            Self::Ready => ready::BUILTIN,

            Self::Setup => setup::BUILTIN,

            Self::Phone => phone::BUILTIN,

            Self::Bests => bests::BUILTIN,

            Self::Form => form::BUILTIN,

            Self::Sheets => sheets::BUILTIN,

            Self::Brand => brand::BUILTIN,

            Self::Format => format::BUILTIN,

            Self::LoseRides => lose_rides::BUILTIN,

            Self::Discard => discard::BUILTIN,

            Self::Workouts => workouts::BUILTIN,

            Self::History => history::BUILTIN,

            Self::Settings => settings::BUILTIN,

            Self::Sensors => sensors::BUILTIN,

            Self::ClearBests => bests::CLEAR,

            Self::BestsNote => bests::NOTE,

            Self::FormNote => form::NOTE,

            Self::FormatWait => format::WAIT,

            Self::Discarding => discard::SAVING,

            Self::TurnOff => power::TURN_OFF,

            Self::SyncFirst => power::SYNC_FIRST,

            Self::AutoOff => power::AUTO_OFF,

            Self::TurningOff => power::TURNING_OFF,

            Self::Charging => power::CHARGING,
            Self::SensorsNearby => sensors::NEARBY,

            Self::SensorDetail => sensors::DETAIL,

            Self::PowerMeter => sensors::POWER_METER,

            Self::ForgetSensor => sensors::FORGET,

            Self::ControlSheet => control_sheet::BUILTIN,
        }
    }
}

/// The rows a static screen keeps for the status bar and the black under
/// it. The scene paints the bar, or leaves it out when a cue sits at the
/// top, so the band holds no tile a cue could crowd.
const BAR_UNITS: u8 = 24;

/// A screen's nodes, preorder.
pub type Nodes = heapless::Vec<Node, MAX_NODES>;

/// What the rider chose that shapes a built-in's tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub main_slots: Slots,
    /// Settings edits a number: its tree is the editor's.
    pub editing: bool,
}

/// A shipped screen: its tree for the rider's choices, and what its
/// buttons do. A test accepts every one on every panel under every flag
/// combination it mentions; a tree that reads `Choices` tests each choice.
pub struct Builtin {
    pub tree: fn(&Choices) -> Nodes,
    pub buttons: Buttons,
}

/// One screen solved on one device for the rider's choices and what the
/// ride shows: where each tile goes. Its tree is the rider's when they
/// changed the screen, else the built-in's. A frame draws from it; input
/// solves it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Laid {
    id: ScreenId,
    screen: Screen,
    /// `screen` is the rider's, not the built-in's for `choices`.
    changed: bool,
    choices: Choices,
    flags: Flags,
    leaves: heapless::Vec<Placed, MAX_LEAVES>,
    /// The scroll shown, and the page of cards it scrolls.
    list: Option<(Scroll, crate::screens::scroll::Layout)>,
    focus: Focus,
}

impl Laid {
    /// `id` on `device`, if it has a built-in, showing `changed`
    /// when the rider changed it.
    #[inline(never)]
    pub fn new(
        id: ScreenId,
        device: &Device,
        choices: Choices,
        flags: Flags,
        changed: Option<&Screen>,
    ) -> Option<Self> {
        let builtin = id.builtin();
        let screen = match changed {
            Some(screen) => screen.clone(),
            None => {
                let controls = Controls::new(builtin.buttons, device.keys);
                Screen::builtin(id.key(), &(builtin.tree)(&choices), device.scaled, controls)
            }
        };
        let mut laid = Self {
            id,
            screen,
            changed: changed.is_some(),
            choices,
            flags,
            leaves: heapless::Vec::new(),
            list: None,
            focus: Focus::default(),
        };
        laid.solve();
        Some(laid)
    }

    /// Solve again for `choices`, `flags` and the rider's `changed` tree;
    /// false when they are what it was solved for.
    #[inline(never)]
    pub fn update(&mut self, choices: Choices, flags: Flags, changed: Option<&Screen>) -> bool {
        let stale = self.changed != changed.is_some()
            || match changed {
                Some(screen) => self.screen != *screen,
                None => choices != self.choices,
            };
        if !stale && (choices, flags) == (self.choices, self.flags) {
            return false;
        }
        if stale {
            self.screen = match changed {
                Some(screen) => screen.clone(),
                None => {
                    let builtin = self.id.builtin();
                    let controls = self.screen.controls().clone();
                    let at = *self.screen.tree().at();
                    Screen::builtin(self.id.key(), &(builtin.tree)(&choices), at, controls)
                }
            };
            self.changed = changed.is_some();
            self.focus = Focus::default();
        }
        self.choices = choices;
        self.flags = flags;
        self.solve();
        true
    }

    #[inline(never)]
    fn solve(&mut self) {
        let mut solved = Solved::EMPTY;
        self.screen.solve(self.flags, &mut solved);
        self.leaves = solved.leaves().iter().copied().collect();
        let at = self.screen.tree().at();
        self.list = solved.scroll().map(|(scroll, _)| (scroll, scroll.page(at)));
    }

    pub fn solved(&self) -> Solved<'_> {
        let mut solved = Solved::EMPTY;
        self.screen.solve(self.flags, &mut solved);
        solved
    }

    pub const fn id(&self) -> ScreenId {
        self.id
    }

    pub const fn choices(&self) -> Choices {
        self.choices
    }

    pub const fn flags(&self) -> Flags {
        self.flags
    }

    pub const fn buttons(&self) -> &Buttons {
        self.screen.controls().buttons()
    }

    pub const fn controls(&self) -> &Controls {
        self.screen.controls()
    }

    pub const fn screen(&self) -> &Screen {
        &self.screen
    }

    pub fn leaves(&self) -> &[Placed] {
        &self.leaves
    }

    pub const fn focus(&self) -> Focus {
        self.focus
    }

    pub fn focus_mut(&mut self) -> &mut Focus {
        &mut self.focus
    }

    /// The scroll shown, if the screen has one.
    pub fn scroll(&self) -> Option<&Scroll> {
        self.list.as_ref().map(|(scroll, _)| scroll)
    }

    /// The page of cards the scroll shown scrolls.
    pub fn page(&self) -> Option<&crate::screens::scroll::Layout> {
        self.list.as_ref().map(|(_, page)| page)
    }

    /// The tiles placed once each, leaving out a scroll's template, which
    /// draws once a card.
    pub fn tiles(&self) -> impl Iterator<Item = (u8, u8, &Placed)> {
        let nodes = self.screen.tree().nodes();
        let template = self.scroll().map_or(0..0, Scroll::tiles);
        let leaves = self.leaves.iter().enumerate();
        leaves
            .filter(move |(index, _)| !template.contains(index))
            .filter_map(
                move |(_, placed)| match nodes[usize::from(placed.node)].what()? {
                    What::Leaf { component, arg } => Some((component, arg, placed)),
                    _ => None,
                },
            )
    }

    /// Draw every tile onto `out`, the focused one by its focused look.
    pub fn prepare(&self, data: &dyn Data, out: &mut Canvas) {
        let tree = self.screen.tree();
        let focused = self.focused();
        super::solve::prepare(tree, &self.leaves, self.scroll(), focused, data, out);
    }

    /// The node of the tile ACT runs, if the focus is on a tile.
    #[inline(never)]
    fn focused(&self) -> Option<u8> {
        match input::focused(self.buttons(), &self.solved(), self.focus)? {
            Stop::Tile(placed) => Some(placed.node),
            Stop::Screen(_) | Stop::List(_) => None,
        }
    }

    /// Where each gliding tile lands for `data`, in the tree's order.
    pub fn glides(&self, data: &dyn Data) -> [Option<Rect16>; GLIDES] {
        let at = self.screen.tree().at();
        let mut landed = [None; GLIDES];
        let glides = self.tiles().filter_map(|(component, arg, placed)| {
            registry::tile(component).glide(arg, placed, at, data)
        });
        for (slot, glide) in landed.iter_mut().zip(glides) {
            *slot = Some(glide.0);
        }
        landed
    }

    /// The box of the first tile of kind `component`, if one shows.
    pub fn box_of(&self, component: u8) -> Option<Rect> {
        self.tiles()
            .find(|(shown, _, _)| *shown == component)
            .map(|(_, _, placed)| placed.rect.rect())
    }
}

/// The main ride page, which every ride page's sign and bar read, and the
/// built-in shown, solved when it opens: two screens' room in the runtime
/// however many screens have trees. A screen that leaves ACT free keeps its
/// focus for the next time it opens, as Home keeps its card.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Laids {
    main: Laid,
    shown: Option<Laid>,
    sheet: Option<Laid>,
    /// Kept here rather than in the frame's `View`, whose stack room is
    /// spent.
    facts: crate::screens::sheet::Facts,
    open: Option<ScreenId>,
    kept: [Focus; ScreenId::COUNT],
}

impl Laids {
    #[inline(never)]
    pub fn new(device: &Device, choices: Choices, flags: Flags, changed: &Screens) -> Self {
        let main = ScreenId::RideMain;
        Self {
            main: Laid::new(main, device, choices, flags, changed.screen(main.key()))
                .expect("the main page has a tree"),
            shown: None,
            sheet: None,
            facts: Default::default(),
            open: None,
            kept: [Focus::default(); ScreenId::COUNT],
        }
    }

    /// The main page solved from `screen`, an accepted tree, in place of
    /// the built-in.
    #[cfg(test)]
    pub fn with_main(screen: Screen, flags: Flags) -> Self {
        let mut main = Laid {
            id: ScreenId::RideMain,
            screen,
            changed: true,
            choices: Choices::default(),
            flags,
            leaves: heapless::Vec::new(),
            list: None,
            focus: Focus::default(),
        };
        main.solve();
        Self {
            main,
            shown: None,
            sheet: None,
            facts: Default::default(),
            open: None,
            kept: [Focus::default(); ScreenId::COUNT],
        }
    }

    pub fn get(&self, id: ScreenId) -> Option<&Laid> {
        match id {
            ScreenId::RideMain => Some(&self.main),
            ScreenId::ControlSheet => self.sheet.as_ref(),
            _ => self.shown.as_ref().filter(|laid| laid.id == id),
        }
    }

    pub fn get_mut(&mut self, id: ScreenId) -> Option<&mut Laid> {
        match id {
            ScreenId::RideMain => Some(&mut self.main),
            ScreenId::ControlSheet => self.sheet.as_mut(),
            _ => self.shown.as_mut().filter(|laid| laid.id == id),
        }
    }

    pub fn main(&self) -> &Laid {
        &self.main
    }

    pub fn sheet(&self) -> Option<&Laid> {
        self.sheet.as_ref()
    }

    pub fn open_sheet(&mut self, device: &Device) {
        if self.sheet.is_none() {
            let (choices, flags) = (self.main.choices, self.main.flags);
            self.sheet = Laid::new(ScreenId::ControlSheet, device, choices, flags, None);
        }
    }

    pub fn close_sheet(&mut self) {
        self.sheet = None;
    }

    pub fn sheet_with_facts(&self) -> Option<(&Laid, &crate::screens::sheet::Facts)> {
        Some((self.sheet.as_ref()?, &self.facts))
    }

    pub fn set_sheet_facts(&mut self, facts: crate::screens::sheet::Facts) {
        self.facts = facts;
    }

    /// Solve the main page and the screen shown again if the choices, the
    /// flags or the screens the rider `changed` did; true when any did.
    pub fn update(&mut self, choices: Choices, flags: Flags, changed: &Screens) -> bool {
        let tree = |id: ScreenId| changed.screen(id.key());
        let main = self.main.update(choices, flags, tree(ScreenId::RideMain));
        let shown = self
            .shown
            .as_mut()
            .is_some_and(|laid| laid.update(choices, flags, tree(laid.id)));
        main || shown
    }

    /// Show `id`: a screen opened afresh is solved, if it has a tree. One
    /// with an ACT of its own starts on it; one that leaves ACT free starts
    /// where its focus was when it last closed, its first stop at first.
    #[inline(never)]
    pub fn show(&mut self, id: ScreenId, device: &Device, changed: &Screens) {
        if self.open == Some(id) {
            return;
        }
        self.open = Some(id);
        self.main.focus = Focus::default();
        if let Some(closed) = &self.shown {
            self.kept[closed.id as usize] = closed.focus;
        }
        self.shown = match id {
            ScreenId::RideMain => None,
            _ => {
                let screen = changed.screen(id.key());
                Laid::new(id, device, self.main.choices, self.main.flags, screen)
            }
        };
        if let Some(laid) = self
            .shown
            .as_mut()
            .filter(|laid| laid.buttons().act.is_none())
        {
            laid.focus = self.kept[id as usize];
        }
    }
}
