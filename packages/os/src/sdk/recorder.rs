//! Incremental scanner and append scheduler for the owned ride region.

use super::{
    ride::Action,
    ride_log::{self, Catalog, Entry, Kind, Sample, Scanner, Slot, Source, Status, Summary},
    ride_reclaim::{Clear, Media, Progress},
};

use super::storage::RideStorage;

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
    scan_sector: ride_log::Sector,
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
    samples: [Sample; ride_log::SAMPLES_PER_BATCH],
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
            scan_sector: ride_log::Sector::default(),
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
            samples: [Sample::default(); ride_log::SAMPLES_PER_BATCH],
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

    pub fn max_erase_ms(&self) -> u32 {
        self.max_erase_ms
    }

    pub fn source(&self) -> Option<Source> {
        self.open_ride.then_some(self.source)
    }

    pub fn summaries(&self) -> [Option<Summary>; ride_log::HISTORY_CAPACITY] {
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
            Action::Finish => self.open_ride && self.status == Status::Paused,
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
        if !super::ride_reclaim::request_allowed(
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
            ride_log::active_at(self.accumulated_ms, self.running_since, self.stop_at, now)
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
            self.next_sample = now + 1_000;
        }

        if self.status == Status::Recovered && self.catalog.recovery.is_some() {
            let recovery = self.catalog.recovery.unwrap();
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
            self.status = if catalog.valid_slots == 0 && catalog.invalid_slots != 0 {
                Status::NeedsInit
            } else if catalog.next_slot >= ride_log::SLOTS {
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
            && ride_log::erased(&self.scan_sector.0);
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
        if self.format_sector == ride_log::SECTORS {
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
                output: &mut ride_log::Sector,
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
        if self.catalog.next_slot >= ride_log::SLOTS - 1 {
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
        if self.catalog.next_slot >= ride_log::SLOTS
            || (!final_slot && self.catalog.next_slot >= ride_log::SLOTS - 1)
        {
            self.freeze(now);
            self.status = Status::Full;
            return false;
        }
        let target = self.catalog.next_slot;
        let mut verify = Slot::default();
        if store.ride_read_slot(target, &mut verify).is_err() || !ride_log::erased(&verify.0) {
            self.freeze(now);
            self.status = Status::Error;
            return false;
        }
        let slot = ride_log::encode(entry);
        let started = clock();
        let _write = store
            .ride_write_slot(target, &slot)
            .and_then(|()| store.ride_commit_slot(target, &ride_log::commit_word()));
        let elapsed = clock().saturating_sub(started).min(u64::from(u32::MAX)) as u32;
        self.max_write_ms = self.max_write_ms.max(elapsed);
        let verified = store.ride_read_slot(target, &mut verify).is_ok()
            && ride_log::decode(&verify).as_ref() == Some(entry);
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
            (self.accumulated_ms, self.running_since, self.stop_at) =
                ride_log::freeze_at(self.accumulated_ms, self.running_since, self.stop_at, now);
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
    }
    impl Default for Memory {
        fn default() -> Self {
            Self {
                bytes: vec![0xff; ride_log::REGION_SIZE],
                mutations: 0,
                fail_commit: false,
            }
        }
    }
    impl RideStorage for Memory {
        type Error = ();
        fn ride_read_sector(
            &mut self,
            sector: usize,
            output: &mut ride_log::Sector,
        ) -> Result<(), ()> {
            output.0.copy_from_slice(
                &self.bytes[sector * ride_log::SECTOR_SIZE..(sector + 1) * ride_log::SECTOR_SIZE],
            );
            Ok(())
        }
        fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), ()> {
            output.0.copy_from_slice(
                &self.bytes[slot * ride_log::SLOT_SIZE..(slot + 1) * ride_log::SLOT_SIZE],
            );
            Ok(())
        }
        fn ride_erase_sector(&mut self, sector: usize) -> Result<(), ()> {
            self.mutations += 1;
            self.bytes[sector * ride_log::SECTOR_SIZE..(sector + 1) * ride_log::SECTOR_SIZE]
                .fill(0xff);
            Ok(())
        }
        fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), ()> {
            self.mutations += 1;
            for (stored, new) in self.bytes
                [slot * ride_log::SLOT_SIZE..(slot + 1) * ride_log::SLOT_SIZE]
                .iter_mut()
                .zip(data.0)
            {
                *stored &= new;
            }
            Ok(())
        }
        fn ride_commit_slot(&mut self, slot: usize, commit: &ride_log::Commit) -> Result<(), ()> {
            self.mutations += 1;
            if self.fail_commit {
                return Err(());
            }
            let end = (slot + 1) * ride_log::SLOT_SIZE;
            self.bytes[end - 4..end].copy_from_slice(&commit.0);
            Ok(())
        }
    }
    fn scan(recorder: &mut Recorder, media: &mut Memory) {
        for _ in 0..ride_log::SECTORS {
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
}
