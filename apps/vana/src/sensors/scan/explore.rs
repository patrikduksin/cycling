use super::*;
use crate::testing::{Holds, Legal, explore};
use core::hash::{Hash, Hasher};
use std::vec::Vec;
use std::{format, vec};

const TIMES: [u64; 5] = [
    0,
    1_000,
    CANCEL_DEADLINE_MS,
    START_DEADLINE_MS,
    START_DEADLINE_MS + CANCEL_DEADLINE_MS,
];
const RADIO: [ScanState; 5] = [
    ScanState::Idle,
    ScanState::Starting,
    ScanState::Active,
    ScanState::Stopping,
    ScanState::Uncertain,
];
const AVAILABILITY: [Availability; 3] = [
    Availability::Ready,
    Availability::Initializing,
    Availability::Failed,
];
const LINKS: [Links; 3] = [Links::Free, Links::Settling, Links::Held];
const ANSWERS: [Result<Admission, Error>; 5] = [
    Ok(Admission::Accepted),
    Err(Error::Busy),
    Err(Error::Unavailable),
    Err(Error::Uncertain),
    Err(Error::InvalidState),
];
const BUDGET: usize = 1_000_000;

#[derive(Clone, Copy, Debug)]
enum Step {
    Rider(Event),
    Answer(Result<Admission, Error>),
    Radio(ScanState),
    Availability(Availability),
    Links(Links),
    At(u64),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct World {
    scan: Scan,
    seen: Seen,
    now: u64,
    asked: Option<Op>,
    requests_this_event: u8,
}

impl Hash for World {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.scan.hash(state);
        (self.seen.radio as u8, self.seen.availability as u8).hash(state);
        self.seen.links.hash(state);
        (self.now, self.asked, self.requests_this_event).hash(state);
    }
}

fn alphabet(world: &World) -> Vec<Step> {
    let mut steps: Vec<Step> = match world.asked {
        Some(_) => ANSWERS.map(Step::Answer).to_vec(),
        None => {
            let mut rider = vec![
                Step::Rider(Event::Start),
                Step::Rider(Event::Cancel),
                Step::Rider(Event::Tick),
            ];
            rider.extend(
                TIMES
                    .iter()
                    .filter(|&&at| at > world.now)
                    .map(|&at| Step::At(at)),
            );
            rider
        }
    };
    steps.extend(RADIO.map(Step::Radio));
    steps.extend(AVAILABILITY.map(Step::Availability));
    steps.extend(LINKS.map(Step::Links));
    steps
}

fn next(world: &World, step: Step) -> World {
    let mut after = *world;
    let stepped = |after: &mut World, event| after.scan.step(event, &after.seen, after.now);
    match step {
        Step::Rider(event) => {
            after.asked = stepped(&mut after, event);
            after.requests_this_event = u8::from(after.asked.is_some());
        }
        Step::Answer(result) => {
            let op = world.asked.expect("an answer follows a request");
            after.asked = stepped(&mut after, Event::Answer(op, result));
            after.requests_this_event += u8::from(after.asked.is_some());
        }
        Step::Radio(radio) => after.seen.radio = radio,
        Step::Availability(availability) => after.seen.availability = availability,
        Step::Links(links) => after.seen.links = links,
        Step::At(now) => after.now = now,
    }
    after
}

fn asks(step: Step, after: &World) -> Option<Op> {
    matches!(step, Step::Rider(_) | Step::Answer(_))
        .then_some(after.asked)
        .flatten()
}

const HOLDS: &[Holds<World>] = &[
    ("a start is asked only for a waiting search", |world| {
        world.asked != Some(Op::Start)
            || (world.scan.pending_start.is_some() && !world.scan.running())
    }),
    ("a stop is asked only while cancelling", |world| {
        world.asked != Some(Op::Stop) || world.scan.phase == Phase::Cancelling
    }),
    ("one event asks at most a stop and a start", |world| {
        world.requests_this_event <= 2
    }),
];

