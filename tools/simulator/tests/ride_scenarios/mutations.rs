use super::*;
use firmware_services::trace::{Event, decode};
use shell_simulator::replay::first_difference;

pub struct Recording {
    pub trace: &'static [u8],
    frames: Vec<u64>,
    backlight: (u8, u32),
}

type Outputs = (Vec<u64>, (u8, u32));

/// Media accesses are checked by rerecording them, so this helper compares frames and backlight
/// state.
fn outputs(sim: &Sim) -> Outputs {
    let power = sim.device.power.0.borrow();
    (sim.frame_hashes(), (power.brightness, power.writes))
}

pub fn record(sim: Sim, scenario: fn(&mut Sim)) -> Recording {
    let mut sim = sim;
    scenario(&mut sim);
    let capture = sim.capture();
    let (frames, backlight) = outputs(&sim);
    Recording {
        trace: Vec::leak(capture),
        frames,
        backlight,
    }
}

/// A world record's method, or another event's name.
pub fn kind(event: &Event<'_>) -> String {
    if let Event::Call(call) = event {
        return shell_simulator::twin::script::Named(call.method).to_string();
    }
    let debug = format!("{event:?}");
    debug
        .split([' ', '(', '{'])
        .next()
        .unwrap_or_default()
        .into()
}

fn replayed(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Call(_)
            | Event::Pass { .. }
            | Event::Command(_)
            | Event::NetworkTime(_)
            | Event::PoweredOff
            | Event::Panel(_)
    )
}

pub fn kinds(trace: &[u8]) -> Vec<String> {
    let (_, events) = decode(trace).unwrap();
    let kinds: std::collections::BTreeSet<String> =
        events.filter(replayed).map(|event| kind(&event)).collect();
    kinds.into_iter().collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mutation {
    Drop,
    Alter,
}

fn altered(event: Event<'static>) -> Option<Event<'static>> {
    Some(match event {
        Event::Call(mut call) => {
            let mut body = call.body.to_vec();
            *body.last_mut()? ^= 1;
            call.body = Vec::leak(body);
            Event::Call(call)
        }
        Event::NetworkTime(mut time) => {
            time.unix_seconds = time.unix_seconds.map(|seconds| seconds + 7 * 3_600);
            Event::NetworkTime(time)
        }
        Event::Pass { now_ms } => Event::Pass { now_ms: now_ms + 5 },
        Event::Panel(digest) => Event::Panel(digest ^ 1),
        _ => return None,
    })
}

pub fn mutated(trace: &'static [u8], kind: &str, how: Mutation) -> Option<Vec<u8>> {
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    let of_kind: Vec<usize> = (0..events.len())
        .filter(|&index| self::kind(&events[index]) == kind)
        .collect();
    let at = *of_kind.get(of_kind.len() / 2)?;
    match how {
        Mutation::Drop => {
            events.remove(at);
        }
        Mutation::Alter => {
            let changed = altered(events[at]).filter(|changed| *changed != events[at])?;
            events[at] = changed;
        }
    }
    Some(shell_simulator::trace::surgery::encode(&header, events))
}

pub fn with_too_large(trace: &'static [u8]) -> Vec<u8> {
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    let second = events
        .iter()
        .position(|event| matches!(event, Event::Pass { .. }))
        .unwrap()
        + 1;
    let (capability, operation) = Event::Command("").id();
    events.insert(
        second,
        Event::TooLarge {
            capability,
            operation,
        },
    );
    shell_simulator::trace::surgery::encode(&header, events)
}

#[derive(Debug)]
pub struct Caught {
    pub report: Option<String>,
    pub visible: bool,
    pub frames: bool,
    pub handed: bool,
}

impl Caught {
    pub fn any(&self) -> bool {
        self.report.is_some() || self.visible || self.handed
    }
}

pub fn replay(recording: &Recording, trace: Vec<u8>) -> Caught {
    let faithful = shell_simulator::replay::Options::default();
    let mut replayed = match Sim::replaying(&trace, faithful, None) {
        Ok(replayed) => replayed,
        Err(boot) => {
            return Caught {
                report: Some(boot.to_string()),
                visible: false,
                frames: false,
                handed: false,
            };
        }
    };
    replayed.hash_frames();
    replayed.replay_until(u64::MAX);
    let report = replayed.device.report().unwrap();
    let report = report
        .first
        .map(|first| format!("{:?} at {} ms", first.kind, first.at_ms));
    let outputs = outputs(&replayed);
    let frames = shown(&outputs.0) != shown(&recording.frames);
    let visible = outputs != (recording.frames.clone(), recording.backlight);
    let handed = first_difference(recording.trace, &replayed.trace()).is_some();
    Caught {
        report,
        visible,
        frames,
        handed,
    }
}

/// What the panel showed, frame by frame, without a frame drawn again
/// as it was.
fn shown(frames: &[u64]) -> Vec<u64> {
    let mut shown = frames.to_vec();
    shown.dedup();
    shown
}

/// Dropping a record can produce a valid trace of a different world, so compare replayed inputs
/// and outputs as well as divergence reports. Check the unmutated trace first to validate the
/// oracle.
#[track_caller]
pub fn assert_every_mutation_shows(recording: &Recording, holds: &[&str]) {
    let clean = replay(recording, recording.trace.to_vec());
    assert!(!clean.any(), "the unmutated trace replays clean: {clean:?}");
    let kinds = kinds(recording.trace);
    let absent: Vec<&&str> = (holds.iter())
        .filter(|kind| !kinds.iter().any(|held| held == **kind))
        .collect();
    assert!(
        absent.is_empty(),
        "the trace holds no {absent:?}: {kinds:?}"
    );
    let (mut missed, mut silent) = (Vec::new(), Vec::new());
    for kind in &kinds {
        for how in [Mutation::Drop, Mutation::Alter] {
            let Some(trace) = mutated(recording.trace, kind, how) else {
                continue;
            };
            let caught = replay(recording, trace);
            if caught.frames && caught.report.is_none() {
                silent.push((kind, how));
            }
            if !caught.any() {
                missed.push((kind, how));
            }
        }
    }
    assert!(missed.is_empty(), "mutations that did not show: {missed:?}");
    assert!(
        silent.is_empty(),
        "replays that showed other frames and reported nothing: {silent:?}"
    );
}
