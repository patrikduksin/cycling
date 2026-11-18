use super::Runtime;
use super::input::HOLD_GAP_MS;
use super::intent::{self, Found};
use super::navigation::To;
use super::preferences::Stored;
use super::storage::{Health, StorageHealth, Store};
use crate::kept::CardRefused;
use crate::ride::log::Status;
use crate::ride::medium::Medium;
use crate::ride::storage::RideStorage;
use crate::screens::format::{Goes, Shows};
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;
use device_api::display::Display;
use device_api::input::{InputSource, Role};
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_services::volume::{self, FILES, Layout, RIDES, STORE, TERRAIN, Volume};
use firmware_services::{files, pack, store};
use firmware_shell::shell::Shell;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Format {
    #[default]
    Unread,
    Idle,
    Offered {
        drawn_at: Option<u64>,
        scope: Scope,
    },
    Declined,
    Resuming {
        scope: Scope,
    },
    Running {
        stage: Stage,
        scope: Scope,
        /// The wipe began on an earlier boot, whose cut erase can leave
        /// cells that read erased yet hold weakly.
        resumed: bool,
        clear: Option<u32>,
    },
    Failed,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Scope(u8);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CardAccess {
    Writable,
    /// Missing or failing.
    Out,
}

impl Scope {
    pub(crate) fn of(stores: impl IntoIterator<Item = Store>) -> Option<Self> {
        let bits = stores
            .into_iter()
            .fold(0, |bits, store| bits | 1 << store as u8);
        (bits != 0).then_some(Self(bits))
    }

    pub(crate) const fn one(store: Store) -> Self {
        Self(1 << store as u8)
    }

    pub(super) fn bits(self) -> u8 {
        self.0
    }

    pub(super) fn covers(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }

    /// The stores a wipe can reach: none on a card that is missing or
    /// failing, as the boot flash's stores must still go.
    fn reachable(self, card: CardAccess) -> Option<Self> {
        Self::from_bits(self.0 & Self::every(card).0)
    }

    pub(super) fn from_bits(bits: u8) -> Option<Self> {
        let every = Self::every(CardAccess::Writable).0;
        (bits != 0 && bits & !every == 0).then_some(Self(bits))
    }

    pub(super) fn foreign(health: &StorageHealth) -> Option<Self> {
        Self::of(
            health
                .iter()
                .filter(|&(_, health)| health == Health::Foreign)
                .map(|(store, _)| store),
        )
    }

    fn every(card: CardAccess) -> Self {
        Self::of(
            Store::ALL
                .into_iter()
                .filter(|store| card == CardAccess::Writable || !store.on_card()),
        )
        .expect("the boot flash always holds stores")
    }

    /// Every other store's state was read at boot, so wiping one needs a
    /// restart; the internal journal's recorder is emptied in place.
    fn ends_without_restart(self) -> bool {
        self == Self::one(Store::Internal)
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
    RecordIntent,
    Card(Card),
    JournalSector(usize),
    ForgetInternalRides,
    Settings,
    SettleIntent,
    Abandon,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Card {
    Table,
    Rides,
    Files,
    Traces,
}

impl Stage {
    const FIRST: Self = Self::RecordIntent;
    /// A resumed wipe's commit was completed at boot, by `intent::at_boot`.
    const RESUMED: Self = Self::Card(Card::Table);

    fn next(self) -> Option<Self> {
        Some(match self {
            Self::RecordIntent => Self::Card(Card::Table),
            Self::Card(Card::Table) => Self::Card(Card::Rides),
            Self::Card(Card::Rides) => Self::Card(Card::Files),
            Self::Card(Card::Files) => Self::Card(Card::Traces),
            Self::Card(Card::Traces) => Self::JournalSector(0),
            Self::JournalSector(_) => Self::ForgetInternalRides,
            Self::ForgetInternalRides => Self::Settings,
            Self::Settings => Self::SettleIntent,
            Self::SettleIntent | Self::Abandon => return None,
        })
    }

    fn runs_for(self, scope: Scope) -> bool {
        use Store::*;
        match self {
            Self::RecordIntent | Self::SettleIntent | Self::Abandon => true,
            Self::Card(Card::Table) => scope.holds(Table),
            Self::Card(Card::Rides) => scope.holds_any(&[Table, Rides]),
            Self::Card(Card::Files) => scope.holds_any(&[Table, Files, Records]),
            Self::Card(Card::Traces) => scope.holds_any(&[Table, Traces]),
            Self::JournalSector(_) => scope.holds(Internal),
            Self::ForgetInternalRides => {
                scope.holds(Internal) && !Self::Card(Card::Files).runs_for(scope)
            }
            Self::Settings => scope.holds(Settings),
        }
    }
}

impl Format {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Idle => "idle",
            Self::Offered { .. } => "offered",
            Self::Declined => "declined",
            Self::Resuming { .. } => "resuming",
            Self::Running { .. } => "running",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    pub(super) fn stops_own_storage(self) -> bool {
        matches!(self, Self::Running { .. } | Self::Failed | Self::Done)
    }

    pub(super) fn holds_host_storage(self) -> bool {
        !matches!(self, Self::Idle | Self::Offered { .. } | Self::Declined)
    }

    pub(super) fn shows(self) -> Shows {
        match self {
            Self::Offered { scope, .. } => Shows::Question(scope.goes()),
            _ => Shows::Wait,
        }
    }

    fn from_stage(stage: Option<Stage>, scope: Scope, resumed: bool, clear: Option<u32>) -> Self {
        core::iter::successors(stage, |stage| stage.next())
            .find(|stage| stage.runs_for(scope))
            .map_or(Self::Done, |stage| Self::Running {
                stage,
                scope,
                resumed,
                clear,
            })
    }

    pub(super) fn admits_rides(self) -> bool {
        matches!(self, Self::Idle | Self::Declined)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    IntentAtBoot(Found),
    /// Storage health settled.
    Read {
        foreign: Option<Scope>,
        hub: bool,
        card: CardAccess,
    },
    Drawn(u64),
    Report {
        button: Role,
        held: bool,
        at: u64,
    },
    Confirm {
        scope: Scope,
        clear: Option<u32>,
    },
    IntentRecorded(Scope),
    IntentNotRecorded,
    Wrote,
    /// A journal sector is wiped and the journal holds more.
    SectorWiped,
    WriteFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Go {
    Stay,
    Question,
    FirstPage,
    Ready { clear: Option<u32> },
    JournalEmptied { clear: Option<u32> },
}

fn began_after_drawn(drawn: u64, at: u64) -> bool {
    at >= drawn + HOLD_GAP_MS
}

pub(super) fn step(format: Format, event: Event, ride_open: bool) -> (Format, Go) {
    use Format::*;
    match (format, event) {
        (Unread, Event::IntentAtBoot(Found::Idle)) => (Idle, Go::Stay),
        (Unread, Event::IntentAtBoot(Found::Pending(scope))) => (Resuming { scope }, Go::Stay),
        // An unread intent may still name a pending wipe, so rides stay
        // blocked until a boot reads it.
        (Unread, Event::Read { .. }) => (Failed, Go::Ready { clear: None }),
        (Resuming { scope }, Event::Read { card, .. }) => {
            let resumed = match scope.reachable(card) {
                Some(reachable) => Format::from_stage(Some(Stage::RESUMED), reachable, true, None),
                None => Format::from_stage(Some(Stage::SettleIntent), scope, true, None),
            };
            (resumed, Go::Question)
        }
        (
            Idle,
            Event::Read {
                foreign: Some(scope),
                hub: true,
                ..
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
            Role::Act if !ride_open => (
                Format::from_stage(Some(Stage::FIRST), scope, false, None),
                Go::Question,
            ),
            Role::Back => (Declined, Go::FirstPage),
            Role::Act | Role::Move => (format, Go::Stay),
        },
        (Idle | Offered { .. } | Declined | Failed, Event::Confirm { scope, clear })
            if !ride_open =>
        {
            (
                Format::from_stage(Some(Stage::FIRST), scope, false, clear),
                Go::Question,
            )
        }
        (
            Running {
                stage,
                resumed,
                clear,
                ..
            },
            Event::IntentRecorded(scope),
        ) => (
            Format::from_stage(stage.next(), scope, resumed, clear),
            Go::Stay,
        ),
        (
            Running {
                stage: Stage::Abandon,
                clear,
                ..
            },
            Event::Wrote | Event::WriteFailed,
        ) => (Failed, Go::Ready { clear }),
        (
            Running {
                stage: Stage::JournalSector(index),
                scope,
                resumed,
                clear,
            },
            Event::SectorWiped,
        ) => (
            Running {
                stage: Stage::JournalSector(index + 1),
                scope,
                resumed,
                clear,
            },
            Go::Stay,
        ),
        (
            Running {
                stage,
                scope,
                resumed,
                clear,
            },
            Event::Wrote,
        ) => match Format::from_stage(stage.next(), scope, resumed, clear) {
            Done if scope.ends_without_restart() => (Idle, Go::JournalEmptied { clear }),
            next => (next, Go::Stay),
        },
        (
            Running {
                scope,
                resumed,
                clear,
                ..
            },
            Event::WriteFailed,
        ) => (
            Running {
                stage: Stage::Abandon,
                scope,
                resumed,
                clear,
            },
            Go::Stay,
        ),
        // An intent that was not recorded may still name a wipe begun
        // earlier, so it is not settled.
        (Running { clear, .. }, Event::IntentNotRecorded) => (Failed, Go::Ready { clear }),
        _ => (format, Go::Stay),
    }
}

impl Runtime {
    /// From the first FORMAT write until the restart, or until a wipe of
    /// the internal journal alone ends, the app's own storage work stops:
    /// boot scans, recording and indexing. A resumed wipe lets the boot
    /// scans run, as its scope on the card depends on them. Trace writes go
    /// on unless `wipes_traces`.
    pub fn stops_own_storage(&self) -> bool {
        self.format.stops_own_storage()
    }

    /// From boot until the intent is read, through a resumed wipe, and from
    /// the first FORMAT write until the restart or a Clear's wipe ends,
    /// storage work from outside the app waits: console and host storage
    /// commands answer `BUSY`, and neither the phone nor the look-ahead is
    /// served.
    pub fn holds_host_storage(&self) -> bool {
        self.format.holds_host_storage()
    }

    /// The wipe running takes STORE, and the trace file goes with it. The
    /// trace writes nothing else, so it goes on through any other wipe.
    pub fn wipes_traces(&self) -> bool {
        matches!(self.format, Format::Running { scope, .. } if Stage::Card(Card::Traces).runs_for(scope))
    }

    pub fn restart_due(&self) -> bool {
        self.format == Format::Done
    }

    pub(super) fn format_event(&mut self, event: Event) {
        let (format, go) = step(self.format, event, self.ride.open());
        self.format = format;
        let page = match go {
            Go::Stay => return self.display_after(format),
            Go::Question => Page::Format,
            Go::FirstPage => self.first_page(),
            Go::Ready { clear } => {
                self.ride.wipe_failed(clear);
                Page::Ready
            }
            Go::JournalEmptied { clear } => {
                self.ride.wiped(clear);
                self.relist_internal();
                self.first_page()
            }
        };
        let brand_leads_on = matches!(go, Go::Question | Go::JournalEmptied { .. })
            && self.navigation.page() == Page::Boot;
        if !brand_leads_on {
            self.navigation.go(To::Page(page));
        }
        self.display.redraw();
    }

    fn display_after(&mut self, format: Format) {
        if format == Format::Done {
            self.display.redraw();
        }
    }

    pub(super) fn card_access(&self) -> CardAccess {
        match self.ride.bulk().status() {
            Status::Unsupported | Status::Unavailable | Status::Error => CardAccess::Out,
            _ => CardAccess::Writable,
        }
    }

    pub(super) fn format_input(&mut self, button: Role, held: bool, at: u64) {
        self.format_event(Event::Report { button, held, at });
    }

    pub(super) fn format_command(
        &mut self,
        argument: &str,
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
            // A wipe takes the internal journal, and with it unsaved rides.
            "CONFIRM" if self.holds_unsynced() => "UNSYNCED",
            "CONFIRM" => {
                let scope = Scope::every(self.card_access());
                self.format_event(Event::Confirm { scope, clear: None });
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
        if self.format == Format::Unread {
            let found = intent::at_boot(system.data_storage().intent());
            self.format_event(Event::IntentAtBoot(found));
        }
        if matches!(
            self.format,
            Format::Unread | Format::Idle | Format::Resuming { .. }
        ) {
            self.probe_storage(bulk);
            let health = self.storage_health();
            if health.settled() {
                let hub = matches!(
                    self.navigation.page(),
                    Page::Boot | Page::Home | Page::Setup
                ) && !self.ride.open();
                let foreign = Scope::foreign(&health);
                let card = self.card_access();
                self.format_event(Event::Read { foreign, hub, card });
            }
        }
        let Format::Running {
            stage,
            scope,
            resumed,
            ..
        } = self.format
        else {
            return;
        };
        let event = self.write(stage, scope, resumed, system, bulk, now);
        self.format_event(event);
    }

    fn write<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        stage: Stage,
        scope: Scope,
        resumed: bool,
        system: &mut Shell<D, I, P, B>,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) -> Event {
        let wrote = match stage {
            Stage::RecordIntent => {
                return intent::begin(system.data_storage().intent(), scope)
                    .map_or(Event::IntentNotRecorded, Event::IntentRecorded);
            }
            Stage::SettleIntent | Stage::Abandon => intent::settle(system.data_storage().intent()),
            Stage::Card(card) => {
                let seed = now ^ bulk.entropy();
                wipe(card, bulk, seed).is_some()
            }
            Stage::JournalSector(index) => {
                let store = &mut system.data_storage();
                let wiped = (!resumed || store.ride_erase_sector(index).is_ok())
                    && crate::ride::journal::wipe_sector(store, index);
                if wiped && index + 1 < store.ride_sectors() {
                    return Event::SectorWiped;
                }
                wiped
            }
            Stage::ForgetInternalRides => self.forget_internal_rides(bulk, now).is_ok(),
            Stage::Settings => {
                self.preferences.stored = Stored::Foreign;
                system.erase_settings()
            }
        };
        if wrote {
            Event::Wrote
        } else {
            Event::WriteFailed
        }
    }

    fn forget_internal_rides(
        &mut self,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) -> Result<(), CardRefused> {
        self.ride.forget_moved_ride();
        match self.card_access() {
            CardAccess::Out => Ok(()),
            CardAccess::Writable => crate::ride::settle::reset(
                Medium::Internal,
                &mut self.facts,
                &mut self.deleted,
                bulk,
                now,
            ),
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
mod tests;
