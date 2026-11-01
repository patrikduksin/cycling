//! A recorded trace standing in for the world: the radios, the receiver, the
//! barometer and the companion report what they reported when it was
//! recorded, and the session runs one pass at each recorded uptime.
//!
//! A strict replay runs the build that recorded the trace and must answer
//! every call from it. A stimulus replay runs another build against the
//! same world: inputs arrive at their recorded pass whatever VANA asks, and
//! the ANT radio is a model that hears the recorded sensors. Either way the
//! replay keeps a [`Report`] of where it parted from the recording.
use crate::ant::{AntSensor, SimulatedAnt, ZeroReply};
use crate::bulk::SimBulk;
use crate::devices::InputDevice;
#[cfg(feature = "cycling")]
use crate::phone::SimAccessPoint;
use crate::storage::FileFlash;
use device_api::ant::{
    self, Admission, Ant, AntOperation, CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Identity, LinkState,
};
use device_api::ble_transport::{self, Ble};
use device_api::bulk::{self, ReadWrite};
use device_api::input::{InputObservation, InputSnapshot};
use device_api::observation::{Availability, Error};
use device_api::positioning::{self, Positioning};
use device_api::sensors;
#[cfg(feature = "cycling")]
use device_api::server::{AccessPoint, AccessPointConfig, AccessPointState, Reach};
use firmware_services::trace::frozen::{Latest, Sink};
use firmware_services::trace::{self, Cursor, Event, Events};
use std::cell::{Cell, RefCell};
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;

/// Sealed trace blocks, back to back.
#[derive(Default)]
pub struct Blocks(pub Vec<u8>);

impl Sink for Blocks {
    fn block(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

/// The media as a session found it before its first pass.
#[derive(Clone)]
pub struct Origin {
    pub(crate) flash: FileFlash,
    pub bulk: SimBulk,
    pub(crate) boot: u32,
}

/// Everything a replay needs: where the media stood and what was read.
pub struct Capture {
    pub origin: Origin,
    pub trace: Vec<u8>,
}

/// What the trace holds next.
pub enum Next {
    Pass,
    Command(String),
    End,
}

/// How a replay treats the recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The build that recorded it: VANA's calls are answered in recorded
    /// order, and any call or answer that differs is a divergence.
    Strict,
    /// Another build: the recording is the world. Queued inputs arrive at
    /// their recorded pass and wait until VANA takes them; latest values
    /// hold from their pass; the ANT radio is a model hearing the recorded
    /// sensors, and answers a request as the recording did when it made the
    /// same one. Differences are counted, not failures.
    Stimulus,
}

/// A way a replay parted from its recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// VANA asked for something no recorded answer covered. A stimulus
    /// replay's model answered it.
    Unanswered,
    /// A recorded answer, input or entropy value the pass did not use.
    LeftOver,
    /// VANA made another ANT request than the one recorded next.
    OtherRequest,
    /// A recorded ANT request the replay did not make within a second.
    NotRequested,
    /// The model refused an ANT request the recording's radio accepted.
    Refused,
    /// A recorded sensor page reached no open channel.
    NotHeard,
    /// Recorded input VANA had not taken by the end of its pass; it waits.
    Carried,
    /// The rider saw another page or heard another sound.
    Seen,
    /// An event the simulator cannot replay.
    Unreplayable,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Divergence {
    /// The pass it happened in.
    pub at_ms: u64,
    pub kind: Kind,
    pub detail: String,
}

/// Where a replay parted from its recording: the first time, and how often
/// each way.
#[derive(Debug, Default)]
pub struct Report {
    pub first: Option<Divergence>,
    pub counts: BTreeMap<Kind, u32>,
}

impl Report {
    fn note(&mut self, at_ms: u64, kind: Kind, count: usize, detail: impl FnOnce() -> String) {
        if count == 0 {
            return;
        }
        *self.counts.entry(kind).or_default() += count as u32;
        self.first.get_or_insert_with(|| Divergence {
            at_ms,
            kind,
            detail: detail(),
        });
    }

