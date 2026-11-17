use super::Runtime;
use super::input::HOLD_GAP_MS;
use super::preferences::Stored;
use super::storage::{Health, StorageHealth, Store};
use crate::ride::log::{SECTORS, Status};
use crate::screens::format::{Goes, Shows};
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;
use device_api::display::Display;
use device_api::input::{Button, InputSource};
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_services::volume::{self, FILES, Layout, RIDES, STORE, TERRAIN, Volume};
use firmware_services::{files, pack, store};
use firmware_shell::shell::Shell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Format {
    #[default]
    Idle,
    Offered {
        drawn_at: Option<u64>,
        scope: Scope,
    },
    Declined,
    Running {
        stage: Stage,
        scope: Scope,
    },
    Failed,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Scope(u8);

impl Scope {
    pub(super) fn of(stores: impl IntoIterator<Item = Store>) -> Option<Self> {
        let bits = stores
            .into_iter()
            .fold(0, |bits, store| bits | 1 << store as u8);
        (bits != 0).then_some(Self(bits))
    }

    pub(super) fn foreign(health: &StorageHealth) -> Option<Self> {
        Self::of(
            health
                .iter()
                .filter(|&(_, health)| health == Health::Foreign)
                .map(|(store, _)| store),
        )
    }

    fn every(card: bool) -> Self {
        Self::of(
            Store::ALL
                .into_iter()
                .filter(|store| card || !store.on_card()),
        )
        .expect("the boot flash always holds stores")
    }

    fn holds(self, store: Store) -> bool {
        self.0 & 1 << store as u8 != 0
    }

    fn holds_any(self, stores: &[Store]) -> bool {
        stores.iter().any(|&store| self.holds(store))
    }

    pub(super) fn goes(self) -> Goes {
        use Store::*;
        const ALONE: [(&[Store], Goes); 6] = [
            (&[Table], Goes::WholeCard),
            (&[Rides], Goes::CardRides),
            (&[Settings], Goes::Settings),
            (&[Files, Records], Goes::CardFiles),
            (&[Internal], Goes::RideLog),
            (&[Traces], Goes::Traces),
        ];
        let rides = self.holds_any(&[Table, Rides]);
        let settings = self.holds(Settings);
        if rides && settings {
            return Goes::RidesAndSettings;
        }
        let mut held = ALONE
            .iter()
            .filter(|(stores, _)| self.holds_any(stores))
            .map(|&(_, goes)| goes);
        if let (Some(goes), None) = (held.next(), held.next()) {
            return goes;
        }
        if rides {
            Goes::RidesAndMore
        } else if settings {
            Goes::SettingsAndMore
        } else if self.holds_any(&[Files, Records]) {
            Goes::FilesAndMore
        } else {
            Goes::RideLogAndMore
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Card(Card),
    JournalSector(usize),
    Settings,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Card {
    Table,
    Rides,
    Files,
    Traces,
}

impl Stage {
    const FIRST: Self = Self::Card(Card::Table);

    fn next(self) -> Option<Self> {
        Some(match self {
            Self::Card(Card::Table) => Self::Card(Card::Rides),
            Self::Card(Card::Rides) => Self::Card(Card::Files),
            Self::Card(Card::Files) => Self::Card(Card::Traces),
            Self::Card(Card::Traces) => Self::JournalSector(0),
            Self::JournalSector(index) if index + 1 < SECTORS => Self::JournalSector(index + 1),
            Self::JournalSector(_) => Self::Settings,
            Self::Settings => return None,
        })
    }

    fn runs_for(self) -> &'static [Store] {
        match self {
            Self::Card(Card::Table) => &[Store::Table],
            Self::Card(Card::Rides) => &[Store::Table, Store::Rides],
            Self::Card(Card::Files) => &[Store::Table, Store::Files, Store::Records],
            Self::Card(Card::Traces) => &[Store::Table, Store::Traces],
            Self::JournalSector(_) => &[Store::Internal],
            Self::Settings => &[Store::Settings],
        }
    }
}

impl Format {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Offered { .. } => "offered",
            Self::Declined => "declined",
            Self::Running { .. } => "running",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    pub(super) fn owns_storage(self) -> bool {
        matches!(self, Self::Running { .. } | Self::Failed | Self::Done)
    }

    pub(super) fn shows(self) -> Shows {
        match self {
            Self::Offered { scope, .. } => Shows::Question(scope.goes()),
            _ => Shows::Wait,
        }
    }

    fn from_stage(stage: Option<Stage>, scope: Scope) -> Self {
        core::iter::successors(stage, |stage| stage.next())
            .find(|stage| scope.holds_any(stage.runs_for()))
            .map_or(Self::Done, |stage| Self::Running { stage, scope })
    }

    pub(super) fn admits_rides(self) -> bool {
        matches!(self, Self::Idle | Self::Declined)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    Read { foreign: Option<Scope>, hub: bool },
    Drawn(u64),
    Report { button: Button, held: bool, at: u64 },
    Confirm(Scope),
    Wrote,
    WriteFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Go {
    Stay,
    Question,
    FirstPage,
    Ready,
}

fn began_after_drawn(drawn: u64, at: u64) -> bool {
    at >= drawn + HOLD_GAP_MS
}

pub(super) fn step(format: Format, event: Event, ride_open: bool) -> (Format, Go) {
    use Format::*;
    match (format, event) {
        (
            Idle,
            Event::Read {
                foreign: Some(scope),
                hub: true,
            },
        ) => (
            Offered {
                drawn_at: None,
                scope,
            },
            Go::Question,
        ),
        (
            Offered {
                drawn_at: None,
                scope,
            },
            Event::Drawn(at),
        ) => (
            Offered {
                drawn_at: Some(at),
                scope,
            },
            Go::Stay,
        ),
        (
            Offered {
                drawn_at: Some(drawn),
                scope,
            },
            Event::Report {
                button,
                held: false,
                at,
            },
        ) if began_after_drawn(drawn, at) => match button {
            Button::BottomRight if !ride_open => {
                (Format::from_stage(Some(Stage::FIRST), scope), Go::Question)
            }
            Button::TopLeft => (Declined, Go::FirstPage),
            Button::BottomRight | Button::BottomLeft | Button::Center => (format, Go::Stay),
        },
        (Idle | Offered { .. } | Declined | Failed, Event::Confirm(scope)) if !ride_open => {
            (Format::from_stage(Some(Stage::FIRST), scope), Go::Question)
        }
        (Running { stage, scope }, Event::Wrote) => {
            (Format::from_stage(stage.next(), scope), Go::Stay)
        }
        (Running { .. }, Event::WriteFailed) => (Failed, Go::Ready),
        _ => (format, Go::Stay),
    }
}

impl Runtime {
    /// From the first FORMAT write until the restart, nothing but FORMAT
    /// reaches storage: no ride starts, the trace stops, and storage
    /// commands answer `BUSY`.
    pub fn owns_storage(&self) -> bool {
        self.format.owns_storage()
    }

    pub fn restart_due(&self) -> bool {
        self.format == Format::Done
    }

    pub(super) fn format_event(&mut self, event: Event, now: u64) {
        let (format, go) = step(self.format, event, self.ride.open());
        self.format = format;
        let page = match go {
            Go::Stay => return self.display_after(format),
            Go::Question => Page::Format,
            Go::FirstPage => self.first_page(),
            Go::Ready => Page::Ready,
        };
        let brand_leads_to_question = go == Go::Question && self.navigation.page == Page::Boot;
        if !brand_leads_to_question {
            self.navigation.page = page;
            self.navigation.page_since = now;
        }
        self.display.redraw();
    }

    fn display_after(&mut self, format: Format) {
        if format == Format::Done {
            self.display.redraw();
        }
    }

    pub(super) fn format_input(&mut self, button: Button, held: bool, at: u64, now: u64) {
        self.format_event(Event::Report { button, held, at }, now);
    }

    pub(super) fn format_command(
        &mut self,
        argument: &str,
        now: u64,
        output: &mut impl core::fmt::Write,
    ) -> &'static str {
        match argument {
            "" => {
                let _ = write!(output, "state={}", self.format.name());
                for (store, health) in self.storage_health().iter() {
                    let _ = write!(output, " {}={}", store.name(), health.name());
                }
                "OK"
            }
            "CONFIRM" => {
                let card = !matches!(
                    self.ride.bulk.status(),
                    Status::Unsupported | Status::Unavailable
                );
                self.format_event(Event::Confirm(Scope::every(card)), now);
                if matches!(self.format, Format::Running { .. }) {
                    "ACCEPTED"
                } else {
                    "STATE"
                }
            }
            _ => "INVALID",
        }
    }

    #[inline(never)]
    pub fn service_format<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        system: &mut Shell<D, I, P, B>,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) {
        if self.format == Format::Idle {
            self.probe_storage(bulk);
            let health = self.storage_health();
            if health.settled() {
                let hub = matches!(self.navigation.page, Page::Boot | Page::Home | Page::Setup)
                    && !self.ride.open();
                let foreign = Scope::foreign(&health);
                self.format_event(Event::Read { foreign, hub }, now);
            }
        }
        let Format::Running { stage, .. } = self.format else {
            return;
        };
        let event = if self.write(stage, system, bulk, now) {
            Event::Wrote
        } else {
            Event::WriteFailed
        };
        self.format_event(event, now);
    }

    fn write<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        stage: Stage,
        system: &mut Shell<D, I, P, B>,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) -> bool {
        match stage {
            Stage::Card(card) => {
                let seed = now ^ bulk.entropy();
                wipe(card, bulk, seed).is_some()
            }
            Stage::JournalSector(index) => {
                crate::ride::journal::wipe_sector(&mut system.data_storage(), index)
            }
            Stage::Settings => {
                self.preferences.stored = Stored::Foreign;
                system.erase_settings()
            }
        }
    }
}

// Not inlined, so its sector buffer stays out of the console task's frame,
// which sits under every frame VANA prepares (#167).
#[inline(never)]
fn wipe(card: Card, bulk: &mut impl ReadWrite, seed: u64) -> Option<()> {
    let mut sector = [0u8; 512];
    match card {
        Card::Table => wipe_table(bulk, &mut sector),
        Card::Rides => {
            let rides = &mut volume::open(bulk, &RIDES, &mut sector).ok()?;
            crate::ride::bulk::wipe(rides, seed, &mut sector).ok()
        }
        Card::Files => in_volume(bulk, &FILES, &mut sector, |files, sector| {
            if files::mount(files, sector) == Err(files::Error::Foreign) {
                files::erase(files, sector).ok()?;
            }
            files::format(files, seed, sector).ok().map(drop)
        }),
        Card::Traces => in_volume(bulk, &STORE, &mut sector, |traces, sector| {
            if store::mount(traces, sector).err() == Some(store::Error::Foreign) {
                store::erase(traces, sector).ok()?;
            }
            store::format(traces, seed, sector).ok().map(drop)
        }),
    }
}

fn wipe_table(bulk: &mut impl ReadWrite, sector: &mut volume::Sector) -> Option<()> {
    let (table, replace) = match volume::read(bulk, sector) {
        Ok(Layout::Table(table)) => (table, false),
        Ok(Layout::Blank | Layout::Lost) | Err(volume::Error::Foreign) => {
            (volume::plan(bulk.owned_info().ok()?.sectors).ok()?, true)
        }
        Err(_) => return None,
    };
    if let Some(extent) = table.find(&TERRAIN) {
        let terrain = &mut Volume::new(bulk, extent);
        if replace || pack::status(terrain, sector) == Err(pack::Error::Foreign) {
            pack::erase(terrain, sector).ok()?;
        }
    }
    if replace {
        volume::replace(bulk, sector).ok()?;
    }
    Some(())
}

fn in_volume<B: ReadWrite>(
    bulk: &mut B,
    name: &volume::Name,
    sector: &mut volume::Sector,
    wipe: impl FnOnce(&mut Volume<'_, B>, &mut volume::Sector) -> Option<()>,
) -> Option<()> {
    match volume::open(bulk, name, sector) {
        Ok(mut volume) => wipe(&mut volume, sector),
        Err(volume::Error::Missing) => Some(()),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Button::{BottomLeft as MOVE, BottomRight as ACT, TopLeft as BACK};
    use Format::*;
    use std::vec;
    use std::vec::Vec;

    fn scope(stores: &[Store]) -> Scope {
        Scope::of(stores.iter().copied()).unwrap()
    }

    const RIDES: Scope = Scope(1 << Store::Rides as u8);
    const ASKED: Format = Offered {
        drawn_at: Some(1_000),
        scope: RIDES,
    };
    const UNDRAWN: Format = Offered {
        drawn_at: None,
        scope: RIDES,
    };

    fn press(button: Button, at: u64) -> Event {
        Event::Report {
            button,
            held: false,
            at,
        }
    }

    fn held(button: Button, at: u64) -> Event {
        Event::Report {
            button,
            held: true,
            at,
        }
    }

    fn read(foreign: bool, hub: bool) -> Event {
        Event::Read {
            foreign: foreign.then_some(RIDES),
            hub,
        }
    }

    fn running(stage: Stage, scope: Scope) -> Format {
        Running { stage, scope }
    }

    #[test]
    fn every_state_and_event_steps_as_the_table_says() {
        let every = Scope::every(true);
        let wiping = running(Stage::FIRST, every);
        let rides = running(Stage::Card(Card::Rides), RIDES);
        let rows = [
            (Idle, read(true, true), UNDRAWN, Go::Question),
            (Idle, read(false, true), Idle, Go::Stay),
            (Idle, read(true, false), Idle, Go::Stay),
            (Declined, read(true, true), Declined, Go::Stay),
            (Failed, read(true, true), Failed, Go::Stay),
            (UNDRAWN, Event::Drawn(1_000), ASKED, Go::Stay),
            (ASKED, Event::Drawn(5_000), ASKED, Go::Stay),
            (UNDRAWN, press(ACT, 9_000), UNDRAWN, Go::Stay),
            (ASKED, press(ACT, 900), ASKED, Go::Stay),
            (ASKED, press(ACT, 1_249), ASKED, Go::Stay),
            (ASKED, held(ACT, 9_000), ASKED, Go::Stay),
            (ASKED, held(BACK, 9_000), ASKED, Go::Stay),
            (ASKED, press(BACK, 1_100), ASKED, Go::Stay),
            (ASKED, press(MOVE, 9_000), ASKED, Go::Stay),
            (ASKED, press(ACT, 1_250), rides, Go::Question),
            (ASKED, press(BACK, 1_250), Declined, Go::FirstPage),
            (Idle, Event::Confirm(every), wiping, Go::Question),
            (ASKED, Event::Confirm(every), wiping, Go::Question),
            (Declined, Event::Confirm(every), wiping, Go::Question),
            (Failed, Event::Confirm(every), wiping, Go::Question),
            (wiping, Event::Confirm(every), wiping, Go::Stay),
            (wiping, press(ACT, 9_000), wiping, Go::Stay),
            (
                wiping,
                Event::Wrote,
                running(Stage::Card(Card::Rides), every),
                Go::Stay,
            ),
            (rides, Event::Wrote, Done, Go::Stay),
            (
                running(Stage::Settings, every),
                Event::Wrote,
                Done,
                Go::Stay,
            ),
            (wiping, Event::WriteFailed, Failed, Go::Ready),
            (Done, Event::Confirm(every), Done, Go::Stay),
            (Done, read(true, true), Done, Go::Stay),
        ];
        for (format, event, after, go) in rows {
            assert_eq!(
                step(format, event, false),
                (after, go),
                "{format:?} {event:?}"
            );
        }
    }

    #[test]
    fn nothing_starts_a_wipe_under_an_open_ride() {
        let every = Event::Confirm(Scope::every(true));
        for (format, event) in [
            (ASKED, press(ACT, 1_250)),
            (Idle, every),
            (UNDRAWN, every),
            (Declined, every),
            (Failed, every),
        ] {
            assert_eq!(step(format, event, true), (format, Go::Stay), "{format:?}");
        }
        assert_eq!(
            step(ASKED, press(BACK, 1_250), true),
            (Declined, Go::FirstPage)
        );
    }

    #[test]
    fn a_wipe_takes_the_stages_of_the_stores_it_is_for_in_order() {
        use Card::{Files, Rides, Table, Traces};
        let journal = || (0..SECTORS).map(Stage::JournalSector);
        let card = [Table, Rides, Files, Traces].map(Stage::Card);
        let cases: [(Scope, Vec<Stage>); 10] = [
            (scope(&[Store::Table]), card.to_vec()),
            (scope(&[Store::Rides]), vec![Stage::Card(Rides)]),
            (scope(&[Store::Files]), vec![Stage::Card(Files)]),
            (scope(&[Store::Records]), vec![Stage::Card(Files)]),
            (scope(&[Store::Traces]), vec![Stage::Card(Traces)]),
            (scope(&[Store::Internal]), journal().collect()),
            (scope(&[Store::Settings]), vec![Stage::Settings]),
            (
                scope(&[Store::Settings, Store::Rides]),
                vec![Stage::Card(Rides), Stage::Settings],
            ),
            (
                Scope::every(true),
                card.into_iter()
                    .chain(journal())
                    .chain([Stage::Settings])
                    .collect(),
            ),
            (
                Scope::every(false),
                journal().chain([Stage::Settings]).collect(),
            ),
        ];
        for (scope, stages) in cases {
            let mut taken = Vec::new();
            let mut format = step(Idle, Event::Confirm(scope), false).0;
            while let Running { stage, .. } = format {
                taken.push(stage);
                format = step(format, Event::Wrote, false).0;
            }
            assert_eq!(format, Done, "{scope:?}");
            assert_eq!(taken, stages, "{scope:?}");
        }
    }

    #[test]
    fn the_question_names_what_goes_by_the_gravest_store() {
        use Store::*;
        let cases: [(&[Store], Goes); 17] = [
            (&[Rides, Settings], Goes::RidesAndSettings),
            (&[Table, Settings, Internal], Goes::RidesAndSettings),
            (&[Table], Goes::WholeCard),
            (&[Rides], Goes::CardRides),
            (&[Files], Goes::CardFiles),
            (&[Records], Goes::CardFiles),
            (&[Files, Records], Goes::CardFiles),
            (&[Traces], Goes::Traces),
            (&[Internal], Goes::RideLog),
            (&[Settings], Goes::Settings),
            (&[Rides, Internal], Goes::RidesAndMore),
            (&[Table, Rides], Goes::RidesAndMore),
            (&[Settings, Records], Goes::SettingsAndMore),
            (&[Settings, Traces], Goes::SettingsAndMore),
            (&[Files, Internal], Goes::FilesAndMore),
            (&[Records, Traces], Goes::FilesAndMore),
            (&[Internal, Traces], Goes::RideLogAndMore),
        ];
        for (stores, goes) in cases {
            assert_eq!(scope(stores).goes(), goes, "{stores:?}");
        }
    }
}
