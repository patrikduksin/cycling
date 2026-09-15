//! Bounded sound operations. Elapsed time is not an acoustic acknowledgment.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pattern {
    pub id: u8,
    pub frequency_hz: u16,
    pub nominal_ms: u16,
    pub guard_ms: u16,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Idle,
    Queued,
    Submitted,
    Elapsed,
    StopQueued,
    StopSubmitted,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: State,
    pub pattern: Option<u8>,
    pub requested_ms: u64,
    pub submitted_ms: Option<u64>,
    pub until_ms: u64,
}
pub trait Sound {
    fn patterns(&self) -> &'static [Pattern];
    fn snapshot(&self) -> Snapshot;
    fn play(&mut self, id: u8, now: u64) -> Result<(), crate::observation::Error>;
    fn stop(&mut self, now: u64) -> Result<(), crate::observation::Error>;
}
