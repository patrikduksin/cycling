//! The screens the rider changed, kept apart from every other store in the
//! two-sector journal of `Region::Screens`. The record is `compose`'s: the
//! catalogue version, then only the changed screens, so a screen the rider
//! never touched shows its built-in and gains each improvement to it.
//! `compose::parse` is the record's only decoder. A record it refuses, one
//! of another catalogue version among them, leaves every screen built-in
//! until the next save overwrites it.
//!
//! A save names the revision it was made against, the sequence of the
//! journal record the phone read, and is refused once another save landed
//! or the journal was read again since the change was made.

use super::Runtime;
use crate::compose::builtin::{Nodes, ScreenId};
use crate::compose::input::Controls;
use crate::compose::parse::RECORD;
use crate::compose::{self, Edit, Reject, Screen, Screens};
use crate::screens::Device;
use device_api::storage::{OwnedFlash, Region};
use firmware_services::storage::{Error, Journal, Record, SECTOR_COUNT, Tip};
use firmware_services::why;

/// A screen the phone may change: its name in routes and traces, and the
/// screen it is, whose key the record names it by.
#[derive(Debug, PartialEq, Eq)]
pub struct Named {
    pub name: &'static str,
    pub id: ScreenId,
}

impl Named {
    pub const fn key(&self) -> u8 {
        self.id.key()
    }

    /// The tree it shows unchanged.
    pub fn builtin(&self) -> Nodes {
        (self.id.builtin().tree)()
    }
}

pub const NAMED: [Named; 1] = [Named {
    name: "ride/main",
    id: ScreenId::RideMain,
}];

pub fn named(name: &str) -> Option<&'static Named> {
    NAMED.iter().find(|named| named.name == name)
}

/// No screen changed: a static, as a temporary would take its 7 KB of stack.
static BUILT_IN: Screens = Screens::EMPTY;

