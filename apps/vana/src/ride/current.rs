//! Two recorders run side by side: the bulk MMC log takes new rides whenever it
//! is ready and has room, and the internal journal takes them otherwise. Both
//! keep their own rides and recover them independently. If the bulk medium
//! fails or fills while a ride is running, the ride continues on the internal
//! journal as a new ride; the bulk part stays exportable.
//!
//! `Ride` is the rider's ride: both recorders and their media, the request
//! tokens, the ride's sums, laps and climbs, automation, and the move from the
//! card to internal flash. The workout player belongs to `Training`.
//!
//! The internal journal is volatile when its reservation's geometry is not
//! durable. Its boot scan then finds it empty, and `service` draws a fresh
//! store id each time it starts empty, at boot or when a wipe ends, so a
//! FILES record of an earlier boot never names a ride of this one. A
//! volatile journal admits `VOLATILE_STARTS` starts, continuations included,
//! until it is cleared; a continuation past that bound does not start.
//!
//! One pass of recording is three `Ride` steps with the glue in
//! `app::recording` between them, in the order the trace hash fixes:
//!
//! 1. `service` runs the recorders. The glue then resets the stores of a
//!    wiped medium.
//! 2. `open_pass` continues a ride the card lost, or opens a new ride's sums.
//!    The glue then loads the new ride's workout and opens its pages.
//! 3. `finish_pass` hands each recorder's result to the rider or the host.
//!    The glue then settles the ended rides through `ride::settle`.
//!
//! `after_settle` then takes the outcome and pauses a continuation that owes
//! a pause.
use crate::ride::auto::{Auto, Call, Decision, Motion};
use crate::ride::bulk::{Bulk, BulkState};
use crate::ride::climb::{Climbs, Event as ClimbEvent};
use crate::ride::elevation::Trace;
use crate::ride::journal::{Internal, InternalState, Read};
use crate::ride::laps::{AutoLap, Lap, Laps};
use crate::ride::log::{Altitude, Sample, Status};
use crate::ride::medium::Medium;
use crate::ride::metrics::Step;
use crate::ride::recorder::{Action, Recorder, Refused, ResultEvent};
use crate::ride::settle::{Hide, Settled};
use crate::ride::storage::RideStorage;
use crate::ride::totals::{Efforts, Totals};
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::workout::player::{Events, Player};
use device_api::bulk::ReadWrite;
use firmware_services::checksum::crc32;
use firmware_services::trace::origin;
use firmware_services::why;

/// Rides a volatile journal may start before it is cleared again.
pub(crate) const VOLATILE_STARTS: usize = 16;
const _: () = assert!(VOLATILE_STARTS <= crate::ride::index::KEPT);

/// One actor's latest recorder operation: queued under its token, then its
/// result.
#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub(crate) enum Request {
    Idle,
    Pending(u32),
    Completed(ResultEvent),
    /// The rider answered BACK to LOSE n UNSAVED RIDES?, so the host's
    /// clear never ran.
    Declined(u32),
}

#[derive(Clone, Copy)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
pub(crate) enum Op {
    Initialize,
    Clear(usize),
    Clearable(usize),
    Request(Action, u64),
}

/// One ride as the rider rides it. A move from the card to internal memory
/// keeps it, though the journals hold two rides.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct Outing(u32);

