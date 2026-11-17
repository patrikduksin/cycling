use super::{Action, Pending, Recorder};
use crate::ride::log::{SAMPLES_PER_BATCH, Status};

invariants! {
    pub enum Rule of recorder: Recorder {
        FormattingDisagreesWithInitialize: "formatting disagrees with a waiting initialize"
            if (recorder.status == Status::Formatting)
                != matches!(recorder.pending, Some(Pending::Initialize(_))),
        ClearingDisagreesWithClear: "clearing disagrees with a waiting clear"
            if (recorder.status == Status::Clearing)
                != matches!(recorder.pending, Some(Pending::Clear(_))),
        StopWaitsUnfrozen: "pause or finish waits with active time unfrozen"
            if recorder.stop_waits() && recorder.stop_at.is_none(),
        FrozenWithoutStop: "active time frozen without a waiting pause or finish"
            if matches!(recorder.status, Status::Recording | Status::Paused)
                && recorder.stop_at.is_some()
                && !recorder.stop_waits(),
        StartFromWrongStatus: "start waits outside ready, saved or recovered"
            if recorder.pending_action() == Some(Action::Start)
                && !matches!(recorder.status, Status::Ready | Status::Saved | Status::Recovered),
        BatchOverflow: "sample batch overflows its buffer"
            if recorder.sample_count > SAMPLES_PER_BATCH,
        SlotBeyondMedium: "next slot beyond the medium"
            if recorder.catalog.next_slot > recorder.slots,
        OpenRideInWrongStatus: "open ride outside recording, paused, recovered, full or error"
            if recorder.open_ride
                && !matches!(
                    recorder.status,
                    Status::Recording
                        | Status::Paused
                        | Status::Recovered
                        | Status::Full
                        | Status::Error
                ),
    }
}

impl Recorder {
    fn stop_waits(&self) -> bool {
        matches!(self.pending_action(), Some(Action::Pause | Action::Finish))
    }
}

#[cfg(test)]
impl Recorder {
    pub(crate) fn in_status(status: Status, open_ride: bool) -> Self {
        Self {
            status,
            open_ride,
            ..Self::default()
        }
    }

    pub(crate) fn violating(rule: Rule) -> Self {
        let riding = Self::in_status(Status::Recording, true);
        match rule {
            Rule::FormattingDisagreesWithInitialize => Self::in_status(Status::Formatting, false),
            Rule::ClearingDisagreesWithClear => Self::in_status(Status::Clearing, false),
            Rule::StopWaitsUnfrozen => Self {
                pending: Some(Pending::Ride(Action::Pause, 0, 1)),
                ..riding
            },
            Rule::FrozenWithoutStop => Self {
                stop_at: Some(1),
                ..riding
            },
            Rule::StartFromWrongStatus => Self {
                pending: Some(Pending::Ride(Action::Start, 0, 1)),
                ..riding
            },
            Rule::BatchOverflow => Self {
                sample_count: SAMPLES_PER_BATCH + 1,
                ..riding
            },
            Rule::SlotBeyondMedium => {
                let mut recorder = riding;
                recorder.catalog.next_slot = recorder.slots + 1;
                recorder
            }
            Rule::OpenRideInWrongStatus => Self::in_status(Status::Ready, true),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_rule_is_broken_by_its_own_state() {
        for rule in Rule::ALL {
            assert_eq!(Rule::of(&Recorder::violating(*rule)), Some(*rule), "{rule}");
        }
    }

    #[test]
    fn a_waiting_initialize_or_clear_needs_its_status() {
        let waits = |pending| Recorder {
            pending: Some(pending),
            ..Recorder::in_status(Status::Ready, false)
        };
        assert_eq!(
            Rule::of(&waits(Pending::Initialize(1))),
            Some(Rule::FormattingDisagreesWithInitialize)
        );
        assert_eq!(
            Rule::of(&waits(Pending::Clear(1))),
            Some(Rule::ClearingDisagreesWithClear)
        );
    }

    #[test]
    fn consistent_recorders_hold() {
        let riding = || Recorder::in_status(Status::Recording, true);
        let consistent = [
            Recorder::default(),
            riding(),
            Recorder::in_status(Status::Paused, true),
            Recorder::in_status(Status::Full, true),
            Recorder::in_status(Status::Error, true),
            Recorder {
                pending: Some(Pending::Ride(Action::Finish, 0, 1)),
                stop_at: Some(1),
                sample_count: SAMPLES_PER_BATCH,
                ..riding()
            },
            Recorder {
                pending: Some(Pending::Ride(Action::Start, 0, 1)),
                ..Recorder::in_status(Status::Saved, false)
            },
            Recorder {
                pending: Some(Pending::Initialize(1)),
                ..Recorder::in_status(Status::Formatting, false)
            },
            Recorder {
                pending: Some(Pending::Clear(1)),
                ..Recorder::in_status(Status::Clearing, false)
            },
            {
                let mut full = riding();
                full.catalog.next_slot = full.slots;
                full
            },
        ];
        for recorder in &consistent {
            assert_eq!(Rule::of(recorder), None, "{:?}", recorder.status);
        }
    }
}
