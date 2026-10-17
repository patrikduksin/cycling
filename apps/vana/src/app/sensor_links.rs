//! ANT sensor links: foreground discovery, the rider's saved selections and
//! their automatic connection.
//!
//! Saved selections are the only connection intent. Connection work runs every
//! tick whatever the open screen or display state, submits at most one radio
//! request per tick, takes saved sensors in turn, and backs off after each
//! failed search. Disconnect removes a selection; its channel is then closed
//! and no later event can reconnect it.
use super::Runtime;
use super::preferences::Persistence;
use crate::screens::sensors::{Action, Connection, MenuMessage, Selected};
use crate::sensors::saved::Saved;
use crate::sensors::scan::Scan;
use device_api::ant::{
    Admission, Ant, AntOperation, CHANNEL_CAPACITY, Error, Identity, LinkState, STALE_MS,
    ScanState, Snapshot,
};
use device_api::observation::Availability;

/// Delay before the next search after this many unsuccessful ones; the
/// transport's own connect timeout comes on top of it.
const fn backoff_ms(failures: u8) -> u64 {
    match failures {
        0 => 0,
        failures => {
            let delay = 1_000u64 << (failures - 1);
            if delay < MAX_BACKOFF_MS {
                delay
            } else {
                MAX_BACKOFF_MS
            }
        }
    }
}
const MAX_BACKOFF_MS: u64 = 30_000;
const MAX_FAILURES: u8 = 6;
/// A request the radio refused as momentarily busy is offered again after
/// this, doubling while the refusals continue.
const BUSY_RETRY_MS: u64 = 250;
const MAX_BUSY_RETRY_MS: u64 = 2_000;
/// Radio-wide refusals are retried no faster than this.
const RADIO_RETRY_MS: u64 = 5_000;
/// A connected channel that has heard nothing for this long is reopened, in
/// case the radio never reports the loss.
const SILENT_RECONNECT_MS: u64 = 30_000;
/// Removed sensors whose channel failed are closed this many times at most.
const CLOSE_ATTEMPTS: u8 = 3;

#[derive(Default)]
pub(super) struct Discovery {
    pub(super) scan: Scan,
    /// The rider asked for a search that has not started yet.
    requested: bool,
    /// Transport scan generation already shown, so a new search clears old results.
    generation: u32,
}

/// Retry state of one saved sensor or closing channel.
#[derive(Clone, Copy, Default)]
struct Schedule {
    /// Unsuccessful requests since data last arrived.
    failures: u8,
    /// Consecutive momentary refusals.
    busy: u8,
    /// Earliest next request, valid while armed.
    due_ms: u64,
    /// The current need has its delay; observations do not move it.
    armed: bool,
    /// The radio's last refusal, until data arrives or the selection changes.
    refused: Option<Error>,
    /// When the current connected session was first observed.
    connected_ms: Option<u64>,
}

impl Schedule {
    fn arm(&mut self, now: u64) {
        if !self.armed {
            self.armed = true;
            self.due_ms = now.saturating_add(backoff_ms(self.failures));
        }
    }

    fn due(&self, now: u64) -> bool {
        self.armed && now >= self.due_ms
    }

    fn submitted(&mut self, result: Result<Admission, Error>, counts: bool, now: u64) {
        match result {
            Ok(_) => {
                if counts {
                    self.failures = (self.failures + 1).min(MAX_FAILURES);
                }
                self.busy = 0;
                self.armed = false;
                self.refused = None;
            }
            // Momentary: a pending companion request, or no channel to close yet.
            Err(Error::Busy | Error::InvalidState | Error::Disconnected) => {
                let delay = (BUSY_RETRY_MS << self.busy.min(3)).min(MAX_BUSY_RETRY_MS);
                self.busy = self.busy.saturating_add(1);
                self.due_ms = now.saturating_add(delay);
            }
            // Retrying cannot help until the rider selects again.
            Err(error @ (Error::UnsupportedType | Error::InvalidIdentity)) => {
                self.refused = Some(error);
                self.due_ms = u64::MAX;
            }
            Err(error) => {
                self.refused = Some(error);
                self.due_ms = now.saturating_add(RADIO_RETRY_MS);
            }
        }
    }
}

/// A removed sensor's device type whose channel may still be open.
#[derive(Clone, Copy)]
struct Closing {
    kind: u8,
    schedule: Schedule,
}

/// What a saved sensor or closing channel needs from the radio now.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Need {
    Nothing,
    Connect,
    Close,
}

#[derive(Clone, Copy)]
enum Target {
    Saved(usize),
    Closing(usize),
}