const LEGAL: &[Legal<World, Step>] = &[
    (
        "a start goes only to an idle, ready, free radio",
        |before, step, after| {
            asks(step, after) != Some(Op::Start)
                || before.seen
                    == (Seen {
                        radio: ScanState::Idle,
                        availability: Availability::Ready,
                        links: Links::Free,
                    })
        },
    ),
    (
        "a stop goes only to a radio that is searching",
        |before, step, after| {
            asks(step, after) != Some(Op::Stop)
                || matches!(
                    before.seen.radio,
                    ScanState::Starting | ScanState::Active | ScanState::Stopping
                )
        },
    ),
    ("an answered start ends the event", |before, step, after| {
        !(before.asked == Some(Op::Start) && matches!(step, Step::Answer(_)))
            || after.asked.is_none()
    }),
    ("a busy start keeps waiting", |before, step, after| {
        !(before.asked == Some(Op::Start) && matches!(step, Step::Answer(Err(Error::Busy))))
            || after.scan == before.scan
    }),
    (
        "any other answer to a start ends the wait",
        |before, step, after| {
            !(before.asked == Some(Op::Start)
                && matches!(step, Step::Answer(result) if result != Err(Error::Busy)))
                || after.scan.pending_start.is_none()
        },
    ),
    (
        "a waiting start is asked again on a tick",
        |before, step, after| {
            let free = before.seen
                == (Seen {
                    radio: ScanState::Idle,
                    availability: Availability::Ready,
                    links: Links::Free,
                });
            let waiting = before
                .scan
                .pending_start
                .is_some_and(|deadline| before.now < deadline);
            !(matches!(step, Step::Rider(Event::Tick)) && free && waiting && !before.scan.running())
                || after.asked == Some(Op::Start)
        },
    ),
    (
        "a waiting start keeps its deadline",
        |before, step, after| {
            !matches!(step, Step::Rider(Event::Start) | Step::Rider(Event::Tick))
                || before.scan.pending_start.is_none()
                || after.scan.pending_start.is_none()
                || after.scan.pending_start == before.scan.pending_start
        },
    ),
    ("a cancel drops the waiting start", |_, step, after| {
        !matches!(step, Step::Rider(Event::Cancel)) || after.scan.pending_start.is_none()
    }),
    (
        "a busy stop is retried only until its deadline",
        |before, step, after| {
            !(before.asked == Some(Op::Stop) && matches!(step, Step::Answer(Err(Error::Busy))))
                || (after.scan.phase == Phase::Cancelling)
                    == (before.now < before.scan.cancel_deadline)
        },
    ),
    (
        "an uncertain radio is never asked",
        |before, step, after| {
            asks(step, after).is_none() || before.seen.radio != ScanState::Uncertain
        },
    ),
];

#[test]
fn the_search_closes_and_keeps_every_rule() {
    let inits: Vec<World> = RADIO
        .iter()
        .flat_map(|&radio| {
            AVAILABILITY.iter().flat_map(move |&availability| {
                LINKS.map(move |links| World {
                    scan: Scan::default(),
                    seen: Seen {
                        radio,
                        availability,
                        links,
                    },
                    now: 0,
                    asked: None,
                    requests_this_event: 0,
                })
            })
        })
        .collect();
    let report = explore(
        &inits,
        alphabet,
        next,
        |world| world.scan.status().name(),
        HOLDS,
        LEGAL,
        BUDGET,
    );
    let broken: Vec<_> = report
        .broken
        .iter()
        .map(|broken| (broken.rule, inits[broken.init_index], broken.path.clone()))
        .collect();
    let text = format!(
        "{} states, closed {}, reached {:?}, broken {broken:#?}",
        report.states, report.closed, report.reached
    );
    std::println!("{text}");
    assert!(report.closed, "{text}");
    assert!(broken.is_empty(), "{text}");
    for status in Status::ALL {
        assert!(
            report.reached.contains(&status.name()),
            "{status:?} unreached: {text}"
        );
    }
}
