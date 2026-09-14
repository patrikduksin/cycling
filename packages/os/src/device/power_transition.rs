//! C606 transition lifecycle and admission barrier, independently host tested.
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use cycling_os::{
    capabilities::Error,
    power::{Failure, Operation, State, Status},
};

pub const PREPARE_MS: u64 = 30_000;
pub const RECOVER_MS: u64 = 30_000;
pub const OBSERVE_MS: u64 = 5_000;

pub struct Transition {
    status: Status,
}
impl Transition {
    pub const fn new() -> Self {
        Self {
            status: Status::IDLE,
        }
    }
    pub fn status(&self) -> Status {
        self.status
    }
    pub fn request(&mut self, operation: Operation, now: u64) -> Result<(), Error> {
        if operation == Operation::Sleep {
            return Err(Error::Unsupported);
        }
        if operation == Operation::Wake {
            return self.wake(now);
        }
        if !self.status.ready {
            return Err(Error::Unavailable);
        }
        self.status = Status {
            sequence: self.status.sequence.wrapping_add(1),
            operation: Some(operation),
            state: State::Requested,
            at_ms: now,
            failure: None,
            ready: false,
        };
        Ok(())
    }
    /// Charging startup prepares hardware without issuing a shutdown command.
    pub fn begin_charging(&mut self, now: u64) {
        if self.status.state == State::Idle {
            self.status.state = State::Quiescing;
            self.status.operation = None;
            self.status.failure = None;
            self.status.ready = false;
            self.status.at_ms = now;
        }
    }
    pub fn charging(&mut self, now: u64) {
        if self.status.state == State::Quiescing && self.status.operation.is_none() {
            self.status.state = State::Charging;
            self.status.at_ms = now;
        }
    }
    pub fn wake(&mut self, now: u64) -> Result<(), Error> {
        if self.status.state != State::Charging {
            return Err(Error::Unavailable);
        }
        self.status = Status {
            sequence: self.status.sequence.wrapping_add(1),
            operation: Some(Operation::Wake),
            state: State::Recovering,
            at_ms: now,
            failure: None,
            ready: false,
        };
        Ok(())
    }
    pub fn quiescing(&mut self, now: u64) {
        if self.status.state == State::Requested {
            self.status.state = State::Quiescing;
            self.status.at_ms = now;
        }
    }
    /// The command might have reached hardware even on a short/failed write.
    /// Neither result establishes shutdown, and neither permits an automatic retry.
    pub fn submitted(&mut self, result: Result<(), ()>, now: u64) {
        if self.status.state != State::Quiescing
            || self.status.operation != Some(Operation::Shutdown)
        {
            return;
        }
        self.status.state = if result.is_ok() {
            State::Submitted
        } else {
            State::Uncertain
        };
        self.status.failure = result.err().map(|()| Failure::Submission);
        self.status.at_ms = now;
    }
    pub fn abort(&mut self, failure: Failure, now: u64) {
        if matches!(self.status.state, State::Requested | State::Quiescing) {
            self.status.state = State::Recovering;
            self.status.failure = Some(failure);
            self.status.at_ms = now;
        }
    }
    pub fn recovered(&mut self) {
        if self.status.state == State::Recovering {
            self.status.state = if self.status.operation == Some(Operation::Wake) {
                State::Completed
            } else {
                State::Failed
            };
            self.status.ready = true;
        }
    }
    pub fn recovery_failed(&mut self) {
        if self.status.state == State::Recovering {
            self.status.state = State::Failed;
            self.status.ready = false;
            self.status.failure = Some(Failure::Recovery);
        }
    }
    pub fn tick(&mut self, now: u64) {
        let elapsed = now.saturating_sub(self.status.at_ms);
        match self.status.state {
            State::Requested | State::Quiescing if elapsed >= PREPARE_MS => {
                self.abort(Failure::PreparationTimeout, now);
            }
            State::Recovering if elapsed >= RECOVER_MS => self.recovery_failed(),
            State::Submitted if elapsed >= OBSERVE_MS => {
                self.status.state = State::Uncertain;
                self.status.failure = Some(Failure::CompletionUnobservable);
            }
            _ => {}
        }
    }
}

/// Close admission before inspecting active IO. An entrant racing closure either
/// holds a counted lease or backs out; the transition never misses accepted work.
pub struct Gate {
    closed: AtomicBool,
    active: AtomicUsize,
}
impl Gate {
    pub const fn new() -> Self {
        Self {
            closed: AtomicBool::new(false),
            active: AtomicUsize::new(0),
        }
    }
    pub fn enter(&self) -> Result<Lease<'_>, Error> {
        if self.closed.load(Ordering::SeqCst) {
            return Err(Error::Unavailable);
        }
        self.active.fetch_add(1, Ordering::SeqCst);
        if self.closed.load(Ordering::SeqCst) {
            self.active.fetch_sub(1, Ordering::SeqCst);
            return Err(Error::Unavailable);
        }
        Ok(Lease(self))
    }
    pub fn close(&self) {
        self.closed.store(true, Ordering::SeqCst);
    }
    pub fn open(&self) {
        self.closed.store(false, Ordering::SeqCst);
    }
    pub fn closed(&self) -> bool {
        self.closed.load(Ordering::SeqCst)
    }
    pub fn drained(&self) -> bool {
        self.active.load(Ordering::SeqCst) == 0
    }
}
pub struct Lease<'a>(&'a Gate);
impl Drop for Lease<'_> {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}

pub fn shutdown_frame() -> [u8; 16] {
    cycling_os::companion::command(16, [0xe2, 2, 0, 0, 0, 0, 0, 0])
}

pub fn recovery_status(
    radios: [Option<cycling_os::power::PeripheralState>; 3],
    gnss: Option<cycling_os::position_control::State>,
    sound: Option<cycling_os::sound::State>,
) -> cycling_os::power::PeripheralState {
    use cycling_os::{
        position_control::State as G, power::PeripheralState as P, sound::State as S,
    };
    if radios.contains(&Some(P::Failed)) || gnss == Some(G::Failed) || sound == Some(S::Failed) {
        P::Failed
    } else if radios
        .iter()
        .all(|state| state.is_none_or(|s| s == P::Running))
        && gnss.is_none_or(|s| s == G::Receiving)
        && sound.is_none_or(|s| s == S::Elapsed)
    {
        P::Running
    } else {
        P::Pending
    }
}
