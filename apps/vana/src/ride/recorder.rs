//! Incremental scanner and append scheduler for one ride journal.
//!
//! The same recorder runs the internal journal and the bulk MMC log; a
//! [`Journal`] supplies the medium. Bulk media that are missing, unformatted or
//! untrustworthy stay in their own state and are never written implicitly.
//!
//! Boot scans without erasing. Unknown occupied data requires explicit initialize,
//! except one torn first append on an otherwise empty journal, which stays an
//! occupied invalid slot. Start never reclaims space and needs room for one
//! sample batch besides the terminal slot. A media failure stops mutation until inspection or
//! reboot/rescan resolves an uncertain commit. Clear verifies each erased sector
//! but is not atomic across power loss. Never retry a timed-out mutation blindly.
//!
//! Pause/finish freeze active duration when accepted, before flushing. Sampling
//! stays on the 1 Hz grid of the start/resume press; late service takes the
//! due sample late, never backfills, and counts wholly skipped periods as
//! dropped. Reset can lose three buffered samples, or four during commit.
//! A lap mark waits for the next stored sample: the ride's first sample
//! carries `LapStart::Ride`, and each later lap its trigger. A pause or finish
//! with a lap waiting stores one more sample, at the stop, to carry it.
//! One operation and its unconsumed completion exclude further mutations.

use crate::ride::control::Action;

use crate::ride::journal::{Journal, Progress, Scan};
use crate::ride::log::Catalog;
use crate::ride::log::Entry;
use crate::ride::log::Kind;
use crate::ride::log::LapStart;
use crate::ride::log::Recovery;
use crate::ride::log::Sample;
use crate::ride::log::Source;
use crate::ride::log::Status;
use crate::ride::log::Summary;

#[derive(Clone, Copy)]
enum Pending {
    Ride(Action, Source, u64, u32),
    Initialize(u32),
    Clear(u32),
}

#[derive(Clone, Copy)]
pub struct ResultEvent {
    pub action: Option<Action>,
    pub token: u32,
    pub ok: bool,
}

pub struct Recorder {
    catalog: Catalog,
    status: Status,
    /// Slots of the medium, including the reserved terminal slot.
    slots: usize,
    /// Initialize or clear step.
    step: usize,
    /// Active time of the last record this ride committed.
    committed_ms: u64,
    /// The journal asked for recovery state to be saved.
    checkpoint_due: bool,
    pending: Option<Pending>,
    result: Option<ResultEvent>,
    ride_id: u32,
    sequence: u32,
    accumulated_ms: u64,
    running_since: u64,
    stop_at: Option<u64>,
    source: Source,
    open_ride: bool,
    summary: Summary,
    samples: [Sample; crate::ride::log::SAMPLES_PER_BATCH],
    sample_count: usize,
    next_lap_mark: Option<LapStart>,
    next_sample: u64,
    written_samples: u32,
    dropped_samples: u32,
    max_write_ms: u32,
    max_erase_ms: u32,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            catalog: Catalog::default(),
            status: Status::Scanning,
            slots: crate::ride::log::SLOTS,
            step: 0,
            committed_ms: 0,
            checkpoint_due: false,
            pending: None,
            result: None,
            ride_id: 0,
            sequence: 0,
            accumulated_ms: 0,
            running_since: 0,
            stop_at: None,
            source: Source::Demo,
            open_ride: false,
            summary: Summary::default(),
            samples: [Sample::default(); crate::ride::log::SAMPLES_PER_BATCH],
            sample_count: 0,
            next_lap_mark: None,
            next_sample: 0,
            written_samples: 0,
            dropped_samples: 0,
            max_write_ms: 0,
            max_erase_ms: 0,
        }
    }
}

impl Recorder {
    pub fn status(&self) -> Status {
        self.status
    }

    pub fn completed(&self) -> u16 {
        self.catalog.completed
    }

    pub fn written_samples(&self) -> u32 {
        self.written_samples
    }

    pub fn dropped_samples(&self) -> u32 {
        self.dropped_samples
    }

    pub fn next_slot(&self) -> usize {
        self.catalog.next_slot
    }

