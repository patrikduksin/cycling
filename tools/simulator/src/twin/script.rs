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
//!
//! A request that repeats is recorded only when it differs from the
//! method's last record or changes an observation, so a call with no record
//! in its epoch is one the device left out. Where a record follows such
//! calls in its own epoch, the log says how many, and that many of the
//! twin's calls pass the record by.
//!
//! A record's body opens with its arguments, or an observation's key, as
//! the recorder put them. The log does not say where they end, so the twin
//! matches a record by that prefix: one method's arguments are always the
//! same types, and no encoding of them is the start of another.
use device_api::capability::{Capability, Kind};
use device_api::journal::MethodId;
use std::collections::BTreeMap;
use std::fmt;

/// The capabilities a world log can hold: the device's and the shell's
/// settings.
pub fn capabilities() -> impl Iterator<Item = &'static Capability> {
    device_api::capability::ALL
        .iter()
        .copied()
        .chain([&firmware_shell::storage::CAPABILITY_SETTINGS])
}

pub fn declared(
    method: MethodId,
) -> Option<(&'static Capability, &'static device_api::capability::Method)> {
    let table = capabilities().find(|table| table.id == method.capability)?;
    Some((table, table.methods.get(usize::from(method.operation))?))
}

pub fn named(capability: &str, method: &str) -> Option<MethodId> {
    let table = capabilities().find(|table| table.name == capability)?;
    let operation = table.methods.iter().position(|m| m.name == method)?;
    Some(MethodId {
        capability: table.id,
        operation: operation as u8,
    })
}

/// A method as a person reads it: `Ant::request`.
pub struct Named(pub MethodId);

impl fmt::Display for Named {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match declared(self.0) {
            Some((table, method)) => write!(f, "{}::{}", table.name, method.name),
            None => write!(f, "{}.{}", self.0.capability, self.0.operation),
        }
    }
}

fn refreshes(method: MethodId) -> bool {
    matches!(
        declared(method).map(|(_, method)| method.kind),
        Some(
            Kind::Take { refreshes: true }
                | Kind::Request {
                    refreshes: true,
                    ..
                }
                | Kind::Sink { refreshes: true }
        )
    )
}

pub(crate) struct Hex<'a>(pub(crate) &'a [u8]);

impl fmt::Display for Hex<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.iter().try_for_each(|byte| write!(f, "{byte:02x}"))
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
        write!(
            f,
            "{}({}) #{} in epoch {}",
            Named(self.method),
            Hex(&self.args),
            self.occurrence,
            self.epoch
        )
    }
}

/// A recorded call the twin never made: its body is its arguments and
/// what it gave.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Left {
    pub method: MethodId,
    pub body: Vec<u8>,
    pub epoch: u32,
}

/// One record of a pass, in the order the device wrote them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Record {
    /// A call: its arguments, then what it gave.
    Called { method: MethodId, body: Vec<u8> },
    /// An observation: its key, then its value.
    Observed { method: MethodId, body: Vec<u8> },
    /// The last call of `method` came after `count` calls of it the device
    /// did not record.
    Repeated { method: MethodId, count: u8 },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Stop {
    Unanswered(Identity),
    Unconsumed(Vec<Left>),
}

