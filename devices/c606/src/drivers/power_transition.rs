//! C606 transition lifecycle and admission barrier, independently host tested.
use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicUsize;
use core::sync::atomic::Ordering;
use device_api::observation::Error;
use device_api::power::Failure;
use device_api::power::Operation;
use device_api::power::State;
use device_api::power::Status;

pub const PREPARE_MS: u64 = 30_000;
pub const RECOVER_MS: u64 = 30_000;
pub const OBSERVE_MS: u64 = 5_000;

pub struct Transition {
    status: Status,
}
impl Default for Transition {
    fn default() -> Self {
        Self::new()
    }
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
        self.request_after(operation, 0, now)
    }
    pub fn request_after(
        &mut self,
        operation: Operation,
        delay_ms: u32,
        now: u64,
    ) -> Result<(), Error> {
        if delay_ms > device_api::power::MAX_DELAY_MS
            || (delay_ms != 0 && operation != Operation::Shutdown)
        {
            return Err(Error::Invalid);
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
            prepare_at_ms: Some(now.saturating_add(u64::from(delay_ms))),
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
            prepare_at_ms: None,
            failure: None,
            ready: false,
        };
        Ok(())
    }
    /// A physical operating-reason report can precede the end of charging
    /// preparation. Preserve accepted work and switch to its restoring path.
    pub fn physical_wake(&mut self, now: u64) {
        if self.status.state == State::Quiescing && self.status.operation.is_none() {
            self.status.state = State::Charging;
        }
        let _ = self.wake(now);
    }
    pub fn standby_failed(&mut self) {
        if self.status.state == State::Recovering && self.status.operation.is_none() {
            self.status.state = State::Failed;
            self.status.ready = false;
        }
    }
    pub fn quiescing(&mut self, now: u64) {
        if self.status.state == State::Requested
            && self.status.prepare_at_ms.is_some_and(|at| now >= at)
        {
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
    pub fn sleeping(&mut self, now: u64) {
        if self.status.state == State::Quiescing && self.status.operation == Some(Operation::Sleep)
        {
            self.status.state = State::Sleeping;
            self.status.at_ms = now;
        }
    }
    pub fn sleep_returned(&mut self, slept: bool, now: u64) {
        if self.status.state == State::Sleeping {
            self.status.state = State::Recovering;
            self.status.failure = if slept { None } else { Some(Failure::Sleep) };
            self.status.at_ms = now;
        }
    }
    pub fn sleep_uncertain(&mut self, now: u64) {
        if self.status.state == State::Sleeping {
            self.status.state = State::Uncertain;
            self.status.failure = Some(Failure::Sleep);
            self.status.at_ms = now;
        }
    }
    pub fn recovered(&mut self) {
        if self.status.state == State::Recovering {
            self.status.state = if matches!(
                self.status.operation,
                Some(Operation::Wake | Operation::Sleep)
            ) && self.status.failure.is_none()
            {
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
            State::Requested
                if self
                    .status
                    .prepare_at_ms
                    .is_some_and(|at| now >= at.saturating_add(PREPARE_MS)) =>
            {
                self.abort(Failure::PreparationTimeout, now);
            }
            State::Quiescing if elapsed >= PREPARE_MS => {
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
impl Default for Gate {
    fn default() -> Self {
        Self::new()
    }
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
    crate::drivers::companion::command(16, [0xe2, 2, 0, 0, 0, 0, 0, 0])
}

pub fn sensors_after(sample: device_api::sensors::Snapshot, boundary: u64) -> bool {
    use device_api::observation::Observation;
    fn after<T>(sample: Observation<T>, boundary: u64) -> bool {
        matches!(sample, Observation::Fresh { received_ms, .. } if received_ms > boundary)
    }
    after(sample.pressure, boundary) && sample.motion.into_iter().all(|s| after(s, boundary))
}

pub fn recovery_status(
    radios: [Option<device_api::power::PeripheralState>; 3],
    gnss: Option<device_api::position_control::State>,
    sound: Option<device_api::sound::State>,
) -> device_api::power::PeripheralState {
    use device_api::position_control::State as G;
    use device_api::power::PeripheralState as P;
    use device_api::sound::State as S;
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
