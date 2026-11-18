//! Incremental scanner and append scheduler for one ride journal.
//!
//! The same recorder runs the internal journal and the bulk MMC log; a
//! [`Journal`] supplies the medium. Bulk media that are missing, unformatted or
//! untrustworthy stay in their own state and are never written implicitly.
//!
//! Boot scans without erasing. Unknown occupied data reads foreign, and only
//! FORMAT erases it, except one torn first append on an otherwise empty
//! journal, which stays an occupied invalid slot. Start never reclaims space
//! and needs room for one sample batch besides the terminal slot. A media
//! failure stops mutation until inspection or reboot/rescan resolves an
//! uncertain commit. Clear verifies each erased sector
//! but is not atomic across power loss. Never retry a timed-out mutation blindly.
//!
//! Pause/finish freeze active duration when accepted, before flushing. Sampling
//! stays on the 1 Hz grid of the start/resume press; late service takes the
//! due sample late, never backfills, and counts wholly skipped periods as
//! dropped. Reset can lose three buffered samples, or four during commit.
//! Lap marks wait in order, each for a stored sample of its own: the ride's
//! first sample carries `LapStart::Ride`, and the first of each later lap its
//! trigger. A pause or finish with laps waiting stores one more sample per
//! waiting mark, at the stop, to carry them.
//! One operation and its unconsumed completion exclude further mutations.

use crate::ride::journal::{Append, Checkpoint, Journal, Progress, Scan};
use crate::ride::laps::Marks;
use crate::ride::log::Catalog;
use crate::ride::log::Entry;
use crate::ride::log::Kind;
use crate::ride::log::LapStart;
use crate::ride::log::Recovery;
use crate::ride::log::SAMPLES_PER_BATCH;
use crate::ride::log::Sample;
use crate::ride::log::Status;
use crate::ride::log::Summary;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[cfg_attr(test, derive(Hash))]
pub enum Action {
    Start,
    Pause,
    Resume,
    Finish,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refused {
    Foreign,
    State,
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, Hash, PartialEq))]
pub struct ResultEvent {
    pub action: Option<Action>,
    pub token: u32,
    pub ok: bool,
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, Hash, PartialEq))]
enum Lifecycle {
    Scanning,
    NeedsInit,
    Foreign,
    Unsupported,
    Unavailable,
    Corrupt,
    Preparing {
        clear: bool,
        step: usize,
        token: u32,
    },
    Ready,
    Saved,
    Running {
        ride: OpenRide,
        since: u64,
        next_sample: u64,
    },
    Stopping {
        ride: OpenRide,
        token: u32,
        stop: Stop,
    },
    Paused {
        ride: OpenRide,
    },
    Recovering,
    Recovered,
    Full {
        open: bool,
    },
    Failed {
        open: bool,
    },
}

impl Lifecycle {
    fn ride(&self) -> Option<&OpenRide> {
        match self {
            Self::Running { ride, .. } | Self::Stopping { ride, .. } | Self::Paused { ride } => {
                Some(ride)
            }
            _ => None,
        }
    }

    fn ride_mut(&mut self) -> Option<&mut OpenRide> {
        match self {
            Self::Running { ride, .. } | Self::Stopping { ride, .. } | Self::Paused { ride } => {
                Some(ride)
            }
            _ => None,
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, Hash, PartialEq))]
enum Stop {
    Pause { since: u64, at: u64 },
    Finish { since: u64, at: u64 },
    FinishPaused,
}

impl Stop {
    fn action(self) -> Action {
        match self {
            Self::Pause { .. } => Action::Pause,
            Self::Finish { .. } | Self::FinishPaused => Action::Finish,
        }
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, Hash, PartialEq))]
struct OpenRide {
    id: u32,
    sequence: u32,
    committed_ms: u64,
    summary: Summary,
}

impl OpenRide {
    fn committed(mut self, active_ms: u64) -> Self {
        self.sequence = self.sequence.wrapping_add(1);
        self.committed_ms = active_ms;
        self
    }
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, Hash, PartialEq))]
enum Inbox {
    Idle,
    Pending(Ask),
    Done(ResultEvent),
}

#[derive(Clone, Copy)]
#[cfg_attr(test, derive(Debug, Hash, PartialEq))]
enum Ask {
    Start { at: u64, token: u32 },
    Resume { at: u64, token: u32 },
}

#[derive(Default)]
#[cfg_attr(test, derive(Clone, Hash))]
struct Batch {
    samples: [Sample; SAMPLES_PER_BATCH],
    count: usize,
    laps: Marks,
}

impl Batch {
    fn full(&self) -> bool {
        self.count == self.samples.len()
    }

    fn stored(&self) -> &[Sample] {
        &self.samples[..self.count]
    }

    fn take(&mut self, sample: Sample, active_ms: u64) {
        let mut sample = sample.without_impossible_battery();
        sample.active_ms = active_ms;
        sample.lap = self.laps.pop();
        self.samples[self.count] = sample;
        self.count += 1;
    }
}

#[derive(Default)]
#[cfg_attr(test, derive(Clone, Hash))]
struct Counters {
    written: u32,
    dropped: u32,
    max_write_ms: u32,
    max_erase_ms: u32,
}

#[derive(Clone, Copy)]
enum Effect {
    Scan,
    Prepare {
        clear: bool,
        step: usize,
        seed: u64,
    },
    Checkpoint,
    Append {
        slot: usize,
        terminal: bool,
        record: Record,
    },
}

#[derive(Clone, Copy)]
enum Record {
    Event {
        kind: Kind,
        ride: u32,
        sequence: u32,
        active_ms: u64,
    },
    Batch {
        ride: u32,
        sequence: u32,
    },
}

#[derive(Clone, Copy)]
enum Found {
    Done { overdue: bool },
    Uninitialized,
    Foreign,
    Unsupported,
    Unavailable,
    Corrupt,
    Failed,
}

#[derive(Clone, Copy)]
enum Outcome {
    Scanning,
    Scanned {
        found: Found,
        slots: usize,
    },
    Preparing {
        elapsed_ms: u32,
    },
    Prepared {
        slots: Option<usize>,
        elapsed_ms: u32,
    },
    Checkpointed(Checkpoint),
    Appended {
        record: Record,
        append: Append,
        active_ms: u64,
        elapsed_ms: u32,
    },
    NoRoom,
}

#[cfg_attr(test, derive(Clone, Hash))]
pub struct Recorder {
    lifecycle: Lifecycle,
    inbox: Inbox,
    checkpoint_due: bool,
    catalog: Catalog,
    /// Slots of the medium, including the reserved terminal slot.
    slots: Option<usize>,
    accumulated_ms: u64,
    batch: Batch,
    counters: Counters,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            lifecycle: Lifecycle::Scanning,
            inbox: Inbox::Idle,
            checkpoint_due: false,
            catalog: Catalog::default(),
            slots: None,
            accumulated_ms: 0,
            batch: Batch::default(),
            counters: Counters::default(),
        }
    }
}

impl Recorder {
    #[cfg(test)]
    pub(crate) const VARIANTS: [&'static str; 19] = [
        "scanning",
        "needs init",
        "foreign",
        "unsupported",
        "unavailable",
        "corrupt",
        "formatting",
        "clearing",
        "ready",
        "saved",
        "running",
        "stopping to pause",
        "stopping to finish",
        "finishing while paused",
        "paused",
        "recovering",
        "recovered",
        "full",
        "failed",
    ];