    pub fn max_write_ms(&self) -> u32 {
        self.max_write_ms
    }

    /// Optimistic 1 Hz recording time left before the journal is full, after
    /// reserving START (when no ride is open) and the terminal slot. Each
    /// pause/resume costs an event slot and may pad a partial batch.
    pub fn remaining_secs(&self) -> u32 {
        if !self.status.usable() {
            return 0;
        }
        let used = self
            .catalog
            .next_slot
            .saturating_sub(usize::from(self.open_ride));
        crate::ride::reclaim::estimated_seconds_in(self.slots, used)
            .saturating_sub(self.sample_count as u32)
    }

    /// Slots of the medium, including the reserved terminal slot.
    pub fn slots(&self) -> usize {
        self.slots
    }

    /// A ride is open and running or paused on this journal.
    pub fn riding(&self) -> bool {
        self.open_ride && matches!(self.status, Status::Recording | Status::Paused)
    }

    /// A pause or finish was accepted and not yet completed.
    pub fn stop_requested(&self) -> bool {
        self.stop_at.is_some()
    }

    /// The ride action accepted and not yet completed.
    pub fn pending_action(&self) -> Option<Action> {
        match self.pending {
            Some(Pending::Ride(action, ..)) => Some(action),
            _ => None,
        }
    }

    /// Recovered with an interrupted ride whose closing record is still to be
    /// appended; a new ride may start once it is.
    pub fn recovering(&self) -> bool {
        self.status == Status::Recovered && self.catalog.recovery.is_some()
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some() || self.result.is_some()
    }

    pub fn max_erase_ms(&self) -> u32 {
        self.max_erase_ms
    }

