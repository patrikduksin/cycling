#[cfg(test)]
use super::{Action, OpenRide, ResultEvent};
use super::{Ask, Inbox, Lifecycle, Recorder};
use crate::ride::log::SAMPLES_PER_BATCH;
#[cfg(test)]
use crate::ride::log::Summary;

invariants! {
    pub enum Rule of recorder: Recorder {
        StartFromWrongStatus: "start waits outside ready, saved or recovered"
            if matches!(recorder.inbox, Inbox::Pending(Ask::Start { .. }))
                && !matches!(
                    recorder.lifecycle,
                    Lifecycle::Ready | Lifecycle::Saved | Lifecycle::Recovered
                ),
        ResumeOutsidePaused: "resume waits outside paused"
            if matches!(recorder.inbox, Inbox::Pending(Ask::Resume { .. }))
                && !matches!(recorder.lifecycle, Lifecycle::Paused { .. }),
        OperationBesideInbox: "a stop, initialize or clear runs with the inbox in use"
            if matches!(
                recorder.lifecycle,
                Lifecycle::Stopping { .. } | Lifecycle::Preparing { .. }
            ) && !matches!(recorder.inbox, Inbox::Idle),
        RecoveringDisagreesWithCatalog: "recovering disagrees with the catalog's recovery"
            if match recorder.lifecycle {
                Lifecycle::Recovering => recorder.catalog.recovery.is_none(),
                Lifecycle::Recovered => recorder.catalog.recovery.is_some(),
                _ => false,
            },
        CheckpointDueWhileFullAndOpen: "a checkpoint is due while full with the ride open"
            if recorder.checkpoint_due
                && matches!(recorder.lifecycle, Lifecycle::Full { open: true }),
        BatchOverflow: "sample batch overflows its buffer"
            if recorder.batch.count > SAMPLES_PER_BATCH,
        SlotBeyondMedium: "next slot beyond the medium"
            if recorder.catalog.next_slot > recorder.capacity(),
    }
}

impl Recorder {
    pub(crate) fn outstanding_token(&self) -> Option<u32> {
        match (&self.lifecycle, &self.inbox) {
            (Lifecycle::Preparing { token, .. }, _) => Some(*token),
            (_, Inbox::Done(result)) => Some(result.token),
            _ => self.waiting().map(|(_, token)| token),
        }
    }
}

#[cfg(test)]
impl Recorder {
    pub(crate) fn running() -> Self {
        Self {
            lifecycle: Lifecycle::Running {
                ride: OpenRide {
                    id: 1,
                    sequence: 1,
                    committed_ms: 0,
                    summary: Summary::started(1),
                },
                since: 0,
                next_sample: 0,
            },
            ..Self::default()
        }
    }

    pub(crate) fn failed() -> Self {
        Self {
            lifecycle: Lifecycle::Failed { open: false },
            ..Self::default()
        }
    }

    pub(crate) fn violating(rule: Rule) -> Self {
        let mut recorder = Self::running();
        match rule {
            Rule::StartFromWrongStatus => {
                recorder.inbox = Inbox::Pending(Ask::Start { at: 0, token: 1 });
            }
            Rule::ResumeOutsidePaused => {
                recorder.inbox = Inbox::Pending(Ask::Resume { at: 0, token: 1 });
            }
            Rule::OperationBesideInbox => {
                recorder.request(Action::Pause, 0, 1);
                recorder.inbox = Inbox::Done(ResultEvent {
                    action: None,
                    token: 2,
                    ok: true,
                });
            }
            Rule::RecoveringDisagreesWithCatalog => recorder.lifecycle = Lifecycle::Recovering,
            Rule::CheckpointDueWhileFullAndOpen => {
                recorder.lifecycle = Lifecycle::Full { open: true };
                recorder.checkpoint_due = true;
            }
            Rule::BatchOverflow => recorder.batch.count = SAMPLES_PER_BATCH + 1,
            Rule::SlotBeyondMedium => recorder.catalog.next_slot = recorder.capacity() + 1,
        }
        recorder
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
    fn consistent_recorders_hold() {
        let in_lifecycle = |lifecycle| Recorder {
            lifecycle,
            ..Recorder::default()
        };
        let mut stopping = Recorder::running();
        stopping.batch.count = SAMPLES_PER_BATCH;
        assert!(stopping.request(Action::Finish, 1, 2));
        let mut starting = in_lifecycle(Lifecycle::Saved);
        assert!(starting.request(Action::Start, 0, 1));
        let mut resuming = Recorder::running();
        assert!(resuming.request(Action::Pause, 0, 1));
        resuming.lifecycle = Lifecycle::Paused {
            ride: *resuming.lifecycle.ride().unwrap(),
        };
        assert!(resuming.request(Action::Resume, 0, 2));
        let mut clearing = in_lifecycle(Lifecycle::Ready);
        assert_eq!(clearing.clear(0, 1), Ok(()));
        let mut full = Recorder::running();
        full.catalog.next_slot = full.capacity();
        let mut full_and_due = in_lifecycle(Lifecycle::Full { open: false });
        full_and_due.checkpoint_due = true;
        let consistent = [
            Recorder::default(),
            Recorder::running(),
            stopping,
            starting,
            resuming,
            clearing,
            full,
            full_and_due,
            in_lifecycle(Lifecycle::Recovered),
            in_lifecycle(Lifecycle::Full { open: true }),
            in_lifecycle(Lifecycle::Failed { open: true }),
        ];
        for recorder in &consistent {
            assert_eq!(Rule::of(recorder), None, "{:?}", recorder.lifecycle);
        }
    }
}
