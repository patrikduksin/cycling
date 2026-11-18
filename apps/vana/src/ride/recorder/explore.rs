//! Every state the recorder reaches within small bounds, searched with
//! `testing::explore` over a six-slot fake journal (`medium`) whose written
//! bytes are part of each state.
//!
//! `Env` holds everything outside the recorder: that journal, the time, the
//! counts that bound a path (clears and laps), a clear still waiting, a torn
//! write, and FORMAT's wipe.
//! `drive` makes one event's calls into the recorder, and `observe` reads it
//! through public methods only. The rules in `rules` read a `World`: the state
//! with its `Env`, what the call returned and what `observe` saw. Its two
//! reads of private state, `broken` and `lap_waiting`, sit at its top.
//!
//! A scan answers `More` before its last answer, as the real journals do, and
//! two of the starting volumes answer `Foreign` and `Corrupt` with no catalog.
//! A probe is a rule the search must break, because the edge that breaks it
//! reaches what the probe names. `rules::SCAN_ANSWER_PROBES` names those three
//! scan answers. `rules::FULL_BATCH` names a full batch flushed while
//! recording, a dropped sample, and a stop's lap samples filling the batch
//! while a lap still waits.
//!
//! A FORMAT wipe of the journal is an outside event here. It finishes, fails
//! with or without the erase, or is cut and finished at the next boot. While
//! FORMAT holds storage, the search makes only the recorder calls the app
//! makes.
use super::{Action, Recorder, Refused, ResultEvent};
use crate::ride::log::{LapStart, SAMPLES_PER_BATCH, Status};
use crate::testing::{Legal, Report, explore};
use core::cell::Cell;
use core::hash::{Hash, Hasher};
use medium::{Answer, Answering, Medium, Volume};
use std::format;
use std::vec;
use std::vec::Vec;

mod medium;
mod rules;