#[derive(Default)]
pub(super) struct Links {
    pub(super) saved: Saved,
    schedules: [Schedule; CHANNEL_CAPACITY],
    closing: heapless::Vec<Closing, CHANNEL_CAPACITY>,
    /// Saved slot served first next time, so no sensor starves the others.
    next: usize,
    /// Save state last shown on the sensor page.
    shown: Option<Persistence>,
}

fn channel_of(channels: &[Option<Snapshot>], kind: u8) -> Option<Snapshot> {
    channels
        .iter()
        .flatten()
        .find(|s| s.selected.is_some_and(|p| p.device_type == kind))
        .copied()
}

impl Links {
    /// Whether a packet belongs to a sensor VANA is closing or replacing.
    pub(super) fn rejects(&self, peer: Identity) -> bool {
        self.closing.iter().any(|c| c.kind == peer.device_type)
            || self
                .saved
                .of_type(peer.device_type)
                .is_some_and(|(_, saved)| saved != peer)
    }

    fn need(&self, slot: usize, peer: Identity, channel: Option<Snapshot>, now: u64) -> Need {
        let Some(channel) = channel else {
            return Need::Connect;
        };
        if channel.selected != Some(peer) {
            // A previous peer of this type must close before this one opens.
            return match channel.link {
                LinkState::Disconnected => Need::Connect,
                LinkState::Disconnecting => Need::Nothing,
                _ => Need::Close,
            };
        }
        match channel.link {
            LinkState::Connecting | LinkState::Disconnecting => Need::Nothing,
            LinkState::Connected => {
                let connected = self.schedules[slot].connected_ms.unwrap_or(now);
                let silent = channel
                    .age_ms
                    .unwrap_or_else(|| now.saturating_sub(connected));
                if silent >= SILENT_RECONNECT_MS {
                    Need::Close
                } else {
                    Need::Nothing
                }
            }
            LinkState::Idle
            | LinkState::Disconnected
            | LinkState::TimedOut
            | LinkState::TransportLost => Need::Connect,
        }
    }

    fn close_need(closing: &Closing, channel: Option<Snapshot>) -> Option<Need> {
        let channel = channel?;
        match channel.link {
            LinkState::Idle | LinkState::Disconnected => None,
            LinkState::Disconnecting => Some(Need::Nothing),
            LinkState::Connecting | LinkState::Connected => Some(Need::Close),
            // A failed channel delivers no data; stop after bounded cleanup.
            LinkState::TimedOut | LinkState::TransportLost => {
                (closing.schedule.failures < CLOSE_ATTEMPTS).then_some(Need::Close)
            }
        }
    }

    /// Track observed link state: fresh data resets the backoff, a new need
    /// is given its delay, and finished closes are dropped.
    fn observe(&mut self, channels: &[Option<Snapshot>], now: u64) {
        for slot in 0..CHANNEL_CAPACITY {
            let Some(peer) = self.saved.get(slot) else {
                continue;
            };
            let channel = channel_of(channels, peer.device_type);
            let ours = channel.filter(|s| s.selected == Some(peer));
            let schedule = &mut self.schedules[slot];
            match ours {
                Some(s) if s.link == LinkState::Connected => {
                    schedule.connected_ms.get_or_insert(now);
                    if s.age_ms.is_some_and(|age| age < STALE_MS) {
                        schedule.failures = 0;
                        schedule.refused = None;
                    }
                }
                _ => schedule.connected_ms = None,
            }
            if self.need(slot, peer, channel, now) == Need::Nothing {
                self.schedules[slot].armed = false;
            } else {
                self.schedules[slot].arm(now);
            }
        }
        self.closing.retain_mut(|closing| {
            match Self::close_need(closing, channel_of(channels, closing.kind)) {
                None => false,
                Some(Need::Close) => {
                    closing.schedule.arm(now);
                    true
                }
                Some(_) => {
                    closing.schedule.armed = false;
                    true
                }
            }
        });
    }

    /// The next due request, closes first; saved sensors are taken in turn.
    fn next_request(
        &mut self,
        channels: &[Option<Snapshot>],
        now: u64,
    ) -> Option<(AntOperation, Target)> {
        for (index, closing) in self.closing.iter().enumerate() {
            if closing.schedule.due(now)
                && Self::close_need(closing, channel_of(channels, closing.kind))
                    == Some(Need::Close)
            {
                return Some((
                    AntOperation::Disconnect(closing.kind),
                    Target::Closing(index),
                ));
            }
        }
        for offset in 0..CHANNEL_CAPACITY {
            let slot = (self.next + offset) % CHANNEL_CAPACITY;
            let Some(peer) = self.saved.get(slot) else {
                continue;
            };
            if !self.schedules[slot].due(now) {
                continue;
            }
            let operation = match self.need(slot, peer, channel_of(channels, peer.device_type), now)
            {
                Need::Nothing => continue,
                Need::Connect => AntOperation::Connect(peer),
                Need::Close => AntOperation::Disconnect(peer.device_type),
            };
            self.next = (slot + 1) % CHANNEL_CAPACITY;
            return Some((operation, Target::Saved(slot)));
        }
        None
    }

