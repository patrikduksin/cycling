//! Record surgery: a trace rewritten event by event, as the device's
//! recorder writes it, to replay a world that differs in one place.
use crate::replay::Blocks;
use crate::trace::timeline::{self, Kind};
use device_api::journal::MethodId;
use firmware_services::trace::recorder::{Record, Recorder};
use firmware_services::trace::{self, Event, Header};

/// `events` under `header`, encoded as the recorder encodes them.
pub fn encode<'a>(header: &Header<'_>, events: impl IntoIterator<Item = Event<'a>>) -> Vec<u8> {
    let mut file = vec![0u8; trace::MAX_BLOCK];
    let len = trace::encode_header(header, &mut file).expect("a header fits a block");
    file.truncate(len);
    let buffer = vec![0u8; trace::MAX_BLOCK].into_boxed_slice();
    let mut recorder = Recorder::new(buffer, Blocks(file));
    for event in events {
        recorder.record(&event);
    }
    let mut bytes = recorder.sink().0.clone();
    bytes.extend_from_slice(recorder.open_block());
    bytes
}

/// One `--set T:<name>[#n]=<answer>`: the `nth` record named so in the
/// first pass at or after `at_ms` that holds that many, removed or given
/// another body.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Set {
    pub at_ms: u64,
    pub of: Of,
    pub nth: usize,
    pub answer: Answer,
}

/// Which records a set counts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Of {
    /// Those `trace timeline` shows under a kind, as it names it:
    /// `ant.request`.
    Kind(Kind),
    /// Those of one method, as a replay's stop names it:
    /// `Ant::take_packet`.
    Method(MethodId),
}

impl Of {
    fn holds(self, call: &trace::Call<'_>) -> bool {
        match self {
            Self::Method(method) => call.method == method,
            Self::Kind(kind) => {
                crate::trace::world::read(call).and_then(|world| timeline::kind(&world))
                    == Some(kind)
            }
        }
    }
}

impl std::fmt::Display for Of {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Kind(kind) => f.write_str(kind.name()),
            Self::Method(method) => write!(f, "{}", crate::twin::script::Named(*method)),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Answer {
    Drop,
    /// The record's whole body, arguments first.
    Body(Vec<u8>),
}

impl Set {
    /// `12:30.250:ant.request#1=drop`, or `=<hex>` for another body; a
    /// method's name, `Ant::request`, also serves. `session_ms` reads the
    /// time.
    pub fn parse(
        text: &str,
        session_ms: impl FnOnce(&str) -> Result<u64, String>,
    ) -> Result<Self, String> {
        let shape = || format!("--set takes T:<kind>[#n]=drop|<hex>, not {text:?}");
        let (place, answer) = text.split_once('=').ok_or_else(shape)?;
        let before_name = place
            .find("::")
            .map_or(place, |method_at| &place[..method_at]);
        let time_end = before_name.rfind(':').ok_or_else(shape)?;
        let (named, nth) = match place[time_end + 1..].split_once('#') {
            Some((named, nth)) => (named, nth.parse().map_err(|_| shape())?),
            None => (&place[time_end + 1..], 0),
        };
        let of = match named.split_once("::") {
            Some((capability, name)) => crate::twin::script::named(capability, name)
                .map(Of::Method)
                .ok_or_else(|| format!("--set: no method {named}"))?,
            None => (Kind::ALL.into_iter().find(|kind| kind.name() == named))
                .map(Of::Kind)
                .ok_or_else(|| {
                    format!("--set: no kind {named:?}; kinds read as trace timeline names them, such as ant.request")
                })?,
        };
        let answer = match answer {
            "drop" => Answer::Drop,
            hex => Answer::Body(unhex(hex).ok_or_else(shape)?),
        };
        Ok(Self {
            at_ms: session_ms(&place[..time_end])?,
            of,
            nth,
            answer,
        })
    }
}

fn unhex(hex: &str) -> Option<Vec<u8>> {
    if !hex.len().is_multiple_of(2) {
        return None;
    }
    (0..hex.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(hex.get(at..at + 2)?, 16).ok())
        .collect()
}

/// `trace` with `set` applied, and the pass it resolved to.
pub fn set(trace: &[u8], set: &Set) -> Result<(Vec<u8>, u64), String> {
    let (header, events) = trace::decode(trace).map_err(|error| error.to_string())?;
    let mut events: Vec<Event<'_>> = events.collect();
    let mut pass_ms = 0;
    let mut found = Vec::new();
    for (at, event) in events.iter().enumerate() {
        match event {
            Event::Pass { now_ms } => {
                if found.len() > set.nth {
                    break;
                }
                pass_ms = *now_ms;
                found.clear();
            }
            Event::Call(call) if pass_ms >= set.at_ms && set.of.holds(call) => {
                found.push(at);
            }
            _ => {}
        }
    }
    let Some(&at) = found.get(set.nth) else {
        return Err(format!(
            "no pass at or after {} holds record #{} of {}",
            timeline::session_clock(set.at_ms),
            set.nth,
            set.of
        ));
    };
    match &set.answer {
        Answer::Drop => {
            events.remove(at);
        }
        Answer::Body(body) => {
            if let Event::Call(call) = &mut events[at] {
                call.body = body;
            }
        }
    }
    Ok((encode(&header, events), pass_ms))
}

#[cfg(test)]
mod tests;