pub(crate) struct Ride {
    internal: Recorder,
    bulk: Recorder,
    internal_media: InternalState,
    bulk_media: BulkState,
    /// The rider's latest operation, from a button or an automatic action;
    /// the ride screen shows its outcome until the next one.
    request: Request,
    /// The medium whose recorder completed `request`.
    request_on: Medium,
    /// The host's latest command, until `RIDE STATUS` reports its outcome.
    /// Rider operations never replace it.
    host: Request,
    next_token: u32,
    /// What this ride has added up so far; kept after stop until the next start.
    totals: Totals,
    /// Recorder active time the totals already cover.
    counted_ms: u64,
    /// This ride's laps, reset with the totals.
    laps: Laps,
    /// Time in each training zone, kept and reset with `totals`.
    zone_time: ZoneTime,
    /// Best power over 5 s to 20 min, kept and reset with `totals`.
    efforts: Efforts,
    /// Altitude over distance for the summary's chart, kept and reset with
    /// `totals`.
    trace: Trace,
    /// Climbs finished this ride.
    climbs_done: u16,
    /// Climb detection and the elevation profile, fed and reset with `totals`.
    climbs: Climbs,
    /// The last step wholly inside active time. A step cut short by an accepted
    /// pause or finish is settled with these readings, not those that arrived
    /// after the stop.
    settled: Option<(Sample, Step, Option<u32>)>,
    /// The last `count` took a fresh pressure reading into the totals' altitude.
    altitude_fresh: bool,
    /// Auto-pause and auto-start state.
    auto: Auto,
    continuation: Continuation,
    /// The medium of the latest ride, whose outcome the rider sees until the
    /// next ride starts, even when the next ride will use the other medium.
    last: Option<Medium>,
    /// The latest ride started, open or not.
    outing: Outing,
    /// Where each journal stood when its boot scan ended.
    scanned: Scanned,
    /// Where the rider's answer to DISCARD RIDE? stands.
    discard: Discard,
    /// Set once the boot scan ends if the internal journal's reservation
    /// loses its rides at power-off: the store id that FILES records of its
    /// rides carry. It is drawn afresh each time the journal starts empty, as
    /// the card's ride log gets a new one, so a record of an earlier boot or
    /// clear never names a ride of this one, whenever the card is read.
    volatile_store: Option<u64>,
    /// A wipe emptied the volatile journal; its next service draws a new
    /// store id before any ride of the emptied journal can start.
    renew_store: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum Continuation {
    #[default]
    None,
    Carrying {
        carry_ms: u64,
        paused_when_lost: bool,
    },
    Moved {
        carried_ms: u64,
        pause_owed: bool,
    },
}

/// The rider's answer to DISCARD RIDE?: the ride is saved as any other, then
/// hidden.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Discard {
    #[default]
    Idle,
    /// The Finish under this token is being written.
    Asked(u32),
    /// The ride was saved, and hidden or not.
    Saved { hidden: bool },
    /// The Finish failed: the ride is still open.
    Failed,
}

#[derive(Clone, Copy, Debug, Default)]
struct Scanned {
    internal: Option<origin::Journal>,
    bulk: Option<origin::RideLog>,
}

/// Where both ride journals stood when their boot scans ended, before VANA
/// wrote to either: what a trace records so that a replay can start from
/// the media as they were at power-on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Journals {
    pub internal: origin::Journal,
    pub bulk: origin::RideLog,
}

impl Default for Ride {
    fn default() -> Self {
        Self {
            internal: Recorder::new(Medium::Internal),
            bulk: Recorder::new(Medium::Bulk),
            internal_media: InternalState::default(),
            bulk_media: BulkState::default(),
            request: Request::Idle,
            request_on: Medium::Internal,
            host: Request::Idle,
            next_token: 1,
            totals: Totals::default(),
            counted_ms: 0,
            laps: Laps::default(),
            zone_time: ZoneTime::default(),
            efforts: Efforts::default(),
            trace: Trace::default(),
            climbs_done: 0,
            climbs: Climbs::default(),
            settled: None,
            altitude_fresh: false,
            auto: Auto::default(),
            continuation: Continuation::None,
            last: None,
            outing: Outing(0),
            scanned: Scanned::default(),
            discard: Discard::Idle,
            volatile_store: None,
            renew_store: false,
        }
    }
}

impl Ride {
    pub(crate) fn internal(&self) -> &Recorder {
        &self.internal
    }

    pub(crate) fn bulk(&self) -> &Recorder {
        &self.bulk
    }

    pub(crate) fn bulk_media(&self) -> &BulkState {
        &self.bulk_media
    }

    /// The rider's latest operation, from a button or an automatic action;
    /// the ride screen shows its outcome until the next one.
    pub(crate) fn request(&self) -> Request {
        self.request
    }

    /// The medium whose recorder completed `request`.
    pub(crate) fn request_on(&self) -> Medium {
        self.request_on
    }

    /// The host's latest command, until `RIDE STATUS` reports its outcome.
    pub(crate) fn host(&self) -> Request {
        self.host
    }

    pub(crate) fn totals(&self) -> &Totals {
        &self.totals
    }

