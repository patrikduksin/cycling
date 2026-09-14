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
    fn play(&mut self, id: u8, now: u64) -> Result<(), crate::capabilities::Error>;
    fn stop(&mut self, now: u64) -> Result<(), crate::capabilities::Error>;
}
pub struct Player {
    snapshot: Snapshot,
    pending: Option<Option<Pattern>>,
}
impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}
impl Player {
    pub const fn new() -> Self {
        Self {
            snapshot: Snapshot {
                state: State::Idle,
                pattern: None,
                requested_ms: 0,
                submitted_ms: None,
                until_ms: 0,
            },
            pending: None,
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        self.snapshot
    }
    pub fn play(&mut self, pattern: Pattern, now: u64) -> Result<(), crate::capabilities::Error> {
        self.tick(now);
        if !matches!(self.snapshot.state, State::Idle | State::Elapsed)
            || now < self.snapshot.until_ms
        {
            return Err(crate::capabilities::Error::Unavailable);
        }
        self.snapshot = Snapshot {
            state: State::Queued,
            pattern: Some(pattern.id),
            requested_ms: now,
            submitted_ms: None,
            until_ms: now.saturating_add(1000),
        };
        self.pending = Some(Some(pattern));
        Ok(())
    }
    pub fn stop(&mut self, now: u64) -> Result<(), crate::capabilities::Error> {
        if matches!(
            self.snapshot.state,
            State::StopQueued | State::StopSubmitted
        ) {
            return Err(crate::capabilities::Error::Unavailable);
        }
        self.pending = Some(None);
        self.snapshot.state = State::StopQueued;
        self.snapshot.requested_ms = now;
        self.snapshot.submitted_ms = None;
        Ok(())
    }
    pub fn tick(&mut self, now: u64) {
        if matches!(self.snapshot.state, State::Submitted | State::StopSubmitted)
            && now >= self.snapshot.until_ms
        {
            self.snapshot.state = State::Elapsed;
        }
        // Do not play a queued sound much later than its request.
        if self.snapshot.state == State::Queued && now >= self.snapshot.until_ms {
            self.pending = None;
            self.snapshot.state = State::Failed;
        }
    }
    /// Exactly one transport attempt. Err is uncertain and requires explicit stop.
    pub fn dispatch(&mut self, now: u64, mut send: impl FnMut(Option<u8>) -> Result<(), ()>) {
        self.tick(now);
        let Some(pattern) = self.pending.take() else {
            return;
        };
        if send(pattern.map(|p| p.id)).is_err() {
            self.snapshot.state = State::Failed;
            return;
        }
        self.snapshot.submitted_ms = Some(now);
        self.snapshot.state = if pattern.is_some() {
            State::Submitted
        } else {
            State::StopSubmitted
        };
        self.snapshot.until_ms = now.saturating_add(pattern.map_or(100, |p| u64::from(p.guard_ms)));
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    const P: Pattern = Pattern {
        id: 0,
        frequency_hz: 3000,
        nominal_ms: 75,
        guard_ms: 500,
    };
    #[test]
    fn rejects_overlap_and_never_replays_failed_submission() {
        let mut p = Player::new();
        p.play(P, 0).unwrap();
        assert!(p.play(P, 1).is_err());
        p.dispatch(2, |_| Err(()));
        p.dispatch(3, |_| panic!("must not retry"));
        assert_eq!(p.snapshot().state, State::Failed);
        assert!(p.play(P, 1000).is_err());
        p.stop(1001).unwrap();
        p.dispatch(1002, |id| {
            assert_eq!(id, None);
            Ok(())
        });
        p.tick(1102);
        p.play(P, 1102).unwrap();
    }
    #[test]
    fn stop_replaces_queued_sound_and_timing_is_not_acknowledgment() {
        let mut p = Player::new();
        p.play(P, 0).unwrap();
        p.stop(1).unwrap();
        p.dispatch(2, |id| {
            assert_eq!(id, None);
            Ok(())
        });
        p.tick(102);
        assert_eq!(p.snapshot().state, State::Elapsed);
        p.play(P, 102).unwrap();
        p.dispatch(103, |_| Ok(()));
        assert!(p.play(P, 602).is_err());
        p.tick(603);
        assert_eq!(p.snapshot().state, State::Elapsed);
    }
    #[test]
    fn expired_queue_cannot_sound_after_a_stall() {
        let mut p = Player::new();
        p.play(P, 0).unwrap();
        p.dispatch(1000, |_| panic!("expired"));
        assert_eq!(p.snapshot().state, State::Failed);
    }
}
