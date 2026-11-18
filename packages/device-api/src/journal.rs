//! What a recording wrapper writes to.
//!
//! `#[capability]` emits a `<Trait>Recorder` beside each recorded trait. It
//! holds the device and forwards every call, and tells a [`Journal`] what the
//! call gave VANA. The journal owns the encoding: a wrapper hands it values
//! through [`Put`] and media accesses through [`Media`], and never sees a
//! byte of the trace.
//!
//! An observe is frozen for the pass: the wrapper samples it once, at
//! [`sample`](crate::ant::AntRecorder::sample) or when VANA first asks, and
//! every read until the next pass returns that sample. A sample is recorded
//! only when it differs from the last recorded value aged to this pass.
//! A call that `refreshes` samples again whatever the pass had sampled.
//!
//! A request that `repeats` is recorded when it differs from the method's
//! last recorded call, or when its refresh records an observation. A call
//! left out gave what that last record holds.
//!
//! The samples live in the capability's `<Trait>Observed`, which the
//! composition keeps for the session. A recorder reaches it and the journal
//! through any handle that dereferences to their `RefCell`, so one that
//! borrows the device for a pass and one a shell owns are the same type.
//!
//! A replay runs the other way. `#[capability]` also emits a
//! `<Trait>Replay`, which holds no device: it answers every call from a
//! [`Script`], the recording read a pass at a time. A recorder around it
//! writes the recording again, so a replay that hands VANA what the
//! recording did records the same bytes.

/// A method of a capability: the capability's id and the method's place in
/// its table.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct MethodId {
    pub capability: u8,
    pub operation: u8,
}

/// The pass a journal is recording: its number since power-on and when it
/// began. A wrapper freezes its observes until the number changes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pass {
    pub number: u32,
    pub now_ms: u64,
}

/// Bytes of a take's `#[out]` buffer one record holds, so that a read of a
/// whole transport buffer splits into records that each fit a block.
pub const PART: usize = 1024;

pub trait Journal {
    fn pass(&self) -> Pass;

    /// Open a record of `method`. What is put until [`Journal::end`] is its
    /// body.
    fn begin(&mut self, method: MethodId);

    fn end(&mut self);

    /// Close the open record and hold it back: the next record a recorder
    /// opens writes it first, and [`Journal::release`] drops it. It is the
    /// record of a request that repeats the method's last recorded call,
    /// which is worth its bytes only when the call changed what the
    /// capability observes.
    fn hold(&mut self);

    /// Drop the record held back, if it still is: whether it was dropped.
    fn release(&mut self) -> bool;

    /// The last record of `method` came after `count` calls of it in this
    /// pass that repeated the record before them and were not recorded.
    fn repeated(&mut self, method: MethodId, count: u8);
}

/// A value a journal can write into the open record.
pub trait Put<T: ?Sized> {
    fn put(&mut self, value: &T);
}

/// A medium a journal records: `At` is the address a media method takes,
/// its `#[at]` arguments in order, and `Outcome` what the method returns.
pub trait Media<At, Outcome> {
    /// A read that left `bytes` in VANA's buffer, or failed.
    fn read(&mut self, method: MethodId, at: At, bytes: &[u8], outcome: &Outcome);

    /// A write of `bytes`, or its failure.
    fn wrote(&mut self, method: MethodId, at: At, bytes: &[u8], outcome: &Outcome);

    fn erased(&mut self, method: MethodId, at: At, length: usize, outcome: &Outcome);
}

/// What a replaying server answers from: one pass of a recording.
///
/// A call the recording holds no answer for has left the recorded world.
/// The script stops the replay there: [`Script::answer`] and
/// [`Script::observed`] do not return.
pub trait Script {
    fn pass(&self) -> Pass;

    /// Open a call of `method`. What is put until it is answered is its
    /// arguments, as its recorder put them, or an observation's key.
    fn begin(&mut self, method: MethodId);

    /// Take the recorded answer to the open call: what is got until
    /// [`Script::end`] is what it gave.
    fn answer(&mut self);

    /// As [`Script::answer`], for a call recorded only when it gave
    /// something: false when the recording holds none.
    fn answered(&mut self) -> bool;

    /// The latest observation under the open key, and the pass time it was
    /// read at.
    fn observed(&mut self) -> u64;

    /// Copy the answer's next byte string into `out`; its length.
    fn fill(&mut self, out: &mut [u8]) -> usize;

    fn end(&mut self);
}

/// A value a script reads from the open answer.
pub trait Get<T> {
    fn get(&mut self) -> T;
}

/// A medium a script serves: the replay's side of [`Media`].
pub trait Stored<At, Outcome> {
    fn read(&mut self, method: MethodId, at: At, out: &mut [u8]) -> Outcome;

    fn write(&mut self, method: MethodId, at: At, bytes: &[u8]) -> Outcome;

    fn erase(&mut self, method: MethodId, at: At, length: usize) -> Outcome;
}

/// What a sink answered when its call was not recorded: see [`Refusal`].
/// `None` for an answer that is always recorded.
pub trait Accepted: Sized {
    fn accepted() -> Option<Self>;
}

impl<E> Accepted for Result<(), E> {
    fn accepted() -> Option<Self> {
        Some(Ok(()))
    }
}

impl Accepted for usize {
    fn accepted() -> Option<Self> {
        None
    }
}

impl Accepted for bool {
    fn accepted() -> Option<Self> {
        Some(true)
    }
}

/// Whether a sink's answer is recorded: a refusal is, and so is a count of
/// what the device took. An accepted output is not.
pub trait Refusal {
    fn refused(&self) -> bool;
}

