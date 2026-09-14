//! Fixed companion suspend and explicit recovery. No command is replayed.
use cycling_os::{capabilities::Observation, companion_sensors::Snapshot};

pub const QUIET_MS: u64 = 500;
pub const SUSPEND_TIMEOUT_MS: u64 = 3_000;
pub const RECOVERY_TIMEOUT_MS: u64 = 8_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    SuspendPending,
    Suspending,
    Suspended,
    ResumePending,
    Resuming,
    Ready,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Suspend,
    Resume,
}
impl Action {
    pub fn frame(self) -> [u8; 16] {
        let value = match self {
            Self::Suspend => 3,
            Self::Resume => 7,
        };
        cycling_os::companion::command(16, [0xe2, 2, 0, 0, 0, value, 0, 0])
    }
}

pub struct Sleep {
    state: State,
    issued: Option<Action>,
    suspend_attempted: bool,
    resume_attempted: bool,
    acknowledged: bool,
    button_seen: bool,
    at: u64,
    quiet_since: u64,
    last: [Option<u64>; 3],
    baseline: [Option<u64>; 3],
    first_recovery: Option<[u64; 3]>,
    losses: u32,
    reports: u32,
    invalid_reports: u32,
}
impl Sleep {
    pub const fn new() -> Self {
        Self {
            state: State::Idle,
            issued: None,
            suspend_attempted: false,
            resume_attempted: false,
            acknowledged: false,
            button_seen: false,
            at: 0,
            quiet_since: 0,
            last: [None; 3],
            baseline: [None; 3],
            first_recovery: None,
            losses: 0,
            reports: 0,
            invalid_reports: 0,
        }
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Call only after other peripheral work is held quiescent. A fresh normal
    /// operating reason must belong to this boot, not a retained diagnostic value.
    pub fn begin(&mut self, sample: Snapshot, now: u64, transport_clean: bool) -> bool {
        if self.state != State::Idle || !transport_clean || fresh(sample, now).is_none() {
            return false;
        }
        self.baseline = times(sample);
        self.last = self.baseline;
        self.losses = sample.losses;
        self.reports = sample.reports;
        self.invalid_reports = sample.invalid_reports;
        self.at = now;
        self.quiet_since = now;
        self.state = State::SuspendPending;
        true
    }

    /// Taking an action consumes permission to send it, including a short write.
    pub fn take_action(&mut self) -> Option<Action> {
        let action = match self.state {
            State::SuspendPending if !self.suspend_attempted => {
                self.suspend_attempted = true;
                self.state = State::Suspending;
                Action::Suspend
            }
            State::ResumePending if !self.resume_attempted => {
                self.resume_attempted = true;
                self.state = State::Resuming;
                Action::Resume
            }
            _ => return None,
        };
        self.issued = Some(action);
        Some(action)
    }

    pub fn submitted(&mut self, action: Action, result: Result<(), ()>, now: u64) {
        if self.issued != Some(action) {
            return;
        }
        self.issued = None;
        self.at = now;
        self.quiet_since = now;
        if result.is_err() {
            self.state = State::Uncertain;
        }
    }

    /// Route an already CRC-validated complete frame from the single IO owner.
    pub fn receive(&mut self, frame: &[u8], now: u64) {
        if frame.len() != 16 || frame[5] != 16 {
            return;
        }
        let payload = &frame[6..14];
        if frame[4] == 5 && payload == [0xe2, 2, 1, 0, 0, 0, 0, 0]
            || frame[4] == 4 && payload[0] == 0x49
        {
            self.report(16, payload, now);
        }
    }

    /// Generic setter response, accepted only in the sole outstanding suspend
    /// window. It proves receiver return, not electrical state or silent radios.
    /// The IO owner must exclude replies to all other e2/02 setter requests.
    fn report(&mut self, group: u8, payload: &[u8], now: u64) {
        if group != 16 {
            return;
        }
        if payload.first() == Some(&0x49) && self.suspend_attempted {
            self.button_seen = true;
            if matches!(self.state, State::Suspending | State::Suspended) {
                self.state = State::Uncertain;
            }
        }
        if self.state == State::Suspending
            && self.issued.is_none()
            && now.saturating_sub(self.at) < SUSPEND_TIMEOUT_MS
            && payload == [0xe2, 2, 1, 0, 0, 0, 0, 0]
        {
            self.acknowledged = true;
        }
    }

    /// Explicit abort or timer recovery is a different fixed operation, not a
    /// retry of suspend. It is safe to attempt once after ambiguous suspend only
    /// with other work still gated and a clean transport. Never invoke merely
    /// because a deadline expired. Button recovery may have changed the retained
    /// subtype and must first be observed passively instead of reinitialized.
    pub fn request_resume(&mut self, sample: Snapshot, now: u64, transport_clean: bool) -> bool {
        if !self.suspend_attempted
            || self.resume_attempted
            || self.issued.is_some()
            || !transport_clean
            || !matches!(
                self.state,
                State::Suspending | State::Suspended | State::Failed | State::Uncertain
            )
        {
            return false;
        }
        // A lost button frame must not turn autonomous recovery into another
        // initialization command. Even one newly received sensor report proves
        // activity beyond the last observed suspend snapshot.
        let autonomous = fresh_times(sample, now)
            .iter()
            .zip(self.last)
            .any(|(new, old)| matches!((new, old), (Some(new), Some(old)) if *new > old));
        self.baseline = times(sample);
        self.last = self.baseline;
        self.first_recovery = None;
        self.losses = sample.losses;
        self.reports = sample.reports;
        self.invalid_reports = sample.invalid_reports;
        self.at = now;
        self.state = if self.button_seen || autonomous {
            // Observe the companion's autonomous recovery. Do not send value7
            // into its subtype-one partial initialization path.
            self.resume_attempted = true;
            State::Resuming
        } else {
            State::ResumePending
        };
        true
    }

    pub fn observe(&mut self, sample: Snapshot, now: u64, transport_clean: bool) {
        if !matches!(
            self.state,
            State::Suspending | State::Suspended | State::Resuming | State::Uncertain
        ) {
            return;
        }
        if !transport_clean || sample.losses != self.losses {
            self.state = State::Uncertain;
            self.first_recovery = None;
            return;
        }
        if sample.invalid_reports != self.invalid_reports {
            self.invalid_reports = sample.invalid_reports;
            self.state = State::Uncertain;
            self.first_recovery = None;
            return;
        }
        let new_reports = sample.reports != self.reports;
        self.reports = sample.reports;
        let next = times(sample);
        if new_reports
            || next
                .iter()
                .zip(self.last)
                .any(|(new, old)| new.is_some() && *new != old)
        {
            self.quiet_since = now;
            self.last = next;
            if self.state == State::Suspended {
                self.state = State::Uncertain;
            }
        }
        if self.resume_attempted && self.issued.is_none() {
            if let Some(stamps) = fresh(sample, now)
                && stamps
                    .iter()
                    .zip(self.baseline)
                    .all(|(new, old)| old.is_none_or(|old| *new > old))
            {
                if let Some(first) = self.first_recovery {
                    if stamps.iter().zip(first).all(|(new, old)| *new > old) {
                        self.state = State::Ready;
                        return;
                    }
                } else {
                    self.first_recovery = Some(stamps);
                }
            }
            if now.saturating_sub(self.at) >= RECOVERY_TIMEOUT_MS {
                self.state = State::Failed;
            }
        } else if self.state == State::Suspending && self.issued.is_none() {
            if now.saturating_sub(self.at) >= SUSPEND_TIMEOUT_MS {
                self.state = State::Uncertain;
            } else if self.acknowledged && now.saturating_sub(self.quiet_since) >= QUIET_MS {
                self.state = State::Suspended;
            }
        }
    }
}

fn times(sample: Snapshot) -> [Option<u64>; 3] {
    fn stamp<T>(value: Observation<T>) -> Option<u64> {
        match value {
            Observation::Fresh { received_ms, .. } | Observation::Stale { received_ms, .. } => {
                Some(received_ms)
            }
            Observation::Unavailable => None,
        }
    }
    [
        stamp(sample.pressure),
        stamp(sample.motion[0]),
        stamp(sample.motion[1]),
    ]
}
fn fresh_times(sample: Snapshot, now: u64) -> [Option<u64>; 3] {
    fn stamp<T>(value: Observation<T>, now: u64) -> Option<u64> {
        match value {
            Observation::Fresh { received_ms, .. }
                if received_ms <= now && now - received_ms < 1_000 =>
            {
                Some(received_ms)
            }
            _ => None,
        }
    }
    [
        stamp(sample.pressure, now),
        stamp(sample.motion[0], now),
        stamp(sample.motion[1], now),
    ]
}
fn fresh(sample: Snapshot, now: u64) -> Option<[u64; 3]> {
    let stamps = fresh_times(sample, now);
    Some([stamps[0]?, stamps[1]?, stamps[2]?])
}
