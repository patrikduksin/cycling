//! ANT sensor links: foreground discovery, rider requests that survive page
//! changes and wait for radio admission, and automatic reconnection.
use super::Runtime;
use crate::screens::sensors::{Action, MenuMessage};
use crate::sensors::scan::Scan;
use device_api::ant::{
    Admission, Ant, AntOperation, CHANNEL_CAPACITY, Error, Identity, LinkState, ScanState, Snapshot,
};

/// A rider request is abandoned if the radio cannot admit it in this time.
const REQUEST_TIMEOUT_MS: u64 = 25_000;
const RECONNECT_INTERVAL_MS: u64 = 5000;

#[derive(Default)]
pub(super) struct Discovery {
    pub(super) scan: Scan,
    /// The rider asked for a search that has not started yet.
    requested: bool,
    /// Transport scan generation already shown, so a new search clears old results.
    generation: u32,
}

#[derive(Clone, Copy)]
pub(super) struct Pending {
    peer: Identity,
    disconnect: bool,
    /// A replacement is waiting for the previous peer of its type to close.
    closing: bool,
    deadline: u64,
}

/// Slots are serviced in index order, which orders their radio requests.
#[derive(Default)]
pub(super) struct Requests([Option<Pending>; CHANNEL_CAPACITY]);

#[derive(Default)]
pub(super) struct Reconnect {
    /// Device types the rider closed or is replacing. Automatic reconnection
    /// must not undo that choice.
    suppressed: heapless::Vec<u8, CHANNEL_CAPACITY>,
    /// Earliest next automatic attempt, by channel index.
    next_attempt_ms: [u64; CHANNEL_CAPACITY],
}

impl Reconnect {
    pub(super) fn is_suppressed(&self, kind: u8) -> bool {
        self.suppressed.contains(&kind)
    }

    fn suppress(&mut self, kind: u8, channels: &[Option<Snapshot>]) {
        // Forget closed types whose channel no longer holds a selection.
        self.suppressed.retain(|&closed| {
            channels
                .iter()
                .flatten()
                .any(|s| s.selected.is_some_and(|p| p.device_type == closed))
        });
        let _ = self.suppressed.push(kind);
    }

    fn allow(&mut self, kind: u8) {
        self.suppressed.retain(|&closed| closed != kind);
    }
}

/// How far a rider request got this tick.
enum Progress {
    Wait,
    /// The previous peer of this type was asked to close.
    Closing,
    Done(Result<Admission, Error>),
}

impl Runtime {
    pub(super) fn service_sensor_links(&mut self, ant: &mut impl Ant, now: u64) {
        let was_scanning = self.discovery.scan.active();
        self.discovery.scan.tick(ant, now);
        self.refresh_discovery_session(ant);
        if was_scanning || self.discovery.scan.active() {
            self.menu.set_message(self.discovery.scan.message());
        }
        self.tick_sensor_requests(ant, now);
        self.start_sensor_search(ant, now);
        self.reconnect_selected(ant, now);
    }

    pub(super) fn request_search(&mut self, ant: &mut impl Ant, now: u64) {
        self.discovery.requested = true;
        self.start_sensor_search(ant, now);
    }

    pub(super) fn cancel_search(&mut self, ant: &mut impl Ant, now: u64) {
        self.discovery.requested = false;
        self.discovery.scan.cancel(ant, now);
    }

    /// Searching waits until no rider request needs the radio.
    fn start_sensor_search(&mut self, ant: &mut impl Ant, now: u64) {
        if self.discovery.requested && self.requests.0.iter().all(Option::is_none) {
            self.discovery.requested = false;
            self.discovery.scan.start(ant, now);
            self.refresh_discovery_session(ant);
            self.menu.set_message(self.discovery.scan.message());
        }
    }

    fn refresh_discovery_session(&mut self, ant: &impl Ant) {
        let generation = ant.scan().generation;
        if self.discovery.generation != generation {
            self.discovery.generation = generation;
            self.menu.search_started();
        }
    }

    fn reconnect_selected(&mut self, ant: &mut impl Ant, now: u64) {
        if !self.display.is_active() || self.discovery.scan.active() {
            return;
        }
        for (index, channel) in ant.channels(now).into_iter().enumerate() {
            let Some(channel) = channel else { continue };
            let Some(peer) = channel.selected else {
                continue;
            };
            if !self.reconnect.is_suppressed(peer.device_type)
                && channel.link == LinkState::Disconnected
                && now >= self.reconnect.next_attempt_ms[index]
                && ant.request(AntOperation::Connect(peer), now) == Ok(Admission::Accepted)
            {
                self.reconnect.next_attempt_ms[index] = now.saturating_add(RECONNECT_INTERVAL_MS);
            }
        }
    }