impl<T, E> Refusal for Result<T, E> {
    fn refused(&self) -> bool {
        self.is_err()
    }
}

impl Refusal for usize {
    fn refused(&self) -> bool {
        true
    }
}

impl Refusal for bool {
    fn refused(&self) -> bool {
        !*self
    }
}

/// How a value read at one pass reads `ms` later, for the parts its producer
/// computes from the time of the read. Most values do not age.
pub trait Aging: Clone + PartialEq {
    fn aged(self, _ms: u64) -> Self {
        self
    }
}

macro_rules! ageless {
    ($($ty:ty),* $(,)?) => {$(impl Aging for $ty {})*};
}

ageless! {
    bool, u8, u16, u32, u64, usize,
    crate::observation::Availability,
    crate::ant::Capabilities, crate::ant::ScanSnapshot, crate::ant::Discovery,
    crate::ble_transport::Snapshot, crate::ble_transport::Discovery,
    crate::bulk::Info,
    crate::connectivity::ControlStatus, crate::connectivity::NetworkDiscovery,
    crate::connectivity::WifiState, crate::connectivity::WifiStats,
    crate::display::Geometry, crate::display::Format,
    crate::gps::Diagnostics,
    crate::input::Controls, crate::input::InputSnapshot,
    crate::position_control::Snapshot,
    crate::power::Capabilities, crate::power::Status,
    crate::sensors::Startup, crate::sensors::Snapshot, crate::sensors::IdentityQuery,
    crate::sensors::SensorSet,
    crate::observation::Observation<crate::sensors::Light>,
    crate::server::AccessPointState, crate::server::Reach, crate::server::Request,
    crate::sound::Snapshot,
    crate::storage::Geometry,
    (u8, u16, u64),
}

impl Aging for crate::ant::Snapshot {
    fn aged(self, ms: u64) -> Self {
        Self {
            age_ms: self.age_ms.map(|age| age.saturating_add(ms)),
            ..self
        }
    }
}

impl Aging for crate::positioning::Snapshot {
    fn aged(mut self, ms: u64) -> Self {
        self.gps.age_ms = self.gps.age_ms.map(|age| age.saturating_add(ms));
        self.gps.utc_ms = self.gps.utc_ms.map(|utc| utc.saturating_add(ms));
        self
    }
}

impl<T: Aging> Aging for Option<T> {
    fn aged(self, ms: u64) -> Self {
        self.map(|value| value.aged(ms))
    }
}

impl<T: Aging, E: Clone + PartialEq> Aging for Result<T, E> {
    fn aged(self, ms: u64) -> Self {
        self.map(|value| value.aged(ms))
    }
}

impl<A: Aging, B: Aging> Aging for (A, B) {
    fn aged(self, ms: u64) -> Self {
        (self.0.aged(ms), self.1.aged(ms))
    }
}

impl<T: PartialEq> Aging for &'static [T] {}

/// The last recorded value of an observation and the pass that read it.
#[derive(Clone, Copy, Debug)]
pub struct Latest<T> {
    recorded: Option<(T, u64)>,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self { recorded: None }
    }
}

impl<T: Aging> Latest<T> {
    /// Whether `value`, read at `now`, differs from the recorded value aged
    /// to `now`. A different value becomes the recorded one.
    pub fn changed(&mut self, value: T, now: u64) -> bool {
        if self.at(now).as_ref() == Some(&value) {
            return false;
        }
        self.recorded = Some((value, now));
        true
    }

    /// Record `value` as read at `at`, as a replay does with a decoded record.
    pub fn set(&mut self, value: T, at: u64) {
        self.recorded = Some((value, at));
    }

    /// The recorded value as it reads at `now`.
    pub fn at(&self, now: u64) -> Option<T> {
        let (value, at) = self.recorded.as_ref()?;
        Some(value.clone().aged(now.saturating_sub(*at)))
    }
}

/// What a recorder keeps between calls: the pass its samples belong to,
/// which observes that pass has sampled, and how many times each sink was
/// called in it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Sampled {
    pass: Option<u32>,
    observes: u32,
    attempts: [u8; Self::SINKS],
    repeats: [u8; Self::REPEATS],
}

impl Sampled {
    /// The most sinks one capability counts.
    pub const SINKS: usize = 2;

    /// The most repeating requests one capability counts.
    pub const REPEATS: usize = 2;

    /// Start `pass` if it is a new one: nothing is sampled in it yet.
    pub fn enter(&mut self, pass: u32) {
        if self.pass != Some(pass) {
            *self = Self {
                pass: Some(pass),
                ..Self::default()
            };
        }
    }

    pub fn has(&self, observe: u32) -> bool {
        self.observes & (1 << observe) != 0
    }

    pub fn mark(&mut self, observe: u32) {
        self.observes |= 1 << observe;
    }

    /// This call's place among the pass's calls of `sink`, from zero.
    pub fn attempt(&mut self, sink: usize) -> u8 {
        let attempt = self.attempts[sink];
        self.attempts[sink] = attempt.saturating_add(1);
        attempt
    }

    /// The calls of `request` this pass left unrecorded since the
    /// capability's last record of a call that refreshes it.
    pub fn repeats(&self, request: usize) -> u8 {
        self.repeats[request]
    }

    /// A call of `request` was left unrecorded.
    pub fn repeat(&mut self, request: usize) {
        self.repeats[request] = self.repeats[request].saturating_add(1);
    }

    /// A call that refreshes the capability was recorded.
    pub fn settle(&mut self) {
        self.repeats = [0; Self::REPEATS];
    }
}

#[cfg(test)]
mod tests;