    #[cfg(test)]
    pub(crate) fn variant(&self) -> &'static str {
        match &self.lifecycle {
            Lifecycle::Scanning => "scanning",
            Lifecycle::NeedsInit => "needs init",
            Lifecycle::Foreign => "foreign",
            Lifecycle::Unsupported => "unsupported",
            Lifecycle::Unavailable => "unavailable",
            Lifecycle::Corrupt => "corrupt",
            Lifecycle::Preparing { clear: false, .. } => "formatting",
            Lifecycle::Preparing { clear: true, .. } => "clearing",
            Lifecycle::Ready => "ready",
            Lifecycle::Saved => "saved",
            Lifecycle::Running { .. } => "running",
            Lifecycle::Stopping {
                stop: Stop::Pause { .. },
                ..
            } => "stopping to pause",
            Lifecycle::Stopping {
                stop: Stop::Finish { .. },
                ..
            } => "stopping to finish",
            Lifecycle::Stopping {
                stop: Stop::FinishPaused,
                ..
            } => "finishing while paused",
            Lifecycle::Paused { .. } => "paused",
            Lifecycle::Recovering => "recovering",
            Lifecycle::Recovered => "recovered",
            Lifecycle::Full { .. } => "full",
            Lifecycle::Failed { .. } => "failed",
        }
    }

    pub fn status(&self) -> Status {
        match &self.lifecycle {
            Lifecycle::Scanning => Status::Scanning,
            Lifecycle::NeedsInit => Status::NeedsInit,
            Lifecycle::Foreign => Status::Foreign,
            Lifecycle::Unsupported => Status::Unsupported,
            Lifecycle::Unavailable => Status::Unavailable,
            Lifecycle::Corrupt => Status::Corrupt,
            Lifecycle::Preparing { clear: false, .. } => Status::Formatting,
            Lifecycle::Preparing { clear: true, .. } => Status::Clearing,
            Lifecycle::Ready => Status::Ready,
            Lifecycle::Saved => Status::Saved,
            Lifecycle::Running { .. }
            | Lifecycle::Stopping {
                stop: Stop::Pause { .. } | Stop::Finish { .. },
                ..
            } => Status::Recording,
            Lifecycle::Paused { .. }
            | Lifecycle::Stopping {
                stop: Stop::FinishPaused,
                ..
            } => Status::Paused,
            Lifecycle::Recovering | Lifecycle::Recovered => Status::Recovered,
            Lifecycle::Full { .. } => Status::Full,
            Lifecycle::Failed { .. } => Status::Error,
        }
    }

    pub fn completed(&self) -> u16 {
        self.catalog.completed
    }

    pub fn written_samples(&self) -> u32 {
        self.counters.written
    }

    pub fn dropped_samples(&self) -> u32 {
        self.counters.dropped
    }

    pub fn next_slot(&self) -> usize {
        self.catalog.next_slot
    }

    pub fn max_write_ms(&self) -> u32 {
        self.counters.max_write_ms
    }

    /// Optimistic 1 Hz recording time left before the journal is full, after
    /// reserving START (when no ride is open) and the terminal slot. Each
    /// pause/resume costs an event slot and may pad a partial batch.
    pub fn remaining_secs(&self) -> u32 {
        if !self.status().usable() {
            return 0;
        }
        let used = self
            .catalog
            .next_slot
            .saturating_sub(usize::from(self.ride_open()));
        crate::ride::reclaim::estimated_seconds_in(self.capacity(), used)
            .saturating_sub(self.batch.count as u32)
    }

    /// Slots of the medium, including the reserved terminal slot.
    pub fn slots(&self) -> Option<usize> {
        self.slots
    }

    fn capacity(&self) -> usize {
        self.slots.unwrap_or(0)
    }

    /// A ride is open and running or paused on this journal.
    pub fn riding(&self) -> bool {
        self.lifecycle.ride().is_some()
    }

    /// The ride action accepted and not yet completed.
    pub fn pending_action(&self) -> Option<Action> {
        self.waiting().map(|(action, _)| action)
    }

    fn waiting(&self) -> Option<(Action, u32)> {
        match (&self.lifecycle, &self.inbox) {
            (Lifecycle::Stopping { token, stop, .. }, _) => Some((stop.action(), *token)),
            (_, Inbox::Pending(Ask::Start { token, .. })) => Some((Action::Start, *token)),
            (_, Inbox::Pending(Ask::Resume { token, .. })) => Some((Action::Resume, *token)),
            _ => None,
        }
    }

    /// Recovered with an interrupted ride whose closing record is still to be
    /// appended; a new ride may start once it is.
    pub fn recovering(&self) -> bool {
        matches!(self.lifecycle, Lifecycle::Recovering)
    }

    pub fn busy(&self) -> bool {
        !matches!(self.inbox, Inbox::Idle)
            || matches!(
                self.lifecycle,
                Lifecycle::Stopping { .. } | Lifecycle::Preparing { .. }
            )
    }

    pub fn max_erase_ms(&self) -> u32 {
        self.counters.max_erase_ms
    }

    pub fn open_ride_id(&self) -> Option<u32> {
        self.lifecycle.ride().map(|ride| ride.id)
    }

    pub fn ride_open(&self) -> bool {
        match &self.lifecycle {
            Lifecycle::Recovering => true,
            Lifecycle::Full { open } | Lifecycle::Failed { open } => *open,
            lifecycle => lifecycle.ride().is_some(),
        }
    }

    pub fn summaries(&self) -> [Option<Summary>; crate::ride::log::HISTORY_CAPACITY] {
        self.catalog.summaries
    }

    /// One kept summary, oldest first, without copying the others.
    pub fn summary(&self, index: usize) -> Option<&Summary> {
        self.catalog.summaries.get(index)?.as_ref()
    }

    pub fn summary_count(&self) -> u8 {
        self.catalog.summary_count
    }

    /// The id the next ride takes; ids count from one after the journal is
    /// emptied.
    pub fn next_ride_id(&self) -> u32 {
        self.catalog.next_ride_id
    }

    #[cfg(test)]
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn exportable(&self) -> bool {
        matches!(self.inbox, Inbox::Idle)
            && matches!(
                self.lifecycle,
                Lifecycle::NeedsInit
                    | Lifecycle::Foreign
                    | Lifecycle::Ready
                    | Lifecycle::Saved
                    | Lifecycle::Recovered
                    | Lifecycle::Full { open: false }
                    | Lifecycle::Corrupt
            )
    }

    pub fn request(&mut self, action: Action, now: u64, token: u32) -> bool {
        if self.busy() {
            return false;
        }
        match (action, self.lifecycle) {
            (Action::Start, Lifecycle::Ready | Lifecycle::Saved | Lifecycle::Recovered)
                if self.catalog.recovery.is_none() =>
            {
                self.inbox = Inbox::Pending(Ask::Start { at: now, token });
            }
            (Action::Pause, Lifecycle::Running { ride, since, .. }) => {
                let stop = Stop::Pause { since, at: now };
                self.lifecycle = Lifecycle::Stopping { ride, token, stop };
            }
            (Action::Resume, Lifecycle::Paused { .. }) => {
                self.inbox = Inbox::Pending(Ask::Resume { at: now, token });
            }
            (Action::Finish, Lifecycle::Running { ride, since, .. }) => {
                let stop = Stop::Finish { since, at: now };
                self.lifecycle = Lifecycle::Stopping { ride, token, stop };
            }
            (Action::Finish, Lifecycle::Paused { ride }) => {
                let stop = Stop::FinishPaused;
                self.lifecycle = Lifecycle::Stopping { ride, token, stop };
            }
            _ => return false,
        }
        true
    }

    /// Explicitly prepare a journal that holds no usable rides. Corrupt bulk
    /// media is only superseded by a new epoch; its sectors stay readable.
    pub fn initialize(&mut self, token: u32) -> Result<(), Refused> {
        if matches!(self.lifecycle, Lifecycle::Foreign) {
            return Err(Refused::Foreign);
        }
        if !matches!(self.lifecycle, Lifecycle::NeedsInit | Lifecycle::Corrupt) || self.busy() {
            return Err(Refused::State);
        }
        self.lifecycle = Lifecycle::Preparing {
            clear: false,
            step: 0,
            token,
        };
        Ok(())
    }

    pub(crate) fn clearable(&self, expected_upper: usize) -> Result<(), Refused> {
        if matches!(self.lifecycle, Lifecycle::Foreign) {
            return Err(Refused::Foreign);
        }
        if !crate::ride::reclaim::request_allowed(
            self.status(),
            self.ride_open(),
            self.busy(),
            expected_upper,
            self.catalog.next_slot,
        ) {
            return Err(Refused::State);
        }
        Ok(())
    }

    pub fn clear(&mut self, expected_upper: usize, token: u32) -> Result<(), Refused> {
        self.clearable(expected_upper)?;
        self.lifecycle = Lifecycle::Preparing {
            clear: true,
            step: 0,
            token,
        };
        Ok(())
    }

    /// FORMAT wiped the journal: it holds no rides, and the host Clear
    /// the wipe carried out, if any, completes.
    pub fn wiped(&mut self, clear: Option<u32>) {
        self.emptied();
        if let Some(token) = clear {
            self.inbox = Inbox::Done(ResultEvent {
                action: None,
                token,
                ok: true,
            });
        }
    }

    fn emptied(&mut self) {
        self.catalog = Catalog {
            next_ride_id: 1,
            ..Catalog::default()
        };
        self.accumulated_ms = 0;
        self.batch.count = 0;
        self.lifecycle = Lifecycle::Ready;
    }

    /// A lap closed: a sample this ride stores, after those of laps already
    /// waiting, begins a new one. Nothing is marked without an open ride.
    pub fn mark_lap(&mut self, start: LapStart) {
        if self.riding() {
            self.batch.laps.push(start);
        }
    }

    pub fn take_result(&mut self) -> Option<ResultEvent> {
        let Inbox::Done(result) = self.inbox else {
            return None;
        };
        self.inbox = Inbox::Idle;
        Some(result)
    }

    /// A pause or finish was accepted and active time is frozen, though the
    /// status stays Recording until its flush completes.
    pub fn stopping(&self) -> bool {
        matches!(
            self.lifecycle,
            Lifecycle::Stopping {
                stop: Stop::Pause { .. } | Stop::Finish { .. },
                ..
            }
        )
    }

    /// When the next 1 Hz sample is due, while one is.
    pub fn sample_due_ms(&self) -> Option<u64> {
        match &self.lifecycle {
            Lifecycle::Running { next_sample, .. } => Some(*next_sample),
            _ => None,
        }
    }

    pub fn active_ms(&self, now: u64) -> u64 {
        let (since, stop) = match &self.lifecycle {
            Lifecycle::Running { since, .. } => (*since, None),
            Lifecycle::Stopping {
                stop: Stop::Pause { since, at } | Stop::Finish { since, at },
                ..
            } => (*since, Some(*at)),
            _ => return self.accumulated_ms,
        };
        crate::ride::log::active_at(self.accumulated_ms, since, stop, now)
    }

    /// Perform at most one scan step, initialize/clear step, checkpoint, or
    /// committed slot append.
    pub fn service(
        &mut self,
        journal: &mut impl Journal,
        now: u64,
        sample: Sample,
        clock: &impl Fn() -> u64,
    ) {
        let before = self.status();
        if let Some(effect) = self.step(now, sample) {
            let outcome = apply(self, effect, journal, clock);
            self.settle(outcome, now);
        }
        let status = self.status();
        if status != before {
            if status == Status::Error {
                log::error!(target: "sdk.recorder", "media_failed previous={} next_slot={}", before.name(), self.catalog.next_slot);
            } else {
                log::info!(target: "sdk.recorder", "state_changed previous={} current={} next_slot={}", before.name(), status.name(), self.catalog.next_slot);
            }
        }
    }

    fn step(&mut self, now: u64, sample: Sample) -> Option<Effect> {
        match self.lifecycle {
            Lifecycle::Scanning => return Some(Effect::Scan),
            Lifecycle::Preparing { clear, step, .. } => {
                let seed = now ^ (u64::from(self.catalog.next_ride_id) << 32) ^ step as u64;
                return Some(Effect::Prepare { clear, step, seed });
            }
            Lifecycle::Failed { .. } => return None,
            _ => {}
        }
        if self.checkpoint_due {
            self.checkpoint_due = false;
            return Some(Effect::Checkpoint);
        }
        self.sample(now, sample);
        if let (Lifecycle::Recovering, Some(recovery)) = (self.lifecycle, self.catalog.recovery) {
            let record = Record::Event {
                kind: Kind::Recovered,
                ride: recovery.ride_id,
                sequence: recovery.next_sequence,
                active_ms: recovery.active_ms,
            };
            return self.append(now, true, record);
        }
        match (self.lifecycle, self.inbox) {
            (Lifecycle::Stopping { ride, stop, .. }, _) => {
                if self.batch.count != 0 {
                    return self.flush(now, ride.id, ride.sequence);
                }
                let (kind, terminal) = match stop {
                    Stop::Pause { .. } => (Kind::Pause, false),
                    Stop::Finish { .. } | Stop::FinishPaused => (Kind::Finish, true),
                };
                let record = Record::Event {
                    kind,
                    ride: ride.id,
                    sequence: ride.sequence,
                    active_ms: self.active_ms(now),
                };
                self.append(now, terminal, record)
            }
            (_, Inbox::Pending(Ask::Start { token, .. })) => {
                if crate::ride::reclaim::estimated_seconds_in(
                    self.capacity(),
                    self.catalog.next_slot,
                ) == 0
                {
                    // No room for START, one sample batch and the terminal record.
                    self.lifecycle = Lifecycle::Full {
                        open: self.ride_open(),
                    };
                    self.inbox = Inbox::Done(ResultEvent {
                        action: Some(Action::Start),
                        token,
                        ok: false,
                    });
                    return None;
                }
                let record = Record::Event {
                    kind: Kind::Start,
                    ride: self.catalog.next_ride_id,
                    sequence: 0,
                    active_ms: 0,
                };
                self.append(now, false, record)
            }
            (Lifecycle::Paused { ride }, Inbox::Pending(Ask::Resume { .. })) => {
                let record = Record::Event {
                    kind: Kind::Resume,
                    ride: ride.id,
                    sequence: ride.sequence,
                    active_ms: self.accumulated_ms,
                };
                self.append(now, false, record)
            }
            (Lifecycle::Running { ride, .. }, _) if self.batch.full() => {
                self.flush(now, ride.id, ride.sequence)
            }
            _ => None,
        }
    }

    fn sample(&mut self, now: u64, sample: Sample) {
        let active_ms = self.active_ms(now);
        match &mut self.lifecycle {
            Lifecycle::Running { next_sample, .. } if now >= *next_sample => {
                if self.batch.full() {
                    self.counters.dropped = self.counters.dropped.saturating_add(1);
                } else {
                    self.batch.take(sample, active_ms);
                }
                let missed = (now - *next_sample) / 1_000;
                self.counters.dropped = self
                    .counters
                    .dropped
                    .saturating_add(missed.min(u64::from(u32::MAX)) as u32);
                *next_sample =
                    next_sample.saturating_add(missed.saturating_add(1).saturating_mul(1_000));
            }
            Lifecycle::Stopping {
                stop: Stop::Pause { .. } | Stop::Finish { .. },
                ..
            } => {
                while !self.batch.full() && self.batch.laps.lap_waiting() {
                    self.batch.take(sample, active_ms);
                }
            }
            _ => {}
        }
    }

    fn flush(&mut self, now: u64, ride: u32, sequence: u32) -> Option<Effect> {
        if self.catalog.next_slot >= self.capacity().saturating_sub(1) {
            self.freeze(now);
            self.counters.dropped = self
                .counters
                .dropped
                .saturating_add(self.batch.count as u32);
            self.batch.count = 0;
            let record = Record::Event {
                kind: Kind::Full,
                ride,
                sequence,
                active_ms: self.active_ms(now),
            };
            return self.append(now, true, record);
        }
        self.append(now, false, Record::Batch { ride, sequence })
    }

    fn append(&mut self, now: u64, terminal: bool, record: Record) -> Option<Effect> {
        let (slot, slots) = (self.catalog.next_slot, self.capacity());
        if slot >= slots || (!terminal && slot >= slots.saturating_sub(1)) {
            self.settle(Outcome::NoRoom, now);
            return None;
        }
        Some(Effect::Append {
            slot,
            terminal,
            record,
        })
    }

    fn settle(&mut self, outcome: Outcome, now: u64) {
        match outcome {
            Outcome::Scanning | Outcome::Checkpointed(Checkpoint::Written) => {}
            Outcome::Scanned { found, slots } => {
                self.slots = Some(slots);
                self.lifecycle = match found {
                    Found::Done { overdue } => {
                        self.checkpoint_due = overdue;
                        if self.catalog.foreign() {
                            Lifecycle::Foreign
                        } else if self.catalog.next_slot >= slots {
                            Lifecycle::Full { open: false }
                        } else if let Some(recovery) = self.catalog.recovery {
                            self.accumulated_ms = recovery.active_ms;
                            Lifecycle::Recovering
                        } else {
                            Lifecycle::Ready
                        }
                    }
                    Found::Uninitialized => Lifecycle::NeedsInit,
                    Found::Foreign => Lifecycle::Foreign,
                    Found::Unsupported => Lifecycle::Unsupported,
                    Found::Unavailable => Lifecycle::Unavailable,
                    Found::Corrupt => Lifecycle::Corrupt,
                    Found::Failed => Lifecycle::Failed { open: false },
                };
            }
            Outcome::Preparing { elapsed_ms } => {
                self.counters.max_erase_ms = self.counters.max_erase_ms.max(elapsed_ms);
                if let Lifecycle::Preparing { step, .. } = &mut self.lifecycle {
                    *step += 1;
                }
            }
            Outcome::Prepared { slots, elapsed_ms } => {
                self.counters.max_erase_ms = self.counters.max_erase_ms.max(elapsed_ms);
                let token = match self.lifecycle {
                    Lifecycle::Preparing { token, .. } => token,
                    _ => 0,
                };
                if let Some(slots) = slots {
                    self.slots = Some(slots);
                    self.emptied();
                } else {
                    self.lifecycle = Lifecycle::Failed { open: false };
                }
                self.inbox = Inbox::Done(ResultEvent {
                    action: None,
                    token,
                    ok: slots.is_some(),
                });
            }
            Outcome::Checkpointed(Checkpoint::Failed) => self.stop(now, false),
            Outcome::Appended {
                record,
                append,
                active_ms,
                elapsed_ms,
            } => {
                self.counters.max_write_ms = self.counters.max_write_ms.max(elapsed_ms);
                match append {
                    Append::Verified { checkpoint_due } => {
                        self.catalog.next_slot += 1;
                        self.catalog.valid_slots = self.catalog.valid_slots.saturating_add(1);
                        self.checkpoint_due |= checkpoint_due;
                        self.committed(record, active_ms);
                    }
                    Append::Uncertain => {
                        self.stop(now, false);
                    }
                }
            }
            Outcome::NoRoom => self.stop(now, true),
        }
    }

    fn stop(&mut self, now: u64, full: bool) {
        self.freeze(now);
        let waiting = self.waiting();
        let open = self.ride_open();
        self.lifecycle = if full {
            Lifecycle::Full { open }
        } else {
            Lifecycle::Failed { open }
        };
        self.fail(waiting);
    }

    fn fail(&mut self, waiting: Option<(Action, u32)>) {
        let Some((action, token)) = waiting else {
            return;
        };
        self.inbox = Inbox::Done(ResultEvent {
            action: Some(action),
            token,
            ok: false,
        });
    }

    fn committed(&mut self, record: Record, active_ms: u64) {
        let done = |action, token| {
            Inbox::Done(ResultEvent {
                action: Some(action),
                token,
                ok: true,
            })
        };
        match (record, self.lifecycle, self.inbox) {
            (Record::Batch { .. }, ..) => {
                if let Some(ride) = self.lifecycle.ride_mut() {
                    *ride = ride.committed(active_ms);
                    for sample in self.batch.stored() {
                        ride.summary.add_sample(*sample);
                    }
                }
                self.counters.written = self
                    .counters
                    .written
                    .saturating_add(self.batch.count as u32);
                self.batch.count = 0;
            }
            (
                Record::Event {
                    kind: Kind::Start,
                    ride,
                    ..
                },
                _,
                Inbox::Pending(Ask::Start { at, token }),
            ) => {
                self.accumulated_ms = 0;
                self.batch.laps = Marks::default();
                self.batch.laps.push(LapStart::Ride);
                self.catalog.next_ride_id = ride.wrapping_add(1).max(1);
                self.lifecycle = Lifecycle::Running {
                    ride: OpenRide {
                        id: ride,
                        sequence: 1,
                        committed_ms: active_ms,
                        summary: Summary::started(ride),
                    },
                    since: at,
                    next_sample: at,
                };
                self.inbox = done(Action::Start, token);
            }
            (
                Record::Event { .. },
                Lifecycle::Paused { ride },
                Inbox::Pending(Ask::Resume { at, token }),
            ) => {
                self.lifecycle = Lifecycle::Running {
                    ride: ride.committed(active_ms),
                    since: at,
                    next_sample: at,
                };
                self.inbox = done(Action::Resume, token);
            }
            (
                Record::Event {
                    kind: Kind::Full, ..
                },
                lifecycle,
                _,
            ) => {
                let waiting = self.waiting();
                if let Some(ride) = lifecycle.ride() {
                    self.close(ride.summary, Kind::Full, active_ms, false);
                }
                self.lifecycle = Lifecycle::Full { open: false };
                self.fail(waiting);
            }
            (Record::Event { .. }, Lifecycle::Stopping { ride, token, stop }, _) => {
                self.accumulated_ms = active_ms;
                match stop {
                    Stop::Pause { .. } => {
                        self.lifecycle = Lifecycle::Paused {
                            ride: ride.committed(active_ms),
                        };
                    }
                    Stop::Finish { .. } | Stop::FinishPaused => {
                        self.batch.laps = Marks::default();
                        self.close(ride.summary, Kind::Finish, active_ms, false);
                        self.catalog.next_ride_id = ride.id.wrapping_add(1).max(1);
                        self.lifecycle = Lifecycle::Saved;
                    }
                }
                self.inbox = done(stop.action(), token);
            }
            (Record::Event { .. }, Lifecycle::Recovering, _) => {
                if let Some(recovery) = self.catalog.recovery.take() {
                    self.accumulated_ms = recovery.active_ms;
                    self.close(
                        recovery.summary,
                        Kind::Recovered,
                        recovery.active_ms,
                        recovery.gap,
                    );
                }
                self.lifecycle = Lifecycle::Recovered;
            }
            (Record::Event { kind, .. }, ..) => {
                unreachable!(
                    "step appends no {kind:?} record in {}",
                    self.status().name()
                )
            }
        }
    }

    fn close(&mut self, mut summary: Summary, kind: Kind, active_ms: u64, gap: bool) {
        summary.finish(kind, active_ms, gap);
        self.catalog.push_summary(summary);
        self.catalog.completed = self.catalog.completed.saturating_add(1);
    }

    /// The state a scan of everything committed so far would rebuild.
    fn checkpoint_catalog(&self) -> Catalog {
        let mut catalog = self.catalog;
        if catalog.recovery.is_none()
            && let Some(ride) = self.lifecycle.ride()
        {
            catalog.recovery = Some(Recovery {
                ride_id: ride.id,
                next_sequence: ride.sequence,
                active_ms: ride.committed_ms,
                gap: false,
                summary: ride.summary,
            });
        }
        catalog
    }

    fn freeze(&mut self, now: u64) {
        self.accumulated_ms = self.active_ms(now);
        match &mut self.lifecycle {
            Lifecycle::Running { since, .. } => *since = now,
            Lifecycle::Stopping {
                stop: Stop::Pause { since, at } | Stop::Finish { since, at },
                ..
            } => (*since, *at) = (now, now),
            _ => {}
        }
    }
}