    pub fn count(&self, kind: Kind) -> u32 {
        self.counts.get(&kind).copied().unwrap_or(0)
    }
}

/// A divergence a radio found, how many times, and what it was.
type Note = (Kind, usize, String);

/// What the rider saw change in a pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    Page(String),
    Sound(u8),
}

pub struct Replay {
    bytes: Vec<u8>,
    cursor: Cursor,
    pub sources: Sources,
    /// What the recording showed in the pass being replayed.
    seen: Vec<Seen>,
}

impl Replay {
    /// `bulk` shares the replaying device's medium.
    pub fn new(
        bytes: Vec<u8>,
        input: InputDevice,
        bulk: SimBulk,
        mode: Mode,
    ) -> Result<Self, trace::Error> {
        let (_, events) = trace::decode(&bytes)?;
        let cursor = events.cursor();
        let ant = match mode {
            Mode::Strict => AntSource::Recorded(Box::default()),
            Mode::Stimulus => AntSource::Model(Box::new(ModelAnt::hearing(events))),
        };
        Ok(Self {
            sources: Sources::new(ant, input, bulk, mode),
            bytes,
            cursor,
            seen: Vec::new(),
        })
    }

    /// Move past what the rider saw to the next pass or command. A command
    /// is taken: the caller executes it.
    pub fn step(&mut self) -> Next {
        loop {
            let mut events = Events::resume(&self.bytes, self.cursor);
            let Some(event) = events.next() else {
                self.sources.drained();
                self.sources.ended();
                return Next::End;
            };
            match event {
                Event::Pass { .. } => return Next::Pass,
                Event::Command(line) => {
                    self.cursor = events.cursor();
                    return Next::Command(line.into());
                }
                event if seen(&event).is_some() => {}
                event => self.sources.note(Kind::Unreplayable, 1, || {
                    format!("{event:?} outside a pass")
                }),
            }
            self.cursor = events.cursor();
        }
    }

    /// Take the next pass and what its devices reported; its uptime.
    pub fn load_pass(&mut self) -> u64 {
        self.sources.drained();
        self.seen.clear();
        let mut events = Events::resume(&self.bytes, self.cursor);
        match events.next() {
            Some(Event::Pass { now_ms }) => self.sources.begin(now_ms),
            other => {
                self.sources.note(Kind::Unanswered, 1, || {
                    format!("a pass the recording did not run; it holds {other:?}")
                });
                return self.sources.now;
            }
        }
        loop {
            let before = events.cursor();
            match events.next() {
                None => break,
                Some(Event::Pass { .. } | Event::Command(_)) => {
                    events = Events::resume(&self.bytes, before);
                    break;
                }
                Some(event) => match seen(&event) {
                    Some(seen) => self.seen.extend(seen),
                    None => self.sources.apply(event),
                },
            }
        }
        self.cursor = events.cursor();
        self.sources.ready();
        self.sources.now
    }

    /// What the rider saw in the pass just run, against the recording.
    pub fn saw(&mut self, seen: &[Seen]) {
        if seen != self.seen.as_slice() {
            let recorded = core::mem::take(&mut self.seen);
            self.sources.note(Kind::Seen, 1, || {
                format!("the rider saw {seen:?}, the recording {recorded:?}")
            });
        }
    }
}

/// `Some` for an output, the rider's view recorded for agents, holding what
/// a replay compares: the page and sounds. The rest only describe.
fn seen(event: &Event<'_>) -> Option<Option<Seen>> {
    match event {
        Event::Page(page) => Some(Some(Seen::Page((*page).into()))),
        Event::Sound(pattern) => Some(Some(Seen::Sound(*pattern))),
        Event::Toast(_) | Event::RideState(_) | Event::Shown(_) | Event::Log { .. } => Some(None),
        _ => None,
    }
}

