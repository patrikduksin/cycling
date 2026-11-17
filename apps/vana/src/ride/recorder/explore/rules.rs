use super::medium::{LAST, Volume};
use super::{Event, Returned, Wipe, World};
use crate::ride::log::{Kind, SAMPLES_PER_BATCH, Slot, Status, decode, erased};
use crate::ride::recorder::Pending;
use crate::ride::recorder::invariants::Rule;
use crate::testing::{Holds, Legal};

fn wrote(before: &World, event: Event, after: &World) -> bool {
    let (was, is) = (&before.state.env.medium, &after.state.env.medium);
    !event.writes_without_the_recorder()
        && (was.volume != is.volume || was.slots != is.slots || was.checkpoint != is.checkpoint)
}

fn appended(before: &World, after: &World) -> bool {
    before
        .state
        .env
        .medium
        .slots
        .iter()
        .zip(after.state.env.medium.slots.iter())
        .any(|(was, is)| erased(was) && !erased(is))
}

fn wipe_waits(world: &World) -> bool {
    matches!(world.state.recorder.pending, Some(Pending::Wipe(_)))
}

pub(super) const HOLDS: &[Holds<World>] = &[
    ("the recorder's invariants hold", |w| {
        Rule::of(&w.state.recorder).is_none()
    }),
    (
        "the catalog's next slot is the journal's head, outside scanning, error and a failed wipe",
        |w| {
            matches!(w.status(), Status::Scanning | Status::Error)
                || w.state.env.wipe == Wipe::Failed
                || w.state.recorder.catalog.next_slot == w.state.env.medium.head()
        },
    ),
    ("a wipe waits only while one runs", |w| {
        !wipe_waits(w) || w.state.env.wipe == Wipe::Running
    }),
    (
        "a waiting wipe implies needs_init, ready, saved, recovered or full, with no ride open",
        |w| {
            !wipe_waits(w)
                || (!w.state.recorder.open_ride
                    && matches!(
                        w.status(),
                        Status::NeedsInit
                            | Status::Ready
                            | Status::Saved
                            | Status::Recovered
                            | Status::Full
                    ))
        },
    ),
];

pub(super) const LEGAL: &[Legal<World, Event>] = &[
    (
        "one journal operation per service call",
        |_, _, after| !matches!(after.returned, Returned::Worked { operations, .. } if operations > 1),
    ),
    (
        "nothing is written after an uncertain append until a scan",
        |before, event, after| {
            !before.state.env.medium.scan_required || !wrote(before, event, after)
        },
    ),
    (
        "the reserved last slot takes only a terminal record",
        |before, event, after| {
            let last = after.state.env.medium.slots[LAST];
            if !wrote(before, event, after) || before.state.env.medium.slots[LAST] == last {
                return true;
            }
            erased(&last)
                || decode(&Slot(last)).is_some_and(|entry| {
                    matches!(entry.kind, Kind::Finish | Kind::Recovered | Kind::Full)
                })
        },
    ),
    (
        "nothing is written in needs_init, foreign, corrupt, unsupported, unavailable or error",
        |before, event, after| {
            !matches!(
                before.status(),
                Status::NeedsInit
                    | Status::Foreign
                    | Status::Corrupt
                    | Status::Unsupported
                    | Status::Unavailable
                    | Status::Error
            ) || !wrote(before, event, after)
        },
    ),
    (
        "an overdue checkpoint comes before any further append",
        |before, event, after| {
            !before.state.env.medium.overdue()
                || event.writes_without_the_recorder()
                || !appended(before, after)
        },
    ),
    (
        "no slot is written while a wipe waits",
        |before, event, after| {
            !wipe_waits(before)
                || event.writes_without_the_recorder()
                || before.state.env.medium.slots == after.state.env.medium.slots
        },
    ),
    (
        "a waiting wipe ends only by wiped, wipe_failed, a cut or an error",
        |before, event, after| {
            !wipe_waits(before)
                || wipe_waits(after)
                || after.status() == Status::Error
                || matches!(
                    event,
                    Event::Wiped | Event::WipeFailed { .. } | Event::Cut(_)
                )
        },
    ),
    (
        "a waiting wipe's result says whether it wiped",
        |before, event, after| {
            let Some(Pending::Wipe(token)) = before.state.recorder.pending else {
                return true;
            };
            let ok = match event {
                Event::Wiped => true,
                Event::WipeFailed { .. } => false,
                _ => return true,
            };
            after
                .state
                .recorder
                .result
                .is_some_and(|result| (result.action, result.token, result.ok) == (None, token, ok))
        },
    ),
    ("wiped leaves an empty ready journal", |_, event, after| {
        !matches!(event, Event::Wiped)
            || (after.status() == Status::Ready
                && after.state.recorder.catalog.next_slot == 0
                && after.state.env.medium.head() == 0)
    }),
    (
        "foreign is left only by a reboot or a wipe",
        |before, event, after| {
            before.status() != Status::Foreign
                || after.status() == Status::Foreign
                || matches!(event, Event::Reboot { .. } | Event::Wiped | Event::Cut(_))
        },
    ),
    (
        "corrupt is left only by a reboot or an initialize",
        |before, event, after| {
            before.status() != Status::Corrupt
                || after.status() == Status::Corrupt
                || matches!(event, Event::Reboot { .. } | Event::Initialize(_))
        },
    ),
    (
        "unsupported is left only by a reboot",
        |before, event, after| {
            before.status() != Status::Unsupported
                || after.status() == Status::Unsupported
                || matches!(event, Event::Reboot { .. })
        },
    ),
];

fn scanned(before: &World, after: &World, status: Status) -> bool {
    before.status() == Status::Scanning && after.status() == status
}

pub(super) const SCAN_ANSWER_PROBES: &[Legal<World, Event>] = &[
    ("a scan answers more", |before, event, after| {
        !(matches!(event, Event::Service(_)) && scanned(before, after, Status::Scanning))
    }),
    (
        "a scan answers foreign, with no catalog",
        |before, _, after| {
            !(scanned(before, after, Status::Foreign)
                && after.state.env.medium.volume == Volume::Foreign)
        },
    ),
    (
        "a scan answers corrupt, with no catalog",
        |before, _, after| {
            !(scanned(before, after, Status::Corrupt)
                && after.state.env.medium.volume == Volume::Corrupt)
        },
    ),
];

fn flushed(before: &World, after: &World) -> bool {
    after.state.recorder.written_samples
        == before.state.recorder.written_samples + SAMPLES_PER_BATCH as u32
}

pub(super) const FULL_BATCH: &[Legal<World, Event>] = &[
    (
        "a full batch is flushed while recording",
        |before, _, after| {
            !(before.status() == Status::Recording
                && before.state.recorder.pending.is_none()
                && flushed(before, after))
        },
    ),
    ("a sample is dropped", |before, _, after| {
        after.state.recorder.dropped_samples <= before.state.recorder.dropped_samples
    }),
    (
        "a stop's lap samples fill the batch with a lap still waiting",
        |before, _, after| {
            !(before.state.recorder.stopping()
                && flushed(before, after)
                && after.state.recorder.lap_marks.lap_waiting())
        },
    ),
];
