//! `trace shrink`: a fixture's slice made as small as it goes while its
//! expectation still fails the same way. One [`ddmin`] runs at each level,
//! each on what the level before kept:
//!
//! - outputs: what the recording showed (pages, frames, health and the
//!   like), which no replay reads;
//! - passes, and the commands between them: the cheap, big cut;
//! - input records in the passes that are left: the minimum.
//!
//! A candidate is reproduced when every expectation fails, passing when one
//! holds, and invalid when the replay no longer reaches the moment. Only the
//! structure the recorder writes is fixed by hand: media records stay (a
//! digest needs the read that carried its bytes, and a part goes on the read
//! before it), a `Repeated` goes with the record before it, and a request
//! that refreshes takes the observations recorded after it. Replay finds
//! every other dependency by classing the candidate invalid.
use super::fixture::{Class, Fixture, judge};
use crate::ddmin::ddmin;
use device_api::capability::Kind as Declared;
use firmware_services::trace::{self, Event, Header};
use serde::Serialize;
use std::fmt;
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Size {
    pub bytes: usize,
    pub passes: usize,
    pub records: usize,
}

impl Size {
    pub fn of(trace: &[u8]) -> Self {
        let Ok((_, events)) = trace::decode(trace) else {
            return Self::default();
        };
        let mut size = Self {
            bytes: trace.len(),
            ..Self::default()
        };
        for event in events {
            match event {
                Event::Pass { .. } => size.passes += 1,
                Event::Call(_) => size.records += 1,
                _ => {}
            }
        }
        size
    }
}

impl fmt::Display for Size {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} bytes, {} passes, {} records",
            self.bytes, self.passes, self.records
        )
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Level {
    pub name: &'static str,
    pub units: usize,
    pub kept: usize,
    pub minimal: bool,
    pub tried: usize,
    pub ms: u128,
}

#[derive(Clone, Debug, Serialize)]
pub struct Shrunk {
    pub before: Size,
    pub after: Size,
    pub levels: Vec<Level>,
}

impl fmt::Display for Shrunk {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "before {}", self.before)?;
        for level in &self.levels {
            writeln!(
                f,
                "  {}: kept {} of {} in {} tries, {:.1} s{}",
                level.name,
                level.kept,
                level.units,
                level.tried,
                level.ms as f64 / 1000.0,
                if level.minimal {
                    ", 1-minimal"
                } else {
                    ", cut short by the budget"
                }
            )?;
        }
        writeln!(f, "after  {}", self.after)
    }
}

/// Shrink the fixture at `dir` within `budget`, rewriting it at every step
/// that still reproduces.
pub fn shrink(dir: &Path, budget: Duration) -> Result<Shrunk, String> {
    let deadline = Instant::now() + budget;
    let mut fixture = Fixture::read(dir)?;
    let verdict = fixture.judge();
    if verdict.class() != Class::Reproduced {
        return Err(format!(
            "{}: the fixture does not fail as cut, so there is nothing to shrink: {verdict}",
            dir.display()
        ));
    }
    let before = Size::of(&fixture.slice);
    let mut levels = Vec::new();
    for (name, units, share) in [
        (
            "outputs",
            outputs as fn(&[Event<'_>]) -> Units,
            Share::AllLeft,
        ),
        ("passes", passes, Share::HalfLeftSoRecordsGetATurn),
        ("records", records, Share::AllLeft),
    ] {
        let started = Instant::now();
        let level_deadline = started + share.of(deadline.saturating_duration_since(started));
        let slice = fixture.slice.clone();
        let (header, events) = decoded(&slice)?;
        let Units {
            fixed,
            removable: units,
        } = units(&events);
        let expect = fixture.expect.clone();
        let mut written = Ok(());
        let shrunk = ddmin(units.clone(), Some(level_deadline), |kept| {
            let slice = encode(&header, &events, &fixed, kept);
            let reproduced = judge(&slice, &expect).class() == Class::Reproduced;
            if reproduced && written.is_ok() {
                let candidate = Fixture {
                    slice,
                    expect: expect.clone(),
                };
                written = candidate.write(dir);
            }
            reproduced
        });
        written?;
        fixture.slice = encode(&header, &events, &fixed, &shrunk.kept);
        levels.push(Level {
            name,
            units: units.len(),
            kept: shrunk.kept.len(),
            minimal: shrunk.minimal,
            tried: shrunk.tried,
            ms: started.elapsed().as_millis(),
        });
    }
    Ok(Shrunk {
        before,
        after: Size::of(&fixture.slice),
        levels,
    })
}

#[derive(Clone, Copy)]
enum Share {
    AllLeft,
    HalfLeftSoRecordsGetATurn,
}

impl Share {
    fn of(self, left: Duration) -> Duration {
        match self {
            Share::AllLeft => left,
            Share::HalfLeftSoRecordsGetATurn => left / 2,
        }
    }
}

#[derive(Debug)]
struct Units {
    fixed: Vec<usize>,
    removable: Vec<Unit>,
}

type Unit = Vec<usize>;

fn decoded(trace: &[u8]) -> Result<(Header<'_>, Vec<Event<'_>>), String> {
    let (header, events) = trace::decode(trace).map_err(|e| e.to_string())?;
    Ok((header, events.collect()))
}

fn encode(header: &Header<'_>, events: &[Event<'_>], fixed: &[usize], kept: &[Unit]) -> Vec<u8> {
    let mut indices: Vec<usize> = (fixed.iter().copied())
        .chain(kept.iter().flatten().copied())
        .collect();
    indices.sort_unstable();
    super::surgery::encode(header, indices.into_iter().map(|at| events[at]))
}

/// What a replay reads of a trace: everything but what the recording
/// showed and measured.
pub fn read_by_replay(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Pass { .. }
            | Event::Call(_)
            | Event::Forget(_)
            | Event::Repeated { .. }
            | Event::Session { .. }
            | Event::NetworkTime(_)
            | Event::Command(_)
            | Event::TooLarge { .. }
            | Event::Lost { .. }
            | Event::PoweredOff
    )
}

