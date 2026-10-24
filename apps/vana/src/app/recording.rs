//! Ride recorder ownership, storage media, request tokens and bounded
//! recording work.
//!
//! Two recorders run side by side: the bulk MMC log takes new rides whenever it
//! is ready and has room, and the internal journal takes them otherwise. Both
//! keep their own rides and recover them independently. If the bulk medium
//! fails or fills while a ride is running, the ride continues on the internal
//! journal as a new ride; the bulk part stays exportable.
use super::Runtime;
use crate::ride::auto::{Auto, Decision};
use crate::ride::bulk::{Bulk, BulkState};
use crate::ride::climb::{Climbs, Event as ClimbEvent};
use crate::ride::control::Action;
use crate::ride::journal::{Internal, InternalState};
use crate::ride::laps::{Lap, Laps};
use crate::ride::log::{Sample, Source, Status};
use crate::ride::metrics::Step;
use crate::ride::recorder::{Recorder, ResultEvent};
use crate::ride::storage::RideStorage;
use crate::ride::totals::{Efforts, Totals};
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;

/// A finished lap shows in the status row this long.
const LAP_TOAST_MS: u64 = 2000;

/// How long the brief notice of an automatic action stays up.
const TOAST_MS: u64 = 3000;

/// Slots the ride index reads per tick: internal flash reads are quick, each
/// MMC read is three card reads (ownership checks, then the record).
const INDEX_INTERNAL_BUDGET: usize = 32;
const INDEX_BULK_BUDGET: usize = 4;

/// The recorder operation a rider or host requested, until someone reads its result.
#[derive(Clone, Copy)]
pub(super) enum Request {
    Idle,
    Pending(u32),
    Completed(ResultEvent),
}

/// Ride-page feedback that does not come from the recorder state.
#[derive(Clone, Copy)]
pub(super) enum RideMessage {
    Ready,
    StorageNotReady,
}

/// Where a recorder keeps its rides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Medium {
    /// The 1 MiB journal in the owned boot-flash reservation.
    Internal,
    /// The sector log in the owned MMC partition.
    Bulk,
}

impl Medium {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Internal => "internal",
            Self::Bulk => "bulk",
        }
    }

    /// Short label for the screen.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Internal => "INT",
            Self::Bulk => "MMC",
        }
    }
}

