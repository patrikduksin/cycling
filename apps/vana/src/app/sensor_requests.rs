//! Bounded rider intents survive page changes and wait for radio admission.
use super::Runtime;
use crate::screens::sensors::Action;
use device_api::ant::{Admission, Ant, AntOperation, Error, Identity, LinkState};

#[derive(Clone, Copy)]
pub(super) struct Pending {
    peer: Identity,
    disconnect: bool,
    closing: bool,
    deadline: u64,
}

impl Runtime {
    pub(super) fn start_sensor_search(&mut self, ant: &mut impl Ant, now: u64) {
        if self.search_requested && self.sensor_requests.iter().all(Option::is_none) {
            self.search_requested = false;
            self.scan.start(ant, now);
            self.refresh_discovery_session(ant);
            self.menu.set_message(self.scan.message());
        }
    }

    pub(super) fn refresh_discovery_session(&mut self, ant: &impl Ant) {
        let generation = ant.scan().generation;
        if self.discovery_generation != generation {
            self.discovery_generation = generation;
            self.menu.search_started();
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
            _ => return,
        };
        if self
            .sensor_requests
            .iter()
            .flatten()
            .any(|p| p.peer.device_type == peer.device_type)
        {
            return;
        }
        let Some(slot) = self.sensor_requests.iter_mut().find(|p| p.is_none()) else {
            self.menu.request_failed(peer, b"TOO MANY REQUESTS");
            return;
        };
        *slot = Some(Pending {
            peer,
            disconnect,
            closing: false,
            deadline: now.saturating_add(25000),
        });
        // An explicit close or replacement must not be undone by automatic reconnect.
        if !self.dropped_ant.contains(&Some(peer.device_type)) {
            let channels = ant.channels(now);
            for dropped in &mut self.dropped_ant {
                if dropped.is_some_and(|kind| {
                    !channels
                        .iter()
                        .flatten()
                        .any(|s| s.selected.is_some_and(|p| p.device_type == kind))
                }) {
                    *dropped = None;
                }
            }
            if let Some(slot) = self.dropped_ant.iter_mut().find(|slot| slot.is_none()) {
                *slot = Some(peer.device_type);
            }
        }
        self.search_requested = false;
        self.scan.cancel(ant, now);
        self.tick_sensor_requests(ant, now);
    }

    pub(super) fn tick_sensor_requests(&mut self, ant: &mut impl Ant, now: u64) {
        for index in 0..self.sensor_requests.len() {
            let Some(mut pending) = self.sensor_requests[index] else {
                continue;
            };
            let result = if now >= pending.deadline {
                Err(Error::Busy)
            } else if ant.scan().state == device_api::ant::ScanState::Uncertain {
                Err(Error::Uncertain)
            } else if self.scan.busy(ant) {
                continue;
            } else {
                let current = ant.channel(pending.peer.device_type, now);
                if !pending.disconnect
                    && current.is_some_and(|s| {
                        s.selected != Some(pending.peer) && s.link != LinkState::Disconnected
                    })
                {
                    if !pending.closing {
                        match ant.request(AntOperation::Disconnect(pending.peer.device_type), now) {
                            Ok(Admission::Accepted) => {
                                pending.closing = true;
                                self.sensor_requests[index] = Some(pending);
                                continue;
                            }
                            Err(Error::Busy) => continue,
                            Err(error) => Err(error),
                            _ => Err(Error::InvalidState),
                        }
                    } else if current.is_some_and(|s| {
                        matches!(s.link, LinkState::TimedOut | LinkState::TransportLost)
                    }) {
                        Err(Error::Uncertain)
                    } else {
                        continue;
                    }
                } else if !pending.disconnect
                    && current.is_some_and(|s| {
                        s.selected == Some(pending.peer)
                            && matches!(s.link, LinkState::Connected | LinkState::Connecting)
                    })
                {
                    Ok(Admission::Accepted)
                } else {
                    ant.request(
                        if pending.disconnect {
                            AntOperation::Disconnect(pending.peer.device_type)
                        } else {
                            AntOperation::Connect(pending.peer)
                        },
                        now,
                    )
                }
            };
            if result == Err(Error::Busy) && now < pending.deadline {
                continue;
            }
            self.sensor_requests[index] = None;
            if result == Ok(Admission::Accepted) {
                self.menu.request_accepted(pending.peer);
                self.menu.set_message(self.scan.message());
                if !pending.disconnect {
                    for dropped in &mut self.dropped_ant {
                        if *dropped == Some(pending.peer.device_type) {
                            *dropped = None;
                        }
                    }
                }
            } else {
                let message: &'static [u8] = match result {
                    Err(Error::Busy) => b"BUSY - RETRY",
                    Err(Error::Unavailable) => b"RADIO UNAVAILABLE",
                    Err(Error::Uncertain) => b"RADIO LOST - REBOOT",
                    Err(Error::UnsupportedType) => b"TYPE UNSUPPORTED",
                    _ => b"REQUEST FAILED",
                };
                if matches!(result, Err(Error::Unavailable | Error::Uncertain)) {
                    self.menu.set_message(message);
                }
                self.menu.request_failed(pending.peer, message);
            }
            self.next_display = 0;
        }
    }
}