fn elapsed_ms(clock: &impl Fn() -> u64, started: u64) -> u32 {
    clock().saturating_sub(started).min(u64::from(u32::MAX)) as u32
}

// Out of line, so the entry and the catalogs it builds stay out of the
// caller's frame, which holds one copy of this per medium otherwise.
#[inline(never)]
fn apply(
    recorder: &mut Recorder,
    effect: Effect,
    journal: &mut impl Journal,
    clock: &impl Fn() -> u64,
) -> Outcome {
    match effect {
        Effect::Scan => {
            let scan = journal.scan();
            if scan == Scan::More {
                return Outcome::Scanning;
            }
            let slots = journal.slots();
            let found = match scan {
                Scan::More => return Outcome::Scanning,
                Scan::Done(catalog) => {
                    #[cfg(feature = "planted-bug")]
                    let catalog = forget_paused_recovery(catalog, journal);
                    recorder.catalog = catalog;
                    Found::Done {
                        overdue: journal.checkpoint_overdue(),
                    }
                }
                Scan::Uninitialized => Found::Uninitialized,
                Scan::Foreign => Found::Foreign,
                Scan::Unsupported => Found::Unsupported,
                Scan::Unavailable => Found::Unavailable,
                Scan::Corrupt => Found::Corrupt,
                Scan::Uncertain(catalog) => {
                    recorder.catalog = catalog;
                    Found::Corrupt
                }
                Scan::Failed => Found::Failed,
            };
            Outcome::Scanned { found, slots }
        }
        Effect::Prepare { clear, step, seed } => {
            let started = clock();
            let progress = if clear {
                journal.clear(step, seed)
            } else {
                journal.initialize(step, seed)
            };
            let elapsed_ms = elapsed_ms(clock, started);
            let slots = match progress {
                Progress::More => return Outcome::Preparing { elapsed_ms },
                Progress::Done => Some(journal.slots()),
                Progress::Failed => None,
            };
            Outcome::Prepared { slots, elapsed_ms }
        }
        Effect::Checkpoint => {
            Outcome::Checkpointed(journal.checkpoint(&recorder.checkpoint_catalog()))
        }
        Effect::Append {
            slot,
            terminal,
            record,
        } => {
            let entry = match record {
                Record::Event {
                    kind,
                    ride,
                    sequence,
                    active_ms,
                } => Entry::event(kind, ride, sequence, active_ms),
                Record::Batch { ride, sequence } => {
                    Entry::batch(ride, sequence, recorder.batch.stored()).unwrap()
                }
            };
            let started = clock();
            let append = journal.append(slot, &entry, terminal);
            Outcome::Appended {
                record,
                append,
                active_ms: entry.active_ms,
                elapsed_ms: elapsed_ms(clock, started),
            }
        }
    }
}