pub(super) struct Ride {
    pub(super) internal: Recorder,
    pub(super) bulk: Recorder,
    internal_media: InternalState,
    pub(super) bulk_media: BulkState,
    pub(super) request: Request,
    /// The medium whose recorder completed `request`.
    pub(super) request_on: Medium,
    /// Token of the latest host command; only its completion is the host's to read.
    pub(super) host_token: Option<u32>,
    next_token: u32,
    pub(super) message: RideMessage,
    /// What this ride has added up so far; kept after stop until the next start.
    pub(super) totals: Totals,
    /// Recorder active time the totals already cover.
    counted_ms: u64,
    /// This ride's laps, reset with the totals.
    pub(super) laps: Laps,
    /// Lap distance that closes an automatic lap, from the rider's options.
    pub(super) auto_lap_mm: Option<u64>,
    /// The last finished lap and when it closed, for the status-row toast.
    lap_toast: Option<(u64, Lap)>,
    /// Time in each training zone, kept and reset with `totals`.
    pub(super) zone_time: ZoneTime,
    /// Best power over 5 s to 20 min, kept and reset with `totals`.
    pub(super) efforts: Efforts,
    /// Climbs finished this ride.
    pub(super) climbs_done: u16,
    /// A lap closed since the last chime.
    pub(super) lap_chime: bool,
    /// Climb detection and the elevation profile, fed and reset with `totals`.
    pub(super) climbs: Climbs,
    /// The last step wholly inside active time. A step cut short by an accepted
    /// pause or finish is settled with these readings, not those that arrived
    /// after the stop.
    settled: Option<(Sample, Step, Option<u32>)>,
    /// Auto-pause and auto-start state.
    pub(super) auto: Auto,
    /// The last automatic action and when it happened, shown briefly.
    pub(super) toast: Option<(&'static str, u64)>,
    /// Active time the bulk medium recorded before this ride moved to internal.
    carried_ms: u64,
    /// Bulk active time awaiting the internal continuation; carried only once
    /// it starts, dropped if it cannot.
    carry_ms: u64,
    /// Bulk media failed mid-ride; the next internal start continues the ride.
    continuing: bool,
    /// The current ride moved from bulk to internal media.
    pub(super) moved: bool,
    /// The bulk ride was paused when lost; pause its continuation once it runs.
    pause_continuation: bool,
    /// The medium of the latest ride, whose outcome the rider sees until the
    /// next ride starts, even when the next ride will use the other medium.
    last: Option<Medium>,
    /// Monotonic clock used to measure synchronous recording work.
    pub(super) clock: Option<fn() -> u64>,
    /// The workout this ride plays, kept after it ends until the rider
    /// leaves the ride screen or starts the next ride.
    pub(super) workout: Option<crate::workout::player::Player>,
    /// What the player reported during the last steps, for the runtime.
    pub(super) workout_events: crate::workout::player::Events,
}

impl Default for Ride {
    fn default() -> Self {
        Self {
            internal: Recorder::default(),
            bulk: Recorder::default(),
            internal_media: InternalState::default(),
            bulk_media: BulkState::default(),
            request: Request::Idle,
            request_on: Medium::Internal,
            host_token: None,
            next_token: 1,
            message: RideMessage::Ready,
            totals: Totals::default(),
            counted_ms: 0,
            laps: Laps::default(),
            auto_lap_mm: Some(crate::ride::laps::AUTO_LAP_MM),
            lap_toast: None,
            zone_time: ZoneTime::default(),
            efforts: Efforts::default(),
            climbs_done: 0,
            lap_chime: false,
            climbs: Climbs::default(),
            settled: None,
            auto: Auto::default(),
            toast: None,
            carried_ms: 0,
            carry_ms: 0,
            continuing: false,
            moved: false,
            pause_continuation: false,
            last: None,
            clock: None,
            workout: None,
            workout_events: crate::workout::player::Events::new(),
        }
    }
}

impl Ride {
    /// The medium the rider's ride is on, or the one a new ride would use:
    /// bulk while it scans, or is ready with room; internal otherwise.
    pub(super) fn medium(&self) -> Medium {
        if self.internal.riding() || self.continuing {
            return Medium::Internal;
        }
        if self.bulk.riding() {
            return Medium::Bulk;
        }
        match self.bulk.status() {
            Status::Scanning | Status::Formatting | Status::Clearing => Medium::Bulk,
            status if status.usable() && self.bulk.remaining_secs() > 0 => Medium::Bulk,
            _ => Medium::Internal,
        }
    }

    pub(super) fn recorder(&self, medium: Medium) -> &Recorder {
        match medium {
            Medium::Internal => &self.internal,
            Medium::Bulk => &self.bulk,
        }
    }

    fn recorder_mut(&mut self, medium: Medium) -> &mut Recorder {
        match medium {
            Medium::Internal => &mut self.internal,
            Medium::Bulk => &mut self.bulk,
        }
    }

    /// The medium of the ride the rider sees: the open ride, else the
    /// outcome of the latest one (saved, full or failed), else where the next
    /// ride will go.
    pub(super) fn shown(&self) -> Medium {
        if self.internal.riding() || self.continuing {
            return Medium::Internal;
        }
        if self.bulk.riding() {
            return Medium::Bulk;
        }
        match self.last {
            Some(medium)
                if matches!(
                    self.recorder(medium).status(),
                    Status::Saved | Status::Full | Status::Error
                ) =>
            {
                medium
            }
            _ => self.medium(),
        }
    }

