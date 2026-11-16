use super::Runtime;
use super::input::HOLD_GAP_MS;
use super::preferences::Stored;
use super::storage::Store;
use crate::ride::log::SECTORS;
use crate::screens::workout::Page;
use device_api::bulk::{Error as MediaError, ReadWrite};
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
    },
    Declined,
    Running(Stage),
    Failed,
    Done,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Stage {
    Mark,
    Wipe(Store),
    JournalSector(usize),
    Settings,
}

impl Format {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Offered { .. } => "offered",
            Self::Declined => "declined",
            Self::Running(_) => "running",
            Self::Failed => "failed",
            Self::Done => "done",
        }
    }

    pub(super) fn owns_storage(self) -> bool {
        matches!(self, Self::Running(_) | Self::Failed | Self::Done)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Event {
    Read { foreign: bool, hub: bool },
    Drawn(u64),
    Report { button: Button, held: bool, at: u64 },
    Confirm,
    Wrote(Next),
    WriteFailed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Go {
    Stay,
    Question,
    FirstPage,
    Ready,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Next {
    Stage(Stage),
    Done,
}

impl Next {
    /// The first stage that wipes `store`; done after the last store.
    fn wiping(store: Option<Store>) -> Self {
        match store {
            Some(Store::Internal) => Self::Stage(Stage::JournalSector(0)),
            Some(Store::Settings) => Self::Stage(Stage::Settings),
            Some(store) => Self::Stage(Stage::Wipe(store)),
            None => Self::Done,
        }
    }
}

fn began_after_drawn(drawn: u64, at: u64) -> bool {
    at >= drawn + HOLD_GAP_MS
}

/// FORMAT never starts under an open ride, whoever asks.
pub(super) fn step(format: Format, event: Event, ride_open: bool) -> (Format, Go) {
    use Format::*;
    match (format, event) {
        (
            Idle,
            Event::Read {
                foreign: true,
                hub: true,
            },
        ) => (Offered { drawn_at: None }, Go::Question),
        (Offered { drawn_at: None }, Event::Drawn(at)) => {
            (Offered { drawn_at: Some(at) }, Go::Stay)
        }
        (
            Offered {
                drawn_at: Some(drawn),
            },
            Event::Report {
                button,
                held: false,
                at,
            },
        ) if began_after_drawn(drawn, at) => match button {
            Button::BottomRight if !ride_open => (Running(Stage::Mark), Go::Stay),
            Button::TopLeft => (Declined, Go::FirstPage),
            Button::BottomRight | Button::BottomLeft | Button::Center => (format, Go::Stay),
        },
        (Idle | Offered { .. } | Declined | Failed, Event::Confirm) if !ride_open => {
            (Running(Stage::Mark), Go::Question)
        }
        (Running(_), Event::Wrote(Next::Stage(next))) => (Running(next), Go::Stay),
        (Running(_), Event::Wrote(Next::Done)) => (Done, Go::Stay),
        (Running(_), Event::WriteFailed) => (Failed, Go::Ready),
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
        if matches!(format, Format::Running(Stage::Mark) | Format::Done) {
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
                self.format_event(Event::Confirm, now);
                if matches!(self.format, Format::Running(_)) {
                    "ACCEPTED"
                } else {
                    "STATE"
                }
            }
            _ => "INVALID",
        }
    }

    /// Offer FORMAT once the stores are read, and run one stage of a wipe a
    /// pass. Call it once a pass, after `tick`: the pass that marks the
    /// settings writes nothing to the card, so composition, seeing
    /// [`Self::owns_storage`], ends its trace before the first card write.
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
                let foreign = health.foreign();
                self.format_event(Event::Read { foreign, hub }, now);
            }
        }
        let Format::Running(stage) = self.format else {
            return;
        };
        let event = match self.write(stage, system, bulk, now) {
            Some(next) => Event::Wrote(next),
            None => Event::WriteFailed,
        };
        self.format_event(event, now);
    }

    fn write<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        stage: Stage,
        system: &mut Shell<D, I, P, B>,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) -> Option<Next> {
        match stage {
            Stage::Mark => {
                if !system.condemn_settings() {
                    return None;
                }
                self.preferences.stored = Stored::Foreign;
                match bulk.owned_info() {
                    Ok(_) => Some(Next::wiping(Some(Store::ALL[0]))),
                    Err(MediaError::Unsupported | MediaError::Unavailable) => {
                        Some(Next::wiping(Some(Store::Internal)))
                    }
                    Err(MediaError::Failed | MediaError::Range) => None,
                }
            }
            Stage::Wipe(store) => {
                let seed = now ^ bulk.entropy();
                wipe(store, bulk, seed)?;
                Some(Next::wiping(store.next()))
            }
            Stage::JournalSector(index) => {
                crate::ride::journal::wipe_sector(&mut system.data_storage(), index).ok()?;
                Some(if index + 1 < SECTORS {
                    Next::Stage(Stage::JournalSector(index + 1))
                } else {
                    Next::wiping(Store::Internal.next())
                })
            }
            Stage::Settings => system
                .erase_settings()
                .then(|| Next::wiping(Store::Settings.next())),
        }
    }
}