struct Bounds {
    times_ms: &'static [u64],
    tokens: &'static [u32],
    answers: &'static [Answer],
    max_laps: u8,
    max_clears: u8,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Event {
    Request { action: Action, token: u32 },
    Initialize(u32),
    Clear { token: u32, past_next_slot: bool },
    MarkLap,
    TakeResult,
    Service(Answer),
    Advance,
    Reboot { tear: bool },
    ClearByWipe { token: u32, past_next_slot: bool },
    Format,
    Wiped,
    WipeFailed { erased: bool },
    Cut(Cut),
}

impl Event {
    fn writes_without_the_recorder(self) -> bool {
        matches!(
            self,
            Self::Reboot { .. } | Self::Wiped | Self::WipeFailed { .. } | Self::Cut(_)
        )
    }
}

const WIPE_ENDINGS: [Event; 3] = [
    Event::Wiped,
    Event::WipeFailed { erased: false },
    Event::WipeFailed { erased: true },
];

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum Wipe {
    Idle,
    Running,
    Resuming,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Cut {
    BeforeIntent,
    BeforeErase,
    AfterErase,
}

#[derive(Clone)]
struct Env {
    bounds: &'static Bounds,
    medium: Medium,
    time_index: usize,
    clears: u8,
    laps: u8,
    tore: bool,
    wipe: Wipe,
    clear: Option<u32>,
}

impl Hash for Env {
    fn hash<H: Hasher>(&self, state: &mut H) {
        let Self {
            bounds: _,
            medium,
            time_index,
            clears,
            laps,
            tore,
            wipe,
            clear,
        } = self;
        (medium, time_index, clears, laps, tore, wipe, clear).hash(state);
    }
}

impl Env {
    fn boot(bounds: &'static Bounds, medium: Medium) -> Self {
        Self {
            bounds,
            medium,
            time_index: 0,
            clears: 0,
            laps: 0,
            tore: false,
            wipe: Wipe::Idle,
            clear: None,
        }
    }

    fn now(&self) -> u64 {
        self.bounds.times_ms[self.time_index]
    }

    fn events_while_format_holds_storage(&self, status: Status) -> Option<Vec<Event>> {
        match self.wipe {
            Wipe::Failed => Some(vec![Event::Reboot { tear: false }]),
            Wipe::Resuming => {
                let mut events: Vec<_> = self
                    .bounds
                    .answers
                    .iter()
                    .copied()
                    .map(Event::Service)
                    .collect();
                events.extend([Event::Cut(Cut::BeforeErase), Event::Cut(Cut::AfterErase)]);
                if status != Status::Scanning {
                    events.extend(WIPE_ENDINGS);
                }
                Some(events)
            }
            Wipe::Idle => None,
            Wipe::Running => Some(
                [Cut::BeforeIntent, Cut::BeforeErase, Cut::AfterErase]
                    .map(Event::Cut)
                    .into_iter()
                    .chain(WIPE_ENDINGS)
                    .chain([Event::Reboot { tear: false }])
                    .collect(),
            ),
        }
    }

    fn alphabet(&self, status: Status) -> Vec<Event> {
        let bounds = self.bounds;
        let mut events = if self.wipe == Wipe::Running {
            vec![]
        } else {
            vec![Event::TakeResult]
        };
        if self.time_index + 1 < bounds.times_ms.len() {
            events.push(Event::Advance);
        }
        if let Some(held) = self.events_while_format_holds_storage(status) {
            events.extend(held);
            return events;
        }
        events.push(Event::Reboot { tear: false });
        if !self.tore && self.medium.tearable.is_some() {
            events.push(Event::Reboot { tear: true });
        }
        let format_offered = status == Status::Foreign && self.clears < bounds.max_clears;
        if format_offered {
            events.push(Event::Format);
        }
        for &token in bounds.tokens {
            for action in [Action::Start, Action::Pause, Action::Resume, Action::Finish] {
                events.push(Event::Request { action, token });
            }
            events.push(Event::Initialize(token));
            if self.clears < bounds.max_clears {
                for past_next_slot in [false, true] {
                    events.push(Event::Clear {
                        token,
                        past_next_slot,
                    });
                    events.push(Event::ClearByWipe {
                        token,
                        past_next_slot,
                    });
                }
            }
        }
        if self.laps < bounds.max_laps {
            events.push(Event::MarkLap);
        }
        events.extend(bounds.answers.iter().copied().map(Event::Service));
        events
    }

    fn settle(&mut self, event: Event, returned: Returned) {
        let accepted = returned == Returned::Refused(None);
        match event {
            Event::Clear { .. } => self.clears += u8::from(accepted),
            Event::ClearByWipe { token, .. } if accepted => {
                self.clears += 1;
                self.wipe = Wipe::Running;
                self.clear = Some(token);
            }
            Event::MarkLap => self.laps += 1,
            Event::Advance => self.time_index += 1,
            Event::Reboot { tear } => {
                if tear {
                    self.medium.tear();
                    self.tore = true;
                }
                self.wipe = Wipe::Idle;
                self.clear = None;
                self.medium.power_on();
            }
            Event::Format => {
                self.clears += 1;
                self.wipe = Wipe::Running;
            }
            Event::Wiped => {
                self.medium.wipe();
                self.wipe = Wipe::Idle;
                self.clear = None;
            }
            Event::WipeFailed { erased } => {
                if erased {
                    self.medium.wipe();
                }
                self.wipe = Wipe::Failed;
                self.clear = None;
            }
            Event::Cut(cut) => {
                if matches!(cut, Cut::AfterErase) {
                    self.medium.wipe();
                }
                self.wipe = match cut {
                    Cut::BeforeIntent => Wipe::Idle,
                    Cut::BeforeErase | Cut::AfterErase => Wipe::Resuming,
                };
                self.clear = None;
                self.medium.power_on();
            }
            Event::Request { .. }
            | Event::Initialize(_)
            | Event::ClearByWipe { .. }
            | Event::TakeResult
            | Event::Service(_) => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Returned {
    Nothing,
    Accepted(bool),
    Refused(Option<Refused>),
    Result(Option<ResultEvent>),
    Serviced { operations: u8 },
}

fn clock_one_ms_per_read(now: u64) -> impl Fn() -> u64 {
    let reads = Cell::new(0);
    move || {
        reads.set(reads.get() + 1);
        now + reads.get()
    }
}

fn drive(recorder: &mut Recorder, env: &mut Env, event: Event) -> Returned {
    let now = env.now();
    let returned = match event {
        Event::Request { action, token } => {
            Returned::Accepted(recorder.request(action, now, token))
        }
        Event::Initialize(token) => Returned::Refused(recorder.initialize(token).err()),
        Event::Clear {
            token,
            past_next_slot,
        } => {
            let upper = recorder.next_slot() + usize::from(past_next_slot);
            Returned::Refused(recorder.clear(upper, token).err())
        }
        Event::ClearByWipe { past_next_slot, .. } => {
            let upper = recorder.next_slot() + usize::from(past_next_slot);
            Returned::Refused(recorder.clearable(upper).err())
        }
        Event::MarkLap => {
            recorder.mark_lap(LapStart::Manual);
            Returned::Nothing
        }
        Event::TakeResult => Returned::Result(recorder.take_result()),
        Event::Service(answer) => {
            let mut journal = Answering {
                medium: &mut env.medium,
                answer,
                operations: 0,
            };
            let clock = clock_one_ms_per_read(now);
            recorder.service(&mut journal, now, medium::SAMPLE, &clock);
            Returned::Serviced {
                operations: journal.operations,
            }
        }
        Event::Reboot { .. } | Event::Cut(_) => {
            *recorder = Recorder::default();
            Returned::Nothing
        }
        Event::Wiped => {
            recorder.wiped(env.clear);
            Returned::Nothing
        }
        Event::Advance | Event::Format | Event::WipeFailed { .. } => Returned::Nothing,
    };
    env.settle(event, returned);
    returned
}

#[derive(Clone)]
struct Observed {
    status: Status,
    result: Option<ResultEvent>,
    pending_action: Option<Action>,
    stopping: bool,
    written_samples: u32,
    dropped_samples: u32,
    next_slot: usize,
}

fn observe(recorder: &Recorder) -> Observed {
    Observed {
        status: recorder.status(),
        result: recorder.clone().take_result(),
        pending_action: recorder.pending_action(),
        stopping: recorder.stopping(),
        written_samples: recorder.written_samples(),
        dropped_samples: recorder.dropped_samples(),
        next_slot: recorder.next_slot(),
    }
}

#[derive(Clone, Hash)]
struct State {
    env: Env,
    recorder: Recorder,
}

#[derive(Clone)]
struct World {
    state: State,
    returned: Returned,
    observed: Observed,
}

impl Hash for World {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.state.hash(hasher);
    }
}

impl World {
    fn boot(env: Env) -> Self {
        let recorder = Recorder::default();
        Self {
            observed: observe(&recorder),
            state: State { env, recorder },
            returned: Returned::Nothing,
        }
    }

    fn alphabet(&self) -> Vec<Event> {
        self.state.env.alphabet(self.status())
    }

    fn next(&self, event: Event) -> Self {
        let mut world = self.clone();
        let State { env, recorder } = &mut world.state;
        world.returned = drive(recorder, env, event);
        world.observed = observe(recorder);
        world
    }

    fn status(&self) -> Status {
        self.observed.status
    }

    fn name(&self) -> &'static str {
        self.state.recorder.variant()
    }
}

struct Found {
    variants: Vec<&'static str>,
    expected_breaks: Vec<&'static str>,
}

fn search_all(
    media: Vec<(&'static str, Medium)>,
    bounds: &'static Bounds,
    search: impl Fn(World) -> Report<Event> + Sync,
    expected_breaks: &[&'static str],
) -> Found {
    let started = std::time::Instant::now();
    let reports: Vec<_> = std::thread::scope(|scope| {
        let searches: Vec<_> = media
            .into_iter()
            .map(|(name, medium)| {
                let search = &search;
                let init = World::boot(Env::boot(bounds, medium));
                scope.spawn(move || (name, init.clone(), search(init)))
            })
            .collect();
        searches
            .into_iter()
            .map(|search| search.join().unwrap())
            .collect()
    });
    let mut found = Found {
        variants: Vec::new(),
        expected_breaks: Vec::new(),
    };
    let mut total = 0;
    for (medium, init, report) in reports {
        std::eprintln!("{medium}: {} states", report.states);
        total += report.states;
        assert!(
            report.closed,
            "{medium}: truncated after {} states",
            report.states
        );
        for broken in report.broken {
            if expected_breaks.contains(&broken.rule) {
                std::eprintln!("{medium}: reached {} after {:?}", broken.rule, broken.path);
                found.expected_breaks.push(broken.rule);
                continue;
            }
            let end = broken
                .path
                .iter()
                .fold(init.clone(), |world, &event| world.next(event));
            let end = match rules::broken(&end) {
                Some(rule) => format!("broken: {rule}"),
                None => format!("{:?}", end.returned),
            };
            panic!("{medium}: {} ({end}) after {:?}", broken.rule, broken.path);
        }
        found.variants.extend(report.reached);
    }
    std::eprintln!("total: {total} states in {:?}", started.elapsed());
    found
}

fn keeps_the_rules(
    media: Vec<(&'static str, Medium)>,
    bounds: &'static Bounds,
    budget: usize,
    expected_breaks: &[Legal<World, Event>],
) -> Vec<&'static str> {
    let legal: Vec<_> = rules::LEGAL
        .iter()
        .chain(expected_breaks)
        .copied()
        .collect();
    let names: Vec<_> = expected_breaks.iter().map(|(name, _)| *name).collect();
    let found = search_all(
        media,
        bounds,
        |world| {
            explore(
                &[world],
                World::alphabet,
                World::next,
                World::name,
                rules::HOLDS,
                &legal,
                budget,
            )
        },
        &names,
    );
    for name in names {
        assert!(
            found.expected_breaks.contains(&name),
            "never reached: {name}"
        );
    }
    found.variants
}

fn keeps_the_rules_from_every_medium(bounds: &'static Bounds, budget: usize) {
    let reached = keeps_the_rules(
        medium::media().into(),
        bounds,
        budget,
        rules::SCAN_ANSWER_PROBES,
    );
    let mut reached = reached;
    reached.sort_unstable();
    reached.dedup();
    let mut variants = Recorder::VARIANTS.to_vec();
    variants.sort_unstable();
    assert_eq!(
        reached, variants,
        "every lifecycle variant, and only those, is reached"
    );
}

static QUICK: Bounds = Bounds {
    times_ms: &[0, 1_000],
    tokens: &[1],
    answers: &[Answer::Works, Answer::Fails],
    max_laps: 0,
    max_clears: 2,
};

static WIDE: Bounds = Bounds {
    times_ms: &[0, 1_000],
    tokens: &[1, 2],
    answers: &[Answer::Works, Answer::Fails, Answer::WritesThenFails],
    max_laps: 1,
    max_clears: 2,
};

const BATCH_TIMES: &[u64] = &[0, 1_000, 2_000, 3_000];
const _: () = assert!(BATCH_TIMES.len() == SAMPLES_PER_BATCH);
const LAPS_OUTNUMBERING_A_STOPS_ROOM: u8 = 2;

static BATCH: Bounds = Bounds {
    times_ms: BATCH_TIMES,
    tokens: &[1],
    answers: &[Answer::Works, Answer::Fails],
    max_laps: LAPS_OUTNUMBERING_A_STOPS_ROOM,
    max_clears: 0,
};

#[test]
fn every_state_of_the_recorder_keeps_the_rules() {
    keeps_the_rules_from_every_medium(&QUICK, 1_000_000);
}

#[test]
#[ignore = "about six minutes on twenty cores, 3.5 GB"]
fn every_state_of_the_recorder_keeps_the_rules_in_wide_bounds() {
    keeps_the_rules_from_every_medium(&WIDE, 8_000_000);
}

#[test]
#[ignore = "about a minute on one core, 1.2 GB"]
fn every_state_of_the_recorder_with_a_full_batch_keeps_the_rules() {
    keeps_the_rules(
        vec![("an empty journal", Medium::new(Volume::Formatted, &[]))],
        &BATCH,
        4_000_000,
        rules::FULL_BATCH,
    );
}

#[test]
fn a_checkpoint_due_before_a_wipe_is_written_after_it() {
    let works = Event::Service(Answer::Works);
    let request = |action| Event::Request { action, token: 1 };
    let clear = Event::ClearByWipe {
        token: 1,
        past_next_slot: false,
    };
    let finished_then_wiped = [
        works,
        works,
        request(Action::Start),
        works,
        Event::TakeResult,
        request(Action::Finish),
        works,
        Event::TakeResult,
        clear,
        Event::Wiped,
    ];
    let mut world = World::boot(Env::boot(&QUICK, Medium::new(Volume::Formatted, &[])));
    for event in finished_then_wiped.into_iter().chain([works]) {
        assert!(
            world.alphabet().contains(&event),
            "{event:?} is not offered"
        );
        let after = world.next(event);
        for (rule, holds) in rules::HOLDS {
            assert!(holds(&after), "{rule} after {event:?}");
        }
        for (rule, legal) in rules::LEGAL {
            assert!(legal(&world, event, &after), "{rule} on {event:?}");
        }
        world = after;
    }
    assert_eq!(world.returned, Returned::Serviced { operations: 1 });
    assert_eq!(world.status(), Status::Ready);
    assert_eq!(world.state.env.medium.head(), 0);
    assert_eq!(
        world.state.env.medium.checkpoint.map(|kept| kept.completed),
        Some(0)
    );
}