/// The devices as the trace reports them, one pass at a time.
pub struct Sources {
    pub ant: AntSource,
    pub ble: ReplayBle,
    pub position: ReplayPosition,
    pub companion: ReplayCompanion,
    pub bulk: ReplayBulk,
    #[cfg(feature = "cycling")]
    pub access_point: ReplayAccessPoint,
    entropy: Entropy,
    startup: Latest<sensors::Startup>,
    physical: Latest<sensors::Snapshot>,
    network_time: Latest<firmware_services::network_time::Snapshot>,
    input: InputDevice,
    mode: Mode,
    now: u64,
    pub report: Report,
}

impl Sources {
    fn new(ant: AntSource, input: InputDevice, media: SimBulk, mode: Mode) -> Self {
        let entropy = Entropy::default();
        Self {
            ant,
            ble: ReplayBle::default(),
            position: ReplayPosition::default(),
            companion: ReplayCompanion::default(),
            bulk: ReplayBulk {
                media,
                entropy: entropy.clone(),
            },
            #[cfg(feature = "cycling")]
            access_point: ReplayAccessPoint {
                inner: SimAccessPoint::default(),
                entropy: entropy.clone(),
            },
            entropy,
            startup: Latest::default(),
            physical: Latest::default(),
            network_time: Latest::default(),
            input,
            mode,
            now: 0,
            report: Report::default(),
        }
    }

    /// A changed build's ANT adapter, answering a request itself before
    /// the model sees it when it returns an answer. Stimulus replays only.
    pub fn adapt_ant(
        &mut self,
        adapter: impl FnMut(&AntOperation, u64) -> Option<Result<Admission, ant::Error>> + 'static,
    ) {
        match &mut self.ant {
            AntSource::Model(model) => model.adapter = Some(Box::new(adapter)),
            AntSource::Recorded(_) => panic!("a strict replay answers from the recording"),
        }
    }

    fn note(&mut self, kind: Kind, count: usize, detail: impl FnOnce() -> String) {
        self.report.note(self.now, kind, count, detail);
    }

    fn begin(&mut self, now: u64) {
        self.now = now;
        self.ant.begin(now);
        self.ble.now = now;
        self.position.now = now;
    }

    /// The pass's events are applied; the radio model steps to it.
    fn ready(&mut self) {
        if let AntSource::Model(model) = &mut self.ant {
            model.step();
        }
        self.drain_notes();
    }

    /// The pass that ran left nothing behind it should have used.
    fn drained(&mut self) {
        let queued = [
            ("BLE packets", self.ble.packets.len()),
            ("BLE battery levels", self.ble.battery.len()),
            ("input edges", self.input.replayed.borrow().len()),
        ];
        let kind = match self.mode {
            Mode::Strict => Kind::LeftOver,
            Mode::Stimulus => Kind::Carried,
        };
        for (what, count) in queued {
            self.note(kind, count, || format!("{count} {what}"));
        }
        if self.mode == Mode::Strict {
            let left = self.entropy.0.borrow().len();
            self.note(Kind::LeftOver, left, || format!("{left} entropy values"));
        }
        let past = self.entropy.1.take() as usize;
        self.note(Kind::Unanswered, past, || {
            format!("{past} entropy values past the recording's")
        });
        if let AntSource::Recorded(ant) = &mut self.ant {
            ant.drained();
        }
        self.drain_notes();
    }

    /// The trace is spent.
    fn ended(&mut self) {
        if let AntSource::Model(model) = &mut self.ant {
            model.expire(u64::MAX);
        }
        self.drain_notes();
    }

    fn drain_notes(&mut self) {
        let now = self.now;
        for (kind, count, detail) in self.ant.notes().drain(..) {
            self.report.note(now, kind, count, || detail);
        }
    }

