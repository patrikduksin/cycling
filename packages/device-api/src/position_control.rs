//! Stream pause/resume observations, without electrical power claims.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Receiving,
    PauseQueued,
    PauseSubmitted,
    SilenceObserved,
    ResumeQueued,
    ResumeSubmitted,
    Failed,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snapshot {
    pub state: State,
    pub at_ms: u64,
    pub resume_at_ms: Option<u64>,
    pub generation: u32,
}
pub trait Control {
    fn control(&self) -> Snapshot;
    fn pause(&mut self, duration_ms: u32, now: u64) -> Result<(), crate::observation::Error>;
    fn resume(&mut self, now: u64) -> Result<(), crate::observation::Error>;
}
