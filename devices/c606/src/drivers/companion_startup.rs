//! Fixed companion startup operations. No runtime power-command interface.
use device_api::observation::Observation;
use device_api::sensors::Startup as Phase;

pub fn frame() -> [u8; 16] {
    crate::drivers::companion::command(16, [0xe2, 2, 0, 0, 0, 1, 0, 0])
}

/// Recovered N22 operation 0/value 7 enters the normal initialization dispatcher.
/// Only used once after an explicit charging acknowledgment with no acquisition.
pub fn wake_frame() -> [u8; 16] {
    crate::drivers::companion::command(16, [0xe2, 2, 0, 0, 0, 7, 0, 0])
}

/// The startup check watches companion traffic this long before deciding.
const PROBE_WINDOW_MS: u64 = 3000;
/// Windows observed again, each twice as long, after a transport fault
/// before the companion is reported unavailable.
const PROBE_RETRIES: u8 = 3;

pub struct Startup {
    status: Phase,
    started_ms: u64,
    first: Option<[u64; 3]>,
    reason: Option<u8>,
    activity: bool,
    /// A transport fault was counted during the current probe window.
    transport_lost: bool,
    /// Transport fault count when the current observation began; only
    /// faults after it make an observation unreliable.
    faults_at: Option<u32>,
    /// Probe windows restarted after a transport fault.
    retries: u8,
    wake_requested: bool,
    wake_authorized: bool,
}

impl Default for Startup {
    fn default() -> Self {
        Self::new()
    }
}
impl Startup {
    pub const fn new() -> Self {
        Self {
            status: Phase::NotSubmitted,
            started_ms: 0,
            first: None,
            reason: None,
            activity: false,
            transport_lost: false,
            faults_at: None,
            retries: 0,
            wake_requested: false,
            wake_authorized: false,
        }
    }

    pub fn begin(&mut self, now: u64) {
        if self.status == Phase::NotSubmitted {
            self.status = Phase::Probing;
            self.started_ms = now;
        }
    }

    pub fn ant_allowed(&self) -> bool {
        matches!(
            self.status,
            Phase::AlreadyRunning | Phase::RunningDegraded | Phase::Ready
        )
    }

    /// Whether transport faults were counted since the current observation
    /// began. `faults` is the cumulative UART, CRC and framing-loss count,
    /// each fault counted in full on the tick it occurs; a fault spread over
    /// two ticks would taint the window that follows it.
    fn faulted(&mut self, faults: u32) -> bool {
        faults != *self.faults_at.get_or_insert(faults)
    }

    /// Observe before considering a single startup acknowledgment. Any sensor
    /// or radio traffic preserves the running companion, even if degraded.
    /// A transport fault hides traffic, so the window is observed again a
    /// bounded number of times before the companion is called unavailable.
    pub fn probe(
        &mut self,
        sample: device_api::sensors::Snapshot,
        now: u64,
        _bridge_fresh: bool,
        faults: u32,
        radio_seen: bool,
    ) -> bool {
        if self.status != Phase::Probing {
            return false;
        }
        self.activity |= radio_seen;
        self.transport_lost |= self.faulted(faults);
        if !matches!(sample.pressure, Observation::Unavailable)
            || sample
                .motion
                .iter()
                .any(|value| !matches!(value, Observation::Unavailable))
        {
            self.activity = true;
        }
        let advancing = self.advancing(sample, now);
        if now.saturating_sub(self.started_ms) < PROBE_WINDOW_MS << self.retries {
            return false;
        }
        if self.transport_lost && self.retries < PROBE_RETRIES {
            self.retries += 1;
            self.started_ms = now;
            self.first = None;
            self.transport_lost = false;
            self.faults_at = Some(faults);
            log::warn!(target: "companion", "startup check saw a transport fault; observing again {}/{}", self.retries, PROBE_RETRIES);
            return false;
        }
        if self.transport_lost {
            log::warn!(target: "companion", "startup check unavailable after {} transport faults", PROBE_RETRIES + 1);
            self.status = Phase::Unavailable;
        } else if self.activity {
            self.status = if advancing {
                Phase::AlreadyRunning
            } else {
                Phase::RunningDegraded
            };
        } else {
            // Battery startup waits for this ACK before periodic battery/power
            // reports begin. Waiting for those reports here deadlocks boot.
            self.status = Phase::Pending;
            self.first = None;
            return true;
        }
        false
    }