    fn apply(&mut self, event: Event<'_>) {
        let now = self.now;
        match event {
            Event::Input(edge) => self.input.replayed.borrow_mut().push_back(edge),
            Event::AntRadio(_)
            | Event::AntDiscoveries(_)
            | Event::AntLink { .. }
            | Event::AntPacket(_)
            | Event::AntRequest { .. }
            | Event::AntSend(_) => self.ant.apply(event),
            Event::BleLink {
                availability,
                snapshot,
            } => self.ble.link.set((availability, snapshot), now),
            Event::BlePacket(packet) => self.ble.packets.push_back(packet),
            Event::BleBattery(level) => self.ble.battery.push_back(level),
            Event::Position {
                availability,
                snapshot,
            } => self.position.latest.set((availability, snapshot), now),
            Event::Physical(snapshot) => self.physical.set(snapshot, now),
            Event::Power(snapshot) => self.companion.0.set(snapshot, now),
            Event::NetworkTime(snapshot) => self.network_time.set(snapshot, now),
            Event::Startup(startup) => self.startup.set(startup, now),
            Event::Entropy(value) => self.entropy.0.borrow_mut().push_back(value),
            event => self.note(Kind::Unreplayable, 1, || {
                format!("{event:?} is not replayed in the simulator")
            }),
        }
    }

    pub fn startup(&self) -> sensors::Startup {
        self.startup
            .at(self.now)
            .unwrap_or(sensors::Startup::Unsupported)
    }

    pub fn physical(&self) -> sensors::Snapshot {
        self.physical.at(self.now).unwrap_or(sensors::Snapshot {
            pressure: device_api::observation::Observation::Unavailable,
            motion: [device_api::observation::Observation::Unavailable; 2],
            identity: device_api::observation::Observation::Unavailable,
            reports: 0,
            invalid_reports: 0,
            losses: 0,
        })
    }

    pub fn network_time(&self) -> firmware_services::network_time::Snapshot {
        self.network_time.at(self.now).unwrap_or_default()
    }
}

/// The capability list as the `&'static` the ANT contract wants. Each
/// distinct list is leaked once per process.
fn intern(types: &[u8]) -> &'static [u8] {
    static KNOWN: std::sync::Mutex<Vec<&'static [u8]>> = std::sync::Mutex::new(Vec::new());
    let mut known = KNOWN
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some(&list) = known.iter().find(|list| **list == types) {
        return list;
    }
    let list: &'static [u8] = Vec::leak(types.to_vec());
    known.push(list);
    list
}

/// The ANT radio a replay presents: the recording's answers, or a model.
pub enum AntSource {
    Recorded(Box<RecordedAnt>),
    Model(Box<ModelAnt>),
}

impl AntSource {
    fn begin(&mut self, now: u64) {
        match self {
            Self::Recorded(ant) => ant.now = now,
            Self::Model(model) => model.now = now,
        }
    }

    fn apply(&mut self, event: Event<'_>) {
        match self {
            Self::Recorded(ant) => ant.apply(event),
            Self::Model(model) => model.apply(event),
        }
    }

    fn notes(&mut self) -> &mut Vec<Note> {
        match self {
            Self::Recorded(ant) => &mut ant.notes,
            Self::Model(model) => &mut model.notes,
        }
    }

    fn radio(&self) -> &dyn Ant {
        match self {
            Self::Recorded(ant) => ant.as_ref(),
            Self::Model(model) => &model.radio,
        }
    }
}

impl Ant for AntSource {
    fn scanning(&self) -> bool {
        self.radio().scanning()
    }
    fn discoveries(&self) -> [Option<ant::Discovery>; DISCOVERY_CAPACITY] {
        self.radio().discoveries()
    }
    fn request(&mut self, operation: AntOperation, now: u64) -> Result<Admission, ant::Error> {
        match self {
            Self::Recorded(ant) => ant.request(operation, now),
            Self::Model(model) => model.request(operation, now),
        }
    }
    fn capabilities(&self) -> ant::Capabilities {
        self.radio().capabilities()
    }
    fn scan(&self) -> ant::ScanSnapshot {
        self.radio().scan()
    }
    fn send_status(&self, id: ant::OperationId) -> Option<ant::SendObservation> {
        self.radio().send_status(id)
    }
    fn availability(&self) -> Availability {
        self.radio().availability()
    }
    fn channels(&self, now: u64) -> [Option<ant::Snapshot>; CHANNEL_CAPACITY] {
        self.radio().channels(now)
    }
    fn take_packet(&mut self) -> Option<ant::Packet> {
        match self {
            Self::Recorded(ant) => ant.take_packet(),
            Self::Model(model) => model.radio.take_packet(),
        }
    }
}