impl fmt::Display for Stop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unanswered(identity) => {
                write!(f, "left the recorded world: {identity} has no answer")
            }
            Self::Unconsumed(left) => {
                write!(f, "left the recorded world: the twin never made")?;
                for (index, call) in left.iter().enumerate() {
                    let separator = if index == 0 { " " } else { ", " };
                    write!(
                        f,
                        "{separator}{} {} in epoch {}",
                        Named(call.method),
                        Hex(&call.body),
                        call.epoch
                    )?;
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
    fn epoch(&self, capability: u8) -> u32 {
        self.epochs.get(&capability).copied().unwrap_or(0)
    }

    fn identity(&self, method: MethodId, args: &[u8]) -> Identity {
        Identity {
            method,
            args: args.to_vec(),
            occurrence: self
                .occurrences
                .get(&(method, args.to_vec()))
                .copied()
                .unwrap_or(0),
            epoch: self.epoch(method.capability),
        }
    }

    fn called(&mut self, method: MethodId, args: &[u8]) {
        *self.occurrences.entry((method, args.to_vec())).or_default() += 1;
        if refreshes(method) {
            *self.epochs.entry(method.capability).or_default() += 1;
        }
    }
}

/// The latest value of each observation, as of the twin's matched calls,
/// and the pass time it was read at. The device records an observation
/// only when it differs from the last one aged to now, so the view carries
/// from one pass to the next, and a value that ages is aged from `at_ms`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct View(BTreeMap<MethodId, Vec<(Vec<u8>, u64)>>);

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Seen {
    pub value: Vec<u8>,
    pub at_ms: u64,
}

impl View {
    /// The latest observation of `method` under `key`. Older ones under
    /// the same key are dropped: nothing reads them again.
    pub fn get(&mut self, method: MethodId, key: &[u8]) -> Option<Seen> {
        let bodies = self.0.get_mut(&method)?;
        let latest = bodies.iter().rposition(|(body, _)| body.starts_with(key))?;
        let mut index = 0;
        bodies.retain(|(body, _)| {
            index += 1;
            index > latest || !body.starts_with(key)
        });
        let (body, at_ms) = bodies.iter().rfind(|(body, _)| body.starts_with(key))?;
        Some(Seen {
            value: body[key.len()..].to_vec(),
            at_ms: *at_ms,
        })
    }

    fn install(&mut self, observed: Vec<(MethodId, Vec<u8>)>, at_ms: u64) {
        for (method, body) in observed {
            self.0.entry(method).or_default().push((body, at_ms));
        }
    }
}

struct Call {
    method: MethodId,
    body: Vec<u8>,
    epoch: u32,
    consumed: bool,
    observed: Vec<(MethodId, Vec<u8>)>,
    /// Calls the device made before this one and did not record: that many
    /// of the twin's calls pass it by.
    unrecorded: u8,
}

pub struct Pass {
    calls: Vec<Call>,
    counts: Counts,
    view: View,
    now_ms: u64,
}

impl Pass {
    /// An observation rides with the last call before it that refreshes its
    /// capability, which started the epoch it was read in.
    pub fn new(now_ms: u64, mut view: View, records: impl IntoIterator<Item = Record>) -> Self {
        let mut calls: Vec<Call> = Vec::new();
        let mut epochs = Counts::default();
        let mut last = BTreeMap::new();
        for record in records {
            match record {
                Record::Called { method, body } => {
                    let epoch = epochs.epoch(method.capability);
                    epochs.called(method, &[]);
                    if refreshes(method) {
                        last.insert(method.capability, calls.len());
                    }
                    calls.push(Call {
                        method,
                        body,
                        epoch,
                        consumed: false,
                        observed: Vec::new(),
                        unrecorded: 0,
                    });
                }
                Record::Repeated { method, count } => {
                    if let Some(call) = calls.iter_mut().rfind(|call| call.method == method) {
                        call.unrecorded = count;
                    }
                }
                Record::Observed { method, body } => match last.get(&method.capability) {
                    Some(&call) => calls[call].observed.push((method, body)),
                    None => view.install(vec![(method, body)], now_ms),
                },
            }
        }
        Self {
            calls,
            counts: Counts::default(),
            view,
            now_ms,
        }
    }

    /// What the call of `method` with `args` gave: the rest of its record.
    pub fn answer(&mut self, method: MethodId, args: &[u8]) -> Result<Vec<u8>, Stop> {
        self.matched(method, args)
            .ok_or_else(|| Stop::Unanswered(self.counts.identity(method, args)))
    }

    pub fn poll(&mut self, method: MethodId) -> Option<Vec<u8>> {
        self.matched(method, &[])
    }

    pub fn observe(&mut self, method: MethodId, key: &[u8]) -> Option<Seen> {
        self.view.get(method, key)
    }

    /// The view the next pass starts from, or the calls the twin never
    /// made, in the order the device made them.
    pub fn end(self) -> Result<View, Stop> {
        let left: Vec<Left> = self
            .calls
            .into_iter()
            .filter(|call| !call.consumed)
            .map(|call| Left {
                method: call.method,
                body: call.body,
                epoch: call.epoch,
            })
            .collect();
        match left.is_empty() {
            true => Ok(self.view),
            false => Err(Stop::Unconsumed(left)),
        }
    }

    fn matched(&mut self, method: MethodId, args: &[u8]) -> Option<Vec<u8>> {
        let identity = self.counts.identity(method, args);
        let call = self
            .calls
            .iter_mut()
            .filter(|call| call.method == method && call.body.starts_with(args))
            .nth(identity.occurrence as usize)
            .filter(|call| call.epoch == identity.epoch && !call.consumed)?;
        if call.unrecorded > 0 {
            call.unrecorded -= 1;
            return None;
        }
        call.consumed = true;
        let outcome = call.body[args.len()..].to_vec();
        let observed = core::mem::take(&mut call.observed);
        self.counts.called(method, args);
        self.view.install(observed, self.now_ms);
        Some(outcome)
    }
}

#[cfg(test)]
mod tests;
