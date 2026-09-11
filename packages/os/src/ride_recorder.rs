//! Incremental scanner and append scheduler for the owned ride region.

use cycling_os::{
    ride::Action,
    ride_log::{self, Catalog, Entry, Kind, Sample, Scanner, Slot, Source, Status},
};

#[derive(Clone, Copy)]
enum Pending {
    Ride(Action, Source, u64, u32),
    Initialize(u32),
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
    pending: Option<Pending>,
    result: Option<ResultEvent>,
    ride_id: u32,
    sequence: u32,
    accumulated_ms: u64,
    running_since: u64,
    stop_at: Option<u64>,
    source: Source,
    open_ride: bool,
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
            pending: None,
            result: None,
            ride_id: 0,
            sequence: 0,
            accumulated_ms: 0,
            running_since: 0,
            stop_at: None,
            source: Source::Demo,
            open_ride: false,
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
        store: &mut crate::persistent::Store,
        now: u64,
        sample: Sample,
    ) -> bool {
        if self.status == Status::Scanning {
            self.scan(store);
            return true;
        }
        if self.status == Status::Formatting {
            self.format(store);
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
            if self.append(store, &entry, true, now) {
                self.catalog.completed = self.catalog.completed.saturating_add(1);
                self.catalog.recovery = None;
                self.accumulated_ms = recovery.active_ms;
                self.open_ride = false;
            }
            return true;
        }

        if let Some(Pending::Initialize(_)) = self.pending {
            return false;
        }
        if let Some(Pending::Ride(action, source, action_ms, token)) = self.pending {
            if self.sample_count != 0 && matches!(action, Action::Pause | Action::Finish) {
                if !self.flush(store, now) {
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
            self.flush(store, now);
            return true;
        }
        false
    }

    fn scan(&mut self, store: &mut crate::persistent::Store) {
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
                Status::Recovered
            } else {
                Status::Ready
            };
        }
    }

    fn format(&mut self, store: &mut crate::persistent::Store) {
        let started = esp_hal::time::Instant::now();
        let ok = store.ride_erase_sector(self.format_sector).is_ok()
            && store
                .ride_read_sector(self.format_sector, &mut self.scan_sector)
                .is_ok()
            && ride_log::erased(&self.scan_sector.0);
        self.max_erase_ms = self
            .max_erase_ms
            .max(started.elapsed().as_millis().min(u64::from(u32::MAX)) as u32);
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

    fn flush(&mut self, store: &mut crate::persistent::Store, now: u64) -> bool {
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
            let _ = self.append(store, &entry, true, now);
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
        if self.append(store, &entry, false, now) {
            self.sequence = self.sequence.wrapping_add(1);
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
        store: &mut crate::persistent::Store,
        entry: &Entry,
        final_slot: bool,
        now: u64,
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
        let started = esp_hal::time::Instant::now();
        let _write = store
            .ride_write_slot(target, &slot)
            .and_then(|()| store.ride_commit_slot(target, &ride_log::commit_word()));
        let elapsed = started.elapsed().as_millis().min(u64::from(u32::MAX)) as u32;
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
