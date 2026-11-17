//! One pass of a world log as replay consumes it: every take, request,
//! sink and command the device recorded, keyed by its identity, and the
//! observations each answer brought.
//!
//! An identity is the method, its redacted arguments, its occurrence in
//! the pass and the capability's epoch. The last two are derived, never
//! written to the log: occurrence counts the earlier calls of that method
//! with those arguments, and the epoch counts the capability's earlier calls
//! that refresh it, both in this pass. So a request the twin makes later
//! than the device did still replays, unless a refreshing call of the same
//! capability came between.
use device_api::capability::{self, Capability, Kind};
use std::collections::BTreeMap;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MethodId {
    pub capability: u8,
    pub index: u8,
}

impl MethodId {
    pub fn named(capability: &str, method: &str) -> Option<Self> {
        let table = capability::ALL
            .iter()
            .find(|table| table.name == capability)?;
        let index = table.methods.iter().position(|m| m.name == method)?;
        Some(Self {
            capability: table.id,
            index: index as u8,
        })
    }

    fn table(&self) -> Option<&'static Capability> {
        capability::ALL
            .iter()
            .copied()
            .find(|table| table.id == self.capability)
    }

    fn refreshes(&self) -> bool {
        let method = self
            .table()
            .and_then(|table| table.methods.get(usize::from(self.index)));
        matches!(
            method.map(|method| method.kind),
            Some(
                Kind::Take { refreshes: true }
                    | Kind::Request { refreshes: true }
                    | Kind::Sink { refreshes: true }
            )
        )
    }
}

impl fmt::Display for MethodId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let table = self.table();
        let method = table.and_then(|table| table.methods.get(usize::from(self.index)));
        match (table, method) {
            (Some(table), Some(method)) => write!(f, "{}::{}", table.name, method.name),
            _ => write!(f, "{}.{}", self.capability, self.index),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Identity {
    pub method: MethodId,
    pub args: Vec<u8>,
    pub occurrence: u32,
    pub epoch: u32,
}

impl fmt::Display for Identity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}(", self.method)?;
        for byte in &self.args {
            write!(f, "{byte:02x}")?;
        }
        write!(f, ") #{} in epoch {}", self.occurrence, self.epoch)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Observed {
    pub method: MethodId,
    pub key: Vec<u8>,
    pub value: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct Answer {
    outcome: Vec<u8>,
    observed: Vec<Observed>,
}

/// One record of a pass, in the order the device wrote them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    Called {
        method: MethodId,
        args: Vec<u8>,
        outcome: Vec<u8>,
    },
    Observed(Observed),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    Unanswered(Identity),
    Unconsumed(Vec<Identity>),
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unanswered(identity) => {
                write!(f, "left the recorded world: {identity} has no answer")
            }
            Self::Unconsumed(identities) => {
                write!(f, "left the recorded world: the twin never made")?;
                for (index, identity) in identities.iter().enumerate() {
                    let separator = if index == 0 { " " } else { ", " };
                    write!(f, "{separator}{identity}")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Default)]
struct Counts {
    occurrences: BTreeMap<(MethodId, Vec<u8>), u32>,
    epochs: BTreeMap<u8, u32>,
}

impl Counts {
    fn identity(&self, method: MethodId, args: &[u8]) -> Identity {
        Identity {
            method,
            args: args.to_vec(),
            occurrence: self
                .occurrences
                .get(&(method, args.to_vec()))
                .copied()
                .unwrap_or(0),
            epoch: self.epochs.get(&method.capability).copied().unwrap_or(0),
        }
    }

    fn called(&mut self, method: MethodId, args: &[u8]) {
        *self.occurrences.entry((method, args.to_vec())).or_default() += 1;
        if method.refreshes() {
            *self.epochs.entry(method.capability).or_default() += 1;
        }
    }
}

/// The latest value of each observation, as of the twin's matched calls,
/// and the pass time it was read at. The device records an observation
/// only when it differs from the last one aged to now, so the view carries
/// from one pass to the next, and a value that ages is aged from `at_ms`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View(BTreeMap<(MethodId, Vec<u8>), Seen>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub value: Vec<u8>,
    pub at_ms: u64,
}

impl View {
    pub fn get(&self, method: MethodId, key: &[u8]) -> Option<&Seen> {
        self.0.get(&(method, key.to_vec()))
    }

    fn install(&mut self, observed: Vec<Observed>, at_ms: u64) {
        for Observed { method, key, value } in observed {
            self.0.insert((method, key), Seen { value, at_ms });
        }
    }
}

pub struct Pass {
    answers: BTreeMap<Identity, Answer>,
    recorded: Vec<Identity>,
    counts: Counts,
    view: View,
    now_ms: u64,
}

impl Pass {
    /// An observation rides with the last call before it that refreshes its
    /// capability, which started the epoch it was read in.
    pub fn new(now_ms: u64, mut view: View, records: impl IntoIterator<Item = Record>) -> Self {
        let mut answers = BTreeMap::new();
        let mut recorded = Vec::new();
        let mut counts = Counts::default();
        let mut last = BTreeMap::new();
        for record in records {
            match record {
                Record::Called {
                    method,
                    args,
                    outcome,
                } => {
                    let identity = counts.identity(method, &args);
                    counts.called(method, &args);
                    answers.insert(
                        identity.clone(),
                        Answer {
                            outcome,
                            observed: Vec::new(),
                        },
                    );
                    if method.refreshes() {
                        last.insert(method.capability, identity.clone());
                    }
                    recorded.push(identity);
                }
                Record::Observed(observed) => match last.get(&observed.method.capability) {
                    Some(identity) => answers
                        .get_mut(identity)
                        .expect("the call it follows was just added")
                        .observed
                        .push(observed),
                    None => view.install(vec![observed], now_ms),
                },
            }
        }
        Self {
            answers,
            recorded,
            counts: Counts::default(),
            view,
            now_ms,
        }
    }

    pub fn answer(&mut self, method: MethodId, args: &[u8]) -> Result<Vec<u8>, Stop> {
        self.matched(method, args)
            .ok_or_else(|| Stop::Unanswered(self.counts.identity(method, args)))
    }

    pub fn poll(&mut self, method: MethodId) -> Option<Vec<u8>> {
        self.matched(method, &[])
    }

    pub fn observe(&self, method: MethodId, key: &[u8]) -> Option<&Seen> {
        self.view.get(method, key)
    }

    /// The view the next pass starts from, or the calls the twin never
    /// made, in the order the device made them.
    pub fn end(self) -> Result<View, Stop> {
        if self.answers.is_empty() {
            return Ok(self.view);
        }
        let left = self.recorded.into_iter();
        Err(Stop::Unconsumed(
            left.filter(|identity| self.answers.contains_key(identity))
                .collect(),
        ))
    }

    fn matched(&mut self, method: MethodId, args: &[u8]) -> Option<Vec<u8>> {
        let identity = self.counts.identity(method, args);
        let answer = self.answers.remove(&identity)?;
        self.counts.called(method, args);
        self.view.install(answer.observed, self.now_ms);
        Some(answer.outcome)
    }
}

#[cfg(test)]
mod tests;
