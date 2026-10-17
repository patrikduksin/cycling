//! Ride recorder ownership, request tokens and bounded recording work.
use super::Runtime;
use crate::ride::auto::{Auto, Decision, Wheel};
use crate::ride::climb::{Climbs, Event as ClimbEvent};
use crate::ride::control::Action;
use crate::ride::laps::{Lap, Laps};
use crate::ride::log::{Sample, Source, Status};
use crate::ride::metrics::Step;
use crate::ride::recorder::{Recorder, ResultEvent};
use crate::ride::storage::RideStorage;
use crate::ride::totals::Totals;
use crate::ride::zones::{TrainingProfile, ZoneTime};
use crate::screens::workout::Page;

/// A second end-ride press within this window finishes the ride.
const STOP_CONFIRM_MS: u64 = 3000;
/// A finished lap shows in the status row this long.
const LAP_TOAST_MS: u64 = 2000;

/// How long the brief notice of an automatic action stays up.
const TOAST_MS: u64 = 3000;

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
    ConfirmStop,
    StorageNotReady,
}

impl RideMessage {
    pub(super) const fn text(self) -> &'static str {
        match self {
            Self::Ready => "READY",
            Self::ConfirmStop => "PRESS AGAIN TO SAVE",
            Self::StorageNotReady => "STORAGE NOT READY",
        }
    }
}

pub(super) struct Ride {
    pub(super) recorder: Recorder,
    pub(super) request: Request,
    /// Token of the latest host command; only its completion is the host's to read.
    pub(super) host_token: Option<u32>,
    next_token: u32,
    stop_armed: Option<u64>,
    pub(super) message: RideMessage,
    /// What this ride has added up so far; kept after stop until the next start.
    pub(super) totals: Totals,
    /// Recorder active time the totals already cover.
    counted_ms: u64,
    /// This ride's laps, reset with the totals.
    pub(super) laps: Laps,
    /// The last finished lap and when it closed, for the status-row toast.
    lap_toast: Option<(u64, Lap)>,
    /// Time in each training zone, kept and reset with `totals`.
    pub(super) zone_time: ZoneTime,
    /// Climb detection and the elevation profile, fed and reset with `totals`.
    pub(super) climbs: Climbs,
    /// The last step wholly inside active time. A step cut short by an accepted
    /// pause or finish is settled with these readings, not those that arrived
    /// after the stop.
    settled: Option<(Sample, Step, Option<u32>)>,
    /// Auto-pause and auto-start state.
    pub(super) auto: Auto,
    /// The last automatic action and when it happened, shown briefly.
    toast: Option<(&'static str, u64)>,
    /// Monotonic clock used to measure synchronous recording work.
    pub(super) clock: Option<fn() -> u64>,
}

impl Default for Ride {
    fn default() -> Self {
        Self {
            recorder: Recorder::default(),
            request: Request::Idle,
            host_token: None,
            next_token: 1,
            stop_armed: None,
            message: RideMessage::Ready,
            totals: Totals::default(),
            counted_ms: 0,
            laps: Laps::default(),
            lap_toast: None,
            zone_time: ZoneTime::default(),
            climbs: Climbs::default(),
            settled: None,
            auto: Auto::default(),
            toast: None,
            clock: None,
        }
    }
}

impl Ride {
    /// The token the next submitted operation will carry.
    pub(super) fn next_token(&self) -> u32 {
        self.next_token
    }