// Not inlined, so its sector buffer stays out of the console task's frame,
// which sits under every frame VANA prepares (#167).
#[inline(never)]
fn wipe(store: Store, bulk: &mut impl ReadWrite, seed: u64) -> Option<()> {
    let mut sector = [0u8; 512];
    match store {
        Store::Table => wipe_table(bulk, &mut sector),
        Store::Rides => {
            let rides = &mut volume::open(bulk, &RIDES, &mut sector).ok()?;
            crate::ride::bulk::wipe(rides, seed, &mut sector).ok()
        }
        Store::Files => in_volume(bulk, &FILES, &mut sector, |files, sector| {
            if files::mount(files, sector) == Err(files::Error::Foreign) {
                files::erase(files, sector).ok()?;
            }
            files::format(files, seed, sector).ok().map(drop)
        }),
        Store::Traces => in_volume(bulk, &STORE, &mut sector, |traces, sector| {
            if store::mount(traces, sector).err() == Some(store::Error::Foreign) {
                store::erase(traces, sector).ok()?;
            }
            store::format(traces, seed, sector).ok().map(drop)
        }),
        // The records go with FILES; the boot flash has stages of its own.
        Store::Records | Store::Internal | Store::Settings => Some(()),
    }
}

/// The default table where the table is not current, and the terrain pack
/// erased with it, or alone when the pack is of another version. The pack
/// goes before the table is published, so a cut between leaves the table
/// still to replace.
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

/// `wipe` in the volume `name`; a volume the table lacks has nothing to wipe.
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

    const ASKED: Format = Offered {
        drawn_at: Some(1_000),
    };
    const UNDRAWN: Format = Offered { drawn_at: None };

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
        Event::Read { foreign, hub }
    }

    #[test]
    fn every_state_and_event_steps_as_the_table_says() {
        let wiping = Running(Stage::Mark);
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
            (ASKED, press(ACT, 1_250), wiping, Go::Stay),
            (ASKED, press(BACK, 1_250), Declined, Go::FirstPage),
            (Idle, Event::Confirm, wiping, Go::Question),
            (Declined, Event::Confirm, wiping, Go::Question),
            (Failed, Event::Confirm, wiping, Go::Question),
            (wiping, Event::Confirm, wiping, Go::Stay),
            (wiping, press(ACT, 9_000), wiping, Go::Stay),
            (
                wiping,
                Event::Wrote(Next::Stage(Stage::Wipe(Store::Table))),
                Running(Stage::Wipe(Store::Table)),
                Go::Stay,
            ),
            (
                Running(Stage::Settings),
                Event::Wrote(Next::Done),
                Done,
                Go::Stay,
            ),
            (wiping, Event::WriteFailed, Failed, Go::Ready),
            (Done, Event::Confirm, Done, Go::Stay),
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
        for (format, event) in [
            (ASKED, press(ACT, 1_250)),
            (Idle, Event::Confirm),
            (UNDRAWN, Event::Confirm),
            (Declined, Event::Confirm),
            (Failed, Event::Confirm),
        ] {
            assert_eq!(step(format, event, true), (format, Go::Stay), "{format:?}");
        }
        assert_eq!(
            step(ASKED, press(BACK, 1_250), true),
            (Declined, Go::FirstPage)
        );
    }
}
