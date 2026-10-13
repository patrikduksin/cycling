//! Physical sensor observations. Motion scaling and axes remain unverified.
use crate::observation::{Error, Observation};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pressure {
    /// Companion-compensated pressure in hundredths of a pascal.
    pub pressure_centi_pa: u32,
    /// Companion-compensated sensor temperature in hundredths of a degree Celsius.
    pub temperature_centi_c: i16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Motion {
    /// Signed wire-order values. Scaling and orientation are unverified.
    pub axes: [i16; 3],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Identity {
    /// Opaque report fields, not a chip ID or an established firmware version.
    pub fields: [u8; 3],
}

#[derive(Clone, Copy, Debug)]
pub struct Snapshot {
    pub pressure: Observation<Pressure>,
    /// Index zero is subtype one; index one is subtype two. No gyroscope claim.
    pub motion: [Observation<Motion>; 2],
    pub identity: Observation<Identity>,
    pub reports: u32,
    pub invalid_reports: u32,
    /// Number of observed transport discontinuities, not the number of lost pages.
    pub losses: u32,
}

/// Sensor companion startup progress. Only `Ready`, `AlreadyRunning` and
/// `RunningDegraded` describe a running companion.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Startup {
    Unsupported,
    NotSubmitted,
    /// Observing existing traffic before considering a startup acknowledgment.
    Probing,
    /// The startup acknowledgment is ready for submission.
    Pending,
    /// Submitted or reported; waiting for every sensor to advance.
    Waiting,
    /// The companion reports charging standby.
    Charging,
    /// An explicitly requested wake is ready for submission.
    WakePending,
    AlreadyRunning,
    /// Activity was observed, but not every sensor advanced.
    RunningDegraded,
    Ready,
    /// A short or failed write may have reached the companion; never retried.
    Uncertain,
    TimedOut,
    Unavailable,
    /// Running, while hardware IO is closed for a power transition.
    PowerTransition,
}

impl Startup {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::NotSubmitted => "not_submitted",
            Self::Probing => "probing",
            Self::Pending => "pending",
            Self::Waiting => "waiting",
            Self::Charging => "charging",
            Self::WakePending => "wake_pending",
            Self::AlreadyRunning => "already_running",
            Self::RunningDegraded => "running_degraded",
            Self::Ready => "ready",
            Self::Uncertain => "uncertain",
            Self::TimedOut => "timed_out",
            Self::Unavailable => "unavailable",
            Self::PowerTransition => "power_transition",
        }
    }
}

/// Progress of the single bounded identity request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IdentityQuery {
    Idle,
    Queued,
    Waiting,
    /// A reply arrived in the window; it is untagged, not a verified version.
    ReplyObserved,
    Timeout,
    Failed,
}

impl IdentityQuery {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Queued => "queued",
            Self::Waiting => "waiting",
            Self::ReplyObserved => "reply_observed",
            Self::Timeout => "timeout",
            Self::Failed => "failed",
        }
    }

    pub const fn outstanding(self) -> bool {
        matches!(self, Self::Queued | Self::Waiting)
    }
}

/// Optional physical sensor observations; motion remains explicitly unscaled
/// until fitted hardware and axes are established.
pub trait Sensors {
    fn startup_status(&self) -> Startup {
        Startup::Unsupported
    }
    fn startup_reason(&self) -> Option<u8> {
        None
    }
    fn snapshot(&self, now: u64) -> crate::sensors::Snapshot;
    fn identity_status(&self) -> IdentityQuery;
    fn query_identity(&mut self, now: u64) -> Result<(), Error>;
}