    /// The open ride is moving from bulk to internal media, and may still
    /// have to be paused there.
    fn moving(&self) -> bool {
        self.continuing || self.pause_continuation
    }

    /// A ride is open on either medium, or moving between them.
    pub(super) fn open(&self) -> bool {
        self.internal.riding() || self.bulk.riding() || self.continuing
    }

    /// The recorder of the ride the rider sees.
    pub(super) fn active(&self) -> &Recorder {
        self.recorder(self.shown())
    }

    /// Active time of the rider's ride, across a move to internal media.
    pub(super) fn active_ms(&self, now: u64) -> u64 {
        let waiting = if self.continuing { self.carry_ms } else { 0 };
        self.carried_ms + waiting + self.active().active_ms(now)
    }

    /// The continuation cannot start: the bulk ride stays the rider's ride.
    fn give_up_continuation(&mut self) {
        self.continuing = false;
        self.pause_continuation = false;
        self.carry_ms = 0;
    }

    /// Bulk media should take rides but cannot, so they go to internal flash:
    /// it is unformatted, missing, damaged, failed, or has no room left.
    pub(super) fn fallback(&self) -> bool {
        let status = self.bulk.status();
        self.medium() == Medium::Internal
            && (matches!(
                status,
                Status::NeedsInit
                    | Status::Unavailable
                    | Status::Corrupt
                    | Status::Error
                    | Status::Full
            ) || (status.usable() && self.bulk.remaining_secs() == 0))
    }

    /// The token the next submitted operation will carry.
    pub(super) fn next_token(&self) -> u32 {
        self.next_token
    }

    /// Start a recorder operation under the next token. `start` reports
    /// whether the recorder accepted it; only then is the token consumed.
    pub(super) fn submit(
        &mut self,
        medium: Medium,
        start: impl FnOnce(&mut Recorder, u32) -> bool,
    ) -> bool {
        let token = self.next_token;
        if !start(self.recorder_mut(medium), token) {
            return false;
        }
        self.next_token = token.wrapping_add(1).max(1);
        self.request = Request::Pending(token);
        true
    }

    pub(super) fn pending(&self) -> bool {
        matches!(self.request, Request::Pending(_))
    }

    /// A finished host command waits for its status read; automatic actions
    /// must not replace its result.
    pub(super) fn host_completion_unread(&self) -> bool {
        matches!(self.request, Request::Completed(r) if Some(r.token) == self.host_token)
    }

    pub(super) fn clear_lap_toast(&mut self) {
        self.lap_toast = None;
    }

    /// The notice of a recent automatic action, if any.
    pub(super) fn toast(&self, now: u64) -> &'static str {
        match self.toast {
            Some((text, at)) if now.saturating_sub(at) <= TOAST_MS => text,
            _ => "",
        }
    }

