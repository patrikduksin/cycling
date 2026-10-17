//! Ride recorder ownership, request tokens and bounded recording work.
use super::Runtime;
use crate::ride::laps::{Lap, Laps};
use crate::ride::log::{Sample, Status};
use crate::ride::metrics::Step;
use crate::ride::recorder::{Recorder, ResultEvent};
use crate::ride::storage::RideStorage;
use crate::ride::totals::Totals;

/// A second end-ride press within this window finishes the ride.
const STOP_CONFIRM_MS: u64 = 3000;
/// A finished lap shows in the status row this long.
const LAP_TOAST_MS: u64 = 2000;

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
    /// Monotonic clock used to measure synchronous recording work.
    pub(super) clock: Option<fn() -> u64>,
}

impl Default for Ride {
    fn default() -> Self {
        Self {
            recorder: Recorder::default(),
            request: Request::Idle,
            next_token: 1,
            stop_armed: None,
            message: RideMessage::Ready,
            totals: Totals::default(),
            counted_ms: 0,
            laps: Laps::default(),
            lap_toast: None,
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
    }

    pub(super) fn disarm_stop(&mut self) {
        self.stop_armed = None;
    }

    /// Add an odometer step to the totals, clipped to the active time the
    /// recorder accrued since the last step. An accepted pause or finish
    /// freezes active time at once, though the status stays Recording until
    /// its flush completes; nothing after that moment counts.
    pub(super) fn count(
        &mut self,
        now: u64,
        step: Step,
        sample: &Sample,
        pressure_centi_pa: Option<u32>,
    ) {
        if self.recorder.status() != Status::Recording {
            return;
        }
        let active = self.recorder.active_ms(now);
        let credited = active.saturating_sub(self.counted_ms).min(step.elapsed_ms);
        self.counted_ms = active;
        if credited == 0 {
            return;
        }
        let distance_mm = step.distance_mm * credited / step.elapsed_ms;
        self.totals
            .add(credited, distance_mm, sample, pressure_centi_pa);
        if let Some(lap) = self.laps.add(credited, distance_mm, sample, now) {
            self.lap_closed(lap, now);
        }
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
        }
        if let Some(result) = self.ride.recorder.take_result() {
            self.ride.request = Request::Completed(result);
        }
    }
}