type Radio = ((Availability, ant::Capabilities), (bool, ant::ScanSnapshot));

/// A recorded change to what the radio reports.
enum AntChange {
    Radio(Radio),
    Discoveries([Option<ant::Discovery>; DISCOVERY_CAPACITY]),
    Link(usize, Option<ant::Snapshot>),
}

/// One request, its answer, and what the request changed.
struct Answer {
    operation: AntOperation,
    result: Result<Admission, ant::Error>,
    after: Vec<AntChange>,
}

/// The radio exactly as recorded: each request takes the next recorded
/// answer, and the changes recorded after it.
#[derive(Default)]
pub struct RecordedAnt {
    radio: Latest<Radio>,
    discoveries: Latest<[Option<ant::Discovery>; DISCOVERY_CAPACITY]>,
    channels: [Latest<Option<ant::Snapshot>>; CHANNEL_CAPACITY],
    packets: VecDeque<ant::Packet>,
    admissions: VecDeque<Answer>,
    sends: RefCell<VecDeque<Option<ant::SendObservation>>>,
    unanswered: Cell<usize>,
    notes: Vec<Note>,
    now: u64,
}

impl RecordedAnt {
    fn apply(&mut self, event: Event<'_>) {
        match event {
            Event::AntRadio(radio) => {
                let types = intern(radio.capabilities.supported_types);
                let capabilities = radio.capabilities.with_types(types);
                self.change(AntChange::Radio((
                    (radio.availability, capabilities),
                    (radio.scanning, radio.scan),
                )));
            }
            Event::AntDiscoveries(found) => self.change(AntChange::Discoveries(found)),
            Event::AntLink { channel, snapshot } if usize::from(channel) < CHANNEL_CAPACITY => {
                self.change(AntChange::Link(channel.into(), snapshot));
            }
            Event::AntPacket(packet) => self.packets.push_back(packet),
            Event::AntRequest { operation, result } => self.admissions.push_back(Answer {
                operation,
                result,
                after: Vec::new(),
            }),
            Event::AntSend(status) => self.sends.borrow_mut().push_back(status),
            event => self
                .notes
                .push((Kind::Unreplayable, 1, format!("{event:?}"))),
        }
    }

    /// A change recorded after a request belongs to that request; one
    /// before any request is how the pass begins.
    fn change(&mut self, change: AntChange) {
        match self.admissions.back_mut() {
            Some(answer) => answer.after.push(change),
            None => self.apply_change(change),
        }
    }

    fn apply_change(&mut self, change: AntChange) {
        let now = self.now;
        match change {
            AntChange::Radio(radio) => self.radio.set(radio, now),
            AntChange::Discoveries(found) => self.discoveries.set(found, now),
            AntChange::Link(channel, snapshot) => self.channels[channel].set(snapshot, now),
        }
    }

    /// Every answer recorded for the pass that ended was used.
    fn drained(&mut self) {
        let left = [
            ("ANT packets", self.packets.len()),
            ("ANT answers", self.admissions.len()),
            ("ANT send states", self.sends.borrow().len()),
        ];
        for (what, count) in left {
            self.notes
                .push((Kind::LeftOver, count, format!("{count} {what}")));
        }
        let unanswered = self.unanswered.take();
        self.notes.push((
            Kind::Unanswered,
            unanswered,
            format!("{unanswered} ANT calls the recording did not answer"),
        ));
    }