    /// Add an odometer step to the totals, clipped to the active time the
    /// recorder accrued since the last step. An accepted pause or finish
    /// freezes active time at once, though the status stays Recording until
    /// its flush completes; nothing after that moment counts. Climbs follow
    /// the totals' smoothed altitude; returns what changed about climbing.
    pub(super) fn count(
        &mut self,
        now: u64,
        step: Step,
        sample: &Sample,
        pressure_centi_pa: Option<u32>,
        profile: &TrainingProfile,
    ) -> Option<ClimbEvent> {
        let recorder = self.active();
        if recorder.status() != Status::Recording {
            return None;
        }
        let active = recorder.active_ms(now);
        let credited = active.saturating_sub(self.counted_ms).min(step.elapsed_ms);
        self.counted_ms = active;
        if credited == 0 {
            return None;
        }
        let cut_short = credited < step.elapsed_ms && self.active().stopping();
        let (sample, step, pressure_centi_pa) = if !cut_short {
            self.settled = Some((*sample, step, pressure_centi_pa));
            (*sample, step, pressure_centi_pa)
        } else {
            // Cut short by a stop: the readings of this step describe the time
            // after it, so the credited part takes the last full step's.
            self.settled?
        };
        let distance_mm = step.distance_mm * credited / step.elapsed_ms;
        self.totals
            .add(credited, distance_mm, &sample, pressure_centi_pa);
        self.zone_time.add(credited, &sample, profile);
        if let Some(watts) = sample.power_watts
            && !cut_short
        {
            self.efforts.add_ms(watts, credited);
        }
        // Only a fresh reading is altitude for the climbs; the totals keep
        // their last one through a gap, which would read as a level road.
        let altitude = pressure_centi_pa.and(self.totals.altitude_dm());
        let climb = self.climbs.add(credited, distance_mm, altitude);
        if let Some(ClimbEvent::Finished(_)) = climb {
            self.climbs_done = self.climbs_done.saturating_add(1);
        }
        let Some(player) = &mut self.workout else {
            if let Some(lap) =
                self.laps
                    .add_every(credited, distance_mm, &sample, now, self.auto_lap_mm)
            {
                self.lap_closed(lap, now);
            }
            return climb;
        };
        // Each workout step's part of this update goes in that step's lap.
        player.read(&sample);
        let (mut ms, mut mm) = (credited, distance_mm);
        for _ in 0..=crate::workout::model::MAX_PLAYED {
            if ms == 0 {
                break;
            }
            let (part_ms, part_mm) = player.until_end(ms, mm);
            ms -= part_ms;
            mm -= part_mm;
            // A workout ride has no distance laps; its steps close them.
            let _ = self.laps.add(part_ms, part_mm, &sample, now);
            let events = player.ride_part(part_ms, part_mm, &sample, profile);
            if events.iter().any(|event| {
                matches!(
                    event,
                    crate::workout::player::Event::Started(_)
                        | crate::workout::player::Event::Finished
                )
            }) {
                let _ = self.laps.split(now);
            }
            for event in events {
                let _ = self.workout_events.push(event);
            }
        }
        climb
    }

    /// Show a finished lap in the status row.
    pub(super) fn lap_closed(&mut self, lap: Lap, now: u64) {
        self.lap_toast = Some((now, lap));
        self.lap_chime = true;
    }

    /// The finished lap the status row shows at `now`, for `LAP_TOAST_MS`.
    pub(super) fn lap_toast(&self, now: u64) -> Option<Lap> {
        self.lap_toast
            .filter(|(at, _)| now.saturating_sub(*at) < LAP_TOAST_MS)
            .map(|(_, lap)| lap)
    }

    /// Read one slot of `medium` for export; `Some(false)` marks an invalid
    /// bulk container, whose slot bytes are zeroed.
    pub(super) fn read_slot(
        &mut self,
        medium: Medium,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        index: usize,
        output: &mut crate::ride::log::Slot,
    ) -> Option<bool> {
        use crate::ride::journal::Journal;
        match medium {
            Medium::Internal => {
                Internal::new(&mut self.internal_media, store).read_slot(index, output)
            }
            Medium::Bulk => Bulk::new(&mut self.bulk_media, bulk).read_slot(index, output),
        }
    }

