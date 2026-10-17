//! Incremental scanner and append scheduler for the owned ride region.
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
//! One operation and its unconsumed completion exclude further mutations.

use crate::ride::control::Action;

use crate::ride::log::Catalog;
use crate::ride::log::Entry;
use crate::ride::log::Kind;
use crate::ride::log::Sample;
use crate::ride::log::Scanner;
use crate::ride::log::Slot;
use crate::ride::log::Source;
use crate::ride::log::Status;
use crate::ride::log::Summary;
use crate::ride::reclaim::Clear;
use crate::ride::reclaim::Media;
use crate::ride::reclaim::Progress;

use crate::ride::storage::RideStorage;

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
    scanner: Option<Scanner>,
    scan_sector: crate::ride::log::Sector,
    catalog: Catalog,
    status: Status,
    format_sector: usize,
    clear: Clear,
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
    next_sample: u64,
    written_samples: u32,
    dropped_samples: u32,
    max_write_ms: u32,
    max_erase_ms: u32,
}

impl Default for Recorder {
    fn default() -> Self {
        Self {
            scanner: Some(Scanner::default()),
            scan_sector: crate::ride::log::Sector::default(),
            catalog: Catalog::default(),
            status: Status::Scanning,
            format_sector: 0,
            clear: Clear::default(),
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
        if matches!(self.status, Status::Full | Status::NeedsInit) {
            return 0;
        }
        let used = self
            .catalog
            .next_slot
            .saturating_sub(usize::from(self.open_ride));
        crate::ride::reclaim::estimated_seconds(used).saturating_sub(self.sample_count as u32)
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

    pub fn summary_count(&self) -> u8 {
        self.catalog.summary_count
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

    pub fn initialize(&mut self, token: u32) -> bool {
        if self.status != Status::NeedsInit || self.pending.is_some() || self.result.is_some() {
            return false;
        }
        self.pending = Some(Pending::Initialize(token));
        self.format_sector = 0;
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
        self.clear = Clear::default();
        self.status = Status::Clearing;
        true
    }

    pub fn take_result(&mut self) -> Option<ResultEvent> {
        self.result.take()
    }

    pub fn active_ms(&self, now: u64) -> u64 {
        if self.status == Status::Recording {
            crate::ride::log::active_at(self.accumulated_ms, self.running_since, self.stop_at, now)
        } else {
            self.accumulated_ms
        }
    }

    /// Perform at most one scan sector, erase sector, or committed slot append.
    pub fn service(
        &mut self,
        store: &mut impl RideStorage,
        now: u64,
        sample: Sample,
        clock: &impl Fn() -> u64,
    ) -> bool {
        let before = self.status;
        let worked = self.service_inner(store, now, sample, clock);
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
        store: &mut impl RideStorage,
        now: u64,
        sample: Sample,
        clock: &impl Fn() -> u64,
    ) -> bool {
        if self.status == Status::Scanning {
            self.scan(store);
            return true;
        }
        if self.status == Status::Formatting {
            self.format(store, clock);
            return true;
        }
        if self.status == Status::Clearing {
            self.clear_all(store, clock);
            return true;
        }

        if self.status == Status::Recording && self.stop_at.is_none() && now >= self.next_sample {
            if self.sample_count < self.samples.len() {
                let mut sample = sample.for_source(self.source);
                sample.active_ms = self.active_ms(now);
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
            self.next_sample += (missed + 1) * 1_000;
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
            if self.append(store, &entry, true, now, clock) {
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
                if !self.flush(store, now, clock) {
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
                if crate::ride::reclaim::estimated_seconds(self.catalog.next_slot) == 0 {
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
                store,
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
                        self.summary = Summary::started(self.ride_id, source);
                        self.status = Status::Recording;
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
            self.flush(store, now, clock);
            return true;
        }
        false
    }

    fn scan(&mut self, store: &mut impl RideStorage) {
        let scanner = self.scanner.as_mut().unwrap();
        let Some(sector) = scanner.next_sector() else {
            return;
        };
        if store
            .ride_read_sector(sector, &mut self.scan_sector)
            .is_err()
        {
            self.status = Status::Error;
            self.scanner = None;
            return;
        }
        scanner.accept(&self.scan_sector);
        if let Some(catalog) = scanner.finish() {
            self.catalog = catalog;
            self.scanner = None;
            self.status = if catalog.unknown_data() {
                Status::NeedsInit
            } else if catalog.next_slot >= crate::ride::log::SLOTS {
                Status::Full
            } else if let Some(recovery) = catalog.recovery {
                self.accumulated_ms = recovery.active_ms;
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
    }

    fn format(&mut self, store: &mut impl RideStorage, clock: &impl Fn() -> u64) {
        let started = clock();
        let ok = store.ride_erase_sector(self.format_sector).is_ok()
            && store
                .ride_read_sector(self.format_sector, &mut self.scan_sector)
                .is_ok()
            && crate::ride::log::erased(&self.scan_sector.0);
        self.max_erase_ms = self
            .max_erase_ms
            .max(clock().saturating_sub(started).min(u64::from(u32::MAX)) as u32);
        if !ok {
            self.status = Status::Error;
            let token = match self.pending.take() {
                Some(Pending::Initialize(token)) => token,
                _ => 0,
            };
            self.result = Some(ResultEvent {
                action: None,
                token,
                ok: false,
            });
            return;
        }
        self.format_sector += 1;
        if self.format_sector == crate::ride::log::SECTORS {
            let token = match self.pending.take() {
                Some(Pending::Initialize(token)) => token,
                _ => 0,
            };
            self.catalog = Catalog {
                next_ride_id: 1,
                ..Catalog::default()
            };
            self.open_ride = false;
            self.status = Status::Ready;
            self.result = Some(ResultEvent {
                action: None,
                token,
                ok: true,
            });
        }
    }

    fn clear_all(&mut self, store: &mut impl RideStorage, clock: &impl Fn() -> u64) {
        struct StoreMedia<'a, S>(&'a mut S);
        impl<S: RideStorage> Media for StoreMedia<'_, S> {
            type Error = ();
            fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
                self.0.ride_erase_sector(sector).map_err(|_| ())
            }
            fn read_sector(
                &mut self,
                sector: usize,
                output: &mut crate::ride::log::Sector,
            ) -> Result<(), Self::Error> {
                self.0.ride_read_sector(sector, output).map_err(|_| ())
            }
        }
        let started = clock();
        let progress = self
            .clear
            .service(&mut StoreMedia(store), &mut self.scan_sector);
        self.max_erase_ms = self
            .max_erase_ms
            .max(clock().saturating_sub(started).min(u64::from(u32::MAX)) as u32);
        match progress {
            Ok(Progress::More) => {}
            Ok(Progress::Complete) => {
                let token = match self.pending.take() {
                    Some(Pending::Clear(token)) => token,
                    _ => 0,
                };
                self.catalog = Catalog {
                    next_ride_id: 1,
                    ..Catalog::default()
                };
                self.ride_id = 0;
                self.sequence = 0;
                self.accumulated_ms = 0;
                self.open_ride = false;
                self.sample_count = 0;
                self.status = Status::Ready;
                self.result = Some(ResultEvent {
                    action: None,
                    token,
                    ok: true,
                });
            }
            Err(_) => {
                let token = match self.pending.take() {
                    Some(Pending::Clear(token)) => token,
                    _ => 0,
                };
                self.status = Status::Error;
                self.result = Some(ResultEvent {
                    action: None,
                    token,
                    ok: false,
                });
            }
        }
    }

    fn flush(&mut self, store: &mut impl RideStorage, now: u64, clock: &impl Fn() -> u64) -> bool {
        if self.catalog.next_slot >= crate::ride::log::SLOTS - 1 {
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
            if self.append(store, &entry, true, now, clock) {
                self.summary.finish(Kind::Full, entry.active_ms, false);
                self.catalog.push_summary(self.summary);
                self.catalog.completed = self.catalog.completed.saturating_add(1);
                self.open_ride = false;
            }
            self.status = Status::Full;
            return false;
        }
        let entry = Entry::batch(
            self.source,
            self.ride_id,
            self.sequence,
            &self.samples[..self.sample_count],
        )
        .unwrap();
        if self.append(store, &entry, false, now, clock) {
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
        store: &mut impl RideStorage,
        entry: &Entry,
        final_slot: bool,
        now: u64,
        clock: &impl Fn() -> u64,
    ) -> bool {
        if self.catalog.next_slot >= crate::ride::log::SLOTS
            || (!final_slot && self.catalog.next_slot >= crate::ride::log::SLOTS - 1)
        {
            self.freeze(now);
            self.status = Status::Full;
            return false;
        }
        let target = self.catalog.next_slot;
        let mut verify = Slot::default();
        if store.ride_read_slot(target, &mut verify).is_err()
            || !crate::ride::log::erased(&verify.0)
        {
            self.freeze(now);
            self.status = Status::Error;
            return false;
        }
        let slot = crate::ride::log::encode(entry);
        let started = clock();
        let _write = store
            .ride_write_slot(target, &slot)
            .and_then(|()| store.ride_commit_slot(target, &crate::ride::log::commit_word()));
        let elapsed = clock().saturating_sub(started).min(u64::from(u32::MAX)) as u32;
        self.max_write_ms = self.max_write_ms.max(elapsed);
        let verified = store.ride_read_slot(target, &mut verify).is_ok()
            && crate::ride::log::decode(&verify).as_ref() == Some(entry);
        if verified {
            self.catalog.next_slot += 1;
            true
        } else {
            // The commit may have reached flash even when verification could not
            // complete. Stop and let the boot scanner reconcile this slot.
            self.freeze(now);
            self.status = Status::Error;
            false
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
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    struct Memory {
        bytes: Vec<u8>,
        mutations: usize,
        fail_commit: bool,
        /// Power is lost during the next body write after this many bytes.
        tear_write: Option<usize>,
    }
    impl Default for Memory {
        fn default() -> Self {
            Self {
                bytes: vec![0xff; crate::ride::log::REGION_SIZE],
                mutations: 0,
                fail_commit: false,
                tear_write: None,
            }
        }
    }
    impl RideStorage for Memory {
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
}
