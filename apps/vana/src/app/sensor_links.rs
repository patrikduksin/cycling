//! The radio has one channel per device type, so of several saved sensors
//! of one type only the active one, the one used last, is connected.
use super::Runtime;
use super::preferences::Persistence;
use crate::screens::sensors::{Action, Connection, MenuMessage, Selected};
use crate::sensors::ant::{Battery, BatteryReports, Product};
use crate::sensors::saved::{SAVED_CAPACITY, Saved};
use crate::sensors::scan::{self, Scan, Status};
use device_api::ant::{
    Admission, Ant, AntOperation, CONNECT_TIMEOUT_MS, DISCOVERY_CAPACITY, Discovery as Found,
    Error, Identity, LinkState, Packet, STALE_MS, ScanState, Snapshot,
};
use device_api::observation::Availability;
use firmware_services::why;

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
const IDLE_CHOICE_MS: u64 = CONNECT_TIMEOUT_MS + 2_000;
const DROPOUT_MS: u64 = 30_000;
const CHOICE_EVERY_MS: u64 = 30_000;
/// A discovery heard this recently still says the sensor is broadcasting.
const HEARD_MS: u64 = 15_000;
/// The radio connects nothing while it searches, so VANA's own search is
/// short: ANT+ sensors broadcast about four times a second.
const CHOICE_SCAN_MS: u32 = 3_000;

#[derive(Default)]
pub(super) struct Discovery {
    pub(super) scan: Scan,
    /// The rider asked for a search that has not started yet.
    requested: bool,
    /// Transport scan generation already shown, so a new search clears old results.
    generation: u32,
    searcher: Searcher,
    choice_due_ms: u64,
}

state!(Discovery {
    scan,
    requested,
    generation,
    searcher,
    choice_due_ms,
});

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
enum Searcher {
    #[default]
    Rider,
    /// VANA choosing among saved sensors of a type; the sensor pages
    /// neither show nor clear for it.
    Choice { recording: bool },
}

impl Discovery {
    /// A search the rider asked for starts or runs; its results arrive a
    /// few packets a pass.
    pub(super) fn searching(&self) -> bool {
        self.requested || self.scan.active() && self.searcher == Searcher::Rider
    }

    pub(super) fn message(&self) -> MenuMessage {
        match self.scan.status() {
            Status::Idle => MenuMessage::PickSensor,
            Status::WaitingForRadio => MenuMessage::SearchPending,
            Status::Starting => MenuMessage::StartingScan,
            Status::Scanning => MenuMessage::Scanning,
            Status::Stopping => MenuMessage::StoppingScan,
            Status::Cancelled => MenuMessage::ScanCancelled,
            Status::Finishing => MenuMessage::FinishingScan,
            Status::Done => MenuMessage::ScanDone,
            Status::Failed => MenuMessage::ScanFailed,
            Status::Uncertain => MenuMessage::RadioLost,
            Status::Unavailable => MenuMessage::ScanUnavailable,
        }
    }

    pub(super) fn step(&mut self, mut event: scan::Event, ant: &mut impl Ant, now: u64) {
        while let Some(op) = self.scan.step(event, &seen(ant, now), now) {
            let operation = match (op, self.searcher) {
                (scan::Op::Start, Searcher::Choice { .. }) => AntOperation::Scan(CHOICE_SCAN_MS),
                _ => op.operation(),
            };
            event = scan::Event::Answer(op, ant.request(operation, now));
        }
    }
}

fn seen(ant: &impl Ant, now: u64) -> scan::Seen {
    scan::Seen {
        radio: ant.scan().state,
        availability: ant.availability(),
        links: scan::Links::of(&ant.channels(now), ant.capabilities().concurrent_scan),
    }
}

/// Retry state of one saved sensor or closing channel.
#[derive(Clone, Copy, Default)]
#[cfg_attr(not(target_os = "none"), derive(Debug))]
struct Schedule {
    /// Unsuccessful requests since data last arrived.
    failures: u8,
    /// Accepted closes since the channel last finished closing; a channel
    /// that keeps failing to close is retried with the same backoff.
    closes: u8,
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
    silent_since_ms: u64,
}

