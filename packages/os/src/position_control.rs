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
    fn pause(&mut self, duration_ms: u32, now: u64) -> Result<(), crate::capabilities::Error>;
    fn resume(&mut self, now: u64) -> Result<(), crate::capabilities::Error>;
}
pub struct Controller {
    status: Snapshot,
    baseline_valid: u32,
    last_receive: Option<u64>,
}
impl Default for Controller {
    fn default() -> Self {
        Self::new()
    }
}
impl Controller {
    pub const fn new() -> Self {
        Self {
            status: Snapshot {
                state: State::ResumeSubmitted,
                at_ms: 0,
                resume_at_ms: None,
                generation: 0,
            },
            baseline_valid: 0,
            last_receive: None,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.status
    }
    pub fn pause(&mut self, duration_ms: u32, now: u64) -> Result<(), crate::capabilities::Error> {
        if !(2000..=10000).contains(&duration_ms) {
            return Err(crate::capabilities::Error::Invalid);
        }
        if self.status.state != State::Receiving {
            return Err(crate::capabilities::Error::Unavailable);
        }
        self.status.state = State::PauseQueued;
        self.status.at_ms = now;
        self.status.resume_at_ms = Some(now.saturating_add(u64::from(duration_ms)));
        self.status.generation = self.status.generation.wrapping_add(1);
        Ok(())
    }
    pub fn resume(&mut self, now: u64) -> Result<(), crate::capabilities::Error> {
        if matches!(
            self.status.state,
            State::ResumeQueued | State::ResumeSubmitted
        ) {
            return Err(crate::capabilities::Error::Unavailable);
        }
        self.status.state = State::ResumeQueued;
        self.status.at_ms = now;
        self.status.resume_at_ms = None;
        self.status.generation = self.status.generation.wrapping_add(1);
        Ok(())
    }
    /// Device power coordination owns the restoring transition. Unlike a
    /// diagnostic pause this has no lease that can reopen GNSS during shutdown.
    pub fn suspend(&mut self, now: u64) -> Result<(), crate::capabilities::Error> {
        if matches!(self.status.state, State::PauseQueued | State::ResumeQueued) {
            return Err(crate::capabilities::Error::Unavailable);
        }
        self.status.state = State::PauseQueued;
        self.status.at_ms = now;
        self.status.resume_at_ms = None;
        self.status.generation = self.status.generation.wrapping_add(1);
        Ok(())
    }
    /// One attempt per transition. The deadline always queues the restoring open,
    /// including after an uncertain close. No terminal connection is needed.
    pub fn tick(
        &mut self,
        now: u64,
        valid: u32,
        received: Option<u64>,
        mut send: impl FnMut(bool) -> Result<(), ()>,
    ) -> bool {
        if self.status.resume_at_ms.is_some_and(|at| now >= at) {
            self.status.state = State::ResumeQueued;
            self.status.resume_at_ms = None;
        }
        let mut boundary = false;
        if matches!(self.status.state, State::PauseQueued | State::ResumeQueued) {
            let open = self.status.state == State::ResumeQueued;
            self.status.at_ms = now;
            self.baseline_valid = valid;
            self.last_receive = received;
            self.status.state = if send(open).is_err() {
                State::Failed
            } else if open {
                State::ResumeSubmitted
            } else {
                State::PauseSubmitted
            };
            boundary = true;
        } else {
            if received != self.last_receive {
                self.last_receive = received;
            }
            match self.status.state {
                State::PauseSubmitted
                    if now.saturating_sub(received.unwrap_or(0).max(self.status.at_ms)) >= 1500 =>
                {
                    self.status.state = State::SilenceObserved
                }
                State::SilenceObserved
                    if received.is_some_and(|at| {
                        at > self.status.at_ms && now.saturating_sub(at) < 1500
                    }) =>
                {
                    self.status.state = State::PauseSubmitted
                }
                State::ResumeSubmitted
                    if valid != self.baseline_valid
                        && received.is_some_and(|at| at >= self.status.at_ms) =>
                {
                    self.status.state = State::Receiving
                }
                State::ResumeSubmitted if now.saturating_sub(self.status.at_ms) > 5000 => {
                    self.status.state = State::Failed
                }
                _ => {}
            }
        }
        boundary
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pause_resumes_without_host_even_after_failed_close() {
        let mut c = Controller::new();
        c.tick(1, 1, Some(1), |_| panic!());
        c.pause(4000, 2).unwrap();
        c.tick(3, 1, Some(1), |open| {
            assert!(!open);
            Err(())
        });
        assert_eq!(c.snapshot().state, State::Failed);
        c.tick(4002, 1, Some(1), |open| {
            assert!(open);
            Ok(())
        });
        assert_eq!(c.snapshot().state, State::ResumeSubmitted);
        c.tick(4100, 2, Some(4100), |_| panic!());
        assert_eq!(c.snapshot().state, State::Receiving);
    }
    #[test]
    fn silence_and_uart_submission_do_not_report_power_or_fixes() {
        let mut c = Controller::new();
        c.tick(1, 1, Some(1), |_| panic!());
        assert!(c.pause(10001, 2).is_err());
        c.pause(4000, 2).unwrap();
        c.tick(3, 1, Some(1), |_| Ok(()));
        c.tick(1600, 1, Some(1), |_| panic!());
        assert_eq!(c.snapshot().state, State::SilenceObserved);
        c.tick(1700, 2, Some(1700), |_| panic!());
        assert_eq!(c.snapshot().state, State::PauseSubmitted);
        c.tick(4002, 2, Some(1700), |_| Ok(()));
        c.tick(10000, 2, Some(1700), |_| panic!());
        assert_eq!(c.snapshot().state, State::Failed);
    }
}