    fn submitted(
        &mut self,
        target: Target,
        connect: bool,
        result: Result<Admission, Error>,
        now: u64,
    ) {
        match target {
            Target::Saved(slot) => self.schedules[slot].submitted(result, connect, now),
            Target::Closing(index) => {
                if let Some(closing) = self.closing.get_mut(index) {
                    closing.schedule.submitted(result, true, now);
                }
            }
        }
    }

    /// A saved sensor still waits for its first connection attempt. A new
    /// choice, or a restart, goes ahead of discovery.
    fn first_attempt_pending(&self, channels: &[Option<Snapshot>], now: u64) -> bool {
        self.saved.peers().any(|(slot, peer)| {
            let schedule = &self.schedules[slot];
            schedule.failures == 0
                && schedule.refused.is_none()
                && self.need(slot, peer, channel_of(channels, peer.device_type), now)
                    != Need::Nothing
        })
    }

    /// The rider's explicit choice; replaces any saved peer of its type.
    fn select(&mut self, peer: Identity) -> bool {
        let Some(slot) = self.saved.select(peer) else {
            return false;
        };
        self.schedules[slot] = Schedule::default();
        self.closing.retain(|c| c.kind != peer.device_type);
        true
    }

    /// Disconnect: forget the selection and close whatever channel it had.
    fn remove(&mut self, kind: u8) -> bool {
        let Some((slot, _)) = self.saved.of_type(kind) else {
            return false;
        };
        self.saved.remove(kind);
        self.schedules[slot] = Schedule::default();
        if !self.closing.iter().any(|c| c.kind == kind) {
            // Cannot fail: one entry per device type, and at most
            // CHANNEL_CAPACITY types are ever selected.
            let _ = self.closing.push(Closing {
                kind,
                schedule: Schedule::default(),
            });
        }
        true
    }

    /// Rider-facing state of a saved sensor, from observations only.
    fn connection(
        &self,
        slot: usize,
        peer: Identity,
        channel: Option<Snapshot>,
        radio: Radio,
    ) -> Connection {
        let schedule = &self.schedules[slot];
        if matches!(
            schedule.refused,
            Some(Error::UnsupportedType | Error::InvalidIdentity)
        ) {
            return Connection::Unsupported;
        }
        match channel {
            Some(s) if s.selected == Some(peer) => match s.link {
                LinkState::Connected if s.age_ms.is_none() => return Connection::WaitingForData,
                LinkState::Connected if s.stale => return Connection::DataLost,
                LinkState::Connected => return Connection::Live,
                LinkState::Connecting => return Connection::Searching,
                _ => {}
            },
            Some(s) if s.link != LinkState::Disconnected => return Connection::Replacing,
            _ => {}
        }
        if radio == Radio::Unavailable
            || matches!(
                schedule.refused,
                Some(Error::Unavailable | Error::Uncertain)
            )
        {
            return Connection::RadioUnavailable;
        }
        let searched = channel.is_some_and(|s| {
            s.selected == Some(peer)
                && matches!(s.link, LinkState::TimedOut | LinkState::Disconnected)
        });
        if radio == Radio::Ready && searched && schedule.failures > 0 {
            Connection::NotFound
        } else {
            Connection::WaitingForRadio
        }
    }
}

/// Whether the radio can take connection work now.
#[derive(Clone, Copy, Eq, PartialEq)]
enum Radio {
    Ready,
    /// Starting, or discovery owns it briefly.
    Waiting,
    /// Unavailable, or its discovery state is uncertain until it restarts.
    Unavailable,
}

impl Runtime {
    pub(super) fn service_sensor_links(&mut self, ant: &mut impl Ant, now: u64) {
        let was_scanning = self.discovery.scan.active();
        self.discovery.scan.tick(ant, now);
        self.refresh_discovery_session(ant);
        if was_scanning || self.discovery.scan.active() {
            self.menu.set_message(self.discovery.scan.message());
        }
        self.start_sensor_search(ant, now);
        self.advance_links(ant, now);
        self.show_persistence();
    }