    pub(super) fn request_sensor(&mut self, action: Action, ant: &mut impl Ant, now: u64) {
        let (peer, disconnect) = match action {
            Action::Connect(peer) => (peer, false),
            Action::Disconnect(kind) => {
                let Some(peer) = ant.channel(kind, now).and_then(|s| s.selected) else {
                    return;
                };
                (peer, true)
            }
            Action::Scan | Action::Ride => return,
        };
        let kind = peer.device_type;
        if self
            .requests
            .0
            .iter()
            .flatten()
            .any(|p| p.peer.device_type == kind)
        {
            return;
        }
        let Some(slot) = self.requests.0.iter_mut().find(|p| p.is_none()) else {
            self.menu.request_failed(peer, MenuMessage::TooManyRequests);
            return;
        };
        *slot = Some(Pending {
            peer,
            disconnect,
            closing: false,
            deadline: now.saturating_add(REQUEST_TIMEOUT_MS),
        });
        if !self.reconnect.is_suppressed(kind) {
            self.reconnect.suppress(kind, &ant.channels(now));
        }
        self.cancel_search(ant, now);
        self.tick_sensor_requests(ant, now);
    }

    fn tick_sensor_requests(&mut self, ant: &mut impl Ant, now: u64) {
        for index in 0..self.requests.0.len() {
            let Some(mut pending) = self.requests.0[index] else {
                continue;
            };
            let result = match self.advance_request(pending, ant, now) {
                Progress::Wait => continue,
                Progress::Closing => {
                    pending.closing = true;
                    self.requests.0[index] = Some(pending);
                    continue;
                }
                Progress::Done(Err(Error::Busy)) if now < pending.deadline => continue,
                Progress::Done(result) => result,
            };
            self.requests.0[index] = None;
            self.finish_request(pending, result);
            self.display.redraw();
        }
    }

    fn advance_request(&self, pending: Pending, ant: &mut impl Ant, now: u64) -> Progress {
        let peer = pending.peer;
        if now >= pending.deadline {
            return Progress::Done(Err(Error::Busy));
        }
        if ant.scan().state == ScanState::Uncertain {
            return Progress::Done(Err(Error::Uncertain));
        }
        if self.discovery.scan.busy(ant) {
            return Progress::Wait;
        }
        if pending.disconnect {
            return Progress::Done(ant.request(AntOperation::Disconnect(peer.device_type), now));
        }
        let current = ant.channel(peer.device_type, now);
        let replacing =
            current.is_some_and(|s| s.selected != Some(peer) && s.link != LinkState::Disconnected);
        if replacing && !pending.closing {
            return match ant.request(AntOperation::Disconnect(peer.device_type), now) {
                Ok(Admission::Accepted) => Progress::Closing,
                Err(Error::Busy) => Progress::Wait,
                Err(error) => Progress::Done(Err(error)),
                Ok(Admission::SendQueued(_)) => Progress::Done(Err(Error::InvalidState)),
            };
        }
        if replacing {
            let lost = current
                .is_some_and(|s| matches!(s.link, LinkState::TimedOut | LinkState::TransportLost));
            return if lost {
                Progress::Done(Err(Error::Uncertain))
            } else {
                Progress::Wait
            };
        }
        let already = current.is_some_and(|s| {
            s.selected == Some(peer)
                && matches!(s.link, LinkState::Connected | LinkState::Connecting)
        });
        if already {
            return Progress::Done(Ok(Admission::Accepted));
        }
        Progress::Done(ant.request(AntOperation::Connect(peer), now))
    }

    fn finish_request(&mut self, pending: Pending, result: Result<Admission, Error>) {
        if result == Ok(Admission::Accepted) {
            self.menu.request_accepted(pending.peer);
            self.menu.set_message(self.discovery.scan.message());
            if !pending.disconnect {
                self.reconnect.allow(pending.peer.device_type);
            }
            return;
        }
        let message = match result {
            Err(Error::Busy) => MenuMessage::Busy,
            Err(Error::Unavailable) => MenuMessage::RadioUnavailable,
            Err(Error::Uncertain) => MenuMessage::RadioLost,
            Err(Error::UnsupportedType) => MenuMessage::TypeUnsupported,
            _ => MenuMessage::RequestFailed,
        };
        if matches!(result, Err(Error::Unavailable | Error::Uncertain)) {
            self.menu.set_message(message);
        }
        self.menu.request_failed(pending.peer, message);
    }
}
