//! Foreground discovery observes the transport without disturbing selected peers.
use device_api::ant::{Admission, Ant, AntOperation, Error, ScanState};

#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Starting,
    Scanning,
    Complete,
    Finishing,
    Cancelling,
    Stopping,
    Cancelled,
    Failed,
    Unavailable,
    Uncertain,
}
#[derive(Default)]
pub struct Scan {
    phase: Phase,
    cancel_deadline: u64,
}
impl Scan {
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            Phase::Starting
                | Phase::Scanning
                | Phase::Finishing
                | Phase::Cancelling
                | Phase::Stopping
        )
    }
    pub fn busy(&self, ant: &impl Ant) -> bool {
        self.active()
            || matches!(
                ant.scan().state,
                ScanState::Starting | ScanState::Active | ScanState::Stopping
            )
    }
    pub fn overview_message(&self) -> Option<&'static [u8]> {
        (self.phase != Phase::Idle).then(|| self.message())
    }
    pub fn message(&self) -> &'static [u8] {
        match self.phase {
            Phase::Idle => b"PICK SENSOR",
            Phase::Starting => b"STARTING SCAN",
            Phase::Scanning => b"SCANNING...",
            Phase::Cancelling | Phase::Stopping => b"STOPPING SCAN",
            Phase::Cancelled => b"SCAN CANCELLED",
            Phase::Finishing => b"FINISHING SCAN",
            Phase::Complete => b"SCAN DONE - PICK",
            Phase::Failed => b"SCAN FAILED - RETRY",
            Phase::Uncertain => b"RADIO LOST - REBOOT",
            Phase::Unavailable => b"SCAN UNAVAILABLE",
        }
    }
    pub fn start(&mut self, ant: &mut impl Ant, now: u64) {
        if ant.scan().state == ScanState::Uncertain {
            self.phase = Phase::Uncertain;
            return;
        }
        if self.active() || ant.scan().state != ScanState::Idle {
            return;
        }
        if ant.availability() != device_api::observation::Availability::Ready
            || (!ant.capabilities().concurrent_scan
                && ant.channels(now).iter().flatten().any(|s| {
                    matches!(
                        s.link,
                        device_api::ant::LinkState::Connected
                            | device_api::ant::LinkState::Connecting
                    )
                }))
        {
            self.phase = Phase::Unavailable;
            return;
        }
        self.phase = match ant.request(AntOperation::Scan(10000), now) {
            Ok(Admission::Accepted) => Phase::Starting,
            Err(Error::Unavailable) => Phase::Unavailable,
            Err(Error::Uncertain) => Phase::Uncertain,
            _ => Phase::Failed,
        };
    }
    pub fn cancel(&mut self, ant: &mut impl Ant, now: u64) {
        if matches!(self.phase, Phase::Cancelling | Phase::Stopping) {
            return;
        }
        if matches!(ant.scan().state, ScanState::Starting | ScanState::Active) {
            self.phase = Phase::Cancelling;
            self.cancel_deadline = now.saturating_add(2000);
            self.tick(ant, now);
        }
    }
    pub fn tick(&mut self, ant: &mut impl Ant, now: u64) {
        if !self.active() {
            return;
        }
        let state = ant.scan().state;
        if state == ScanState::Uncertain {
            self.phase = Phase::Uncertain;
        } else if state == ScanState::Idle {
            self.phase = if matches!(self.phase, Phase::Cancelling | Phase::Stopping) {
                Phase::Cancelled
            } else {
                Phase::Complete
            };
        } else if self.phase == Phase::Cancelling {
            self.phase = match ant.request(AntOperation::StopScan, now) {
                Ok(Admission::Accepted) => Phase::Stopping,
                Err(Error::Busy) if now < self.cancel_deadline => Phase::Cancelling,
                _ => Phase::Failed,
            };
        } else if self.phase != Phase::Stopping {
            self.phase = match state {
                ScanState::Starting => Phase::Starting,
                ScanState::Stopping => Phase::Finishing,
                _ => Phase::Scanning,
            };
        }
    }
}