    pub(super) fn request_search(&mut self, ant: &mut impl Ant, now: u64) {
        self.discovery.requested = true;
        self.start_sensor_search(ant, now);
    }

    pub(super) fn cancel_search(&mut self, ant: &mut impl Ant, now: u64) {
        self.discovery.requested = false;
        self.discovery.scan.cancel(ant, now);
    }

    /// Searching waits until newly selected or restored sensors have been
    /// asked for once; their searches then delay discovery, not the reverse.
    fn start_sensor_search(&mut self, ant: &mut impl Ant, now: u64) {
        if !self.discovery.requested {
            return;
        }
        if ant.availability() == Availability::Ready
            && self.links.first_attempt_pending(&ant.channels(now), now)
        {
            self.menu.set_message(MenuMessage::SearchPending);
            return;
        }
        self.discovery.requested = false;
        self.discovery.scan.start(ant, now);
        self.refresh_discovery_session(ant);
        self.menu.set_message(self.discovery.scan.message());
    }

    fn refresh_discovery_session(&mut self, ant: &impl Ant) {
        let generation = ant.scan().generation;
        if self.discovery.generation != generation {
            self.discovery.generation = generation;
            self.menu.search_started();
        }
    }

    fn radio(&self, ant: &impl Ant) -> Radio {
        match ant.availability() {
            Availability::Ready if ant.scan().state == ScanState::Uncertain => Radio::Unavailable,
            Availability::Ready if self.discovery.scan.busy(ant) => Radio::Waiting,
            Availability::Ready => Radio::Ready,
            Availability::Initializing => Radio::Waiting,
            _ => Radio::Unavailable,
        }
    }

    /// Submit at most one due connection request.
    fn advance_links(&mut self, ant: &mut impl Ant, now: u64) {
        let channels = ant.channels(now);
        self.links.observe(&channels, now);
        if self.radio(ant) != Radio::Ready {
            return;
        }
        let Some((operation, target)) = self.links.next_request(&channels, now) else {
            return;
        };
        let connect = matches!(operation, AntOperation::Connect(_));
        let result = ant.request(operation, now);
        self.links.submitted(target, connect, result, now);
        self.display.redraw();
    }

    pub(super) fn request_sensor(&mut self, action: Action, ant: &mut impl Ant, now: u64) {
        let changed = match action {
            Action::Connect(peer) => {
                let selected = self.links.select(peer);
                if !selected {
                    self.menu.set_message(MenuMessage::SlotsFull);
                }
                selected
            }
            Action::Disconnect(kind) => self.links.remove(kind),
            Action::Scan | Action::Ride => false,
        };
        if changed {
            self.preferences.set_sensors(self.links.saved, now);
            // Let the radio serve the change before any further discovery.
            self.cancel_search(ant, now);
            self.advance_links(ant, now);
        }
    }

    /// Saved sensors in their tile slots, with their observed state.
    pub(super) fn sensor_tiles(
        &self,
        ant: &impl Ant,
        now: u64,
    ) -> [Option<Selected>; CHANNEL_CAPACITY] {
        let channels = ant.channels(now);
        let radio = self.radio(ant);
        core::array::from_fn(|slot| {
            let peer = self.links.saved.get(slot)?;
            let channel = channel_of(&channels, peer.device_type);
            Some(Selected {
                identity: peer,
                connection: self.links.connection(slot, peer, channel, radio),
            })
        })
    }

    pub(super) fn refresh_sensor_menu(&mut self, ant: &impl Ant, now: u64) {
        let tiles = self.sensor_tiles(ant, now);
        self.menu
            .refresh(ant.discoveries(), tiles, self.discovery.scan.busy(ant));
    }

    /// Sensors saved for automatic connection, in tile order.
    pub fn saved_sensors(&self) -> Saved {
        self.links.saved
    }

    /// Report save trouble on the sensor page when it starts and ends.
    fn show_persistence(&mut self) {
        let persistence = self.preferences.persistence();
        if self.links.shown == Some(persistence) {
            return;
        }
        let message = match persistence {
            Persistence::Failed => Some(MenuMessage::NotSaved),
            Persistence::Locked => Some(MenuMessage::SavedUnreadable),
            // Only clear a problem this page showed.
            _ if matches!(
                self.links.shown,
                Some(Persistence::Failed | Persistence::Locked)
            ) =>
            {
                Some(self.discovery.scan.message())
            }
            _ => None,
        };
        self.links.shown = Some(persistence);
        if let Some(message) = message {
            self.menu.set_message(message);
            self.display.redraw();
        }
    }
}