/// What a change asks of the record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change<'a> {
    /// Screen `key` shows the tree of these stored nodes.
    Set { key: u8, nodes: &'a [u8] },
    /// Screen `key` shows its built-in again.
    Reset(u8),
    /// Every screen shows its built-in again.
    ResetAll,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Saved {
    Revision(u32),
    /// The journal is at this revision now: another save landed after the
    /// phone read its own, or the change was made over screens the journal
    /// had not given. The phone reads the screens again.
    Stale(u32),
    /// The journal could not be read or written; the screens shown are
    /// what it held when last read.
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Stored {
    Unread,
    Unreadable,
    /// Bytes of no journal, such as an older image's: every screen shows
    /// its built-in, and the next save erases them first.
    Foreign,
    /// The scan the screens shown were read from, which the next save
    /// follows. `reset` when its newest record was not one this build
    /// accepts, so every screen shows its built-in.
    Read {
        tip: Tip,
        reset: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Save {
    Idle,
    Waiting { base: u32 },
    Done(Saved),
}

/// The screens shown and the ones proposed beside them, too large to live
/// in the runtime.
pub struct Overrides {
    screens: [Screens; 2],
    shown: usize,
    /// The proposal's record, its first `proposed` bytes.
    record: [u8; RECORD],
    proposed: usize,
    stored: Stored,
    save: Save,
}

impl Overrides {
    pub const fn new() -> Self {
        Self {
            screens: [Screens::EMPTY, Screens::EMPTY],
            shown: 0,
            record: [0; RECORD],
            proposed: 0,
            stored: Stored::Unread,
            save: Save::Idle,
        }
    }

    pub(crate) fn shown(&self) -> &Screens {
        &self.screens[self.shown]
    }

    /// The screens [`Overrides::propose`] last accepted.
    pub(crate) fn proposed(&self) -> &Screens {
        &self.screens[1 - self.shown]
    }

    /// The sequence of the journal's newest record, 0 when it holds none;
    /// none before its first read or after a failed one.
    pub(crate) fn revision(&self) -> Option<u32> {
        match self.stored {
            Stored::Read { tip, .. } => Some(tip.newest().map_or(0, |record| record.sequence)),
            Stored::Foreign => Some(0),
            Stored::Unread | Stored::Unreadable => None,
        }
    }

    /// The journal held a record this build refused.
    pub(crate) fn reset(&self) -> bool {
        matches!(
            self.stored,
            Stored::Foreign | Stored::Read { reset: true, .. }
        )
    }

    /// Accept the screens shown with `change` made, as the proposal a save
    /// would write.
    pub(crate) fn propose(&mut self, change: Change<'_>, device: &Device) -> Result<(), Reject> {
        self.proposed = 0;
        let set;
        let edit = match change {
            Change::Set { key, nodes } => {
                set = Screen::parse(
                    key,
                    nodes,
                    &device.scaled,
                    ScreenId::controls(key, device.keys),
                )?;
                Some(Edit::Set(&set))
            }
            Change::Reset(key) => Some(Edit::Reset(key)),
            Change::ResetAll => None,
        };
        let [first, second] = &mut self.screens;
        let (shown, proposed) = if self.shown == 0 {
            (&*first, second)
        } else {
            (&*second, first)
        };
        let length = match edit {
            Some(edit) => shown.edited(edit, &mut self.record),
            None => BUILT_IN.encode(&mut self.record),
        }?;
        compose::parse(
            &self.record[..length],
            &device.scaled,
            &controls(device),
            proposed,
        )?;
        self.proposed = length;
        Ok(())
    }

    /// Save the proposal on the next [`Runtime::serve_screens`], if the
    /// journal is still at revision `base` then.
    pub(crate) fn save(&mut self, base: u32) {
        self.save = if self.proposed == 0 {
            Save::Done(Saved::Failed)
        } else {
            Save::Waiting { base }
        };
    }

    /// The outcome of the last save, once.
    pub(crate) fn saved(&mut self) -> Option<Saved> {
        match self.save {
            Save::Done(saved) => {
                self.save = Save::Idle;
                Some(saved)
            }
            Save::Idle | Save::Waiting { .. } => None,
        }
    }

    fn serve(&mut self, flash: impl OwnedFlash, device: &Device) {
        let waiting = match self.save {
            Save::Waiting { base } => Some(base),
            Save::Idle | Save::Done(_) => None,
        };
        let read = match (self.stored, waiting) {
            (Stored::Unread, _) | (Stored::Unreadable, Some(_)) => true,
            (_, None) => return,
            (_, Some(_)) => false,
        };
        let mut journal = Journal::new(flash, Region::Screens);
        if read {
            self.load(&mut journal, device);
        }
        let Some(base) = waiting else {
            return;
        };
        // A read replaces the proposal, which was made over screens the
        // journal had not given.
        let saved = match (read, self.revision()) {
            (false, _) => self.commit(&mut journal, base, device),
            (true, Some(revision)) => Saved::Stale(revision),
            (true, None) => Saved::Failed,
        };
        why!("screens.saved", saved, base);
        self.save = Save::Done(saved);
    }

    /// Read the journal's newest record into the screens shown.
    #[inline(never)]
    fn load<F: OwnedFlash>(&mut self, journal: &mut Journal<F>, device: &Device) {
        let shown = &mut self.screens[self.shown];
        self.proposed = 0;
        let loaded = match read(journal, &mut self.record) {
            // A commit word a cut left marginal read whole, then torn: the
            // save behind it was whole, so commit it again and read again.
            Err(Error::ReadbackFailed) => {
                for sector in 0..SECTOR_COUNT {
                    let _ = journal.recommit(sector);
                }
                read(journal, &mut self.record)
            }
            loaded => loaded,
        };
        self.stored = match loaded {
            Ok((tip, None)) => {
                *shown = Screens::EMPTY;
                Stored::Read { tip, reset: false }
            }
            Ok((tip, Some(record))) => {
                let bytes = &self.record[..record.length];
                let accepted = compose::parse(bytes, &device.scaled, &controls(device), shown);
                if let Err(reject) = accepted {
                    why!("screens.refused", reject, revision = record.sequence);
                }
                Stored::Read {
                    tip,
                    reset: accepted.is_err(),
                }
            }
            Err(Error::UnrecognizedJournal) => {
                *shown = Screens::EMPTY;
                Stored::Foreign
            }
            Err(_) => Stored::Unreadable,
        };
        why!(
            "screens.loaded",
            stored = self.stored,
            screens = shown.screens().len()
        );
    }

    #[inline(never)]
    fn commit<F: OwnedFlash>(
        &mut self,
        journal: &mut Journal<F>,
        base: u32,
        device: &Device,
    ) -> Saved {
        let Some(revision) = self.revision() else {
            return Saved::Failed;
        };
        if revision != base {
            return Saved::Stale(revision);
        }
        let tip = match self.stored {
            Stored::Read { tip, .. } => Some(tip),
            Stored::Foreign => journal.erase().and_then(|()| journal.tip()).ok(),
            Stored::Unread | Stored::Unreadable => return Saved::Failed,
        };
        let bytes = &self.record[..self.proposed];
        let saved = tip.and_then(|mut tip| {
            let record = journal.save_after(&mut tip, bytes).ok()?;
            Some((record, tip))
        });
        if let Some((record, tip)) = saved {
            self.shown = 1 - self.shown;
            self.proposed = 0;
            self.stored = Stored::Read { tip, reset: false };
            return Saved::Revision(record.sequence);
        }
        // A failed write may still have committed: read back what the
        // journal holds and answer from that.
        let proposed = 1 - self.shown;
        self.load(journal, device);
        match self.revision() {
            Some(now) if now != revision && self.screens[self.shown] == self.screens[proposed] => {
                Saved::Revision(now)
            }
            _ => Saved::Failed,
        }
    }
}

/// One scan of the journal, and the newest record it names read into
/// `output`.
fn read<F: OwnedFlash>(
    journal: &mut Journal<F>,
    output: &mut [u8],
) -> Result<(Tip, Option<Record>), Error<F::Error>> {
    let tip = journal.tip()?;
    Ok((tip, journal.read(tip, output)?))
}

impl Default for Overrides {
    fn default() -> Self {
        Self::new()
    }
}

/// What each stored screen's buttons do and the cues they show on
/// `device`: its built-in's, as what a button does stays in code.
fn controls(device: &Device) -> impl Fn(u8) -> Controls {
    let keys = device.keys;
    move |key| ScreenId::controls(key, keys)
}

/// The screens the rider changed, for the composition to show in place of
/// their built-ins.
pub(super) fn shown<'a>(overrides: &'a Option<&'static mut Overrides>) -> &'a Screens {
    overrides.as_deref().map_or(&BUILT_IN, Overrides::shown)
}

impl Runtime {
    /// Give the rider's screens their working memory; without it every
    /// screen shows its built-in and the phone cannot change one.
    pub fn attach_screens(&mut self, overrides: &'static mut Overrides) {
        self.overrides = Some(overrides);
    }

    /// Read the rider's screens on the first pass, and write a save the
    /// phone asked for.
    #[inline(never)]
    pub fn serve_screens(&mut self, flash: impl OwnedFlash) {
        if self.format.holds_host_storage() {
            return;
        }
        if let Some(overrides) = self.overrides.as_deref_mut() {
            overrides.serve(flash, self.device);
        }
    }

    /// The screens the rider changed, for the composition to show in
    /// place of their built-ins.
    pub fn screens(&self) -> &Screens {
        shown(&self.overrides)
    }
}

#[cfg(test)]
mod tests;
