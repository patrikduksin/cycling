//! Foreground scan transaction: close selected channels, discover, restore selections.
use device_api::ant::Admission;
use device_api::ant::Ant;
use device_api::ant::AntOperation;
use device_api::ant::CHANNEL_CAPACITY;
use device_api::ant::Error;
use device_api::ant::Identity;
use device_api::ant::LinkState;
#[derive(Default, Clone, Copy, PartialEq, Eq)]
enum Phase {
    #[default]
    Idle,
    Closing,
    Scanning,
    Restoring,
    Failed,
}
#[derive(Default)]
pub struct Scan {
    phase: Phase,
    peers: [Option<Identity>; CHANNEL_CAPACITY],
    sent: [bool; CHANNEL_CAPACITY],
    deadline: u64,
    next: u64,
}
impl Scan {
    pub fn active(&self) -> bool {
        matches!(
            self.phase,
            Phase::Closing | Phase::Scanning | Phase::Restoring
        )
    }
    pub fn message(&self) -> &'static [u8] {
        match self.phase {
            Phase::Idle => b"PICK SENSOR",
            Phase::Closing => b"PAUSING SENSORS",
            Phase::Scanning => b"SCANNING...",
            Phase::Restoring => b"RECONNECTING",
            Phase::Failed => b"SCAN FAILED - BACK",
        }
    }
    pub fn start(&mut self, ant: &mut impl Ant, now: u64, dropped: &[Option<u8>]) {
        if self.active() {
            return;
        }
        self.peers = ant.channels(now).map(|s| {
            s.and_then(|s| s.selected)
                .filter(|peer| !dropped.contains(&Some(peer.device_type)))
        });
        self.sent.fill(false);
        self.phase = Phase::Closing;
        self.deadline = now + 15000;
        self.next = now;
        self.tick(ant, now);
    }
    pub fn tick(&mut self, ant: &mut impl Ant, now: u64) {
        if !self.active() || now < self.next {
            return;
        }
        self.next = now + 100;
        if now >= self.deadline {
            self.phase = Phase::Failed;
            return;
        }
        match self.phase {
            Phase::Closing => {
                for (i, peer) in self.peers.iter().enumerate() {
                    let Some(peer) = peer else {
                        continue;
                    };
                    let channel = ant.channel(peer.device_type, now);
                    if channel
                        .is_none_or(|s| matches!(s.link, LinkState::Disconnected | LinkState::Idle))
                    {
                        continue;
                    }
                    if !self.sent[i] {
                        match ant.request(AntOperation::Disconnect(peer.device_type), now) {
                            Ok(Admission::Accepted) => self.sent[i] = true,
                            Err(Error::Busy) => {}
                            _ => self.phase = Phase::Failed,
                        }
                    }
                    return;
                }
                match ant.request(AntOperation::Scan(10000), now) {
                    Ok(Admission::Accepted) => {
                        self.phase = Phase::Scanning;
                        self.deadline = now + 15000;
                    }
                    Err(Error::Busy) => {}
                    _ => self.phase = Phase::Failed,
                }
            }
            Phase::Scanning if !ant.scanning() => {
                self.phase = Phase::Restoring;
                self.sent.fill(false);
                self.deadline = now + 15000;
            }
            Phase::Restoring => {
                for (i, peer) in self.peers.iter().enumerate() {
                    let Some(peer) = peer else {
                        continue;
                    };
                    if self.sent[i] {
                        continue;
                    }
                    match ant.request(AntOperation::Connect(*peer), now) {
                        Ok(Admission::Accepted) => self.sent[i] = true,
                        Err(Error::Busy) => {}
                        _ => self.phase = Phase::Failed,
                    }
                    return;
                }
                self.phase = Phase::Idle;
            }
            _ => {}
        }
    }
}
