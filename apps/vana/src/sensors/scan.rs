//! Foreground discovery observes the transport without disturbing selected peers.
use device_api::ant::{Admission, AntOperation, Error, LinkState, ScanState, Snapshot};
use device_api::observation::Availability;

#[cfg(test)]
mod explore;

const SCAN_MS: u32 = 10_000;
const START_DEADLINE_MS: u64 = 15_000;
const CANCEL_DEADLINE_MS: u64 = 2_000;

#[derive(Default, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(test, derive(Hash, Debug))]
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

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    Idle,
    WaitingForRadio,
    Starting,
    Scanning,
    Stopping,
    Cancelled,
    Finishing,
    Done,
    Failed,
    Unavailable,
    Uncertain,
}

#[cfg(test)]
impl Status {
    pub const ALL: [Status; 11] = [
        Status::Idle,
        Status::WaitingForRadio,
        Status::Starting,
        Status::Scanning,
        Status::Stopping,
        Status::Cancelled,
        Status::Finishing,
        Status::Done,
        Status::Failed,
        Status::Unavailable,
        Status::Uncertain,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Status::Idle => "Idle",
            Status::WaitingForRadio => "WaitingForRadio",
            Status::Starting => "Starting",
            Status::Scanning => "Scanning",
            Status::Stopping => "Stopping",
            Status::Cancelled => "Cancelled",
            Status::Finishing => "Finishing",
            Status::Done => "Done",
            Status::Failed => "Failed",
            Status::Unavailable => "Unavailable",
            Status::Uncertain => "Uncertain",
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(Hash))]
pub enum Op {
    Start,
    Stop,
}

