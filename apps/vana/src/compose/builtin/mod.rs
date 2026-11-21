//! The screens the firmware ships: one file per screen, each naming its
//! tree and buttons in `BUILTIN`, and the table that finds them by
//! [`ScreenId`]. A screen whose file says `None` still draws from its legacy
//! arm in `View::prepare`.
use super::data::Data;
use super::input::{Buttons, Controls, Focus};
use super::node::{Flags, MAX_NODES, Node, What};
use super::parse::Screen;
use super::registry;
use super::solve::{MAX_LEAVES, Placed, Rect16, Solved};
use crate::screens::Layout;
use crate::screens::canvas::Canvas;
use crate::screens::chrome::Rect;
use crate::screens::ride::RidePage;
use crate::screens::ride::slots::Slots;
use crate::screens::workout::Page;

mod bests;
mod brand;
mod discard;
mod form;
mod format;
mod history;
mod home;
mod lose_rides;
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
mod ride_stats;
mod ride_summary;
mod ride_workout;
mod sensors;
mod settings;
mod setup;
mod sheets;
#[cfg(test)]
mod tests;
mod workouts;

pub use ride_main::{GLIDES, tree as main_tree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ScreenId {
    RideMain,
    RideStats,
    RideLap,
    RideClimb,
    RideWorkout,
    RideMap,
    RidePaused,
    RideEnd,
    RideSummary,
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
    pub const COUNT: usize = 34;
    pub const ALL: [Self; Self::COUNT] = [
        Self::RideMain,
        Self::RideStats,
        Self::RideLap,
        Self::RideClimb,
        Self::RideWorkout,
        Self::RideMap,
        Self::RidePaused,
        Self::RideEnd,
        Self::RideSummary,
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
    ];

    pub const fn of(page: Page, ride: RidePage, paused: bool, sheet: bool, asks: Asks) -> Self {
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
                RidePage::Stats => Self::RideStats,
                RidePage::Lap => Self::RideLap,
                RidePage::Climb => Self::RideClimb,
                RidePage::Workout => Self::RideWorkout,
                RidePage::Map => Self::RideMap,
            },
            Page::EndRide => Self::RideEnd,
            Page::DiscardRide if matches!(asks, Asks::Wait) => Self::Discarding,
            Page::DiscardRide => Self::Discard,
            Page::Summary | Page::PastRide => Self::RideSummary,
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

    /// The pages whose built-in must set MOVE to `Go::Next`.
    pub const fn deck(self) -> bool {
        matches!(
            self,
            Self::RideMain
                | Self::RideStats
                | Self::RideLap
                | Self::RideClimb
                | Self::RideWorkout
                | Self::RideMap
                | Self::RideSummary
        )
    }

    pub const fn builtin(self) -> Option<&'static Builtin> {
        match self {
            Self::RideMain => ride_main::BUILTIN,

            Self::RideStats => ride_stats::BUILTIN,

            Self::RideLap => ride_lap::BUILTIN,

            Self::RideClimb => ride_climb::BUILTIN,

            Self::RideWorkout => ride_workout::BUILTIN,

            Self::RideMap => ride_map::BUILTIN,

            Self::RidePaused => ride_paused::BUILTIN,

            Self::RideEnd => ride_end::BUILTIN,

            Self::RideSummary => ride_summary::BUILTIN,

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
        }
    }
}

/// A screen's nodes, preorder.
pub type Nodes = heapless::Vec<Node, MAX_NODES>;

/// What the rider chose that shapes a built-in's tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Choices {
    pub main_slots: Slots,
}

/// A shipped screen: its tree for the rider's choices, and what its
/// buttons do. A test accepts every one on every panel under every flag
/// combination it mentions; a tree that reads `Choices` tests each choice.
pub struct Builtin {
    pub tree: fn(&Choices) -> Nodes,
    pub buttons: Buttons,
}

/// One built-in solved on one device for the rider's choices and what the
/// ride shows: where each tile goes. A frame draws from it; input solves
/// it again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Laid {
    id: ScreenId,
    screen: Screen,
    choices: Choices,
    flags: Flags,
    leaves: heapless::Vec<Placed, MAX_LEAVES>,
    focus: Focus,
}

