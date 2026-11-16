use super::Runtime;
use super::input::HOLD_GAP_MS;
use super::preferences::Stored;
use crate::ride::log::{SECTORS, SLOTS_PER_SECTOR, Slot, erased};
use crate::ride::storage::RideStorage;
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;
use device_api::display::Display;
use device_api::input::{Button, InputSource};
use device_api::power::Power;
use device_api::storage::OwnedFlash;
use firmware_services::volume::{self, FILES, Layout, RIDES, STORE, TERRAIN, Volume};
use firmware_services::{files, pack, sector_log, store};
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
    Table,
    Rides,
    Files,
    Traces,
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
    Confirm { ride_open: bool },
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

fn began_after_drawn(drawn: u64, at: u64) -> bool {
    at >= drawn + HOLD_GAP_MS
}

pub(super) fn step(format: Format, event: Event) -> (Format, Go) {
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
            Button::BottomRight => (Running(Stage::Mark), Go::Stay),
            Button::TopLeft => (Declined, Go::FirstPage),
            Button::BottomLeft | Button::Center => (format, Go::Stay),
        },
        (Idle | Offered { .. } | Declined | Failed, Event::Confirm { ride_open: false }) => {
            (Running(Stage::Mark), Go::Question)
        }
        (Running(_), Event::Wrote(Next::Stage(next))) => (Running(next), Go::Stay),
        (Running(_), Event::Wrote(Next::Done)) => (Done, Go::Stay),
        (Running(_), Event::WriteFailed) => (Failed, Go::Ready),
        _ => (format, Go::Stay),
    }
}

impl Runtime {
    pub fn formatting(&self) -> bool {
        matches!(self.format, Format::Running(_))
    }

    pub fn restart_due(&self) -> bool {
        self.format == Format::Done
    }

    pub(super) fn format_event(&mut self, event: Event, now: u64) {
        let (format, go) = step(self.format, event);
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
                for (name, health) in self.storage_health().0 {
                    let _ = write!(output, " {name}={}", health.name());
                }
                "OK"
            }
            "CONFIRM" => {
                let ride_open = self.ride.open();
                self.format_event(Event::Confirm { ride_open }, now);
                if self.formatting() {
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

    // Not inlined, so its sector buffer stays out of the console task's
    // frame, which sits under every frame VANA prepares (#167).
    #[inline(never)]
    fn write<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
        &mut self,
        stage: Stage,
        system: &mut Shell<D, I, P, B>,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) -> Option<Next> {
        let mut sector = [0u8; 512];
        let seed = now ^ bulk.entropy();
        let next = match stage {
            Stage::Mark => {
                if !system.condemn_settings() {
                    return None;
                }
                self.preferences.stored = Stored::Foreign;
                match bulk.owned_info() {
                    Ok(_) => Stage::Table,
                    Err(_) => Stage::JournalSector(0),
                }
            }
            Stage::Table => {
                let kept = matches!(volume::read(bulk, &mut sector), Ok(Layout::Table(_)));
                let table = if kept {
                    volume::create(bulk, &mut sector)
                } else {
                    volume::replace(bulk, &mut sector)
                }
                .ok()?;
                if let Some(extent) = table.find(&TERRAIN) {
                    let terrain = &mut Volume::new(bulk, extent);
                    let foreign = pack::status(terrain, &mut sector) == Err(pack::Error::Foreign);
                    if !kept || foreign {
                        pack::erase(terrain, &mut sector).ok()?;
                    }
                }
                Stage::Rides
            }
            Stage::Rides => {
                let initial = crate::ride::bulk::encode_catalog(&crate::ride::log::Catalog {
                    next_ride_id: 1,
                    ..Default::default()
                });
                let rides = &mut volume::open(bulk, &RIDES, &mut sector).ok()?;
                let tag = crate::ride::bulk::TAG;
                if sector_log::formatted(rides, tag, &mut sector) == Err(sector_log::Error::Foreign)
                {
                    sector_log::erase(rides, &mut sector).ok()?;
                }
                sector_log::format(rides, tag, seed, &initial, &mut sector).ok()?;
                Stage::Files
            }
            Stage::Files => {
                match volume::open(bulk, &FILES, &mut sector) {
                    Ok(mut files) => {
                        if files::mount(&mut files, &mut sector) == Err(files::Error::Foreign) {
                            files::erase(&mut files, &mut sector).ok()?;
                        }
                        files::format(&mut files, seed, &mut sector).ok()?;
                    }
                    Err(volume::Error::Missing) => {}
                    Err(_) => return None,
                }
                Stage::Traces
            }
            Stage::Traces => {
                match volume::open(bulk, &STORE, &mut sector) {
                    Ok(mut traces) => {
                        if store::mount(&mut traces, &mut sector).err()
                            == Some(store::Error::Foreign)
                        {
                            store::erase(&mut traces, &mut sector).ok()?;
                        }
                        store::format(&mut traces, seed, &mut sector).ok()?;
                    }
                    Err(volume::Error::Missing) => {}
                    Err(_) => return None,
                }
                Stage::JournalSector(0)
            }
            Stage::JournalSector(index) => {
                let mut journal = system.data_storage();
                let mut slot = Slot::default();
                let mut used = false;
                for at in index * SLOTS_PER_SECTOR..(index + 1) * SLOTS_PER_SECTOR {
                    journal.ride_read_slot(at, &mut slot).ok()?;
                    used |= !erased(&slot.0);
                }
                if used {
                    journal.ride_erase_sector(index).ok()?;
                }
                if index + 1 < SECTORS {
                    Stage::JournalSector(index + 1)
                } else {
                    Stage::Settings
                }
            }
            Stage::Settings => {
                if !system.erase_settings() {
                    return None;
                }
                return Some(Next::Done);
            }
        };
        Some(Next::Stage(next))
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
            (
                Idle,
                Event::Confirm { ride_open: false },
                wiping,
                Go::Question,
            ),
            (
                Declined,
                Event::Confirm { ride_open: false },
                wiping,
                Go::Question,
            ),
            (
                Failed,
                Event::Confirm { ride_open: false },
                wiping,
                Go::Question,
            ),
            (Idle, Event::Confirm { ride_open: true }, Idle, Go::Stay),
            (
                wiping,
                Event::Confirm { ride_open: false },
                wiping,
                Go::Stay,
            ),
            (wiping, press(ACT, 9_000), wiping, Go::Stay),
            (
                wiping,
                Event::Wrote(Next::Stage(Stage::Table)),
                Running(Stage::Table),
                Go::Stay,
            ),
            (
                Running(Stage::Settings),
                Event::Wrote(Next::Done),
                Done,
                Go::Stay,
            ),
            (wiping, Event::WriteFailed, Failed, Go::Ready),
            (Done, Event::Confirm { ride_open: false }, Done, Go::Stay),
            (Done, read(true, true), Done, Go::Stay),
        ];
        for (format, event, after, go) in rows {
            assert_eq!(step(format, event), (after, go), "{format:?} {event:?}");
        }
    }
}