    pub(crate) fn laps(&self) -> &Laps {
        &self.laps
    }

    pub(crate) fn zone_time(&self) -> &ZoneTime {
        &self.zone_time
    }

    pub(crate) fn efforts(&self) -> &Efforts {
        &self.efforts
    }

    pub(crate) fn trace(&self) -> &Trace {
        &self.trace
    }

    pub(crate) fn climbs_done(&self) -> u16 {
        self.climbs_done
    }

    pub(crate) fn climbs(&self) -> &Climbs {
        &self.climbs
    }

    pub(crate) fn auto(&self) -> &Auto {
        &self.auto
    }

    pub(crate) fn discard(&self) -> Discard {
        self.discard
    }

    /// The medium the rider's ride is on, or the one a new ride would use:
    /// bulk while it scans, or is ready with room; internal otherwise.
    pub(crate) fn medium(&self) -> Medium {
        if self.internal.riding() || self.continuing() {
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

    pub(crate) fn recorder(&self, medium: Medium) -> &Recorder {
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
    pub(crate) fn shown(&self) -> Medium {
        if self.internal.riding() || self.continuing() {
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

    pub(crate) fn target(&self, action: Action) -> Medium {
        if action == Action::Start {
            self.medium()
        } else {
            self.shown()
        }
    }

    fn continuing(&self) -> bool {
        matches!(self.continuation, Continuation::Carrying { .. })
    }

    /// The current ride moved from bulk to internal media.
    pub(crate) fn moved(&self) -> bool {
        matches!(self.continuation, Continuation::Moved { .. })
    }

    /// The open ride is moving from bulk to internal media, and may still
    /// have to be paused there.
    pub(crate) fn moving(&self) -> bool {
        matches!(
            self.continuation,
            Continuation::Carrying { .. }
                | Continuation::Moved {
                    pause_owed: true,
                    ..
                }
        )
    }

    /// A ride is open on either medium, or moving between them.
    pub(crate) fn open(&self) -> bool {
        self.internal.riding() || self.bulk.riding() || self.continuing()
    }

    /// The ride open on either medium, or moving between them.
    pub(crate) fn outing(&self) -> Option<Outing> {
        self.open().then_some(self.outing)
    }

    /// The recorder of the ride the rider sees.
    pub(crate) fn active(&self) -> &Recorder {
        self.recorder(self.shown())
    }

    /// Active time of the rider's ride, across a move to internal media.
    pub(crate) fn active_ms(&self, now: u64) -> u64 {
        let carried = match self.continuation {
            Continuation::None => 0,
            Continuation::Carrying { carry_ms, .. } => carry_ms,
            Continuation::Moved { carried_ms, .. } => carried_ms,
        };
        carried + self.active().active_ms(now)
    }

    /// The continuation cannot start: the bulk ride stays the rider's ride.
    fn give_up_continuation(&mut self) {
        why!(
            "ride.continuation_given_up",
            continuation = self.continuation
        );
        self.continuation = match self.continuation {
            Continuation::Moved { carried_ms, .. } => Continuation::Moved {
                carried_ms,
                pause_owed: false,
            },
            _ => Continuation::None,
        };
    }

    /// Bulk media should take rides but cannot, so they go to internal flash:
    /// it is unformatted, missing, damaged, failed, or has no room left.
    pub(crate) fn fallback(&self) -> bool {
        let status = self.bulk.status();
        self.medium() == Medium::Internal
            && (matches!(
                status,
                Status::NeedsInit
                    | Status::Foreign
                    | Status::Unavailable
                    | Status::Corrupt
                    | Status::Error
                    | Status::Full
            ) || (status.usable() && self.bulk.remaining_secs() == 0))
    }

    /// The journal on `medium` loses its rides at power-off.
    pub(crate) fn volatile(&self, medium: Medium) -> bool {
        medium == Medium::Internal && self.volatile_store.is_some()
    }

    /// The internal ride open now, if its journal is volatile.
    pub(crate) fn volatile_ride(&self) -> Option<u32> {
        self.volatile_store?;
        self.internal.open_ride_id()
    }

    /// A ride may start on `medium`: a volatile journal takes
    /// `VOLATILE_STARTS` rides between clears.
    pub(crate) fn admits_start(&self, medium: Medium) -> bool {
        !self.volatile(medium) || self.volatile_starts() < VOLATILE_STARTS
    }

    /// Rides started on the volatile journal since it was last emptied.
    pub(crate) fn volatile_starts(&self) -> usize {
        if self.volatile_store.is_none() {
            return 0;
        }
        self.internal.next_ride_id().saturating_sub(1) as usize
    }

    /// The token the next submitted operation will carry.
    pub(crate) fn next_token(&self) -> u32 {
        self.next_token
    }

    pub(crate) fn submit(&mut self, medium: Medium, action: Action, now: u64) -> bool {
        let Ok(token) = self.start(medium, Op::Request(action, now)) else {
            return false;
        };
        self.request = Request::Pending(token);
        true
    }

    pub(crate) fn submit_host(&mut self, medium: Medium, op: Op) -> Result<u32, Refused> {
        let token = self.start(medium, op)?;
        self.host = Request::Pending(token);
        if matches!(self.request, Request::Completed(_)) {
            self.request = Request::Idle;
        }
        Ok(token)
    }

    fn start(&mut self, medium: Medium, op: Op) -> Result<u32, Refused> {
        let token = self.next_token;
        if matches!(op, Op::Request(Action::Start, _)) && !self.admits_start(medium) {
            why!(
                "ride.volatile_starts_used",
                medium,
                starts = self.volatile_starts(),
                limit = VOLATILE_STARTS
            );
            return Err(Refused::State);
        }
        let recorder = self.recorder_mut(medium);
        let started = match op {
            Op::Initialize => recorder.initialize(token),
            Op::Clear(bound) => recorder.clear(bound, token),
            Op::Clearable(bound) => recorder.clearable(bound),
            Op::Request(action, now) => recorder
                .request(action, now, token)
                .then_some(())
                .ok_or(Refused::State),
        };
        why!("ride.submitted", medium, op, token, refused = started.err());
        started?;
        self.next_token = token.wrapping_add(1).max(1);
        Ok(token)
    }

    pub(crate) fn clear_request(&mut self) {
        self.request = Request::Idle;
    }

    pub(crate) fn host_reported(&mut self) {
        self.host = Request::Idle;
    }

    pub(crate) fn ask_discard(&mut self) {
        if let Request::Pending(token) = self.request {
            self.discard = Discard::Asked(token);
        }
    }

    pub(crate) fn clear_discard(&mut self) {
        self.discard = Discard::Idle;
    }

    pub(crate) fn auto_manual(&mut self) {
        self.auto.manual();
    }

    pub(crate) fn auto_accepted(&mut self, decision: Decision) {
        self.auto.accepted(decision);
    }

    pub(crate) fn auto_declined(&mut self) {
        self.auto.declined();
    }

    pub(crate) fn auto_update(
        &mut self,
        now: u64,
        motion: Motion,
        ready: bool,
        automatic: bool,
    ) -> Option<Call> {
        let status = self.active().status();
        self.auto.update(now, status, motion, ready, automatic)
    }

    pub(crate) fn lap(&mut self, now: u64) -> Option<Lap> {
        self.laps.lap(now)
    }

    pub(crate) fn set_laps_by_distance(&mut self, by_distance: bool) {
        self.laps.by_distance = by_distance;
    }

    pub(crate) fn wiped(&mut self, clear: Option<u32>) {
        self.internal.wiped(clear);
        self.renew_store = self.volatile_store.is_some();
    }

    pub(crate) fn clear_sums(&mut self) {
        self.totals = Totals::default();
        self.zone_time = ZoneTime::default();
        self.efforts = Efforts::default();
        self.trace.reset();
    }

    pub(crate) fn scanning(&self) -> bool {
        self.internal.status() == Status::Scanning || self.bulk.status() == Status::Scanning
    }

    /// An operation of either actor is queued: the recorders take one at a
    /// time, so nothing else is submitted until it completes.
    pub(crate) fn pending(&self) -> bool {
        self.pending_token().is_some()
    }

    pub(crate) fn wipe_failed(&mut self, clear: Option<u32>) {
        let failed = clear.map(|token| ResultEvent {
            action: None,
            token,
            ok: false,
        });
        if let Some(result) = self.internal.take_result().or(failed)
            && matches!(self.host, Request::Pending(token) if token == result.token)
        {
            self.host = Request::Completed(result);
        }
    }

    pub(crate) fn refuse_clear(&mut self, token: u32) {
        if matches!(self.host, Request::Pending(queued) if queued == token) {
            self.host = Request::Declined(token);
        }
    }

    /// The token of the queued operation, whichever actor asked for it.
    pub(crate) fn pending_token(&self) -> Option<u32> {
        match (self.host, self.request) {
            (Request::Pending(token), _) | (_, Request::Pending(token)) => Some(token),
            _ => None,
        }
    }

    /// The altitude to record with a sample: the totals' smoothed one, only
    /// while the last step took a fresh reading into it and `pressure` is
    /// still fresh, so it is never older than a step. Kept to the journal's
    /// resolution.
    pub(crate) fn recorded_altitude(&self, pressure_centi_pa: Option<u32>) -> Option<Altitude> {
        pressure_centi_pa?;
        self.altitude_fresh
            .then(|| self.totals.altitude_dm().map(Altitude::from_dm))
            .flatten()
    }

    /// Add an odometer step to the totals, clipped to the active time the
    /// recorder accrued since the last step. An accepted pause or finish
    /// freezes active time at once, though the status stays Recording until
    /// its flush completes; nothing after that moment counts. Climbs follow
    /// the totals' smoothed altitude. A workout's `player` laps the ride by
    /// its steps and reports into `events`. Returns the lap a distance
    /// closed.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn count(
        &mut self,
        now: u64,
        step: Step,
        sample: &Sample,
        pressure_centi_pa: Option<u32>,
        profile: &TrainingProfile,
        auto_lap: AutoLap,
        player: Option<&mut Player>,
        events: &mut Events,
    ) -> Option<Lap> {
        self.altitude_fresh = false;
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
        self.altitude_fresh = altitude.is_some();
        self.trace.add(self.totals.distance_mm(), altitude);
        if let Some(ClimbEvent::Finished(_)) = self.climbs.add(credited, distance_mm, altitude) {
            self.climbs_done = self.climbs_done.saturating_add(1);
        }
        let Some(player) = player else {
            return self
                .laps
                .add_every(credited, distance_mm, &sample, now, auto_lap.every_mm());
        };
        // Each workout step's part of this update goes in that step's lap.
        player.read(&sample);
        let (mut ms, mut mm) = (credited, distance_mm);
        for _ in 0..=crate::workout::model::MAX_PLAYED {
            if ms == 0 {
                break;
            }
            let (part_ms, part_mm) = player.until_end(ms, mm);
            let distance_step = player.distance_step();
            ms -= part_ms;
            mm -= part_mm;
            // A workout ride has no distance laps; its steps close them.
            let _ = self.laps.add(part_ms, part_mm, &sample, now);
            let played = player.ride_part(part_ms, part_mm, &sample, profile);
            if played.iter().any(|event| {
                matches!(
                    event,
                    crate::workout::player::Event::Started(_)
                        | crate::workout::player::Event::Finished
                )
            }) {
                let _ = self.laps.split(now, distance_step);
            }
            for event in played {
                let _ = events.push(event);
            }
        }
        None
    }

    pub(crate) fn passed_start(
        &mut self,
        auto_lap: AutoLap,
        playing: bool,
        now: u64,
    ) -> Option<Lap> {
        // A paused ride is standing still: a pass then is not a lap.
        if auto_lap != AutoLap::AtStart || playing || self.active().status() != Status::Recording {
            return None;
        }
        self.laps.at_start(now)
    }

    /// The internal journal where its boot scan ended. Not inlined, so its
    /// slot stays out of `record`'s frame.
    #[inline(never)]
    fn internal_at_boot(&mut self, store: &mut impl RideStorage) -> origin::Journal {
        use crate::ride::journal::Journal;
        if matches!(
            self.internal.status(),
            Status::Error | Status::Unavailable | Status::Unsupported
        ) {
            return origin::Journal::Unusable;
        }
        let next = self.internal.next_slot();
        let mut slot = crate::ride::log::Slot::default();
        let last = next.checked_sub(1).and_then(|index| {
            match Internal::new(&mut self.internal_media, store).read_slot(index, &mut slot) {
                Read::Slot | Read::Invalid => Some(crc32(&slot.0)),
                Read::Failed => None,
            }
        });
        origin::Journal::Scanned {
            next: u32::try_from(next).unwrap_or(u32::MAX),
            last,
        }
    }

    pub(crate) fn forget_moved_ride(&mut self) {
        if self.moved() {
            self.continuation = Continuation::None;
        }
    }

    /// The card's ride log where its boot scan ended. A scan that could not
    /// prove the log's end keeps it, read only, and reports it corrupt.
    fn bulk_at_boot(&self) -> origin::RideLog {
        match (self.bulk.status(), self.bulk_media.log()) {
            (Status::Corrupt, Some(log)) => origin::RideLog::Uncertain {
                store: log.store_id(),
                head: log.head(),
            },
            (_, Some(log)) => origin::RideLog::Log {
                store: log.store_id(),
                head: log.head(),
            },
            (Status::NeedsInit, None) => origin::RideLog::Uninitialized,
            (_, None) => origin::RideLog::Unusable,
        }
    }

    pub(crate) fn read_slot(
        &mut self,
        medium: Medium,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        index: usize,
        output: &mut crate::ride::log::Slot,
    ) -> Read {
        use crate::ride::journal::Journal;
        match medium {
            Medium::Internal => {
                Internal::new(&mut self.internal_media, store).read_slot(index, output)
            }
            Medium::Bulk => Bulk::new(&mut self.bulk_media, bulk).read_slot(index, output),
        }
    }

    /// Start the internal continuation of a ride the bulk medium lost. Waits
    /// for the internal scan; gives up if that journal cannot take a ride.
    fn continue_internally(&mut self, now: u64) {
        if !self.continuing() || self.internal.riding() || self.internal.busy() {
            return;
        }
        // Wait for the scan, and for recovery to close an interrupted ride:
        // START is refused only until that record is appended.
        if self.internal.status() == Status::Scanning || self.internal.recovering() {
            return;
        }
        // Token 0: no rider or host waits for this start.
        if !self.admits_start(Medium::Internal) || !self.internal.request(Action::Start, now, 0) {
            self.give_up_continuation();
        }
    }

    pub(crate) fn journals_at_boot(&self) -> Option<Journals> {
        let Scanned { internal, bulk } = self.scanned;
        Some(Journals {
            internal: internal?,
            bulk: bulk?,
        })
    }

    pub(crate) fn recording(&self) -> bool {
        self.open()
            || self.moving()
            || [&self.internal, &self.bulk].iter().any(|r| {
                r.pending_action().is_some()
                    || matches!(r.status(), Status::Recording | Status::Paused)
            })
    }

    pub(crate) fn busy(&self) -> bool {
        self.recording()
            || self.pending()
            || [&self.internal, &self.bulk]
                .iter()
                .any(|r| matches!(r.status(), Status::Formatting | Status::Clearing))
    }

    /// Mark closed laps, service both recorders, note where each boot scan
    /// ended, and forget a moved ride once an internal reset ends. The first
    /// of a pass's three steps: the caller then resets the stores of
    /// `Serviced.reset`, and calls `open_pass`.
    pub(crate) fn service(
        &mut self,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        now: u64,
        sample: Sample,
        clock: &impl Fn() -> u64,
    ) -> Serviced {
        let before = self.active().status();
        let was_scanning = self.scanning();
        let resetting = |status| matches!(status, Status::Clearing | Status::Formatting);
        let internal_reset = resetting(self.internal.status());
        let bulk_reset = resetting(self.bulk.status());
        // An open bulk ride that the rider is not finishing continues on
        // internal media if the bulk medium fails; `Some(true)` if paused.
        let lost_paused = if self.bulk.riding() {
            match self.bulk.pending_action() {
                Some(Action::Finish) => None,
                Some(Action::Resume) => Some(false),
                Some(Action::Pause) => Some(true),
                _ => Some(self.bulk.status() == Status::Paused),
            }
        } else {
            None
        };
        self.continue_internally(now);
        while let Some(start) = self.laps.take_closed() {
            self.internal.mark_lap(start);
            self.bulk.mark_lap(start);
        }
        self.internal.service(
            &mut Internal::new(&mut self.internal_media, store),
            now,
            sample,
            clock,
        );
        let mut internal_booted_empty = false;
        if self.scanned.internal.is_none() && self.internal.status() != Status::Scanning {
            self.scanned.internal = Some(self.internal_at_boot(store));
            internal_booted_empty =
                self.internal.status() != Status::Unsupported && !store.ride_geometry().durable;
        }
        let internal_reset_ended = internal_reset && !resetting(self.internal.status());
        let emptied = internal_reset_ended || core::mem::take(&mut self.renew_store);
        if internal_booted_empty || emptied && self.volatile_store.is_some() {
            self.volatile_store = Some(bulk.entropy());
        }
        self.bulk.service(
            &mut Bulk::new(&mut self.bulk_media, bulk),
            now,
            sample,
            clock,
        );
        if self.scanned.bulk.is_none() && self.bulk.status() != Status::Scanning {
            self.scanned.bulk = Some(self.bulk_at_boot());
        }
        if internal_reset_ended {
            self.forget_moved_ride();
        }
        let ready = |reset: bool, medium| {
            (reset && self.recorder(medium).status() == Status::Ready).then_some(medium)
        };
        Serviced {
            before,
            was_scanning,
            reset: [
                ready(internal_reset || internal_booted_empty, Medium::Internal),
                ready(bulk_reset, Medium::Bulk),
            ],
            lost_paused,
        }
    }

    /// Continue a ride the bulk medium lost on internal media, and open a
    /// new ride's totals and laps from zero; resume and a move to internal
    /// media keep them. Returns whether a new ride started, whose workout
    /// and pages the caller opens before `finish_pass`.
    pub(crate) fn open_pass(&mut self, serviced: &Serviced, now: u64, sample: &Sample) -> bool {
        if let Some(paused) = serviced.lost_paused
            && matches!(self.bulk.status(), Status::Error | Status::Full)
        {
            self.continuation = Continuation::Carrying {
                carry_ms: self.bulk.active_ms(now),
                paused_when_lost: paused,
            };
            why!(
                "ride.continues_internally",
                bulk = self.bulk.status(),
                continuation = self.continuation
            );
            log::warn!(target: "sdk.recorder", "bulk_lost state={} continuing=internal", self.bulk.status().name());
            self.continue_internally(now);
        }
        let before = serviced.before;
        let mut new_ride = false;
        if self.active().status() == Status::Recording
            && !matches!(before, Status::Recording | Status::Paused)
        {
            self.last = Some(self.shown());
            if let Continuation::Carrying {
                carry_ms,
                paused_when_lost,
            } = self.continuation
            {
                self.continuation = Continuation::Moved {
                    carried_ms: carry_ms,
                    pause_owed: paused_when_lost,
                };
                why!("ride.moved_internally", continuation = self.continuation);
            } else {
                new_ride = true;
                self.outing = Outing(self.outing.0.wrapping_add(1));
                why!("ride.new_ride", outing = self.outing, medium = self.last);
                self.continuation = Continuation::None;
                self.laps = Laps::default();
                self.clear_sums();
                self.climbs_done = 0;
                self.climbs = Climbs::default();
            }
            self.counted_ms = 0;
        }
        // A started or resumed segment settles with its own readings, not those
        // from before the pause, or none at all before its first full step.
        if before != Status::Recording && self.active().status() == Status::Recording {
            let distance_mm = u64::from(sample.speed_mm_s.unwrap_or(0));
            let step = Step {
                elapsed_ms: 1000,
                distance_mm,
            };
            self.settled = Some((*sample, step, None));
        }
        new_ride
    }

    /// Hand each recorder's result to the actor that asked for it. Returns
    /// the rides whose Finish was written, for the caller to settle.
    pub(crate) fn finish_pass(&mut self) -> [Option<Ended>; 2] {
        let mut ended = [None; 2];
        for (index, medium) in [Medium::Internal, Medium::Bulk].into_iter().enumerate() {
            let Some(result) = self.recorder_mut(medium).take_result() else {
                continue;
            };
            let discarded = result.token != 0 && self.discard == Discard::Asked(result.token);
            why!(
                "ride.result",
                medium,
                result,
                discarded,
                host = matches!(self.host, Request::Pending(token) if token == result.token)
            );
            if discarded && !result.ok {
                self.discard = Discard::Failed;
            }
            if result.ok && result.action == Some(Action::Finish) {
                ended[index] = Some(Ended { medium, discarded });
            }
            if result.token != 0 {
                // A ride the rider or host asked for that failed to start is
                // the latest ride, so its storage error is what shows.
                if result.action == Some(Action::Start) && !result.ok {
                    self.last = Some(medium);
                }
                // Each actor reads only its own outcome.
                if matches!(self.host, Request::Pending(token) if token == result.token) {
                    self.host = Request::Completed(result);
                } else {
                    self.request = Request::Completed(result);
                    self.request_on = medium;
                }
            } else if !result.ok {
                self.give_up_continuation();
            }
        }
        ended
    }

    pub(crate) fn after_settle(&mut self, settled: &Settled, now: u64) {
        match settled.hide {
            Hide::NotAsked => {}
            Hide::Kept => self.discard = Discard::Saved { hidden: false },
        }
        // A ride lost while paused continues paused; automation waits until
        // it is, so an automatic pause stays automatic.
        if let Continuation::Moved {
            carried_ms,
            pause_owed: true,
        } = self.continuation
        {
            let landed = Continuation::Moved {
                carried_ms,
                pause_owed: false,
            };
            match self.internal.status() {
                Status::Paused => self.continuation = landed,
                Status::Recording => {
                    if self.internal.pending_action().is_none() {
                        why!("ride.pause_owed", continuation = self.continuation);
                        self.internal.request(Action::Pause, now, 0);
                    }
                }
                _ if !self.internal.riding() => self.continuation = landed,
                _ => {}
            }
        }
    }
}

#[cfg(not(target_os = "none"))]
impl why::State for Ride {
    fn fields(&self, out: &mut why::Fields<'_>) {
        out.field("request", &self.request);
        out.field("request_on", &self.request_on);
        out.field("host", &self.host);
        out.field("next_token", &self.next_token);
        out.field("totals", &self.totals);
        out.field("counted_ms", &self.counted_ms);
        out.field("zone_time", &self.zone_time);
        out.field("efforts", &self.efforts);
        out.field("climbs_done", &self.climbs_done);
        out.field("settled", &self.settled);
        out.field("altitude_fresh", &self.altitude_fresh);
        out.field("auto", &self.auto);
        out.field("continuation", &self.continuation);
        out.field("last", &self.last);
        out.field("outing", &self.outing);
        out.field("scanned", &self.scanned);
        out.field("discard", &self.discard);
        out.field("volatile_store", &self.volatile_store);
        out.field("renew_store", &self.renew_store);
        out.owner("laps", &self.laps);
        out.owner("recorder_internal", &self.internal);
        out.owner("recorder_bulk", &self.bulk);
    }
}

#[derive(Clone, Copy)]
pub(crate) struct Serviced {
    pub(crate) before: Status,
    pub(crate) was_scanning: bool,
    pub(crate) reset: [Option<Medium>; 2],
    lost_paused: Option<bool>,
}

#[derive(Clone, Copy)]
pub(crate) struct Ended {
    pub(crate) medium: Medium,
    pub(crate) discarded: bool,
}

#[cfg(any(test, feature = "invariants"))]
impl Ride {
    pub(crate) fn set_host(&mut self, host: Request) {
        self.host = host;
    }
}

#[cfg(test)]
impl Ride {
    pub(crate) fn internal_mut(&mut self) -> &mut Recorder {
        &mut self.internal
    }

    pub(crate) fn set_volatile(&mut self) {
        self.volatile_store = Some(0);
    }

    pub(crate) fn bulk_mut(&mut self) -> &mut Recorder {
        &mut self.bulk
    }

    pub(crate) fn totals_mut(&mut self) -> &mut Totals {
        &mut self.totals
    }

    pub(crate) fn climbs_mut(&mut self) -> &mut Climbs {
        &mut self.climbs
    }

    pub(crate) fn set_request(&mut self, request: Request, on: Medium) {
        self.request = request;
        self.request_on = on;
    }
}
