use device_api::sound::*;

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
    pub fn play(
        &mut self,
        pattern: Pattern,
        now: u64,
    ) -> Result<(), device_api::observation::Error> {
        self.tick(now);
        if !matches!(self.snapshot.state, State::Idle | State::Elapsed)
            || now < self.snapshot.until_ms
        {
            return Err(device_api::observation::Error::Unavailable);
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
    pub fn stop(&mut self, now: u64) -> Result<(), device_api::observation::Error> {
        if matches!(
            self.snapshot.state,
            State::StopQueued | State::StopSubmitted
        ) {
            return Err(device_api::observation::Error::Unavailable);
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

/// The longest run a `Sequence` holds.
pub const SEQUENCE_CAPACITY: usize = 4;
/// A pattern that cannot start this long after it was due is dropped with
/// the rest of its run: a late cue would describe something else.
pub const SEQUENCE_LATE_MS: u64 = 1_000;

/// Plays a short run of patterns one after another through a `Sound`, each
/// once the sound accepts it (after the previous pattern's guard). It holds
/// no policy: the caller picks the patterns and what may replace a run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sequence {
    ids: [u8; SEQUENCE_CAPACITY],
    len: u8,
    next: u8,
    due_ms: u64,
}
impl Sequence {
    pub const fn new() -> Self {
        Self {
            ids: [0; SEQUENCE_CAPACITY],
            len: 0,
            next: 0,
            due_ms: 0,
        }
    }
    /// Play `ids` from now, replacing any run in progress. Ids beyond
    /// `SEQUENCE_CAPACITY` are dropped.
    pub fn start(&mut self, ids: &[u8], now: u64) {
        let len = ids.len().min(SEQUENCE_CAPACITY);
        self.ids[..len].copy_from_slice(&ids[..len]);
        self.len = len as u8;
        self.next = 0;
        self.due_ms = now;
    }
    pub fn is_playing(&self) -> bool {
        self.next < self.len
    }
    pub fn clear(&mut self) {
        self.next = self.len;
    }
    /// Offer the next pattern to `sound`. Call once per loop.
    pub fn pump(&mut self, sound: &mut impl Sound, now: u64) {
        if !self.is_playing() {
            return;
        }
        if now > self.due_ms.saturating_add(SEQUENCE_LATE_MS) {
            self.clear();
            return;
        }
        let id = self.ids[usize::from(self.next)];
        if sound.play(id, now).is_ok() {
            self.next += 1;
            let guard = sound
                .patterns()
                .iter()
                .find(|pattern| pattern.id == id)
                .map_or(0, |pattern| u64::from(pattern.guard_ms));
            self.due_ms = now.saturating_add(guard);
        }
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
    /// A sound that accepts a pattern once the last one's guard is over.
    struct Buzzer {
        busy_until: u64,
        played: heapless::Vec<(u64, u8), 8>,
    }
    const PATTERNS: [Pattern; 2] = [
        P,
        Pattern {
            id: 10,
            frequency_hz: 4000,
            nominal_ms: 100,
            guard_ms: 500,
        },
    ];
    impl Sound for Buzzer {
        fn patterns(&self) -> &'static [Pattern] {
            &PATTERNS
        }
        fn snapshot(&self) -> Snapshot {
            Player::new().snapshot()
        }
        fn play(&mut self, id: u8, now: u64) -> Result<(), device_api::observation::Error> {
            if now < self.busy_until {
                return Err(device_api::observation::Error::Unavailable);
            }
            self.busy_until = now + 500;
            let _ = self.played.push((now, id));
            Ok(())
        }
        fn stop(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
            Ok(())
        }
    }
    #[test]
    fn a_sequence_plays_each_pattern_after_the_last_guard() {
        let mut buzzer = Buzzer {
            busy_until: 0,
            played: heapless::Vec::new(),
        };
        let mut run = Sequence::new();
        run.start(&[0, 10, 0, 10, 0], 0);
        for now in (0..3000).step_by(50) {
            run.pump(&mut buzzer, now);
        }
        assert_eq!(buzzer.played, [(0, 0), (500, 10), (1000, 0), (1500, 10)]);
        assert!(!run.is_playing());
        // A new run replaces the rest of an old one.
        run.start(&[0, 10], 3000);
        run.pump(&mut buzzer, 3000);
        run.start(&[10], 3100);
        for now in (3100..5000).step_by(50) {
            run.pump(&mut buzzer, now);
        }
        assert_eq!(&buzzer.played[4..], [(3000, 0), (3500, 10)]);
    }
    #[test]
    fn a_late_sequence_is_dropped() {
        let mut buzzer = Buzzer {
            busy_until: 5_000,
            played: heapless::Vec::new(),
        };
        let mut run = Sequence::new();
        run.start(&[0, 10], 0);
        for now in (0..6000).step_by(100) {
            run.pump(&mut buzzer, now);
        }
        assert!(buzzer.played.is_empty());
        assert!(!run.is_playing());
    }
}