    /// Start a recorder operation under the next token. `start` reports
    /// whether the recorder accepted it; only then is the token consumed.
    pub(super) fn submit(&mut self, start: impl FnOnce(&mut Recorder, u32) -> bool) -> bool {
        let token = self.next_token;
        if !start(&mut self.recorder, token) {
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

    /// Stop is armed: only a paused ride stops, within `STOP_CONFIRM_MS`.
    pub(super) fn stop_confirming(&self, now: u64) -> bool {
        self.recorder.status() == Status::Paused
            && self
                .stop_armed
                .is_some_and(|at| now.saturating_sub(at) <= STOP_CONFIRM_MS)
    }

    pub(super) fn arm_stop(&mut self, now: u64) {
        self.stop_armed = Some(now);
        self.message = RideMessage::ConfirmStop;
        // The confirmation replaces any lap toast still showing.
        self.lap_toast = None;
    }

    /// The notice of a recent automatic action, if any.
    pub(super) fn toast(&self, now: u64) -> &'static str {
        match self.toast {
            Some((text, at)) if now.saturating_sub(at) <= TOAST_MS => text,
            _ => "",
        }
    }

    pub(super) fn disarm_stop(&mut self) {
        self.stop_armed = None;
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
        if self.recorder.status() != Status::Recording {
            return None;
        }
        let active = self.recorder.active_ms(now);
        let credited = active.saturating_sub(self.counted_ms).min(step.elapsed_ms);
        self.counted_ms = active;
        if credited == 0 {
            return None;
        }
        let cut_short = credited < step.elapsed_ms && self.recorder.stopping();
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
        // Only a fresh reading is altitude for the climbs; the totals keep
        // their last one through a gap, which would read as a level road.
        let altitude = pressure_centi_pa.and(self.totals.altitude_dm());
        let climb = self.climbs.add(credited, distance_mm, altitude);
        if let Some(lap) = self.laps.add(credited, distance_mm, &sample, now) {
            self.lap_closed(lap, now);
        }
        climb
    }

    /// Show a finished lap in the status row.
    pub(super) fn lap_closed(&mut self, lap: Lap, now: u64) {
        self.lap_toast = Some((now, lap));
    }

    /// The finished lap the status row shows at `now`, for `LAP_TOAST_MS`.
    pub(super) fn lap_toast(&self, now: u64) -> Option<Lap> {
        self.lap_toast
            .filter(|(at, _)| now.saturating_sub(*at) < LAP_TOAST_MS)
            .map(|(_, lap)| lap)
    }
}

impl Runtime {
    pub fn recording(&self) -> bool {
        // Protect accepted writes and active/paused rides from host-triggered restart.
        self.ride.pending()
            || matches!(
                self.ride.recorder.status(),
                Status::Recording | Status::Paused | Status::Formatting | Status::Clearing
            )
    }

    /// Pause, resume or start the ride from wheel speed (see `ride::auto`),
    /// through the same request path as the buttons.
    pub(super) fn auto_control(&mut self, now: u64, speed_mm_s: Option<u32>) {
        if self.ride.pending() || self.ride.host_completion_unread() {
            return;
        }
        // The first press to save an auto-paused ride takes it over: moving or
        // a lost sensor must not resume it under the rider's confirmation.
        if self.ride.stop_confirming(now) {
            self.ride.auto.manual();
            return;
        }
        let ready = self.navigation.page == Page::Ride;
        let status = self.ride.recorder.status();
        let wheel = match speed_mm_s {
            Some(speed) => Wheel::Measured(speed),
            None if self.readings.wheel_fresh => Wheel::Unknown,
            None => Wheel::Lost,
        };
        let Some(decision) = self.ride.auto.update(now, status, wheel, ready) else {
            return;
        };
        let (action, toast) = match decision {
            Decision::Pause => (Action::Pause, ""),
            Decision::Resume => (Action::Resume, "AUTO RESUME"),
            Decision::Start => (Action::Start, "AUTO START"),
        };
        self.ride.request = Request::Idle;
        if self
            .ride
            .submit(|recorder, token| recorder.request(action, Source::Live, now, token))
        {
            self.ride.auto.accepted(decision);
            self.ride.toast = Some((toast, now));
            self.ride.message = RideMessage::Ready;
            self.ride.disarm_stop();
            self.display.redraw();
        } else {
            self.ride.auto.declined();
            if decision == Decision::Start {
                self.ride.message = RideMessage::StorageNotReady;
            }
        }
    }

    pub(super) fn record(&mut self, store: &mut impl RideStorage, now: u64, sample: Sample) {
        let clock = self.ride.clock;
        let before = self.ride.recorder.status();
        self.ride
            .recorder
            .service(store, now, sample, &|| clock.map_or(now, |clock| clock()));
        // A new ride, from any source, starts its totals and laps from zero;
        // resume keeps them.
        if self.ride.recorder.status() == Status::Recording
            && !matches!(before, Status::Recording | Status::Paused)
        {
            self.ride.totals = Totals::default();
            self.ride.counted_ms = 0;
            self.ride.laps = Laps::default();
            self.ride.lap_toast = None;
            self.ride.zone_time = ZoneTime::default();
            self.ride.climbs = Climbs::default();
            self.navigation.climb_return = None;
        }
        // A started or resumed segment settles with its own readings, not those
        // from before the pause, or none at all before its first full step.
        if before != Status::Recording && self.ride.recorder.status() == Status::Recording {
            let distance_mm = u64::from(sample.speed_mm_s.unwrap_or(0));
            let step = Step {
                elapsed_ms: 1000,
                distance_mm,
            };
            self.ride.settled = Some((sample, step, None));
        }
        // Stamp milestones after the synchronous write, so its cost shows.
        let done = clock.map_or(now, |clock| clock()).max(now);
        let status = self.ride.recorder.status();
        if before == Status::Scanning && status != Status::Scanning {
            self.timing
                .storage(done, status.name(), status != Status::Error);
        }
        if self.ride.recorder.status() == Status::Recording {
            self.timing.recording(done);
        }
        if let Some(result) = self.ride.recorder.take_result() {
            self.ride.request = Request::Completed(result);
        }
    }
}
