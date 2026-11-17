//! Stream pause/resume observations, without electrical power claims.
use crate::capability::capability;

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
#[capability(id = 9)]
pub trait PositionControl {
    #[observe]
    fn control(&self) -> Snapshot;
    #[request(refreshes)]
    fn pause(
        &mut self,
        duration_ms: u32,
        #[clock] now: u64,
    ) -> Result<(), crate::observation::Error>;
    #[request(refreshes)]
    fn resume(&mut self, #[clock] now: u64) -> Result<(), crate::observation::Error>;
    /// Excerpts of the receiver stream around framing events; None before
    /// acquisition starts or where no receiver stream exists.
    #[observe]
    fn diagnostics(&self) -> Option<crate::gps::Diagnostics>;
}
