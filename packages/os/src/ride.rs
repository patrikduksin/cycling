//! Deterministic demo ride state driven only by caller-provided monotonic time.

pub const DEMO_SPEED_MM_S: u32 = 5_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Phase {
    Ready,
    Running,
    Paused,
}

impl Phase {
    pub fn name(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::Running => "running",
            Self::Paused => "paused",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Metrics {
    pub speed_mm_s: u32,
    pub distance_mm: u64,
    pub active_ms: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Ride {
    phase: Phase,
    accumulated_ms: u64,
    running_since: u64,
}

impl Default for Ride {
    fn default() -> Self {
        Self {
            phase: Phase::Ready,
            accumulated_ms: 0,
            running_since: 0,
        }
    }
}

impl Ride {
    pub fn phase(&self) -> Phase {
        self.phase
    }

    pub fn start_or_resume(&mut self, now: u64) {
        if matches!(self.phase, Phase::Ready | Phase::Paused) {
            self.phase = Phase::Running;
            self.running_since = now;
        }
    }

    pub fn pause(&mut self, now: u64) {
        if self.phase == Phase::Running {
            self.accumulated_ms = self
                .accumulated_ms
                .saturating_add(now.saturating_sub(self.running_since));
            self.phase = Phase::Paused;
        }
    }

    pub fn toggle(&mut self, now: u64) {
        match self.phase {
            Phase::Running => self.pause(now),
            Phase::Ready | Phase::Paused => self.start_or_resume(now),
        }
    }

    pub fn reset(&mut self) {
        if self.phase != Phase::Running {
            *self = Self::default();
        }
    }

    pub fn metrics(&self, now: u64) -> Metrics {
        let active_ms = if self.phase == Phase::Running {
            self.accumulated_ms
                .saturating_add(now.saturating_sub(self.running_since))
        } else {
            self.accumulated_ms
        };
        Metrics {
            speed_mm_s: if self.phase == Phase::Running {
                DEMO_SPEED_MM_S
            } else {
                0
            },
            distance_mm: active_ms.saturating_mul(u64::from(DEMO_SPEED_MM_S)) / 1_000,
            active_ms,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_pause_resume_and_reset_are_monotonic() {
        let mut ride = Ride::default();
        ride.pause(50);
        assert_eq!(ride.metrics(50).active_ms, 0);
        ride.start_or_resume(100);
        ride.start_or_resume(500);
        assert_eq!(ride.metrics(2_100).distance_mm, 10_000);
        ride.pause(2_100);
        ride.pause(3_000);
        assert_eq!(ride.metrics(20_000).active_ms, 2_000);
        assert_eq!(ride.metrics(20_000).speed_mm_s, 0);
        ride.start_or_resume(20_000);
        assert_eq!(ride.metrics(21_000).active_ms, 3_000);
        ride.pause(21_000);
        ride.reset();
        assert_eq!(ride, Ride::default());
    }

    #[test]
    fn elapsed_is_frame_rate_independent_and_saturates() {
        let mut one_sample = Ride::default();
        let mut many_samples = Ride::default();
        one_sample.start_or_resume(10);
        many_samples.start_or_resume(10);
        for now in 11..1_000 {
            let _ = many_samples.metrics(now);
        }
        assert_eq!(one_sample.metrics(1_000), many_samples.metrics(1_000));
        one_sample.pause(u64::MAX);
        assert_eq!(one_sample.metrics(u64::MAX).distance_mm, u64::MAX / 1_000);
    }
}