    fn radio(&self) -> Radio {
        self.radio.at(self.now).unwrap_or((
            (Availability::Unsupported, ant::Capabilities::UNAVAILABLE),
            (
                false,
                ant::ScanSnapshot {
                    state: ant::ScanState::Idle,
                    generation: 0,
                    discovery_overflows: 0,
                },
            ),
        ))
    }
}

impl Ant for RecordedAnt {
    fn scanning(&self) -> bool {
        self.radio().1.0
    }
    fn discoveries(&self) -> [Option<ant::Discovery>; DISCOVERY_CAPACITY] {
        self.discoveries
            .at(self.now)
            .unwrap_or([None; DISCOVERY_CAPACITY])
    }
    fn request(&mut self, operation: AntOperation, _: u64) -> Result<Admission, ant::Error> {
        let Some(answer) = self.admissions.pop_front() else {
            self.unanswered.set(self.unanswered.get() + 1);
            return Err(ant::Error::Unavailable);
        };
        if answer.operation != operation {
            self.notes.push((
                Kind::OtherRequest,
                1,
                format!(
                    "VANA asked {operation:?}, the recording {:?}",
                    answer.operation
                ),
            ));
        }
        for change in answer.after {
            self.apply_change(change);
        }
        answer.result
    }
    fn capabilities(&self) -> ant::Capabilities {
        self.radio().0.1
    }
    fn scan(&self) -> ant::ScanSnapshot {
        self.radio().1.1
    }
    fn send_status(&self, _: ant::OperationId) -> Option<ant::SendObservation> {
        self.sends.borrow_mut().pop_front().unwrap_or_else(|| {
            self.unanswered.set(self.unanswered.get() + 1);
            None
        })
    }
    fn availability(&self) -> Availability {
        self.radio().0.0
    }
    fn channels(&self, now: u64) -> [Option<ant::Snapshot>; CHANNEL_CAPACITY] {
        self.channels.map(|latest| latest.at(now).flatten())
    }
    fn take_packet(&mut self) -> Option<ant::Packet> {
        self.packets.pop_front()
    }
}

/// How long a recorded request waits for the replay to make the same one.
const MATCH_MS: u64 = 1_000;
/// A recorded sensor is in range this close to a page or discovery of it.
const EVIDENCE_MS: u64 = 3_000;

/// A changed build's admission ahead of the model: `Some` answers the request.
type Adapter = dyn FnMut(&AntOperation, u64) -> Option<Result<Admission, ant::Error>>;

/// A request the recording made, kept for the replay's same request.
struct Recorded {
    at_ms: u64,
    operation: AntOperation,
    result: Result<Admission, ant::Error>,
}

/// The ANT radio for another build: the simulator's companion over the
/// production channel state, admitting as C606 does, with the recorded
/// sensors in range wherever the recording heard them.
pub struct ModelAnt {
    radio: SimulatedAnt,
    /// When the recording heard each of `radio.sensors`, in order.
    heard: Vec<Vec<u64>>,
    /// Pages the recording received this pass.
    pages: Vec<(Identity, [u8; 8])>,
    /// Recorded requests the replay has yet to make.
    recorded: VecDeque<Recorded>,
    adapter: Option<Box<Adapter>>,
    notes: Vec<Note>,
    now: u64,
}