    /// Read up to `budget` more slots of a saved ride of `medium` into the
    /// ride sums, through the export path's reader.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn replay(
        &mut self,
        medium: Medium,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        replay: &mut crate::ride::replay::Replay,
        budget: usize,
        profile: &TrainingProfile,
    ) {
        use crate::ride::journal::Journal;
        let mut sums = crate::ride::replay::Sums {
            totals: &mut self.totals,
            zone_time: &mut self.zone_time,
            efforts: &mut self.efforts,
        };
        let (internal, bulk_media) = (&mut self.internal_media, &mut self.bulk_media);
        let mut slot = crate::ride::log::Slot::default();
        replay.step(budget, profile, &mut sums, |index| {
            let valid = match medium {
                Medium::Internal => Internal::new(internal, store).read_slot(index, &mut slot),
                Medium::Bulk => Bulk::new(bulk_media, bulk).read_slot(index, &mut slot),
            };
            match valid {
                Some(true) => crate::ride::log::decode(&slot),
                _ => None,
            }
        });
    }

    /// Index a few more saved rides of each journal, through the export
    /// path's reader, while no ride is open. Returns whether any was added.
    /// Whether the index has read every slot both journals hold; a journal
    /// that cannot be read yet holds nothing to count.
    pub(super) fn directory_done(&self, index: &crate::ride::directory::RideIndex) -> bool {
        let caught_up =
            |exportable: bool, scanned: usize, next: usize| !exportable || scanned >= next;
        caught_up(
            self.internal.exportable(),
            index.internal.scanned(),
            self.internal.next_slot(),
        ) && caught_up(
            self.bulk.exportable(),
            index.bulk.scanned(),
            self.bulk.next_slot(),
        )
    }

    pub(super) fn directory_step(
        &mut self,
        index: &mut crate::ride::directory::RideIndex,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
    ) -> bool {
        use crate::ride::directory::Read;
        use crate::ride::journal::Journal;
        if self.internal.riding() || self.bulk.riding() {
            return false;
        }
        let mut slot = crate::ride::log::Slot::default();
        let read = |valid: Option<bool>| match valid {
            Some(true) => Read::Slot,
            Some(false) => Read::Invalid,
            None => Read::Failed,
        };
        let mut added = false;
        if self.internal.exportable() {
            let media = &mut self.internal_media;
            added |= index.internal.step(
                self.internal.next_slot(),
                INDEX_INTERNAL_BUDGET,
                &mut slot,
                |i, out| read(Internal::new(media, store).read_slot(i, out)),
            );
        }
        if self.bulk.exportable() {
            let media = &mut self.bulk_media;
            added |= index.bulk.step(
                self.bulk.next_slot(),
                INDEX_BULK_BUDGET,
                &mut slot,
                |i, out| read(Bulk::new(media, bulk).read_slot(i, out)),
            );
        }
        added
    }

    /// Start the internal continuation of a ride the bulk medium lost. Waits
    /// for the internal scan; gives up if that journal cannot take a ride.
    fn continue_internally(&mut self, now: u64) {
        if !self.continuing || self.internal.riding() || self.internal.busy() {
            return;
        }
        // Wait for the scan, and for recovery to close an interrupted ride:
        // START is refused only until that record is appended.
        if self.internal.status() == Status::Scanning || self.internal.recovering() {
            return;
        }
        // Token 0: no rider or host waits for this start.
        if !self.internal.request(Action::Start, Source::Live, now, 0) {
            self.give_up_continuation();
        }
    }
}

impl Runtime {
    pub fn recording(&self) -> bool {
        // Protect accepted writes and active/paused rides from host-triggered
        // restart, including a ride moving to internal media whose start or
        // pause is queued but not yet written.
        self.ride.pending()
            || self.ride.open()
            || self.ride.moving()
            || [&self.ride.internal, &self.ride.bulk].iter().any(|r| {
                r.pending_action().is_some()
                    || matches!(
                        r.status(),
                        Status::Recording | Status::Paused | Status::Formatting | Status::Clearing
                    )
            })
    }

