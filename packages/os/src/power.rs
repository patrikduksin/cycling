//! Hardware power contracts. Timeouts, gestures and recording policy belong to callers.
pub const MAX_DELAY_MS: u32 = 30_000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Support {
    Unsupported,
    Unknown,
    Supported,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WakeSources {
    pub button: Support,
    pub usb: Support,
    pub timer: Support,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub shutdown: Support,
    pub sleep: Support,
    /// Fixed device wake timer, if sleep is currently bounded to this duration.
    pub sleep_timer_ms: Option<u32>,
    /// Explicitly resume from charging mode; this does not describe USB insertion.
    pub wake: Support,
    pub shutdown_wake: WakeSources,
    pub sleep_wake: WakeSources,
}
impl Capabilities {
    pub const UNSUPPORTED: Self = Self {
        shutdown: Support::Unsupported,
        sleep: Support::Unsupported,
        sleep_timer_ms: None,
        wake: Support::Unsupported,
        shutdown_wake: WakeSources {
            button: Support::Unsupported,
            usb: Support::Unsupported,
            timer: Support::Unsupported,
        },
        sleep_wake: WakeSources {
            button: Support::Unsupported,
            usb: Support::Unsupported,
            timer: Support::Unsupported,
        },
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Shutdown,
    Sleep,
    Wake,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Initializing,
    Idle,
    Charging,
    Requested,
    Quiescing,
    Submitted,
    Sleeping,
    Recovering,
    Completed,
    Failed,
    Uncertain,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Busy,
    Storage,
    Display,
    Wifi,
    Ble,
    Ant,
    Gnss,
    Sound,
    Companion,
    Sleep,
    PreparationTimeout,
    Submission,
    CompletionUnobservable,
    Recovery,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub sequence: u32,
    pub operation: Option<Operation>,
    pub state: State,
    pub at_ms: u64,
    /// Earliest preparation time for an accepted shutdown, in monotonic milliseconds.
    pub prepare_at_ms: Option<u64>,
    pub failure: Option<Failure>,
    /// False while work is gated, including after an uncertain power command.
    pub ready: bool,
}
impl Status {
    pub const IDLE: Self = Self {
        sequence: 0,
        operation: None,
        state: State::Idle,
        at_ms: 0,
        prepare_at_ms: None,
        failure: None,
        ready: true,
    };
}

/// Device-internal coordination observations, separate from radio link status.
/// Quiescent means completed cessation of work, not measured electrical off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PeripheralState {
    Running,
    Pending,
    Quiescent,
    Failed,
}

pub trait Control {
    fn capabilities(&self) -> Capabilities;
    fn status(&self) -> Status;
    /// Acceptance is not completion. Callers finish their durable domain work
    /// before requesting; the device separately protects accepted hardware IO.
    fn request(&mut self, operation: Operation) -> Result<(), crate::capabilities::Error>;
    /// Close hardware IO admission immediately, then wait before preparation.
    /// Only shutdown permits a delay, bounded by MAX_DELAY_MS.
    fn request_after(
        &mut self,
        operation: Operation,
        delay_ms: u32,
    ) -> Result<(), crate::capabilities::Error> {
        if delay_ms == 0 {
            self.request(operation)
        } else if delay_ms > MAX_DELAY_MS || operation != Operation::Shutdown {
            Err(crate::capabilities::Error::Invalid)
        } else {
            Err(crate::capabilities::Error::Unsupported)
        }
    }
}