impl Schedule {
    fn arm(&mut self, need: Need, now: u64) {
        if !self.armed {
            self.armed = true;
            let attempts = if need == Need::Close {
                self.closes
            } else {
                self.failures
            };
            self.due_ms = now.saturating_add(backoff_ms(attempts));
        }
    }

    fn due(&self, now: u64) -> bool {
        self.armed && now >= self.due_ms
    }

    fn submitted(&mut self, result: Result<Admission, Error>, connect: bool, now: u64) {
        match result {
            Ok(_) => {
                if connect {
                    self.failures = (self.failures + 1).min(MAX_FAILURES);
                } else {
                    self.closes = (self.closes + 1).min(MAX_FAILURES);
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
#[cfg_attr(not(target_os = "none"), derive(Debug))]
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

#[derive(Clone, Copy, Debug)]
struct Heard {
    peer: Identity,
    batteries: BatteryReports,
    product: Product,
}

#[derive(Default)]
pub(super) struct Links {
    schedules: [Schedule; SAVED_CAPACITY],
    closing: heapless::Vec<Closing, SAVED_CAPACITY>,
    /// Saved slot served first next time, so no sensor starves the others.
    next: usize,
    /// Save state last shown on the sensor page.
    shown: Option<Persistence>,
    heard: [Option<Heard>; SAVED_CAPACITY],
}

state!(Links {
    schedules,
    closing,
    next,
    shown,
    heard,
});

fn channel_of(channels: &[Option<Snapshot>], kind: u8) -> Option<Snapshot> {
    channels
        .iter()
        .flatten()
        .find(|s| s.selected.is_some_and(|p| p.device_type == kind))
        .copied()
}

impl Links {
    pub(super) fn note_page(&mut self, saved: &Saved, packet: &Packet) {
        let peer = packet.identity;
        let Some(slot) = saved.slot_of(peer) else {
            return;
        };
        let heard = match &mut self.heard[slot] {
            Some(heard) if heard.peer == peer => heard,
            held => held.insert(Heard {
                peer,
                batteries: BatteryReports::default(),
                product: Product::default(),
            }),
        };
        if let Some(battery) = Battery::decode(peer.device_type, packet.data) {
            heard.batteries.note(battery);
        }
        heard.product.note(peer.device_type, packet.data);
    }

    fn heard(&self, slot: usize, peer: Identity) -> Option<&Heard> {
        self.heard[slot].as_ref().filter(|heard| heard.peer == peer)
    }

    pub(super) fn battery(&self, slot: usize, peer: Identity) -> Option<Battery> {
        self.heard(slot, peer)?.batteries.worst()
    }

    pub(super) fn product(&self, slot: usize, peer: Identity) -> Product {
        self.heard(slot, peer)
            .map_or_else(Product::default, |heard| heard.product)
    }

    pub(super) fn rejects(&self, saved: &Saved, peer: Identity) -> bool {
        self.closing.iter().any(|c| c.kind == peer.device_type)
            || saved
                .active(peer.device_type)
                .is_some_and(|(_, active)| active != peer)
    }

    fn due_for_choice(&self, saved: &Saved, slot: usize, recording: bool, now: u64) -> bool {
        let Some(peer) = saved.get(slot) else {
            return false;
        };
        let wait = if recording {
            DROPOUT_MS
        } else {
            IDLE_CHOICE_MS
        };
        saved.is_active(slot)
            && saved.count_of(peer.device_type) > 1
            && now.saturating_sub(self.schedules[slot].silent_since_ms) >= wait
    }

    fn choose(
        &mut self,
        saved: &mut Saved,
        heard: &[Option<Found>],
        recording: bool,
        now: u64,
    ) -> bool {
        let mut changed = false;
        for slot in 0..SAVED_CAPACITY {
            if !self.due_for_choice(saved, slot, recording, now) {
                continue;
            }
            let Some(kind) = saved.get(slot).map(|peer| peer.device_type) else {
                continue;
            };
            // The transport keeps the last search's list until the next
            // begins, so only what was heard since this sensor fell silent,
            // and lately, counts.
            let since = self.schedules[slot].silent_since_ms;
            let fresh: [Option<Found>; DISCOVERY_CAPACITY] = core::array::from_fn(|index| {
                heard.get(index).copied().flatten().filter(|found| {
                    found.seen_ms >= since && now.saturating_sub(found.seen_ms) <= HEARD_MS
                })
            });
            let Some(pick) = saved.choose(kind, &fresh) else {
                continue;
            };
            if saved.used(pick) {
                why!("links.chose", kind, pick);
                if let Some(slot) = saved.slot_of(pick) {
                    self.activated(slot, now);
                }
                changed = true;
            }
        }
        changed
    }

    fn activated(&mut self, slot: usize, now: u64) {
        self.schedules[slot] = Schedule {
            silent_since_ms: now,
            ..Schedule::default()
        };
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
                (closing.schedule.closes < CLOSE_ATTEMPTS).then_some(Need::Close)
            }
        }
    }

    /// Track observed link state: fresh data resets the backoff, a new need
    /// is given its delay, and finished closes are dropped.
    fn observe(&mut self, saved: &Saved, channels: &[Option<Snapshot>], now: u64) {
        for slot in 0..SAVED_CAPACITY {
            let Some(peer) = saved.get(slot) else {
                continue;
            };
            let schedule = &mut self.schedules[slot];
            if !saved.is_active(slot) {
                schedule.armed = false;
                schedule.connected_ms = None;
                continue;
            }
            let channel = channel_of(channels, peer.device_type);
            let ours = channel.filter(|s| s.selected == Some(peer));
            match ours {
                Some(s) if s.link == LinkState::Connected => {
                    schedule.connected_ms.get_or_insert(now);
                    if s.age_ms.is_some_and(|age| age < STALE_MS) {
                        schedule.failures = 0;
                        schedule.refused = None;
                        schedule.silent_since_ms = now;
                    }
                }
                _ => schedule.connected_ms = None,
            }
            match self.need(slot, peer, channel, now) {
                Need::Nothing => self.schedules[slot].armed = false,
                need => {
                    if need == Need::Connect {
                        // Any earlier channel has finished closing.
                        self.schedules[slot].closes = 0;
                    }
                    self.schedules[slot].arm(need, now);
                }
            }
        }
        self.closing.retain_mut(|closing| {
            match Self::close_need(closing, channel_of(channels, closing.kind)) {
                None => false,
                Some(Need::Close) => {
                    closing.schedule.arm(Need::Close, now);
                    true
                }
                Some(_) => {
                    closing.schedule.armed = false;
                    true
                }
            }
        });
    }

    fn next_request(
        &mut self,
        saved: &Saved,
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
        for offset in 0..SAVED_CAPACITY {
            let slot = (self.next + offset) % SAVED_CAPACITY;
            let Some(peer) = saved.get(slot) else {
                continue;
            };
            if !saved.is_active(slot) || !self.schedules[slot].due(now) {
                continue;
            }
            let operation = match self.need(slot, peer, channel_of(channels, peer.device_type), now)
            {
                Need::Nothing => continue,
                Need::Connect => AntOperation::Connect(peer),
                Need::Close => AntOperation::Disconnect(peer.device_type),
            };
            self.next = (slot + 1) % SAVED_CAPACITY;
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
                    closing.schedule.submitted(result, false, now);
                }
            }
        }
    }

    fn first_attempt_pending(
        &self,
        saved: &Saved,
        channels: &[Option<Snapshot>],
        now: u64,
    ) -> bool {
        saved.peers().any(|(slot, peer)| {
            let schedule = &self.schedules[slot];
            saved.is_active(slot)
                && schedule.failures == 0
                && schedule.closes == 0
                && schedule.refused.is_none()
                && self.need(slot, peer, channel_of(channels, peer.device_type), now)
                    != Need::Nothing
        })
    }

    fn add(&mut self, saved: &mut Saved, peer: Identity, now: u64) -> bool {
        let Some(slot) = saved.add(peer) else {
            return false;
        };
        self.activated(slot, now);
        self.closing.retain(|c| c.kind != peer.device_type);
        true
    }

    fn forget(&mut self, saved: &mut Saved, peer: Identity, now: u64) -> bool {
        let kind = peer.device_type;
        let was_active = saved.active(kind).map(|(_, active)| active) == Some(peer);
        let Some(slot) = saved.remove(peer) else {
            return false;
        };
        self.schedules[slot] = Schedule::default();
        self.heard[slot] = None;
        match saved.active(kind) {
            Some((next, _)) if was_active => self.activated(next, now),
            Some(_) => {}
            None if !self.closing.iter().any(|c| c.kind == kind) => {
                let _ = self.closing.push(Closing {
                    kind,
                    schedule: Schedule::default(),
                });
            }
            None => {}
        }
        true
    }

    fn connection(
        &self,
        slot: usize,
        peer: Identity,
        channel: Option<Snapshot>,
        radio: Radio,
        active: bool,
        in_range: bool,
    ) -> Connection {
        if !active {
            return if in_range {
                Connection::Standby
            } else {
                Connection::NotInRange
            };
        }
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
            Some(s) if s.link != LinkState::Disconnected => return Connection::Switching,
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
        self.discovery.step(scan::Event::Tick, ant, now);
        self.refresh_discovery_session(ant);
        let scanning = self.discovery.scan.active();
        if (was_scanning || scanning) && self.discovery.searcher == Searcher::Rider {
            self.menu.set_message(self.discovery.message());
        }
        if scanning
            && self.discovery.searcher == (Searcher::Choice { recording: false })
            && self.ride.recording()
        {
            self.cancel_search(ant, now);
        }
        if was_scanning && !scanning {
            self.discovery.searcher = Searcher::Rider;
            self.choose_sensors(ant, now);
        }
        self.start_sensor_search(ant, now);
        self.start_choice_search(ant, now);
        self.advance_links(ant, now);
        self.show_persistence();
    }

    pub(super) fn request_search(&mut self, ant: &mut impl Ant, now: u64) {
        self.discovery.requested = true;
        self.discovery.searcher = Searcher::Rider;
        self.start_sensor_search(ant, now);
    }

    fn choose_sensors(&mut self, ant: &impl Ant, now: u64) {
        let mut saved = self.preferences.sensors;
        let recording = self.ride.recording();
        if self
            .links
            .choose(&mut saved, &ant.discoveries(), recording, now)
        {
            self.preferences.set_sensors(saved, now);
        }
    }

    fn start_choice_search(&mut self, ant: &mut impl Ant, now: u64) {
        let discovery = &self.discovery;
        if discovery.requested || discovery.scan.active() || now < discovery.choice_due_ms {
            return;
        }
        let sensor_pages = self.navigation.page() == crate::screens::workout::Page::Sensors;
        if sensor_pages || self.radio(ant) != Radio::Ready || ant.scan().state != ScanState::Idle {
            return;
        }
        let saved = &self.preferences.sensors;
        let channels = ant.channels(now);
        let recording = self.ride.recording();
        if self.links.first_attempt_pending(saved, &channels, now)
            || !(0..SAVED_CAPACITY)
                .any(|slot| self.links.due_for_choice(saved, slot, recording, now))
        {
            return;
        }
        why!("links.choice_search", recording);
        self.discovery.searcher = Searcher::Choice { recording };
        self.discovery.choice_due_ms = now.saturating_add(CHOICE_EVERY_MS);
        self.discovery.step(scan::Event::Start, ant, now);
        self.refresh_discovery_session(ant);
    }

    pub(super) fn cancel_search(&mut self, ant: &mut impl Ant, now: u64) {
        self.discovery.requested = false;
        self.discovery.step(scan::Event::Cancel, ant, now);
    }

    /// Searching waits until newly selected or restored sensors have been
    /// asked for once; their searches then delay discovery, not the reverse.
    fn start_sensor_search(&mut self, ant: &mut impl Ant, now: u64) {
        if !self.discovery.requested {
            return;
        }
        if ant.availability() == Availability::Ready
            && self
                .links
                .first_attempt_pending(&self.preferences.sensors, &ant.channels(now), now)
        {
            self.menu.set_message(MenuMessage::SearchPending);
            return;
        }
        self.discovery.requested = false;
        self.discovery.step(scan::Event::Start, ant, now);
        self.refresh_discovery_session(ant);
        self.menu.set_message(self.discovery.message());
    }

    fn refresh_discovery_session(&mut self, ant: &impl Ant) {
        let generation = ant.scan().generation;
        if self.discovery.generation != generation {
            self.discovery.generation = generation;
            if self.discovery.searcher == Searcher::Rider {
                self.menu.search_started();
            }
        }
    }

    fn radio(&self, ant: &impl Ant) -> Radio {
        match ant.availability() {
            Availability::Ready if ant.scan().state == ScanState::Uncertain => Radio::Unavailable,
            Availability::Ready if self.discovery.scan.busy(ant.scan().state) => Radio::Waiting,
            Availability::Ready => Radio::Ready,
            Availability::Initializing => Radio::Waiting,
            _ => Radio::Unavailable,
        }
    }

    /// Submit at most one due connection request.
    fn advance_links(&mut self, ant: &mut impl Ant, now: u64) {
        let channels = ant.channels(now);
        self.links
            .observe(&self.preferences.sensors, &channels, now);
        if self.radio(ant) != Radio::Ready {
            return;
        }
        let Some((operation, target)) =
            self.links
                .next_request(&self.preferences.sensors, &channels, now)
        else {
            return;
        };
        let connect = matches!(operation, AntOperation::Connect(_));
        let result = ant.request(operation, now);
        self.links.submitted(target, connect, result, now);
    }

    pub(super) fn request_sensor(&mut self, action: Action, ant: &mut impl Ant, now: u64) {
        let mut saved = self.preferences.sensors;
        let changed = match action {
            Action::Connect(peer) => {
                let added = self.links.add(&mut saved, peer, now);
                if !added {
                    self.menu.set_message(MenuMessage::SlotsFull);
                }
                added
            }
            Action::Forget(peer) => self.links.forget(&mut saved, peer, now),
            Action::Scan | Action::StopScan | Action::Calibrate(_) | Action::Ride => false,
        };
        if changed {
            self.preferences.set_sensors(saved, now);
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
    ) -> [Option<Selected>; SAVED_CAPACITY] {
        let channels = ant.channels(now);
        let radio = self.radio(ant);
        let found = ant.discoveries();
        let saved = &self.preferences.sensors;
        core::array::from_fn(|slot| {
            let peer = saved.get(slot)?;
            let channel = channel_of(&channels, peer.device_type);
            let in_range = found.iter().flatten().any(|f| f.identity == peer);
            let active = saved.is_active(slot);
            Some(Selected {
                identity: peer,
                connection: self
                    .links
                    .connection(slot, peer, channel, radio, active, in_range),
                battery: self.links.battery(slot, peer),
                product: self.links.product(slot, peer),
            })
        })
    }

    pub(super) fn refresh_sensor_menu(&mut self, ant: &impl Ant, now: u64) {
        let tiles = self.sensor_tiles(ant, now);
        let busy = self.discovery.scan.busy(ant.scan().state);
        self.menu.refresh(
            ant.discoveries(),
            tiles,
            busy && self.discovery.searcher == Searcher::Rider,
        );
        let note = self.calibration.note();
        self.menu.set_calibration(self.calibrated.zip(note));
    }

    /// Sensors saved for automatic connection, in tile order.
    pub fn saved_sensors(&self) -> Saved {
        self.preferences.sensors
    }

    /// Report save trouble on the sensor page when it starts and ends.
    fn show_persistence(&mut self) {
        let persistence = self.preferences.persistence();
        if self.links.shown == Some(persistence) {
            return;
        }
        self.links.shown = Some(persistence);
        self.menu.set_notice(match persistence {
            Persistence::Failed => Some(MenuMessage::NotSaved),
            Persistence::Locked => Some(MenuMessage::SavedUnreadable),
            Persistence::Saved | Persistence::Pending => None,
        });
        self.display.redraw();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter(device_number: u16) -> Identity {
        Identity {
            device_type: 11,
            device_number,
            transmission_type: 1,
        }
    }

    fn heard(peer: Identity, seen_ms: u64) -> [Option<Found>; 1] {
        [Some(Found {
            identity: peer,
            rssi: -50,
            seen_ms,
        })]
    }

    #[test]
    fn a_recording_ride_switches_sensor_only_after_a_long_dropout() {
        let (bike, spare) = (meter(1), meter(2));
        let mut saved = Saved::default();
        saved.add(spare);
        saved.add(bike);
        let mut links = Links::default();
        links.activated(1, 0);
        let choose = |recording, now| {
            let (mut saved, mut links) = (saved, Links::default());
            links.activated(1, 0);
            links.choose(&mut saved, &heard(spare, now), recording, now);
            saved.active(11).map(|(_, peer)| peer)
        };
        assert_eq!(choose(false, IDLE_CHOICE_MS - 1), Some(bike));
        assert_eq!(choose(false, IDLE_CHOICE_MS), Some(spare));
        assert_eq!(choose(true, IDLE_CHOICE_MS), Some(bike), "mid-ride");
        assert_eq!(choose(true, DROPOUT_MS - 1), Some(bike));
        assert_eq!(choose(true, DROPOUT_MS), Some(spare));
        let channels = [Some(Snapshot {
            scanning: false,
            link: LinkState::Connected,
            selected: Some(bike),
            generation: 1,
            packets: 1,
            dropped_packets: 0,
            discovery_overflows: 0,
            transport_losses: 0,
            tx_failures: 0,
            age_ms: Some(0),
            stale: false,
        })];
        links.observe(&saved, &channels, 29_000);
        assert!(!links.choose(&mut saved, &heard(spare, DROPOUT_MS), true, DROPOUT_MS));
        assert!(links.choose(
            &mut saved,
            &heard(spare, 29_000 + DROPOUT_MS),
            true,
            29_000 + DROPOUT_MS
        ));
        assert_eq!(saved.active(11), Some((0, spare)));
    }

    #[test]
    fn a_type_with_one_saved_sensor_never_chooses() {
        let mut saved = Saved::default();
        saved.add(meter(1));
        let mut links = Links::default();
        assert!(!links.choose(&mut saved, &heard(meter(2), 600_000), false, 600_000));
        assert!(!links.due_for_choice(&saved, 0, false, 600_000));
    }

    #[test]
    fn a_stale_discovery_list_never_switches_the_sensor() {
        let (bike, spare) = (meter(1), meter(2));
        let mut saved = Saved::default();
        saved.add(spare);
        saved.add(bike);
        let mut links = Links::default();
        links.activated(1, 40_000);
        let now = 40_000 + DROPOUT_MS;
        // Heard before the bike's meter fell silent: the spare was at home.
        assert!(!links.choose(&mut saved, &heard(spare, 39_000), true, now));
        // Heard after, but too long ago to say it broadcasts now.
        assert!(!links.choose(&mut saved, &heard(spare, now - HEARD_MS - 1), true, now));
        assert!(links.choose(&mut saved, &heard(spare, now - 1_000), true, now));
    }
}