impl ModelAnt {
    /// Every sensor the recording discovered or heard, with when.
    fn hearing(events: Events<'_>) -> Self {
        let mut radio = SimulatedAnt::new(Availability::Unsupported);
        let mut heard: Vec<Vec<u64>> = Vec::new();
        let mut evidence = |radio: &mut SimulatedAnt, identity: Identity, at: u64, rssi| {
            let index = match radio.sensors.iter().position(|s| s.identity == identity) {
                Some(index) => index,
                None => {
                    let mut sensor = AntSensor::new(identity.device_type, identity.device_number);
                    sensor.identity = identity;
                    // Its answer to a zero request is among the recorded pages.
                    sensor.zero = ZeroReply::Silence;
                    radio.sensors.push(sensor);
                    heard.push(Vec::new());
                    radio.sensors.len() - 1
                }
            };
            if let Some(rssi) = rssi {
                radio.sensors[index].rssi = rssi;
            }
            heard[index].push(at);
        };
        let mut now = 0;
        for event in events {
            match event {
                Event::Pass { now_ms } => now = now_ms,
                Event::AntDiscoveries(found) => {
                    for discovery in found.into_iter().flatten() {
                        evidence(
                            &mut radio,
                            discovery.identity,
                            discovery.seen_ms,
                            Some(discovery.rssi),
                        );
                    }
                }
                Event::AntPacket(packet) => {
                    evidence(&mut radio, packet.identity, packet.received_ms, None)
                }
                Event::AntLink {
                    snapshot: Some(snapshot),
                    ..
                } if snapshot.link == LinkState::Connected && !snapshot.stale => {
                    if let Some(identity) = snapshot.selected {
                        evidence(&mut radio, identity, now, None);
                    }
                }
                _ => {}
            }
        }
        for times in &mut heard {
            times.sort_unstable();
        }
        Self {
            radio,
            heard,
            pages: Vec::new(),
            recorded: VecDeque::new(),
            adapter: None,
            notes: Vec::new(),
            now: 0,
        }
    }

    fn apply(&mut self, event: Event<'_>) {
        match event {
            Event::AntRadio(radio) => self.radio.availability = radio.availability,
            Event::AntPacket(packet) => self.pages.push((packet.identity, packet.data)),
            Event::AntRequest { operation, result } => self.recorded.push_back(Recorded {
                at_ms: self.now,
                operation,
                result,
            }),
            // The model keeps its own channels, scan and sends.
            _ => {}
        }
    }

    /// Move the model to this pass: sensors in range as recorded, and the
    /// recorded pages sent.
    fn step(&mut self) {
        let now = self.now;
        self.expire(now);
        for (sensor, times) in self.radio.sensors.iter_mut().zip(&self.heard) {
            let first = times.partition_point(|&at| at + EVIDENCE_MS < now);
            sensor.present = times.get(first).is_some_and(|&at| at <= now + EVIDENCE_MS);
        }
        let pages = core::mem::take(&mut self.pages);
        for identity in self.radio.tick_hearing(now, pages) {
            self.notes.push((
                Kind::NotHeard,
                1,
                format!("a page from {identity:?} with no channel open to it"),
            ));
        }
    }

    /// Recorded requests the replay did not make in time are dropped.
    fn expire(&mut self, now: u64) {
        while let Some(recorded) = self.recorded.front() {
            if recorded.at_ms.saturating_add(MATCH_MS) >= now {
                break;
            }
            let operation = recorded.operation;
            self.recorded.pop_front();
            self.notes.push((
                Kind::NotRequested,
                1,
                format!("the recording asked {operation:?}"),
            ));
        }
    }

    /// The adapter's answer, else the recorded answer to the same request
    /// when the recording failed it, else the model's.
    fn request(&mut self, operation: AntOperation, now: u64) -> Result<Admission, ant::Error> {
        if let Some(adapter) = &mut self.adapter
            && let Some(answer) = adapter(&operation, now)
        {
            return answer;
        }
        let same = self
            .recorded
            .iter()
            .position(|recorded| same_request(&recorded.operation, &operation));
        let Some(recorded) = same.and_then(|index| self.recorded.remove(index)) else {
            self.notes.push((
                Kind::Unanswered,
                1,
                format!("{operation:?} answered by the model"),
            ));
            return self.radio.request(operation, now);
        };
        recorded.result?;
        let answer = self.radio.request(operation, now);
        if let Err(error) = answer {
            self.notes.push((
                Kind::Refused,
                1,
                format!("{operation:?}: the model answered {error:?}"),
            ));
        }
        answer
    }
}