impl Op {
    pub const fn operation(self) -> AntOperation {
        match self {
            Op::Start => AntOperation::Scan(SCAN_MS),
            Op::Stop => AntOperation::StopScan,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[cfg_attr(test, derive(Hash))]
pub enum Links {
    Free,
    Settling,
    Held,
}

impl Links {
    pub fn of(channels: &[Option<Snapshot>], concurrent_scan: bool) -> Links {
        let links = || channels.iter().flatten().map(|snapshot| snapshot.link);
        if !concurrent_scan
            && links().any(|link| matches!(link, LinkState::Connected | LinkState::Connecting))
        {
            Links::Held
        } else if links()
            .any(|link| matches!(link, LinkState::Connecting | LinkState::Disconnecting))
        {
            Links::Settling
        } else {
            Links::Free
        }
    }
}

/// The radio as the search reads it, taken afresh before every step.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Seen {
    pub radio: ScanState,
    pub availability: Availability,
    pub links: Links,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Start,
    Cancel,
    Tick,
    Answer(Op, Result<Admission, Error>),
}

#[derive(Default)]
#[cfg_attr(test, derive(Clone, Copy, PartialEq, Eq, Hash, Debug))]
pub struct Scan {
    phase: Phase,
    cancel_deadline: u64,
    pending_start: Option<u64>,
}

impl Scan {
    pub fn active(&self) -> bool {
        self.pending_start.is_some() || self.running()
    }

    fn running(&self) -> bool {
        matches!(
            self.phase,
            Phase::Starting
                | Phase::Scanning
                | Phase::Finishing
                | Phase::Cancelling
                | Phase::Stopping
        )
    }

    pub fn busy(&self, radio: ScanState) -> bool {
        self.active()
            || matches!(
                radio,
                ScanState::Starting | ScanState::Active | ScanState::Stopping
            )
    }

    pub fn status(&self) -> Status {
        if self.pending_start.is_some() && !self.running() {
            return Status::WaitingForRadio;
        }
        match self.phase {
            Phase::Idle => Status::Idle,
            Phase::Starting => Status::Starting,
            Phase::Scanning => Status::Scanning,
            Phase::Cancelling | Phase::Stopping => Status::Stopping,
            Phase::Cancelled => Status::Cancelled,
            Phase::Finishing => Status::Finishing,
            Phase::Complete => Status::Done,
            Phase::Failed => Status::Failed,
            Phase::Uncertain => Status::Uncertain,
            Phase::Unavailable => Status::Unavailable,
        }
    }

    /// Take `event` and return the request to submit, if any. A returned
    /// request's answer comes back as the next event, before any other.
    pub fn step(&mut self, event: Event, seen: &Seen, now: u64) -> Option<Op> {
        match event {
            Event::Start => self.start(seen, now),
            Event::Cancel => self.cancel(seen, now),
            Event::Tick => self.tick(seen, now),
            Event::Answer(Op::Start, result) => {
                if result == Err(Error::Busy) {
                    return None;
                }
                self.pending_start = None;
                self.phase = match result {
                    Ok(Admission::Accepted) => Phase::Starting,
                    Err(Error::Unavailable) => Phase::Unavailable,
                    Err(Error::Uncertain) => Phase::Uncertain,
                    _ => Phase::Failed,
                };
                None
            }
            Event::Answer(Op::Stop, result) => {
                self.phase = match result {
                    Ok(Admission::Accepted) => Phase::Stopping,
                    Err(Error::Busy) if now < self.cancel_deadline => Phase::Cancelling,
                    _ => Phase::Failed,
                };
                self.retry(seen, now)
            }
        }
    }

    fn start(&mut self, seen: &Seen, now: u64) -> Option<Op> {
        if seen.radio == ScanState::Uncertain {
            self.phase = Phase::Uncertain;
            return None;
        }
        if matches!(self.phase, Phase::Starting | Phase::Scanning) {
            return None;
        }
        self.pending_start
            .get_or_insert(now.saturating_add(START_DEADLINE_MS));
        self.try_start(seen, now)
    }

    fn cancel(&mut self, seen: &Seen, now: u64) -> Option<Op> {
        self.pending_start = None;
        if matches!(self.phase, Phase::Cancelling | Phase::Stopping) {
            return None;
        }
        if !matches!(seen.radio, ScanState::Starting | ScanState::Active) {
            return None;
        }
        self.phase = Phase::Cancelling;
        self.cancel_deadline = now.saturating_add(CANCEL_DEADLINE_MS);
        self.tick(seen, now)
    }

    fn tick(&mut self, seen: &Seen, now: u64) -> Option<Op> {
        if let Some(stop) = self.advance(seen) {
            return Some(stop);
        }
        self.retry(seen, now)
    }

    fn retry(&mut self, seen: &Seen, now: u64) -> Option<Op> {
        if seen.radio == ScanState::Uncertain {
            self.pending_start = None;
            return None;
        }
        self.try_start(seen, now)
    }

    fn try_start(&mut self, seen: &Seen, now: u64) -> Option<Op> {
        let deadline = self.pending_start?;
        if now >= deadline {
            self.pending_start = None;
            if !self.running() {
                self.phase = Phase::Failed;
            }
            return None;
        }
        if self.running()
            || seen.radio != ScanState::Idle
            || seen.availability == Availability::Initializing
        {
            return None;
        }
        if seen.availability != Availability::Ready || seen.links == Links::Held {
            self.phase = Phase::Unavailable;
            self.pending_start = None;
            return None;
        }
        (seen.links == Links::Free).then_some(Op::Start)
    }

    fn advance(&mut self, seen: &Seen) -> Option<Op> {
        if !self.running() {
            return None;
        }
        match seen.radio {
            ScanState::Uncertain => self.phase = Phase::Uncertain,
            ScanState::Idle => {
                self.phase = if matches!(self.phase, Phase::Cancelling | Phase::Stopping) {
                    Phase::Cancelled
                } else {
                    Phase::Complete
                };
            }
            _ if self.phase == Phase::Cancelling => return Some(Op::Stop),
            _ if self.phase == Phase::Stopping => {}
            ScanState::Starting => self.phase = Phase::Starting,
            ScanState::Stopping => self.phase = Phase::Finishing,
            ScanState::Active => self.phase = Phase::Scanning,
        }
        None
    }
}
