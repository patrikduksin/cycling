//! Bounded sound operations. Elapsed time is not an acoustic acknowledgment.
use crate::capability::capability;

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
#[capability(id = 16)]
pub trait Sound {
    #[observe]
    fn patterns(&self) -> &'static [Pattern];
    #[observe]
    fn snapshot(&self) -> Snapshot;
    #[request(refreshes)]
    fn play(&mut self, id: u8, #[clock] now: u64) -> Result<(), crate::observation::Error>;
    #[request(refreshes)]
    fn stop(&mut self, #[clock] now: u64) -> Result<(), crate::observation::Error>;
}
