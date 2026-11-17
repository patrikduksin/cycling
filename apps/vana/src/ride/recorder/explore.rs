use super::invariants::Rule;
use super::{Action, Recorder, Refused, ResultEvent};
use crate::ride::log::{HISTORY_CAPACITY, LapStart, SAMPLES_PER_BATCH, Status, Summary};
use crate::testing::{Legal, Report, explore};
use core::cell::Cell;
use core::fmt::Debug;
use core::hash::{Hash, Hasher};
use medium::{Answer, Answering, Medium, Volume};
use std::format;
use std::string::String;
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

#[derive(Clone, Copy, Debug)]
enum Event {
    Request { action: Action, token: u32 },
    Initialize(u32),
    Clear { token: u32, past_next_slot: bool },
    MarkLap,
    TakeResult,
    Service(Answer),
    Advance,
    Reboot { tear: bool },
    AwaitWipe { token: u32, past_next_slot: bool },
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

#[derive(Clone, Copy, Debug)]
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
        } = self;
        (medium, time_index, clears, laps, tore, wipe).hash(state);
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
            Wipe::Running | Wipe::Idle => None,
        }
    }

    fn alphabet(&self, status: Status) -> Vec<Event> {
        let bounds = self.bounds;
        let mut events = vec![Event::TakeResult];
        if self.time_index + 1 < bounds.times_ms.len() {
            events.push(Event::Advance);
        }
        if let Some(held) = self.events_while_format_holds_storage(status) {
            events.extend(held);
            return events;
        }
        if self.wipe == Wipe::Running {
            events.extend(WIPE_ENDINGS);
            events.extend([Cut::BeforeIntent, Cut::BeforeErase, Cut::AfterErase].map(Event::Cut));
        } else {
            events.push(Event::Reboot { tear: false });
            if !self.tore && self.medium.tearable.is_some() {
                events.push(Event::Reboot { tear: true });
            }
            let format_offered = status == Status::Foreign && self.clears < bounds.max_clears;
            if format_offered {
                events.push(Event::Format);
            }
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
                    events.push(Event::AwaitWipe {
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
            Event::AwaitWipe { .. } if accepted => {
                self.clears += 1;
                self.wipe = Wipe::Running;
            }
            Event::MarkLap => self.laps += 1,
            Event::Advance => self.time_index += 1,
            Event::Reboot { tear } => {
                if tear {
                    self.medium.tear();
                    self.tore = true;
                }
                self.wipe = Wipe::Idle;
                self.medium.power_on();
            }
            Event::Format => {
                self.clears += 1;
                self.wipe = Wipe::Running;
            }
            Event::Wiped => {
                self.medium.wipe();
                self.wipe = Wipe::Idle;
            }
            Event::WipeFailed { erased } => {
                if erased {
                    self.medium.wipe();
                }
                self.wipe = Wipe::Failed;
            }
            Event::Cut(cut) => {
                if matches!(cut, Cut::AfterErase) {
                    self.medium.wipe();
                }
                self.wipe = match cut {
                    Cut::BeforeIntent => Wipe::Idle,
                    Cut::BeforeErase | Cut::AfterErase => Wipe::Resuming,
                };
                self.medium.power_on();
            }
            Event::Request { .. }
            | Event::Initialize(_)
            | Event::AwaitWipe { .. }
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
    Worked { worked: bool, operations: u8 },
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
        Event::AwaitWipe {
            token,
            past_next_slot,
        } => {
            let upper = recorder.next_slot() + usize::from(past_next_slot);
            Returned::Refused(recorder.await_wipe(upper, token).err())
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
            let worked = recorder.service(&mut journal, now, medium::SAMPLE, &clock);
            Returned::Worked {
                worked,
                operations: journal.operations,
            }
        }
        Event::Reboot { .. } | Event::Cut(_) => {
            *recorder = Recorder::default();
            Returned::Nothing
        }
        Event::Wiped => {
            recorder.wiped();
            Returned::Nothing
        }
        Event::WipeFailed { .. } => {
            recorder.wipe_failed();
            Returned::Nothing
        }
        Event::Advance | Event::Format => Returned::Nothing,
    };
    env.settle(event, returned);
    returned
}

#[derive(Debug, PartialEq)]
struct Observed {
    status: &'static str,
    pending_action: Option<Action>,
    stopping: bool,
    riding: bool,
    ride_open: bool,
    active_ms: [u64; 3],
    sample_due_ms: Option<u64>,
    exportable: bool,
    recovering: bool,
    busy: bool,
    summaries: [Option<Summary>; HISTORY_CAPACITY],
    summary_by_index: [Option<Summary>; HISTORY_CAPACITY + 1],
    summary_count: u8,
    completed: u16,
    written_samples: u32,
    dropped_samples: u32,
    next_slot: usize,
    slots: usize,
    remaining_secs: u32,
    max_write_ms: u32,
    max_erase_ms: u32,
    stop_requested: bool,
}

fn observe(recorder: &Recorder, now: u64) -> Observed {
    Observed {
        status: recorder.status().name(),
        pending_action: recorder.pending_action(),
        stopping: recorder.stopping(),
        riding: recorder.riding(),
        ride_open: recorder.ride_open(),
        active_ms: [now, now + 1_000, now + 3_600_000].map(|at| recorder.active_ms(at)),
        sample_due_ms: recorder.sample_due_ms(),
        exportable: recorder.exportable(),
        recovering: recorder.recovering(),
        busy: recorder.busy(),
        summaries: recorder.summaries(),
        summary_by_index: core::array::from_fn(|index| recorder.summary(index).copied()),
        summary_count: recorder.summary_count(),
        completed: recorder.completed(),
        written_samples: recorder.written_samples(),
        dropped_samples: recorder.dropped_samples(),
        next_slot: recorder.next_slot(),
        slots: recorder.slots(),
        remaining_secs: recorder.remaining_secs(),
        max_write_ms: recorder.max_write_ms(),
        max_erase_ms: recorder.max_erase_ms(),
        stop_requested: recorder.stop_requested(),
    }
}

impl Observed {
    fn differences(&self, new: &Self) -> Vec<String> {
        let Self {
            status,
            pending_action,
            stopping,
            riding,
            ride_open,
            active_ms,
            sample_due_ms,
            exportable,
            recovering,
            busy,
            summaries,
            summary_by_index,
            summary_count,
            completed,
            written_samples,
            dropped_samples,
            next_slot,
            slots,
            remaining_secs,
            max_write_ms,
            max_erase_ms,
            stop_requested,
        } = self;
        let pairs: [(&str, &dyn Debug, &dyn Debug); 22] = [
            ("status", status, &new.status),
            ("pending_action", pending_action, &new.pending_action),
            ("stopping", stopping, &new.stopping),
            ("riding", riding, &new.riding),
            ("ride_open", ride_open, &new.ride_open),
            ("active_ms", active_ms, &new.active_ms),
            ("sample_due_ms", sample_due_ms, &new.sample_due_ms),
            ("exportable", exportable, &new.exportable),
            ("recovering", recovering, &new.recovering),
            ("busy", busy, &new.busy),
            ("summaries", summaries, &new.summaries),
            ("summary(index)", summary_by_index, &new.summary_by_index),
            ("summary_count", summary_count, &new.summary_count),
            ("completed", completed, &new.completed),
            ("written_samples", written_samples, &new.written_samples),
            ("dropped_samples", dropped_samples, &new.dropped_samples),
            ("next_slot", next_slot, &new.next_slot),
            ("slots", slots, &new.slots),
            ("remaining_secs", remaining_secs, &new.remaining_secs),
            ("max_write_ms", max_write_ms, &new.max_write_ms),
            ("max_erase_ms", max_erase_ms, &new.max_erase_ms),
            ("stop_requested", stop_requested, &new.stop_requested),
        ];
        pairs
            .into_iter()
            .map(|(name, old, new)| (name, format!("{old:?}"), format!("{new:?}")))
            .filter(|(_, old, new)| old != new)
            .map(|(name, old, new)| format!("{name}: old {old}, new {new}"))
            .collect()
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
}

impl Hash for World {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.state.hash(hasher);
    }
}

impl World {
    fn boot(env: Env) -> Self {
        Self {
            state: State {
                env,
                recorder: Recorder::default(),
            },
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
        world
    }

    fn status(&self) -> Status {
        self.state.recorder.status()
    }

    fn name(&self) -> &'static str {
        self.status().name()
    }
}

#[derive(Clone, Hash)]
struct TwinState {
    env: Env,
    old: Recorder,
    new: Recorder,
}

#[derive(Clone)]
struct Twin {
    state: TwinState,
    differences: Vec<String>,
}

impl Hash for Twin {
    fn hash<H: Hasher>(&self, hasher: &mut H) {
        self.state.hash(hasher);
    }
}

impl Twin {
    fn boot(env: Env) -> Self {
        Self {
            state: TwinState {
                env,
                old: Recorder::default(),
                new: Recorder::default(),
            },
            differences: Vec::new(),
        }
    }

    fn next(&self, event: Event) -> Self {
        let TwinState {
            mut env,
            mut old,
            mut new,
        } = self.state.clone();
        let mut new_env = env.clone();
        let returned = (
            drive(&mut old, &mut env, event),
            drive(&mut new, &mut new_env, event),
        );
        let mut differences = Vec::new();
        if env.medium != new_env.medium {
            differences.push(String::from("the journal's bytes"));
        }
        if returned.0 != returned.1 {
            differences.push(format!(
                "returned: old {:?}, new {:?}",
                returned.0, returned.1
            ));
        }
        let observed = (observe(&old, env.now()), observe(&new, env.now()));
        if observed.0 != observed.1 {
            differences.extend(observed.0.differences(&observed.1));
        }
        Self {
            state: TwinState { env, old, new },
            differences,
        }
    }
}

struct Found {
    statuses: Vec<&'static str>,
    expected_breaks: Vec<&'static str>,
}

fn search_all<S: Clone + Send>(
    media: Vec<(&'static str, Medium)>,
    bounds: &'static Bounds,
    boot: fn(Env) -> S,
    search: impl Fn(S) -> Report<Event> + Sync,
    next: fn(&S, Event) -> S,
    describe_end: fn(&S) -> String,
    expected_breaks: &[&'static str],
) -> Found {
    let started = std::time::Instant::now();
    let reports: Vec<_> = std::thread::scope(|scope| {
        let searches: Vec<_> = media
            .into_iter()
            .map(|(name, medium)| {
                let search = &search;
                let init = boot(Env::boot(bounds, medium));
                scope.spawn(move || (name, init.clone(), search(init)))
            })
            .collect();
        searches
            .into_iter()
            .map(|search| search.join().unwrap())
            .collect()
    });
    let mut found = Found {
        statuses: Vec::new(),
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
                .fold(init.clone(), |state, &event| next(&state, event));
            panic!(
                "{medium}: {} ({}) after {:?}",
                broken.rule,
                describe_end(&end),
                broken.path
            );
        }
        found.statuses.extend(report.reached);
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
        World::boot,
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
        World::next,
        |world| match Rule::of(&world.state.recorder) {
            Some(rule) => format!("broken: {rule}"),
            None => format!("{:?}", world.returned),
        },
        &names,
    );
    for name in names {
        assert!(
            found.expected_breaks.contains(&name),
            "never reached: {name}"
        );
    }
    found.statuses
}

fn agree_on_every_edge(media: Vec<(&'static str, Medium)>, bounds: &'static Bounds, budget: usize) {
    const AGREE: &[Legal<Twin, Event>] = &[("old and new agree", |_, _, after| {
        after.differences.is_empty()
    })];
    search_all(
        media,
        bounds,
        Twin::boot,
        |twin| {
            explore(
                &[twin],
                |twin| twin.state.env.alphabet(twin.state.old.status()),
                Twin::next,
                |twin| twin.state.old.status().name(),
                &[],
                AGREE,
                budget,
            )
        },
        Twin::next,
        |twin| twin.differences.join("; "),
        &[],
    );
}

const STATUSES: [Status; 15] = {
    const fn _every_status_is_in_statuses(status: Status) {
        match status {
            Status::Scanning
            | Status::NeedsInit
            | Status::Foreign
            | Status::Formatting
            | Status::Clearing
            | Status::Ready
            | Status::Recording
            | Status::Paused
            | Status::Saved
            | Status::Recovered
            | Status::Full
            | Status::Error
            | Status::Unsupported
            | Status::Unavailable
            | Status::Corrupt => {}
        }
    }
    [
        Status::Scanning,
        Status::NeedsInit,
        Status::Foreign,
        Status::Formatting,
        Status::Clearing,
        Status::Ready,
        Status::Recording,
        Status::Paused,
        Status::Saved,
        Status::Recovered,
        Status::Full,
        Status::Error,
        Status::Unsupported,
        Status::Unavailable,
        Status::Corrupt,
    ]
};

fn keeps_the_rules_from_every_medium(bounds: &'static Bounds, budget: usize) {
    let reached = keeps_the_rules(
        medium::media().into(),
        bounds,
        budget,
        rules::SCAN_ANSWER_PROBES,
    );
    for status in STATUSES {
        assert!(
            reached.contains(&status.name()),
            "{} never reached",
            status.name()
        );
    }
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
fn every_state_of_todays_recorder_keeps_the_rules() {
    keeps_the_rules_from_every_medium(&QUICK, 1_000_000);
}

#[test]
#[ignore = "about four and a half minutes on twenty cores, 4 GB"]
fn every_state_of_todays_recorder_keeps_the_rules_in_wide_bounds() {
    keeps_the_rules_from_every_medium(&WIDE, 8_000_000);
}

#[test]
#[ignore = "about a minute and a half on one core, 1.2 GB"]
fn every_state_of_todays_recorder_with_a_full_batch_keeps_the_rules() {
    keeps_the_rules(
        vec![("an empty journal", Medium::new(Volume::Formatted, &[]))],
        &BATCH,
        4_000_000,
        rules::FULL_BATCH,
    );
}

#[test]
#[ignore = "evidence that old and new agree on a full batch, run on recorder PRs"]
fn todays_recorder_agrees_with_itself_with_a_full_batch() {
    agree_on_every_edge(
        vec![("an empty journal", Medium::new(Volume::Formatted, &[]))],
        &BATCH,
        4_000_000,
    );
}

#[test]
#[ignore = "evidence for the differential harness, run on request"]
fn todays_recorder_agrees_with_itself_on_every_edge() {
    agree_on_every_edge(medium::media().into(), &QUICK, 1_000_000);
}

#[test]
#[ignore = "about eight minutes on twenty cores, 6 GB"]
fn todays_recorder_agrees_with_itself_on_every_edge_in_wide_bounds() {
    agree_on_every_edge(medium::media().into(), &WIDE, 8_000_000);
}
