//! Availability, errors and explicitly aged observations.

/// Availability describes support and initialization, separately from observation freshness.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Availability {
    Unsupported,
    Initializing,
    Unconfigured,
    Ready,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Unsupported,
    Unavailable,
    Failed,
    Invalid,
}

impl Availability {
    /// Reject operations before enqueueing when no initialized owner can act.
    pub fn require_ready(self) -> Result<(), Error> {
        match self {
            Self::Ready => Ok(()),
            Self::Unsupported => Err(Error::Unsupported),
            Self::Initializing | Self::Unconfigured => Err(Error::Unavailable),
            Self::Failed => Err(Error::Failed),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Observation<T> {
    Unavailable,
    Fresh { value: T, received_ms: u64 },
    Stale { value: T, received_ms: u64 },
}

pub fn observation<T: Copy>(value: Option<(T, u64)>, now: u64, stale_ms: u64) -> Observation<T> {
    match value {
        None => Observation::Unavailable,
        Some((value, received_ms)) if now.saturating_sub(received_ms) <= stale_ms => {
            Observation::Fresh { value, received_ms }
        }
        Some((value, received_ms)) => Observation::Stale { value, received_ms },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn stale_values_keep_observation_time_and_unavailable_stays_absent() {
        assert_eq!(
            observation::<u8>(None, 9000, 5000),
            Observation::Unavailable
        );
        assert_eq!(
            observation(Some((50, 10)), 5011, 5000),
            Observation::Stale {
                value: 50,
                received_ms: 10
            }
        );
        assert_eq!(
            observation(Some((50, 10)), 5010, 5000),
            Observation::Fresh {
                value: 50,
                received_ms: 10
            }
        );
    }
}
