//! Hardware power contracts. Timeouts, gestures and recording policy belong to callers.
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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Capabilities {
    pub shutdown: Support,
    pub sleep: Support,
    pub shutdown_wake: WakeSources,
    pub sleep_wake: WakeSources,
}
impl Capabilities {
    pub const UNSUPPORTED: Self = Self {
        shutdown: Support::Unsupported,
        sleep: Support::Unsupported,
        shutdown_wake: WakeSources {
            button: Support::Unsupported,
            usb: Support::Unsupported,
        },
        sleep_wake: WakeSources {
            button: Support::Unsupported,
            usb: Support::Unsupported,
        },
    };
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Operation {
    Shutdown,
    Sleep,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Requested,
    Quiescing,
    Submitted,
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
}