    /// Pause, resume or start the ride from its speed source (see `ride::auto`),
    /// through the same request path as the buttons.
    pub(super) fn auto_control(&mut self, now: u64) {
        if !self.preferences.options.auto_pause {
            return;
        }
        // A ride moving to internal media keeps its automatic pause: the
        // continuation passes through Ready and Recording before it pauses.
        if self.ride.pending() || self.ride.host_completion_unread() || self.ride.moving() {
            return;
        }
        // The first press to save an auto-paused ride takes it over: moving or
        // a lost sensor must not resume it under the rider's confirmation.
        if self.navigation.page == Page::EndRide && self.ride.active().status() == Status::Paused {
            self.ride.auto.manual();
            return;
        }
        // Rolling off starts a ride from Ready or the ride screen.
        let ready = matches!(self.navigation.page, Page::Ride | Page::Ready);
        let status = self.ride.active().status();
        let motion = self.readings.motion;
        let Some(decision) = self.ride.auto.update(now, status, motion, ready) else {
            return;
        };
        let (action, toast) = match decision {
            Decision::Pause => (Action::Pause, ""),
            Decision::Resume => (Action::Resume, "AUTO RESUME"),
            Decision::Start => (Action::Start, "AUTO START"),
        };
        self.ride.request = Request::Idle;
        // A new ride goes where the next ride is recorded; the open one
        // stays on its medium.
        let medium = if action == Action::Start {
            self.ride.medium()
        } else {
            self.ride.shown()
        };
        if self.ride.submit(medium, |recorder, token| {
            recorder.request(action, Source::Live, now, token)
        }) {
            self.ride.auto.accepted(decision);
            self.ride.toast = Some((toast, now));
            self.ride.message = RideMessage::Ready;
            self.display.redraw();
        } else {
            self.ride.auto.declined();
            if decision == Decision::Start {
                self.ride.message = RideMessage::StorageNotReady;
            }
        }
    }