/// The same request, but for the channel generation a send names: that is
/// the radio's own count, and the model keeps another.
fn same_request(a: &AntOperation, b: &AntOperation) -> bool {
    match (a, b) {
        (
            AntOperation::Send { target, data },
            AntOperation::Send {
                target: other,
                data: other_data,
            },
        ) => target.identity == other.identity && data == other_data,
        _ => a == b,
    }
}

#[derive(Default)]
pub struct ReplayBle {
    link: Latest<(Availability, ble_transport::Snapshot)>,
    packets: VecDeque<ble_transport::Packet>,
    battery: VecDeque<ble_transport::BatteryLevel>,
    now: u64,
}

impl Ble for ReplayBle {
    fn availability(&self) -> Availability {
        self.link
            .at(self.now)
            .map_or(Availability::Unsupported, |link| link.0)
    }
    fn snapshot(&self) -> ble_transport::Snapshot {
        self.link
            .at(self.now)
            .map(|link| link.1)
            .unwrap_or_default()
    }
    fn take_packet(&mut self) -> Option<ble_transport::Packet> {
        self.packets.pop_front()
    }
    fn take_battery(&mut self) -> Option<ble_transport::BatteryLevel> {
        self.battery.pop_front()
    }
    fn reconnect(&mut self) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

#[derive(Default)]
pub struct ReplayPosition {
    latest: Latest<(Availability, Option<positioning::Snapshot>)>,
    now: u64,
}

impl Positioning for ReplayPosition {
    fn availability(&self) -> Availability {
        self.latest
            .at(self.now)
            .map_or(Availability::Unsupported, |position| position.0)
    }
    fn snapshot(&self, now: u64) -> Option<positioning::Snapshot> {
        self.latest.at(now).and_then(|position| position.1)
    }
}

#[derive(Default)]
pub struct ReplayCompanion(Latest<Option<InputSnapshot>>);

impl InputObservation for ReplayCompanion {
    fn snapshot(&self, now: u64) -> Option<InputSnapshot> {
        self.0.at(now).flatten()
    }
}

/// Entropy values in the order the recording consumed them, shared by the
/// medium and the access point, and how many were asked for past the end.
#[derive(Clone, Default)]
struct Entropy(Rc<RefCell<VecDeque<u64>>>, Rc<Cell<u32>>);

impl Entropy {
    fn take(&self) -> Option<u64> {
        let value = self.0.borrow_mut().pop_front();
        if value.is_none() {
            self.1.set(self.1.get() + 1);
        }
        value
    }
}

/// The replaying device's medium, with the recorded entropy; past the
/// recording it draws on the medium's own.
pub struct ReplayBulk {
    media: SimBulk,
    entropy: Entropy,
}

impl ReadWrite for ReplayBulk {
    fn owned_info(&mut self) -> Result<bulk::OwnedInfo, bulk::Error> {
        self.media.owned_info()
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; bulk::SECTOR_SIZE],
    ) -> Result<(), bulk::Error> {
        self.media.owned_read(sector, output)
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; bulk::SECTOR_SIZE],
    ) -> Result<(), bulk::Error> {
        self.media.owned_write(sector, input)
    }
    fn entropy(&mut self) -> u64 {
        self.entropy.take().unwrap_or_else(|| self.media.entropy())
    }
}

/// An access point that follows VANA's requests, with the recorded entropy.
#[cfg(feature = "cycling")]
pub struct ReplayAccessPoint {
    inner: SimAccessPoint,
    entropy: Entropy,
}

#[cfg(feature = "cycling")]
impl AccessPoint for ReplayAccessPoint {
    fn start(&mut self, config: AccessPointConfig) -> Result<(), Error> {
        self.inner.start(config)
    }
    fn stop(&mut self) -> Result<(), Error> {
        self.inner.stop()
    }
    fn state(&self) -> AccessPointState {
        self.inner.state()
    }
    fn reach(&self) -> Reach {
        self.inner.reach()
    }
    fn entropy(&mut self) -> u32 {
        // The recording's access point gave 32 bits.
        self.entropy
            .take()
            .map_or_else(|| self.inner.entropy(), |value| value as u32)
    }
}