fn outputs(events: &[Event<'_>]) -> Units {
    let (fixed, shown): (Vec<usize>, Vec<usize>) =
        (0..events.len()).partition(|&at| read_by_replay(&events[at]));
    let removable = match shown.is_empty() {
        true => Vec::new(),
        false => vec![shown],
    };
    Units { fixed, removable }
}

fn passes(events: &[Event<'_>]) -> Units {
    let starts: Vec<usize> = (0..events.len())
        .filter(|&at| matches!(events[at], Event::Pass { .. } | Event::Command(_)))
        .collect();
    let Some(&judged_pass) = starts.last() else {
        return Units {
            fixed: (0..events.len()).collect(),
            removable: Vec::new(),
        };
    };
    let boot = 0..starts[0];
    let removable = starts.windows(2).map(|w| (w[0]..w[1]).collect()).collect();
    let fixed = boot.chain(judged_pass..events.len()).collect();
    Units { fixed, removable }
}

fn records(events: &[Event<'_>]) -> Units {
    let mut units: Vec<Unit> = Vec::new();
    let mut fixed = Vec::new();
    let mut refreshing: Option<(u8, usize)> = None;
    for (at, event) in events.iter().enumerate() {
        let joined = match event {
            Event::Pass { .. } | Event::Command(_) => {
                refreshing = None;
                false
            }
            Event::Repeated { method, .. } => units.last().is_some_and(|last| {
                last.last() == Some(&(at - 1))
                    && matches!(events[at - 1], Event::Call(call) if call.method == *method)
            }),
            Event::Call(call) => match declared(call.method) {
                Some(Declared::Observe(_)) => {
                    refreshing.is_some_and(|(capability, unit)| {
                        capability == call.method.capability && unit + 1 == units.len()
                    }) && units
                        .last()
                        .is_some_and(|last| last.last() == Some(&(at - 1)))
                }
                _ => false,
            },
            _ => false,
        };
        if joined {
            units.last_mut().expect("a unit to join").push(at);
            continue;
        }
        let Event::Call(call) = event else {
            fixed.push(at);
            continue;
        };
        match declared(call.method) {
            Some(Declared::Media(_)) | None => fixed.push(at),
            Some(kind) => {
                refreshing = match kind {
                    Declared::Take { refreshes: true }
                    | Declared::Request {
                        refreshes: true, ..
                    }
                    | Declared::Sink { refreshes: true } => {
                        Some((call.method.capability, units.len()))
                    }
                    _ => refreshing.filter(|(capability, _)| *capability != call.method.capability),
                };
                units.push(vec![at]);
            }
        }
    }
    Units {
        fixed,
        removable: units,
    }
}

fn declared(method: device_api::journal::MethodId) -> Option<Declared> {
    crate::twin::script::declared(method).map(|(_, method)| method.kind)
}

#[cfg(test)]
mod tests;