    pub(super) fn record(
        &mut self,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        now: u64,
        sample: Sample,
    ) {
        let clock = self.ride.clock;
        let clock = &|| clock.map_or(now, |clock| clock());
        let ride = &mut self.ride;
        let before = ride.active().status();
        let was_scanning =
            ride.internal.status() == Status::Scanning || ride.bulk.status() == Status::Scanning;
        let resetting = |status| matches!(status, Status::Clearing | Status::Formatting);
        let internal_reset = resetting(ride.internal.status());
        let bulk_reset = resetting(ride.bulk.status());
        // An open bulk ride that the rider is not finishing continues on
        // internal media if the bulk medium fails; `Some(true)` if paused.
        let lost_paused = if ride.bulk.riding() {
            match ride.bulk.pending_action() {
                Some(Action::Finish) => None,
                Some(Action::Resume) => Some(false),
                Some(Action::Pause) => Some(true),
                _ => Some(ride.bulk.status() == Status::Paused),
            }
        } else {
            None
        };
        ride.continue_internally(now);
        ride.internal.service(
            &mut Internal::new(&mut ride.internal_media, store),
            now,
            sample,
            clock,
        );
        ride.bulk.service(
            &mut Bulk::new(&mut ride.bulk_media, bulk),
            now,
            sample,
            clock,
        );
        // Clearing or initializing internal flash removes the ride a moved
        // ride continued as, and with it the bulk time carried into it.
        if internal_reset && !resetting(ride.internal.status()) {
            ride.carried_ms = 0;
            ride.moved = false;
        }
        // A cleared or initialized journal numbers its rides from 1 again:
        // its old workout records would name rides that are gone.
        for (reset, medium, journal) in [
            (internal_reset, Medium::Internal, b'i'),
            (bulk_reset, Medium::Bulk, b'b'),
        ] {
            if reset
                && !resetting(ride.recorder(medium).status())
                && let Err(error) = crate::workout::history::forget(bulk, journal)
            {
                log::warn!(target: "sdk.workout", "history_forget_failed error={}", error.name());
            }
        }
        if let Some(paused) = lost_paused
            && matches!(ride.bulk.status(), Status::Error | Status::Full)
        {
            ride.carry_ms = ride.bulk.active_ms(now);
            ride.continuing = true;
            ride.pause_continuation = paused;
            log::warn!(target: "sdk.recorder", "bulk_lost state={} continuing=internal", ride.bulk.status().name());
            ride.continue_internally(now);
        }
        // A new ride, from any source, starts its totals and laps from zero;
        // resume and a move to internal media keep them.
        if ride.active().status() == Status::Recording
            && !matches!(before, Status::Recording | Status::Paused)
        {
            ride.last = Some(ride.shown());
            if ride.continuing {
                ride.continuing = false;
                ride.carried_ms += core::mem::take(&mut ride.carry_ms);
                ride.moved = true;
            } else {
                ride.totals = Totals::default();
                ride.carried_ms = 0;
                ride.moved = false;
                ride.laps = Laps::default();
                ride.lap_toast = None;
                ride.zone_time = ZoneTime::default();
                ride.efforts = Efforts::default();
                ride.climbs_done = 0;
                ride.climbs = Climbs::default();
                self.navigation.climb_return = None;
                // The chosen workout starts with the ride, its steps read
                // now; its steps lap it. One that cannot be read rides free.
                ride.workout = match self.training.load_chosen(bulk) {
                    Ok(player) => player,
                    Err(error) => {
                        log::warn!(target: "sdk.workout", "load_failed error={}", error.name());
                        self.training.selected = None;
                        self.training.missing = true;
                        None
                    }
                };
                ride.workout_events.clear();
                // The last workout's summary belongs to the ride it ended with.
                self.training.last = None;
                ride.laps.by_distance = ride.workout.is_none();
            }
            ride.counted_ms = 0;
        }
        // A started or resumed segment settles with its own readings, not those
        // from before the pause, or none at all before its first full step.
        if before != Status::Recording && ride.active().status() == Status::Recording {
            let distance_mm = u64::from(sample.speed_mm_s.unwrap_or(0));
            let step = Step {
                elapsed_ms: 1000,
                distance_mm,
            };
            ride.settled = Some((sample, step, None));
        }
        let mut finished = None;
        for medium in [Medium::Internal, Medium::Bulk] {
            let Some(result) = ride.recorder_mut(medium).take_result() else {
                continue;
            };
            if result.ok && result.action == Some(Action::Finish) {
                finished = Some(medium);
            }
            if result.token != 0 {
                // A ride the rider or host asked for that failed to start is
                // the latest ride, so its storage error is what shows.
                if result.action == Some(Action::Start) && !result.ok {
                    ride.last = Some(medium);
                }
                ride.request = Request::Completed(result);
                ride.request_on = medium;
            } else if !result.ok {
                ride.give_up_continuation();
            }
        }
        // A saved ride keeps how its workout went beside it on FILES.
        if let (Some(medium), Some(player)) = (finished, ride.workout.as_ref())
            && let Some(saved) = ride.recorder(medium).summaries().iter().flatten().last()
        {
            let record = crate::workout::history::Record {
                journal: match medium {
                    Medium::Internal => b'i',
                    Medium::Bulk => b'b',
                },
                ride_id: saved.ride_id,
                started_utc_ms: saved.first_utc_ms,
                name: player.workout().name,
                test: player.workout().test,
                summary: player.summary(),
            };
            if let Err(error) = crate::workout::history::save(bulk, &record) {
                log::warn!(target: "sdk.workout", "history_failed error={}", error.name());
            }
        }
        // A ride lost while paused continues paused; automation waits until
        // it is, so an automatic pause stays automatic.
        if ride.pause_continuation && !ride.continuing {
            match ride.internal.status() {
                Status::Paused => ride.pause_continuation = false,
                Status::Recording => {
                    if ride.internal.pending_action().is_none() {
                        ride.internal.request(Action::Pause, Source::Live, now, 0);
                    }
                }
                _ if !ride.internal.riding() => ride.pause_continuation = false,
                _ => {}
            }
        }
        // Stamp milestones after the synchronous writes, so their cost shows:
        // storage once neither journal scans, recording once the ride does.
        let done = clock().max(now);
        let scanning = |ride: &Ride| {
            ride.internal.status() == Status::Scanning || ride.bulk.status() == Status::Scanning
        };
        if was_scanning && !scanning(ride) {
            let status = ride.recorder(ride.medium()).status();
            self.timing
                .storage(done, status.name(), status != Status::Error);
        }
        if ride.active().status() == Status::Recording {
            self.timing.recording(done);
        }
    }
}
