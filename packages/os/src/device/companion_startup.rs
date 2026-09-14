//! Fixed companion startup operations. No runtime power-command interface.
use cycling_os::capabilities::Observation;

pub fn frame() -> [u8; 16] {
    cycling_os::companion::command(16, [0xe2, 2, 0, 0, 0, 1, 0, 0])
}

/// Recovered N22 operation 0/value 7 enters the normal initialization dispatcher.
/// Only used once after an explicit charging acknowledgment with no acquisition.
pub fn wake_frame() -> [u8; 16] {
    cycling_os::companion::command(16, [0xe2, 2, 0, 0, 0, 7, 0, 0])
}

pub struct Startup {
    status: &'static str,
    started_ms: u64,
    first: Option<[u64; 3]>,
    reason: Option<u8>,
    activity: bool,
    transport_lost: bool,
    wake_requested: bool,
}

impl Startup {
    pub const fn new() -> Self {
        Self {
            status: "not_submitted",
            started_ms: 0,
            first: None,
            reason: None,
            activity: false,
            transport_lost: false,
            wake_requested: false,
        }
    }

    pub fn begin(&mut self, now: u64) {
        if self.status == "not_submitted" {
            self.status = "probing";
            self.started_ms = now;
        }
    }

    pub fn ant_allowed(&self) -> bool {
        matches!(
            self.status,
            "already_running" | "running_degraded" | "ready"
        )
    }

    /// Observe before considering a single startup acknowledgment. Any sensor
    /// or radio traffic preserves the running companion, even if degraded.
    pub fn probe(
        &mut self,
        sample: cycling_os::companion_sensors::Snapshot,
        now: u64,
        bridge_fresh: bool,
        transport_clean: bool,
        radio_seen: bool,
    ) -> bool {
        if self.status != "probing" {
            return false;
        }
        self.activity |= radio_seen;
        self.transport_lost |= !transport_clean;
        if !matches!(sample.pressure, Observation::Unavailable)
            || sample
                .motion
                .iter()
                .any(|value| !matches!(value, Observation::Unavailable))
        {
            self.activity = true;
        }
        let advancing = self.advancing(sample, now);
        if now.saturating_sub(self.started_ms) < 3000 {
            return false;
        }
        if self.transport_lost {
            self.status = "unavailable";
        } else if self.activity {
            self.status = if advancing {
                "already_running"
            } else {
                "running_degraded"
            };
        } else if !bridge_fresh {
            // A late battery/power report is not a transport failure. Keep
            // observing without transmitting until the bridge is established.
            return false;
        } else {
            self.status = "pending";
            self.first = None;
            return true;
        }
        false
    }

    /// A charging boot needs the receiver's normal initialization transition.
    /// Never reinitialize a companion that has shown sensor or radio activity.
    pub fn wake(
        &mut self,
        sample: cycling_os::companion_sensors::Snapshot,
        bridge_fresh: bool,
        transport_clean: bool,
        radio_seen: bool,
    ) -> bool {
        if self.status != "charging" || self.reason != Some(4) || self.wake_requested {
            return false;
        }
        self.activity |= radio_seen
            || !matches!(sample.pressure, Observation::Unavailable)
            || sample
                .motion
                .iter()
                .any(|value| !matches!(value, Observation::Unavailable));
        self.transport_lost |= !transport_clean;
        if self.activity {
            self.status = "running_degraded";
        } else if self.transport_lost {
            self.status = "unavailable";
        } else if bridge_fresh {
            self.wake_requested = true;
            self.status = "wake_pending";
            return true;
        }
        false
    }

    pub fn wake_submitted(&mut self, result: Result<usize, ()>, now: u64) {
        if self.status != "wake_pending" {
            return;
        }
        self.started_ms = now;
        self.first = None;
        self.status = if result == Ok(16) {
            "waiting"
        } else {
            "uncertain"
        };
    }

    /// A short FIFO write may have reached the companion. Never retry it.
    pub fn submitted(&mut self, result: Result<usize, ()>, now: u64) {
        if self.status != "pending" {
            return;
        }
        self.started_ms = now;
        self.status = if result == Ok(16) {
            "waiting"
        } else {
            "uncertain"
        };
    }

    pub fn status(&self) -> &'static str {
        self.status
    }

    pub fn reason(&self) -> Option<u8> {
        self.reason
    }

    fn expire(&mut self, now: u64) {
        if self.status == "waiting" && now.saturating_sub(self.started_ms) >= 10_000 {
            self.status = "timed_out";
        }
    }

    /// The caller validates the frame class and CRC. Charging acknowledgment
    /// does not start acquisition; a later manual-on report may do so.
    pub fn report(&mut self, group: u8, payload: &[u8], now: u64) {
        if group == 16 && matches!(payload, [0xf1, 1..=3, ..]) {
            self.activity = true;
        }
        self.expire(now);
        if !matches!(self.status, "waiting" | "charging") || group != 16 {
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
            self.status = "charging";
        } else {
            self.status = "waiting";
            self.started_ms = now;
        }
    }

    /// Each sensor must be fresh and advance independently. An identity or
    /// power-reason reply and a successful UART write cannot establish readiness.
    pub fn observe(&mut self, sample: cycling_os::companion_sensors::Snapshot, now: u64) {
        self.expire(now);
        if self.status != "waiting" || !matches!(self.reason, Some(5 | 6)) {
            return;
        }
        if self.advancing(sample, now) {
            self.status = "ready";
        }
    }

    fn advancing(&mut self, sample: cycling_os::companion_sensors::Snapshot, now: u64) -> bool {
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