#[cfg(feature = "planted-bug")]
fn forget_paused_recovery(mut catalog: Catalog, journal: &mut impl Journal) -> Catalog {
    let mut last = crate::ride::log::Slot::default();
    let paused = catalog.next_slot > 0
        && journal.read_slot(catalog.next_slot - 1, &mut last) == crate::ride::journal::Read::Slot
        && crate::ride::log::decode(&last).is_some_and(|e| e.kind == Kind::Pause);
    if paused {
        catalog.recovery = None;
    }
    catalog
}

#[cfg(any(test, feature = "invariants"))]
pub mod invariants;

#[cfg(test)]
mod explore;

#[cfg(test)]
mod properties;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::journal::{Internal, InternalState, Read};
    use crate::ride::log::Slot;
    use crate::ride::storage::RideStorage;
    use std::vec;
    use std::vec::Vec;

    /// Internal journal media and its scan state, rebuilt on every reboot.
    #[derive(Default)]
    struct Memory {
        flash: Flash,
        state: InternalState,
    }
    impl core::ops::Deref for Memory {
        type Target = Flash;
        fn deref(&self) -> &Flash {
            &self.flash
        }
    }
    impl core::ops::DerefMut for Memory {
        fn deref_mut(&mut self) -> &mut Flash {
            &mut self.flash
        }
    }
    impl Journal for Memory {
        fn slots(&self) -> usize {
            crate::ride::log::SLOTS
        }
        fn scan(&mut self) -> Scan {
            Internal::new(&mut self.state, &mut self.flash).scan()
        }
        fn clear(&mut self, step: usize, seed: u64) -> Progress {
            Internal::new(&mut self.state, &mut self.flash).clear(step, seed)
        }
        fn append(&mut self, index: usize, entry: &Entry, terminal: bool) -> Append {
            Internal::new(&mut self.state, &mut self.flash).append(index, entry, terminal)
        }
        fn checkpoint(&mut self, catalog: &Catalog) -> Checkpoint {
            Internal::new(&mut self.state, &mut self.flash).checkpoint(catalog)
        }
        fn read_slot(&mut self, index: usize, output: &mut Slot) -> Read {
            Internal::new(&mut self.state, &mut self.flash).read_slot(index, output)
        }
    }

    struct Flash {
        bytes: Vec<u8>,
        mutations: usize,
        fail_commit: bool,
        /// Power is lost during the next body write after this many bytes.
        tear_write: Option<usize>,
    }
    impl Default for Flash {
        fn default() -> Self {
            Self {
                bytes: vec![0xff; crate::ride::log::REGION_SIZE],
                mutations: 0,
                fail_commit: false,
                tear_write: None,
            }
        }
    }
    impl RideStorage for Flash {
        type Error = ();
        fn ride_geometry(&mut self) -> device_api::storage::Geometry {
            crate::ride::log::GEOMETRY
        }
        fn ride_read_sector(
            &mut self,
            sector: usize,
            output: &mut crate::ride::log::Sector,
        ) -> Result<(), ()> {
            output.0.copy_from_slice(
                &self.bytes[sector * crate::ride::log::SECTOR_SIZE
                    ..(sector + 1) * crate::ride::log::SECTOR_SIZE],
            );
            Ok(())
        }
        fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), ()> {
            output.0.copy_from_slice(
                &self.bytes
                    [slot * crate::ride::log::SLOT_SIZE..(slot + 1) * crate::ride::log::SLOT_SIZE],
            );
            Ok(())
        }
        fn ride_erase_sector(&mut self, sector: usize) -> Result<(), ()> {
            self.mutations += 1;
            self.bytes[sector * crate::ride::log::SECTOR_SIZE
                ..(sector + 1) * crate::ride::log::SECTOR_SIZE]
                .fill(0xff);
            Ok(())
        }
        fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), ()> {
            self.mutations += 1;
            let torn = self.tear_write.take();
            for (stored, new) in self.bytes
                [slot * crate::ride::log::SLOT_SIZE..(slot + 1) * crate::ride::log::SLOT_SIZE]
                .iter_mut()
                .zip(data.0)
                .take(torn.unwrap_or(crate::ride::log::SLOT_SIZE))
            {
                *stored &= new;
            }
            if torn.is_some() { Err(()) } else { Ok(()) }
        }
        fn ride_commit_slot(
            &mut self,
            slot: usize,
            commit: &crate::ride::log::Commit,
        ) -> Result<(), ()> {
            self.mutations += 1;
            if self.fail_commit {
                return Err(());
            }
            let end = (slot + 1) * crate::ride::log::SLOT_SIZE;
            self.bytes[end - 4..end].copy_from_slice(&commit.0);
            Ok(())
        }
    }
    fn scan(recorder: &mut Recorder, media: &mut Memory) {
        media.state = InternalState::default();
        for _ in 0..crate::ride::log::SECTORS {
            recorder.service(media, 0, Sample::default(), &|| 0);
        }
    }
    #[test]
    fn a_foreign_journal_reads_foreign_and_no_initialize_or_clear_erases_it() {
        let mut media = Memory::default();
        media.bytes[0] = 0;
        let original = media.bytes.clone();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert_eq!(recorder.status(), Status::Foreign);
        assert_eq!(recorder.initialize(1), Err(Refused::Foreign));
        assert_eq!(
            recorder.clear(recorder.next_slot(), 1),
            Err(Refused::Foreign)
        );
        recorder.service(&mut media, 0, Sample::default(), &|| 0);
        assert_eq!(recorder.status(), Status::Foreign);
        assert_eq!(media.mutations, 0);
        assert_eq!(media.bytes, original);
    }
    #[test]
    fn ride_advances_without_a_screen_or_terminal_and_recovers_after_restart() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert_eq!(media.mutations, 0);
        assert!(recorder.request(Action::Start, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        recorder.service(&mut media, 1_100, Sample::default(), &|| 1_100);
        assert!(recorder.request(Action::Pause, 2_100, 2));
        recorder.service(&mut media, 2_100, Sample::default(), &|| 2_100);
        recorder.service(&mut media, 2_100, Sample::default(), &|| 2_100);
        assert!(recorder.take_result().unwrap().ok);
        assert_eq!(recorder.status(), Status::Paused);
        let mut reopened = Recorder::default();
        scan(&mut reopened, &mut media);
        assert_eq!(reopened.status(), Status::Recovered);
        reopened.service(&mut media, 3_000, Sample::default(), &|| 3_000);
        assert_eq!(reopened.completed(), 1);
        assert!(reopened.exportable());
    }
    #[test]
    fn finish_running_flushes_and_freezes_duration() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert!(recorder.request(Action::Start, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        recorder.service(
            &mut media,
            1100,
            Sample {
                power_watts: Some(215),
                speed_mm_s: Some(8000),
                ..Sample::default()
            },
            &|| 1100,
        );
        assert!(recorder.request(Action::Finish, 2100, 2));
        assert_eq!(recorder.active_ms(9000), 2000);
        for _ in 0..3 {
            recorder.service(&mut media, 9000, Sample::default(), &|| 9000);
        }
        assert!(recorder.take_result().unwrap().ok);
        assert_eq!(recorder.status(), Status::Saved);
        assert_eq!(recorder.active_ms(10000), 2000);
        assert_eq!(recorder.written_samples(), 1);
        let mut reopened = Recorder::default();
        scan(&mut reopened, &mut media);
        assert_eq!(reopened.completed(), 1);
        assert!(reopened.exportable());
    }
    #[test]
    fn a_lap_closed_since_the_last_sample_begins_on_a_sample_at_the_finish() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert!(recorder.request(Action::Start, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        recorder.mark_lap(LapStart::Manual);
        assert!(recorder.request(Action::Finish, 900, 2));
        for _ in 0..3 {
            recorder.service(&mut media, 9000, Sample::default(), &|| 9000);
        }
        assert!(recorder.take_result().unwrap().ok);
        assert_eq!(recorder.status(), Status::Saved);
        let samples: Vec<(u64, Option<LapStart>)> = media
            .bytes
            .as_chunks::<{ crate::ride::log::SLOT_SIZE }>()
            .0
            .iter()
            .filter_map(|bytes| crate::ride::log::decode(&Slot(*bytes)))
            .filter(|entry| entry.kind == Kind::Samples)
            .flat_map(|entry| entry.samples.into_iter().take(usize::from(entry.count)))
            .map(|sample| (sample.active_ms, sample.lap))
            .collect();
        assert_eq!(
            samples,
            [(0, Some(LapStart::Ride)), (800, Some(LapStart::Manual))]
        );
    }
    #[test]
    fn laps_closed_within_one_sample_begin_on_samples_of_their_own() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert!(recorder.request(Action::Start, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        recorder.mark_lap(LapStart::Manual);
        for now in [100, 1100] {
            recorder.service(&mut media, now, Sample::default(), &|| now);
        }
        recorder.mark_lap(LapStart::Manual);
        recorder.mark_lap(LapStart::Position);
        for now in [2100, 3100] {
            recorder.service(&mut media, now, Sample::default(), &|| now);
        }
        recorder.mark_lap(LapStart::Manual);
        recorder.mark_lap(LapStart::Step);
        assert!(recorder.request(Action::Finish, 3900, 2));
        for _ in 0..3 {
            recorder.service(&mut media, 9000, Sample::default(), &|| 9000);
        }
        assert!(recorder.take_result().unwrap().ok);
        assert_eq!(recorder.status(), Status::Saved);
        let samples: Vec<(u64, Option<LapStart>)> = media
            .bytes
            .as_chunks::<{ crate::ride::log::SLOT_SIZE }>()
            .0
            .iter()
            .filter_map(|bytes| crate::ride::log::decode(&Slot(*bytes)))
            .filter(|entry| entry.kind == Kind::Samples)
            .flat_map(|entry| entry.samples.into_iter().take(usize::from(entry.count)))
            .map(|sample| (sample.active_ms, sample.lap))
            .collect();
        assert_eq!(
            samples,
            [
                (0, Some(LapStart::Ride)),
                (1000, Some(LapStart::Manual)),
                (2000, Some(LapStart::Manual)),
                (3000, Some(LapStart::Position)),
                (3800, Some(LapStart::Manual)),
                (3800, Some(LapStart::Step)),
            ]
        );
    }
    #[test]
    fn a_lap_before_the_first_sample_is_kept_by_a_pause() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert!(recorder.request(Action::Start, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        recorder.mark_lap(LapStart::Manual);
        assert!(recorder.request(Action::Pause, 600, 2));
        for _ in 0..3 {
            recorder.service(&mut media, 600, Sample::default(), &|| 600);
        }
        assert!(recorder.take_result().unwrap().ok);
        assert_eq!(recorder.status(), Status::Paused);
        let samples: Vec<(u64, Option<LapStart>)> = media
            .bytes
            .as_chunks::<{ crate::ride::log::SLOT_SIZE }>()
            .0
            .iter()
            .filter_map(|bytes| crate::ride::log::decode(&Slot(*bytes)))
            .filter(|entry| entry.kind == Kind::Samples)
            .flat_map(|entry| entry.samples.into_iter().take(usize::from(entry.count)))
            .map(|sample| (sample.active_ms, sample.lap))
            .collect();
        assert_eq!(
            samples,
            [(500, Some(LapStart::Ride)), (500, Some(LapStart::Manual))]
        );
    }
    #[test]
    fn odd_and_out_of_range_altitudes_pass_the_internal_read_back_check() {
        use crate::ride::log::Altitude;
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert!(recorder.request(Action::Start, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        // Four seconds of altitudes that are not on the 0.2 m grid, or past its range.
        for (second, dm) in [(1u64, 1_235), (2, -9_001), (3, i32::MAX), (4, 7)] {
            let now = 100 + second * 1000;
            let sample = Sample {
                speed_mm_s: Some(8000),
                altitude: Some(Altitude::from_dm(dm)),
                ..Sample::default()
            };
            recorder.service(&mut media, now, sample, &|| now);
        }
        assert!(recorder.request(Action::Finish, 4100, 2));
        for _ in 0..3 {
            recorder.service(&mut media, 9000, Sample::default(), &|| 9000);
        }
        assert!(recorder.take_result().unwrap().ok);
        assert_eq!(recorder.status(), Status::Saved);
        // A failed read-back would have stopped the ride in an error instead.
        assert!(
            recorder.written_samples() >= 3,
            "{}",
            recorder.written_samples()
        );
        let mut reopened = Recorder::default();
        scan(&mut reopened, &mut media);
        assert_eq!(reopened.completed(), 1);
    }
    #[test]
    fn ambiguous_append_stops_without_retrying_or_erasing() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        media.fail_commit = true;
        assert!(recorder.request(Action::Start, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert_eq!(recorder.status(), Status::Error);
        assert!(!recorder.take_result().unwrap().ok);
        let mutations = media.mutations;
        recorder.service(&mut media, 200, Sample::default(), &|| 200);
        assert_eq!(media.mutations, mutations);
    }

    fn start(recorder: &mut Recorder, media: &mut Memory, now: u64) {
        assert!(recorder.request(Action::Start, now, 1));
        recorder.service(media, now, Sample::default(), &|| now);
        assert!(recorder.take_result().unwrap().ok);
    }

    #[test]
    fn torn_first_write_on_an_empty_journal_keeps_recording_available() {
        // Power is lost while the very first START of a fresh or cleared
        // journal is programmed: commit missing, or only a body prefix.
        for torn in [crate::ride::log::SLOT_SIZE, 10, 5] {
            let mut media = Memory::default();
            let mut recorder = Recorder::default();
            scan(&mut recorder, &mut media);
            media.tear_write = Some(torn);
            assert!(recorder.request(Action::Start, 100, 1));
            recorder.service(&mut media, 100, Sample::default(), &|| 100);
            assert_eq!(recorder.status(), Status::Error);

            let mut rebooted = Recorder::default();
            scan(&mut rebooted, &mut media);
            assert_eq!(rebooted.status(), Status::Ready, "torn after {torn} bytes");
            assert_eq!(rebooted.next_slot(), 1);
            start(&mut rebooted, &mut media, 200);
            assert!(rebooted.request(Action::Finish, 1_200, 2));
            for now in [1_200, 1_200] {
                rebooted.service(&mut media, now, Sample::default(), &|| now);
            }
            assert!(rebooted.take_result().unwrap().ok);
            let mut again = Recorder::default();
            scan(&mut again, &mut media);
            assert_eq!((again.status(), again.completed()), (Status::Ready, 1));
        }
    }

    #[test]
    fn torn_appends_with_no_record_between_them_keep_recording_available() {
        let tears = [crate::ride::log::SLOT_SIZE, 10, 5];
        for count in [2, 3] {
            let mut media = Memory::default();
            for (index, torn) in tears.into_iter().take(count).enumerate() {
                let mut recorder = Recorder::default();
                scan(&mut recorder, &mut media);
                assert_eq!(
                    (recorder.status(), recorder.next_slot()),
                    (Status::Ready, index),
                    "after {index} torn appends"
                );
                media.tear_write = Some(torn);
                assert!(recorder.request(Action::Start, 100, 1));
                recorder.service(&mut media, 100, Sample::default(), &|| 100);
                assert_eq!(recorder.status(), Status::Error);
            }

            let mut rebooted = Recorder::default();
            scan(&mut rebooted, &mut media);
            assert_eq!(
                (rebooted.status(), rebooted.next_slot()),
                (Status::Ready, count),
                "after {count} torn appends"
            );
            start(&mut rebooted, &mut media, 200);
            assert!(rebooted.request(Action::Finish, 1_200, 2));
            for now in [1_200, 1_200] {
                rebooted.service(&mut media, now, Sample::default(), &|| now);
            }
            assert!(rebooted.take_result().unwrap().ok);
            let mut again = Recorder::default();
            scan(&mut again, &mut media);
            assert_eq!(
                (again.status(), again.completed()),
                (Status::Ready, 1),
                "after {count} torn appends"
            );
        }
    }

    #[test]
    fn foreign_data_in_the_first_slot_reads_foreign() {
        for first in [[0u8; 5], *b"RIDE\x01", [0xff, 0xff, 0xff, 0xff, 0xff]] {
            let mut media = Memory::default();
            media.bytes[..5].copy_from_slice(&first);
            media.bytes[100] = 0;
            let mut recorder = Recorder::default();
            scan(&mut recorder, &mut media);
            assert_eq!(recorder.status(), Status::Foreign, "{first:?}");
        }
    }

    #[test]
    fn a_torn_append_beside_a_foreign_slot_reads_foreign() {
        let slot = crate::ride::log::SLOT_SIZE;
        for (torn, foreign) in [(0, slot), (slot, 0)] {
            let mut media = Memory::default();
            media.bytes[torn..torn + 5].copy_from_slice(b"RIDE\x02");
            media.bytes[foreign..foreign + 5].fill(0);
            let mut recorder = Recorder::default();
            scan(&mut recorder, &mut media);
            assert_eq!(recorder.status(), Status::Foreign, "torn slot at {torn}");
        }
    }

    #[test]
    fn committed_records_of_a_newer_version_read_foreign_and_are_never_cleared() {
        use crate::ride::log::{Entry, Kind, SLOT_SIZE, commit_word, encode};
        let mut media = Memory::default();
        for (index, kind) in [Kind::Start, Kind::Finish].into_iter().enumerate() {
            let mut slot = encode(&Entry::event(kind, 1, index as u32, 0));
            slot.0[4] = 3;
            slot.0[SLOT_SIZE - 4..].copy_from_slice(&commit_word().0);
            media.bytes[index * SLOT_SIZE..][..SLOT_SIZE].copy_from_slice(&slot.0);
        }
        let before = media.bytes.clone();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert_eq!(recorder.status(), Status::Foreign);
        assert_eq!(recorder.clear(2, 0), Err(Refused::Foreign));
        assert_eq!(media.bytes, before);
    }

    #[test]
    fn late_service_keeps_one_sample_per_active_second() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        start(&mut recorder, &mut media, 100);
        // The main loop serves the recorder every 30 ms, never exactly on time.
        let mut now = 100;
        while now < 100 + 600_000 {
            now += 30;
            recorder.service(&mut media, now, Sample::default(), &|| now);
        }
        assert!(
            recorder.written_samples() >= 596,
            "{}",
            recorder.written_samples()
        );
        assert_eq!(recorder.dropped_samples(), 0);
    }

    #[test]
    fn resume_samples_from_the_resume_press_without_counting_the_pause_as_dropped() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        start(&mut recorder, &mut media, 0);
        for now in (0..=10_000).step_by(10) {
            recorder.service(&mut media, now, Sample::default(), &|| now);
        }
        assert!(recorder.request(Action::Pause, 10_005, 2));
        for _ in 0..3 {
            recorder.service(&mut media, 10_005, Sample::default(), &|| 10_005);
        }
        assert!(recorder.take_result().unwrap().ok);
        assert!(recorder.request(Action::Resume, 70_000, 3));
        recorder.service(&mut media, 70_000, Sample::default(), &|| 70_000);
        assert!(recorder.take_result().unwrap().ok);
        for now in (70_000..=80_000).step_by(10) {
            recorder.service(&mut media, now, Sample::default(), &|| now);
        }
        assert!(recorder.request(Action::Finish, 80_005, 4));
        for _ in 0..3 {
            recorder.service(&mut media, 80_005, Sample::default(), &|| 80_005);
        }
        assert!(recorder.take_result().unwrap().ok);
        // Eleven samples on each side of the pause, none counted as dropped.
        assert_eq!(recorder.dropped_samples(), 0);
        assert_eq!(recorder.written_samples(), 22);
    }

    #[test]
    fn remaining_time_counts_down_and_start_refuses_a_journal_without_room() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        // 4094 sample slots of four 1 Hz samples: 4 h 32 min 56 s when empty.
        assert_eq!(recorder.remaining_secs(), 16_376);
        start(&mut recorder, &mut media, 0);
        assert_eq!(recorder.remaining_secs(), 16_376);
        recorder.service(&mut media, 0, Sample::default(), &|| 0);
        assert_eq!(recorder.remaining_secs(), 16_375);

        // A journal with only the terminal slot and one sample slot left has no
        // room for START plus a sample, so the rider is told FULL at once.
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        start(&mut recorder, &mut media, 0);
        assert!(recorder.request(Action::Finish, 0, 2));
        recorder.service(&mut media, 0, Sample::default(), &|| 0);
        assert!(recorder.take_result().unwrap().ok);
        media.bytes[2 * crate::ride::log::SLOT_SIZE
            ..(crate::ride::log::SLOTS - 2) * crate::ride::log::SLOT_SIZE]
            .fill(0);
        let mut nearly = Recorder::default();
        scan(&mut nearly, &mut media);
        assert_eq!(nearly.status(), Status::Ready);
        assert_eq!(nearly.remaining_secs(), 0);
        assert!(nearly.request(Action::Start, 0, 3));
        nearly.service(&mut media, 0, Sample::default(), &|| 0);
        assert!(!nearly.take_result().unwrap().ok);
        assert_eq!(nearly.status(), Status::Full);
        assert_eq!(nearly.next_slot(), crate::ride::log::SLOTS - 2);
    }

    #[test]
    fn a_battery_reading_a_slot_cannot_hold_is_dropped_and_recording_goes_on() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        start(&mut recorder, &mut media, 0);
        let sample = Sample {
            battery_percent: Some(101),
            power_watts: Some(180),
            ..Sample::default()
        };
        for now in [0, 1_000, 2_000, 3_000, 3_000] {
            recorder.service(&mut media, now, sample, &|| now);
        }
        assert_eq!(recorder.status(), Status::Recording);
        assert_eq!(recorder.written_samples(), 4);
        let mut slot = Slot::default();
        assert_ne!(media.read_slot(1, &mut slot), Read::Failed);
        let batch = crate::ride::log::decode(&slot).unwrap();
        assert_eq!(batch.samples[0].battery_percent, None);
        assert_eq!(batch.samples[0].power_watts, Some(180));
    }

    #[test]
    fn an_unverified_full_record_is_a_media_failure_not_a_closed_ride() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        start(&mut recorder, &mut media, 0);
        assert!(recorder.request(Action::Finish, 0, 2));
        recorder.service(&mut media, 0, Sample::default(), &|| 0);
        assert!(recorder.take_result().unwrap().ok);
        // Room for START, one batch and the terminal record.
        let last = crate::ride::log::SLOTS - 3;
        media.bytes[2 * crate::ride::log::SLOT_SIZE..last * crate::ride::log::SLOT_SIZE].fill(0);
        let mut nearly = Recorder::default();
        scan(&mut nearly, &mut media);
        start(&mut nearly, &mut media, 0);
        for now in (0..4_000).step_by(1_000) {
            nearly.service(&mut media, now, Sample::default(), &|| now);
        }
        assert_eq!(nearly.next_slot(), crate::ride::log::SLOTS - 1);
        media.fail_commit = true;
        for now in (4_000..9_000).step_by(1_000) {
            nearly.service(&mut media, now, Sample::default(), &|| now);
        }
        assert_eq!(nearly.status(), Status::Error);
        assert!(!nearly.exportable());
    }
}