    pub fn source(&self) -> Option<Source> {
        self.open_ride.then_some(self.source)
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

    #[cfg(test)]
    pub(crate) fn catalog(&self) -> &Catalog {
        &self.catalog
    }

    pub fn exportable(&self) -> bool {
        !self.open_ride
            && self.pending.is_none()
            && self.result.is_none()
            && matches!(
                self.status,
                Status::NeedsInit
                    | Status::Ready
                    | Status::Saved
                    | Status::Recovered
                    | Status::Full
                    | Status::Corrupt
            )
    }

    pub fn request(&mut self, action: Action, source: Source, now: u64, token: u32) -> bool {
        if self.pending.is_some() || self.result.is_some() {
            return false;
        }
        let allowed = match action {
            Action::Start => {
                self.catalog.recovery.is_none()
                    && matches!(
                        self.status,
                        Status::Ready | Status::Saved | Status::Recovered
                    )
            }
            Action::Pause => self.status == Status::Recording,
            Action::Resume => self.status == Status::Paused,
            Action::Finish => {
                self.open_ride && matches!(self.status, Status::Recording | Status::Paused)
            }
        };
        if allowed {
            let source = if action == Action::Start {
                source
            } else {
                self.source
            };
            if matches!(action, Action::Pause | Action::Finish) {
                self.stop_at = Some(now);
            }
            self.pending = Some(Pending::Ride(action, source, now, token));
        }
        allowed
    }

    /// Explicitly prepare a journal that holds no usable rides. Corrupt bulk
    /// media is only superseded by a new epoch; its sectors stay readable.
    pub fn initialize(&mut self, token: u32) -> bool {
        if !matches!(self.status, Status::NeedsInit | Status::Corrupt)
            || self.pending.is_some()
            || self.result.is_some()
        {
            return false;
        }
        self.pending = Some(Pending::Initialize(token));
        self.step = 0;
        self.status = Status::Formatting;
        true
    }

    pub fn clear(&mut self, expected_upper: usize, token: u32) -> bool {
        if !crate::ride::reclaim::request_allowed(
            self.status,
            self.open_ride,
            self.pending.is_some() || self.result.is_some(),
            expected_upper,
            self.catalog.next_slot,
        ) {
            return false;
        }
        self.pending = Some(Pending::Clear(token));
        self.step = 0;
        self.status = Status::Clearing;
        true
    }

    /// A lap closed: the next sample this ride stores begins a new one.
    /// Nothing is marked without an open ride.
    pub fn mark_lap(&mut self, start: LapStart) {
        if self.riding() {
            self.next_lap_mark = Some(start);
        }
    }

    pub fn take_result(&mut self) -> Option<ResultEvent> {
        self.result.take()
    }

    /// A pause or finish was accepted and active time is frozen, though the
    /// status stays Recording until its flush completes.
    pub fn stopping(&self) -> bool {
        self.status == Status::Recording && self.stop_at.is_some()
    }

    pub fn active_ms(&self, now: u64) -> u64 {
        if self.status == Status::Recording {
            crate::ride::log::active_at(self.accumulated_ms, self.running_since, self.stop_at, now)
        } else {
            self.accumulated_ms
        }
    }

    /// Perform at most one scan step, initialize/clear step, checkpoint, or
    /// committed slot append.
    pub fn service(
        &mut self,
        journal: &mut impl Journal,
        now: u64,
        sample: Sample,
        clock: &impl Fn() -> u64,
    ) -> bool {
        let before = self.status;
        let worked = self.service_inner(journal, now, sample, clock);
        if self.status != before {
            if self.status == Status::Error {
                log::error!(target: "sdk.recorder", "media_failed previous={} next_slot={}", before.name(), self.catalog.next_slot);
            } else {
                log::info!(target: "sdk.recorder", "state_changed previous={} current={} next_slot={}", before.name(), self.status.name(), self.catalog.next_slot);
            }
        }
        worked
    }

    fn service_inner(
        &mut self,
        journal: &mut impl Journal,
        now: u64,
        sample: Sample,
        clock: &impl Fn() -> u64,
    ) -> bool {
        if self.status == Status::Scanning {
            self.scan(journal);
            return true;
        }
        if matches!(self.status, Status::Formatting | Status::Clearing) {
            self.prepare(journal, clock);
            return true;
        }
        if self.status == Status::Error {
            // The medium's state is uncertain: write nothing until recovery,
            // and fail any action still queued so its token completes.
            if let Some(Pending::Ride(action, .., token)) = self.pending.take() {
                self.stop_at = None;
                self.result = Some(ResultEvent {
                    action: Some(action),
                    token,
                    ok: false,
                });
                return true;
            }
            return false;
        }
        if self.checkpoint_due {
            self.checkpoint_due = false;
            let catalog = self.checkpoint_catalog();
            if !journal.checkpoint(&catalog) {
                self.freeze(now);
                self.status = Status::Error;
                return self.service_inner(journal, now, sample, clock);
            }
            return true;
        }

        if self.status == Status::Recording && self.stop_at.is_none() && now >= self.next_sample {
            if self.sample_count < self.samples.len() {
                let mut sample = sample.for_source(self.source);
                sample.active_ms = self.active_ms(now);
                sample.lap = self.next_lap_mark.take();
                self.samples[self.sample_count] = sample;
                self.sample_count += 1;
            } else {
                self.dropped_samples = self.dropped_samples.saturating_add(1);
            }
            // Stay on the 1 Hz grid from the start/resume press so service
            // latency does not stretch every period; count skipped periods.
            let missed = (now - self.next_sample) / 1_000;
            self.dropped_samples = self
                .dropped_samples
                .saturating_add(missed.min(u64::from(u32::MAX)) as u32);
            self.next_sample = self
                .next_sample
                .saturating_add(missed.saturating_add(1).saturating_mul(1_000));
        }

        // A lap that closed since the last sample has no later sample to
        // begin on once recording stops, so the stop stores one more.
        if self.stopping()
            && self.sample_count < self.samples.len()
            && self
                .next_lap_mark
                .is_some_and(|start| start != LapStart::Ride)
        {
            let mut sample = sample.for_source(self.source);
            sample.active_ms = self.active_ms(now);
            sample.lap = self.next_lap_mark.take();
            self.samples[self.sample_count] = sample;
            self.sample_count += 1;
        }

        if self.status == Status::Recovered
            && let Some(recovery) = self.catalog.recovery
        {
            let entry = Entry::event(
                Kind::Recovered,
                recovery.source,
                recovery.ride_id,
                recovery.next_sequence,
                recovery.active_ms,
            );
            if self.append(journal, &entry, true, now, clock) {
                self.catalog.completed = self.catalog.completed.saturating_add(1);
                self.catalog.recovery = None;
                self.accumulated_ms = recovery.active_ms;
                self.open_ride = false;
                let mut summary = recovery.summary;
                summary.finish(Kind::Recovered, recovery.active_ms, recovery.gap);
                self.catalog.push_summary(summary);
            }
            return true;
        }

        if let Some(Pending::Initialize(_)) = self.pending {
            return false;
        }
        if let Some(Pending::Clear(_)) = self.pending {
            return false;
        }
        if let Some(Pending::Ride(action, source, action_ms, token)) = self.pending {
            if self.sample_count != 0 && matches!(action, Action::Pause | Action::Finish) {
                if !self.flush(journal, now, clock) {
                    self.pending = None;
                    self.result = Some(ResultEvent {
                        action: Some(action),
                        token,
                        ok: false,
                    });
                }
                return true;
            }
            let active_ms = match action {
                Action::Start => 0,
                _ => self.active_ms(action_ms),
            };
            let kind = match action {
                Action::Start => Kind::Start,
                Action::Pause => Kind::Pause,
                Action::Resume => Kind::Resume,
                Action::Finish => Kind::Finish,
            };
            if action == Action::Start {
                if crate::ride::reclaim::estimated_seconds_in(self.slots, self.catalog.next_slot)
                    == 0
                {
                    // No room for START, one sample batch and the terminal record.
                    self.status = Status::Full;
                    self.pending = None;
                    self.result = Some(ResultEvent {
                        action: Some(action),
                        token,
                        ok: false,
                    });
                    return true;
                }
                self.ride_id = self.catalog.next_ride_id;
                self.sequence = 0;
            }
            let entry = Entry::event(kind, source, self.ride_id, self.sequence, active_ms);
            let ok = self.append(
                journal,
                &entry,
                matches!(kind, Kind::Finish | Kind::Recovered | Kind::Full),
                now,
                clock,
            );
            if ok {
                self.sequence = self.sequence.wrapping_add(1);
                match action {
                    Action::Start => {
                        self.accumulated_ms = 0;
                        self.running_since = action_ms;
                        self.next_sample = action_ms;
                        self.stop_at = None;
                        self.source = source;
                        self.open_ride = true;
                        self.next_lap_mark = Some(LapStart::Ride);
                        self.summary = Summary::started(self.ride_id, source);
                        self.status = Status::Recording;
                        // As a scan would see it once START is committed.
                        self.catalog.next_ride_id = self.ride_id.wrapping_add(1).max(1);
                    }
                    Action::Pause => {
                        self.accumulated_ms = active_ms;
                        self.stop_at = None;
                        self.status = Status::Paused;
                    }
                    Action::Resume => {
                        self.running_since = action_ms;
                        self.next_sample = action_ms;
                        self.stop_at = None;
                        self.status = Status::Recording;
                    }
                    Action::Finish => {
                        self.accumulated_ms = active_ms;
                        self.stop_at = None;
                        self.open_ride = false;
                        self.next_lap_mark = None;
                        self.status = Status::Saved;
                        self.summary.finish(Kind::Finish, active_ms, false);
                        self.catalog.push_summary(self.summary);
                        self.catalog.completed = self.catalog.completed.saturating_add(1);
                        self.catalog.next_ride_id = self.ride_id.wrapping_add(1).max(1);
                    }
                }
            }
            self.pending = None;
            self.result = Some(ResultEvent {
                action: Some(action),
                token,
                ok,
            });
            return true;
        }

        if self.status == Status::Recording && self.sample_count == self.samples.len() {
            self.flush(journal, now, clock);
            return true;
        }
        false
    }

    fn scan(&mut self, journal: &mut impl Journal) {
        let scan = journal.scan();
        if scan == Scan::More {
            return;
        }
        // Whatever the outcome, report this journal's capacity: none for a
        // medium without an epoch.
        self.slots = journal.slots();
        let catalog = match scan {
            Scan::More => return,
            Scan::Done(catalog) => catalog,
            Scan::Uninitialized => {
                self.status = Status::NeedsInit;
                return;
            }
            Scan::Unsupported => {
                self.status = Status::Unsupported;
                return;
            }
            Scan::Unavailable => {
                self.status = Status::Unavailable;
                return;
            }
            Scan::Corrupt => {
                self.status = Status::Corrupt;
                return;
            }
            Scan::Uncertain(catalog) => {
                // Exportable up to the extent seen; nothing is written and
                // only an explicit initialize starts over.
                self.catalog = catalog;
                self.status = Status::Corrupt;
                return;
            }
            Scan::Failed => {
                self.status = Status::Error;
                return;
            }
        };
        self.catalog = catalog;
        // Write an overdue checkpoint before any append, so repeated cuts
        // cannot grow what the next boot replays.
        self.checkpoint_due = journal.checkpoint_overdue();
        self.status = if catalog.unknown_data() {
            Status::NeedsInit
        } else if catalog.next_slot >= self.slots {
            Status::Full
        } else if let Some(recovery) = catalog.recovery {
            self.accumulated_ms = recovery.active_ms;
            self.committed_ms = recovery.active_ms;
            self.ride_id = recovery.ride_id;
            self.sequence = recovery.next_sequence;
            self.source = recovery.source;
            self.open_ride = true;
            self.summary = recovery.summary;
            Status::Recovered
        } else {
            Status::Ready
        };
    }

    /// One initialize or clear step; completion leaves an empty journal.
    fn prepare(&mut self, journal: &mut impl Journal, clock: &impl Fn() -> u64) {
        let clearing = self.status == Status::Clearing;
        let started = clock();
        // A new bulk epoch needs an identity that differs from the last one.
        let seed = started ^ (u64::from(self.catalog.next_ride_id) << 32) ^ self.step as u64;
        let progress = if clearing {
            journal.clear(self.step, seed)
        } else {
            journal.initialize(self.step, seed)
        };
        self.max_erase_ms = self
            .max_erase_ms
            .max(clock().saturating_sub(started).min(u64::from(u32::MAX)) as u32);
        let ok = match progress {
            Progress::More => {
                self.step += 1;
                return;
            }
            Progress::Done => true,
            Progress::Failed => false,
        };
        let token = match self.pending.take() {
            Some(Pending::Initialize(token) | Pending::Clear(token)) => token,
            _ => 0,
        };
        if ok {
            self.slots = journal.slots();
            self.catalog = Catalog {
                next_ride_id: 1,
                ..Catalog::default()
            };
            self.ride_id = 0;
            self.sequence = 0;
            self.accumulated_ms = 0;
            self.committed_ms = 0;
            self.open_ride = false;
            self.sample_count = 0;
            self.status = Status::Ready;
        } else {
            self.status = Status::Error;
        }
        self.result = Some(ResultEvent {
            action: None,
            token,
            ok,
        });
    }

    /// The state a scan of everything committed so far would rebuild.
    fn checkpoint_catalog(&self) -> Catalog {
        let mut catalog = self.catalog;
        if self.open_ride && catalog.recovery.is_none() {
            catalog.recovery = Some(Recovery {
                ride_id: self.ride_id,
                source: self.source,
                next_sequence: self.sequence,
                active_ms: self.committed_ms,
                gap: false,
                summary: self.summary,
            });
        }
        catalog
    }

    fn flush(&mut self, journal: &mut impl Journal, now: u64, clock: &impl Fn() -> u64) -> bool {
        if self.catalog.next_slot >= self.slots.saturating_sub(1) {
            self.freeze(now);
            self.dropped_samples = self
                .dropped_samples
                .saturating_add(self.sample_count as u32);
            self.sample_count = 0;
            let entry = Entry::event(
                Kind::Full,
                self.source,
                self.ride_id,
                self.sequence,
                self.active_ms(now),
            );
            if self.append(journal, &entry, true, now, clock) {
                self.summary.finish(Kind::Full, entry.active_ms, false);
                self.catalog.push_summary(self.summary);
                self.catalog.completed = self.catalog.completed.saturating_add(1);
                self.open_ride = false;
            }
            // An uncertain FULL record is a media failure, not a closed ride.
            if self.status != Status::Error {
                self.status = Status::Full;
            }
            return false;
        }
        let entry = Entry::batch(
            self.source,
            self.ride_id,
            self.sequence,
            &self.samples[..self.sample_count],
        )
        .unwrap();
        if self.append(journal, &entry, false, now, clock) {
            self.sequence = self.sequence.wrapping_add(1);
            for sample in &self.samples[..self.sample_count] {
                self.summary.add_sample(*sample);
            }
            self.written_samples = self
                .written_samples
                .saturating_add(self.sample_count as u32);
            self.sample_count = 0;
            true
        } else {
            false
        }
    }

    fn append(
        &mut self,
        journal: &mut impl Journal,
        entry: &Entry,
        final_slot: bool,
        now: u64,
        clock: &impl Fn() -> u64,
    ) -> bool {
        if self.catalog.next_slot >= self.slots
            || (!final_slot && self.catalog.next_slot >= self.slots.saturating_sub(1))
        {
            self.freeze(now);
            self.status = Status::Full;
            return false;
        }
        let started = clock();
        let appended = journal.append(self.catalog.next_slot, entry, final_slot);
        let elapsed = clock().saturating_sub(started).min(u64::from(u32::MAX)) as u32;
        self.max_write_ms = self.max_write_ms.max(elapsed);
        match appended {
            Some(checkpoint) => {
                self.catalog.next_slot += 1;
                self.catalog.valid_slots = self.catalog.valid_slots.saturating_add(1);
                self.committed_ms = entry.active_ms;
                self.checkpoint_due |= checkpoint;
                true
            }
            None => {
                // The commit may have reached the medium even when verification
                // could not complete. Stop and let the boot scan reconcile it.
                self.freeze(now);
                self.status = Status::Error;
                false
            }
        }
    }

    fn freeze(&mut self, now: u64) {
        if self.status == Status::Recording {
            (self.accumulated_ms, self.running_since, self.stop_at) = crate::ride::log::freeze_at(
                self.accumulated_ms,
                self.running_since,
                self.stop_at,
                now,
            );
        }
    }
}

#[cfg(test)]
mod properties;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ride::journal::{Internal, InternalState};
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
        fn initialize(&mut self, step: usize, seed: u64) -> Progress {
            Internal::new(&mut self.state, &mut self.flash).initialize(step, seed)
        }
        fn clear(&mut self, step: usize, seed: u64) -> Progress {
            Internal::new(&mut self.state, &mut self.flash).clear(step, seed)
        }
        fn append(&mut self, index: usize, entry: &Entry, terminal: bool) -> Option<bool> {
            Internal::new(&mut self.state, &mut self.flash).append(index, entry, terminal)
        }
        fn checkpoint(&mut self, catalog: &Catalog) -> bool {
            Internal::new(&mut self.state, &mut self.flash).checkpoint(catalog)
        }
        fn read_slot(&mut self, index: usize, output: &mut Slot) -> Option<bool> {
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
    fn scan_preserves_unknown_data_and_requires_explicit_initialization() {
        let mut media = Memory::default();
        media.bytes[0] = 0;
        let original = media.bytes.clone();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert_eq!(recorder.status(), Status::NeedsInit);
        assert_eq!(media.mutations, 0);
        assert_eq!(media.bytes, original);
    }
    #[test]
    fn ride_advances_without_a_screen_or_terminal_and_recovers_after_restart() {
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert_eq!(media.mutations, 0);
        assert!(recorder.request(Action::Start, Source::Live, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        recorder.service(&mut media, 1_100, Sample::default(), &|| 1_100);
        assert!(recorder.request(Action::Pause, Source::Live, 2_100, 2));
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
        assert!(recorder.request(Action::Start, Source::Live, 100, 1));
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
        assert!(recorder.request(Action::Finish, Source::Live, 2100, 2));
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
        assert!(recorder.request(Action::Start, Source::Live, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert!(recorder.take_result().unwrap().ok);
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        recorder.mark_lap(LapStart::Manual);
        assert!(recorder.request(Action::Finish, Source::Live, 900, 2));
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
    fn odd_and_out_of_range_altitudes_pass_the_internal_read_back_check() {
        use crate::ride::log::Altitude;
        let mut media = Memory::default();
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert!(recorder.request(Action::Start, Source::Live, 100, 1));
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
        assert!(recorder.request(Action::Finish, Source::Live, 4100, 2));
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
        assert!(recorder.request(Action::Start, Source::Live, 100, 1));
        recorder.service(&mut media, 100, Sample::default(), &|| 100);
        assert_eq!(recorder.status(), Status::Error);
        assert!(!recorder.take_result().unwrap().ok);
        let mutations = media.mutations;
        assert!(!recorder.service(&mut media, 200, Sample::default(), &|| 200));
        assert_eq!(media.mutations, mutations);
    }

    fn start(recorder: &mut Recorder, media: &mut Memory, now: u64) {
        assert!(recorder.request(Action::Start, Source::Live, now, 1));
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
            assert!(recorder.request(Action::Start, Source::Live, 100, 1));
            recorder.service(&mut media, 100, Sample::default(), &|| 100);
            assert_eq!(recorder.status(), Status::Error);

            let mut rebooted = Recorder::default();
            scan(&mut rebooted, &mut media);
            assert_eq!(rebooted.status(), Status::Ready, "torn after {torn} bytes");
            assert_eq!(rebooted.next_slot(), 1);
            start(&mut rebooted, &mut media, 200);
            assert!(rebooted.request(Action::Finish, Source::Live, 1_200, 2));
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
    fn foreign_data_in_the_first_slot_still_requires_initialization() {
        for first in [[0u8; 5], *b"RIDE\x02", [0xff, 0xff, 0xff, 0xff, 0xff]] {
            let mut media = Memory::default();
            media.bytes[..5].copy_from_slice(&first);
            media.bytes[100] = 0;
            let mut recorder = Recorder::default();
            scan(&mut recorder, &mut media);
            assert_eq!(recorder.status(), Status::NeedsInit, "{first:?}");
        }
        // Two unknown slots are never treated as one torn append.
        let mut media = Memory::default();
        media.bytes[..5].copy_from_slice(b"RIDE\x01");
        media.bytes[256..261].copy_from_slice(b"RIDE\x01");
        let mut recorder = Recorder::default();
        scan(&mut recorder, &mut media);
        assert_eq!(recorder.status(), Status::NeedsInit);
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
        assert!(recorder.request(Action::Pause, Source::Live, 10_005, 2));
        for _ in 0..3 {
            recorder.service(&mut media, 10_005, Sample::default(), &|| 10_005);
        }
        assert!(recorder.take_result().unwrap().ok);
        assert!(recorder.request(Action::Resume, Source::Live, 70_000, 3));
        recorder.service(&mut media, 70_000, Sample::default(), &|| 70_000);
        assert!(recorder.take_result().unwrap().ok);
        for now in (70_000..=80_000).step_by(10) {
            recorder.service(&mut media, now, Sample::default(), &|| now);
        }
        assert!(recorder.request(Action::Finish, Source::Live, 80_005, 4));
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
        assert!(recorder.request(Action::Finish, Source::Live, 0, 2));
        recorder.service(&mut media, 0, Sample::default(), &|| 0);
        assert!(recorder.take_result().unwrap().ok);
        media.bytes[2 * crate::ride::log::SLOT_SIZE
            ..(crate::ride::log::SLOTS - 2) * crate::ride::log::SLOT_SIZE]
            .fill(0);
        let mut nearly = Recorder::default();
        scan(&mut nearly, &mut media);
        assert_eq!(nearly.status(), Status::Ready);
        assert_eq!(nearly.remaining_secs(), 0);
        assert!(nearly.request(Action::Start, Source::Live, 0, 3));
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
        media.read_slot(1, &mut slot).unwrap();
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
        assert!(recorder.request(Action::Finish, Source::Live, 0, 2));
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
