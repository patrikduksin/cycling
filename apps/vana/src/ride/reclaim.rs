//! Capacity estimates and Clear admission for the ride journals.

use crate::ride::log::SAMPLES_PER_BATCH;
use crate::ride::log::SLOTS;
use crate::ride::log::Status;

pub const fn free_slots(used: usize) -> usize {
    SLOTS.saturating_sub(if used > SLOTS { SLOTS } else { used })
}

/// Optimistic 1 Hz sample time, reserving START and terminal event slots.
pub const fn estimated_seconds(used: usize) -> u32 {
    estimated_seconds_in(SLOTS, used)
}

/// `estimated_seconds` for a journal of `slots` slots, saturating at `u32::MAX`.
pub const fn estimated_seconds_in(slots: usize, used: usize) -> u32 {
    let free = slots.saturating_sub(if used > slots { slots } else { used });
    let seconds = (free.saturating_sub(2) as u64).saturating_mul(SAMPLES_PER_BATCH as u64);
    if seconds > u32::MAX as u64 {
        u32::MAX
    } else {
        seconds as u32
    }
}

pub const fn request_allowed(
    status: Status,
    open_ride: bool,
    busy: bool,
    expected_upper: usize,
    actual_upper: usize,
) -> bool {
    !open_ride
        && !busy
        && expected_upper == actual_upper
        && matches!(
            status,
            Status::NeedsInit | Status::Ready | Status::Saved | Status::Recovered | Status::Full
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capacity_is_bounded_and_reserves_terminal_slots() {
        assert_eq!(free_slots(30), 4066);
        assert_eq!(estimated_seconds(30), 16_256);
        assert_eq!(free_slots(SLOTS + 1), 0);
        assert_eq!(estimated_seconds(SLOTS), 0);
    }

    #[test]
    fn clear_refuses_busy_uncertain_and_changed_journals_but_accepts_full() {
        assert!(request_allowed(Status::Full, false, false, SLOTS, SLOTS));
        assert!(request_allowed(Status::NeedsInit, false, false, 4, 4));
        assert!(!request_allowed(Status::Recording, true, false, 4, 4));
        assert!(!request_allowed(Status::Scanning, false, false, 4, 4));
        assert!(!request_allowed(Status::Error, false, false, 4, 4));
        assert!(!request_allowed(Status::Ready, false, true, 4, 4));
        assert!(!request_allowed(Status::Ready, false, false, 3, 4));
    }
}