impl Laid {
    /// `id` on `layout`'s device, if it has a tree.
    #[inline(never)]
    pub fn new(id: ScreenId, layout: &Layout, choices: Choices, flags: Flags) -> Option<Self> {
        let builtin = id.builtin()?;
        let controls = Controls::new(builtin.buttons, layout.corners);
        let mut laid = Self {
            id,
            screen: Screen::builtin(id.key(), &(builtin.tree)(&choices), layout.scaled, controls),
            choices,
            flags,
            leaves: heapless::Vec::new(),
            focus: Focus::default(),
        };
        laid.solve();
        Some(laid)
    }

    /// Solve again for `choices` and `flags`; false when they are what it
    /// was solved for.
    pub fn update(&mut self, choices: Choices, flags: Flags) -> bool {
        if (choices, flags) == (self.choices, self.flags) {
            return false;
        }
        if choices != self.choices {
            let builtin = self.id.builtin().expect("a laid screen has a tree");
            let controls = self.screen.controls().clone();
            let at = *self.screen.tree().at();
            self.screen = Screen::builtin(self.id.key(), &(builtin.tree)(&choices), at, controls);
            self.choices = choices;
        }
        self.flags = flags;
        self.solve();
        true
    }

    #[inline(never)]
    fn solve(&mut self) {
        let mut solved = Solved::EMPTY;
        self.screen.solve(self.flags, &mut solved);
        self.leaves = solved.leaves().iter().copied().collect();
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

    fn tiles(&self) -> impl Iterator<Item = (u8, u8, &Placed)> {
        let nodes = self.screen.tree().nodes();
        self.leaves.iter().filter_map(move |placed| {
            match nodes[usize::from(placed.node)].what()? {
                What::Leaf { component, arg } => Some((component, arg, placed)),
                _ => None,
            }
        })
    }

    /// Draw every tile onto `out`.
    pub fn prepare(&self, data: &dyn Data, out: &mut Canvas) {
        let at = self.screen.tree().at();
        for (component, arg, placed) in self.tiles() {
            registry::tile(component).prepare(arg, placed, at, data, out);
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
/// however many screens have trees.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Laids {
    main: Laid,
    shown: Option<Laid>,
    open: Option<ScreenId>,
}

impl Laids {
    #[inline(never)]
    pub fn new(layout: &Layout, choices: Choices, flags: Flags) -> Self {
        Self {
            main: Laid::new(ScreenId::RideMain, layout, choices, flags)
                .expect("the main page has a tree"),
            shown: None,
            open: None,
        }
    }

    /// The main page solved from `screen`, an accepted tree, in place of
    /// the built-in.
    #[cfg(test)]
    pub fn with_main(screen: Screen, flags: Flags) -> Self {
        let mut main = Laid {
            id: ScreenId::RideMain,
            screen,
            choices: Choices::default(),
            flags,
            leaves: heapless::Vec::new(),
            focus: Focus::default(),
        };
        main.solve();
        Self {
            main,
            shown: None,
            open: None,
        }
    }

    pub fn get(&self, id: ScreenId) -> Option<&Laid> {
        match id {
            ScreenId::RideMain => Some(&self.main),
            _ => self.shown.as_ref().filter(|laid| laid.id == id),
        }
    }

    pub fn get_mut(&mut self, id: ScreenId) -> Option<&mut Laid> {
        match id {
            ScreenId::RideMain => Some(&mut self.main),
            _ => self.shown.as_mut().filter(|laid| laid.id == id),
        }
    }

    pub fn main(&self) -> &Laid {
        &self.main
    }

    /// Solve the main page and the screen shown again if the choices or
    /// flags changed; true when either did.
    pub fn update(&mut self, choices: Choices, flags: Flags) -> bool {
        let main = self.main.update(choices, flags);
        let shown = self
            .shown
            .as_mut()
            .is_some_and(|laid| laid.update(choices, flags));
        main || shown
    }

    /// Show `id`: a screen opened afresh is solved, if it has a tree, and
    /// starts on its first stop.
    #[inline(never)]
    pub fn show(&mut self, id: ScreenId, layout: &Layout) {
        if self.open == Some(id) {
            return;
        }
        self.open = Some(id);
        self.main.focus = Focus::default();
        self.shown = match id {
            ScreenId::RideMain => None,
            _ => Laid::new(id, layout, self.main.choices, self.main.flags),
        };
    }
}