    pub fn request_wake(&mut self) -> Result<(), device_api::observation::Error> {
        if self.status != Phase::Charging || self.wake_requested || self.wake_authorized {
            return Err(device_api::observation::Error::Unavailable);
        }
        self.wake_authorized = true;
        Ok(())
    }

    /// Charging standby is stable until an explicit request or physical startup.
    /// Never reinitialize a companion that has shown sensor or radio activity,
    /// and never decide while transport faults are being counted.
    pub fn wake(
        &mut self,
        sample: device_api::sensors::Snapshot,
        bridge_fresh: bool,
        faults: u32,
        radio_seen: bool,
    ) -> bool {
        if !self.wake_authorized
            || self.status != Phase::Charging
            || self.reason != Some(4)
            || self.wake_requested
        {
            return false;
        }
        self.activity |= radio_seen
            || !matches!(sample.pressure, Observation::Unavailable)
            || sample
                .motion
                .iter()
                .any(|value| !matches!(value, Observation::Unavailable));
        if self.activity {
            self.status = Phase::RunningDegraded;
        } else if self.faulted(faults) {
            // Traffic may have been missed; wait for a fault-free tick.
            self.faults_at = Some(faults);
        } else if bridge_fresh {
            self.wake_requested = true;
            self.status = Phase::WakePending;
            return true;
        }
        false
    }

    pub fn wake_submitted(&mut self, result: Result<usize, ()>, now: u64) {
        if self.status != Phase::WakePending {
            return;
        }
        self.started_ms = now;
        self.first = None;
        self.status = if result == Ok(16) {
            Phase::Waiting
        } else {
            Phase::Uncertain
        };
    }

    /// A short FIFO write may have reached the companion. Never retry it.
    pub fn submitted(&mut self, result: Result<usize, ()>, now: u64) {
        if self.status != Phase::Pending {
            return;
        }
        self.started_ms = now;
        self.status = if result == Ok(16) {
            Phase::Waiting
        } else {
            Phase::Uncertain
        };
    }

    pub fn status(&self) -> Phase {
        self.status
    }

    pub fn reason(&self) -> Option<u8> {
        self.reason
    }

    fn expire(&mut self, now: u64) {
        if self.status == Phase::Waiting && now.saturating_sub(self.started_ms) >= 10_000 {
            self.status = Phase::TimedOut;
        }
    }

    /// The caller validates the frame class and CRC. Charging acknowledgment
    /// does not start acquisition; a later manual-on report may do so.
    pub fn report(&mut self, group: u8, payload: &[u8], now: u64) {
        if group == 16 && matches!(payload, [0xf1, 1..=3, ..]) {
            self.activity = true;
        }
        self.expire(now);
        if !matches!(self.status, Phase::Waiting | Phase::Charging) || group != 16 {
            return;
        }
        let [0xe2, 2, 0, 0, 0xff, reason @ (4..=6), 0, 0xff] = payload else {
            return;
        };
        // The first operating reason starts one deadline. Duplicate reports
        // and older charging reports cannot reset it or the sample baseline.
        if matches!(self.reason, Some(5 | 6)) || self.reason == Some(*reason) {
            return;
        }
        self.reason = Some(*reason);
        self.first = None;
        if *reason == 4 {
            self.status = Phase::Charging;
        } else {
            self.status = Phase::Waiting;
            self.started_ms = now;
        }
    }

    /// Each sensor must be fresh and advance independently. An identity or
    /// power-reason reply and a successful UART write cannot establish readiness.
    pub fn observe(&mut self, sample: device_api::sensors::Snapshot, now: u64) {
        self.expire(now);
        if self.status != Phase::Waiting || !matches!(self.reason, Some(5 | 6)) {
            return;
        }
        if self.advancing(sample, now) {
            self.status = Phase::Ready;
        }
    }

    fn advancing(&mut self, sample: device_api::sensors::Snapshot, now: u64) -> bool {
        let timestamp = |value| match value {
            Observation::Fresh { received_ms, .. } => Some(received_ms),
            _ => None,
        };
        let pressure = match sample.pressure {
            Observation::Fresh { received_ms, .. } => received_ms,
            _ => return false,
        };
        let (Some(first), Some(second)) =
            (timestamp(sample.motion[0]), timestamp(sample.motion[1]))
        else {
            return false;
        };
        let current = [pressure, first, second];
        if current.iter().any(|at| *at <= self.started_ms || *at > now) {
            return false;
        }
        match self.first {
            Some(previous) if current.iter().zip(previous).all(|(next, old)| *next > old) => {
                return true;
            }
            None => self.first = Some(current),
            _ => {}
        }
        false
    }
}
