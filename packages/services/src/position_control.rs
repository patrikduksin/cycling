use device_api::position_control::*;

/// A paused stream is silent once nothing arrived for this long.
const SILENCE_MS: u64 = 1500;
/// A resumed stream that has not talked after this long failed.
const RESUME_MS: u64 = 5000;
/// How often a resume waits on the stream: it ends when bytes arrive, which
/// the positioning owner observes, not this one.
pub const STREAM_WATCH_MS: u64 = 100;
/// How often observed silence is checked: a stream that talks again must be
/// seen inside the `SILENCE_MS` window that tells it from silence.
pub const SILENCE_WATCH_MS: u64 = 1000;

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
    pub fn pause(
        &mut self,
        duration_ms: u32,
        now: u64,
    ) -> Result<(), device_api::observation::Error> {
        if !(2000..=10000).contains(&duration_ms) {
            return Err(device_api::observation::Error::Invalid);
        }
        if self.status.state != State::Receiving {
            return Err(device_api::observation::Error::Unavailable);
        }
        self.status.state = State::PauseQueued;
        self.status.at_ms = now;
        self.status.resume_at_ms = Some(now.saturating_add(u64::from(duration_ms)));
        self.status.generation = self.status.generation.wrapping_add(1);
        Ok(())
    }
    pub fn resume(&mut self, now: u64) -> Result<(), device_api::observation::Error> {
        if matches!(
            self.status.state,
            State::ResumeQueued | State::ResumeSubmitted
        ) {
            return Err(device_api::observation::Error::Unavailable);
        }
        self.status.state = State::ResumeQueued;
        self.status.at_ms = now;
        self.status.resume_at_ms = None;
        self.status.generation = self.status.generation.wrapping_add(1);
        Ok(())
    }
    /// Device power coordination owns the restoring transition. Unlike a
    /// diagnostic pause this has no lease that can reopen GNSS during shutdown.
    pub fn suspend(&mut self, now: u64) -> Result<(), device_api::observation::Error> {
        if matches!(self.status.state, State::PauseQueued | State::ResumeQueued) {
            return Err(device_api::observation::Error::Unavailable);
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
                    if now.saturating_sub(received.unwrap_or(0).max(self.status.at_ms))
                        >= SILENCE_MS =>
                {
                    self.status.state = State::SilenceObserved
                }
                State::SilenceObserved
                    if received.is_some_and(|at| {
                        at > self.status.at_ms && now.saturating_sub(at) < SILENCE_MS
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
                State::ResumeSubmitted if now.saturating_sub(self.status.at_ms) > RESUME_MS => {
                    self.status.state = State::Failed
                }
                _ => {}
            }
        }
        boundary
    }
    /// When `tick` must run again, given the stream's last receive time:
    /// `now` for a queued command, a lease's resume, the end of the silence
    /// wait, and the next look at the stream while a state waits on it.
    /// `None` while receiving or failed without a lease.
    pub fn due(&self, now: u64, received: Option<u64>) -> Option<u64> {
        let at = self.status.at_ms;
        let state = match self.status.state {
            State::PauseQueued | State::ResumeQueued => Some(now),
            State::PauseSubmitted => Some(received.unwrap_or(0).max(at) + SILENCE_MS),
            State::SilenceObserved => Some(now + SILENCE_WATCH_MS),
            State::ResumeSubmitted => Some((now + STREAM_WATCH_MS).min(at + RESUME_MS + 1)),
            State::Receiving | State::Failed => None,
        };
        state.into_iter().chain(self.status.resume_at_ms).min()
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

    /// Ticks `c` only at its due times from `from` to `until`, as the IO task
    /// does, over a stream whose valid count and receive time at a moment
    /// `stream` gives. Each state change with its time.
    fn at_due_times(
        c: &mut Controller,
        from: u64,
        until: u64,
        stream: impl Fn(u64) -> (u32, Option<u64>),
    ) -> std::vec::Vec<(u64, State)> {
        let mut changes = std::vec::Vec::new();
        let mut now = from;
        while now < until {
            let (valid, received) = stream(now);
            let before = c.snapshot().state;
            c.tick(now, valid, received, |_| Ok(()));
            if c.snapshot().state != before {
                changes.push((now, c.snapshot().state));
            }
            let Some(due) = c.due(now, received) else {
                break;
            };
            now = due.max(now + 1);
        }
        changes
    }

    #[test]
    fn ticked_only_when_due_a_lease_runs_on_time() {
        let mut c = Controller::new();
        c.tick(0, 1, Some(0), |_| panic!());
        assert_eq!(c.due(0, Some(0)), None, "receiving needs no tick");
        c.pause(4000, 10).unwrap();
        assert_eq!(c.due(10, Some(0)), Some(10));
        // The receiver stops 5 ms after the close and talks again 30 ms
        // after the open.
        let stream = |now: u64| match now {
            ..15 => (1, Some(now)),
            15..4040 => (1, Some(15)),
            _ => (2, Some(now)),
        };
        let changes = at_due_times(&mut c, 10, 6000, stream);
        assert_eq!(
            changes,
            [
                (10, State::PauseSubmitted),
                (1515, State::SilenceObserved),
                (4010, State::ResumeSubmitted),
                (4110, State::Receiving),
            ]
        );
    }

    #[test]
    fn ticked_only_when_due_silence_and_resume_failures_are_seen() {
        // A closed stream that keeps talking is never called silent; once it
        // stops, silence is seen on time, and a word inside the window after
        // it within the watch.
        let mut c = Controller::new();
        c.tick(0, 1, Some(0), |_| panic!());
        c.suspend(0).unwrap();
        let stream = |now: u64| match now {
            ..2000 => (1, Some(now - now % 100)),
            2000..5000 => (1, Some(1900)),
            _ => (1, Some(5000)),
        };
        let changes = at_due_times(&mut c, 0, 6000, stream);
        assert_eq!(
            changes,
            [
                (0, State::PauseSubmitted),
                (3400, State::SilenceObserved),
                (5400, State::PauseSubmitted),
            ]
        );
        assert!(5400 - 5000 < SILENCE_MS, "seen inside the window");
        // A resume the stream never answers fails just after its timeout.
        let mut c = Controller::new();
        let changes = at_due_times(&mut c, 0, 9000, |_| (0, None));
        assert_eq!(changes, [(RESUME_MS + 1, State::Failed)]);
    }
}
