//! A recorded trace standing in for the world: the radios, the receiver, the
//! barometer and the companion report what they reported when it was
//! recorded, and the session runs one pass at each recorded uptime.
use crate::bulk::SimBulk;
use crate::devices::InputDevice;
use crate::storage::FileFlash;
use device_api::ant::{self, Ant, AntOperation, CHANNEL_CAPACITY, DISCOVERY_CAPACITY};
use device_api::ble_transport::{self, Ble};
use device_api::input::{InputObservation, InputSnapshot};
use device_api::observation::{Availability, Error};
use device_api::positioning::{self, Positioning};
use device_api::sensors;
use firmware_services::trace::frozen::{Latest, Sink};
use firmware_services::trace::{self, Cursor, Event, Events};
use std::cell::{Cell, RefCell};
use std::collections::VecDeque;

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
    pub(crate) bulk: SimBulk,
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

pub struct Replay {
    bytes: Vec<u8>,
    cursor: Cursor,
    pub sources: Sources,
}

impl Replay {
    pub fn new(bytes: Vec<u8>, input: InputDevice) -> Result<Self, trace::Error> {
        let cursor = trace::decode(&bytes)?.1.cursor();
        Ok(Self {
            bytes,
            cursor,
            sources: Sources::new(input),
        })
    }

    /// Move past what the rider saw to the next pass or command. A command
    /// is taken: the caller executes it.
    pub fn step(&mut self) -> Next {
        loop {
            let mut events = Events::resume(&self.bytes, self.cursor);
            let Some(event) = events.next() else {
                self.sources.drained();
                return Next::End;
            };
            match event {
                Event::Pass { .. } => return Next::Pass,
                Event::Command(line) => {
                    self.cursor = events.cursor();
                    return Next::Command(line.into());
                }
                event if seen(&event) => {}
                event => self.sources.diverge(format!("{event:?} outside a pass")),
            }
            self.cursor = events.cursor();
        }
    }

    /// Take the next pass and what its devices reported; its uptime.
    pub fn load_pass(&mut self) -> u64 {
        self.sources.drained();
        let mut events = Events::resume(&self.bytes, self.cursor);
        match events.next() {
            Some(Event::Pass { now_ms }) => self.sources.begin(now_ms),
            other => {
                self.sources.diverge(format!(
                    "a pass the recording did not run; it holds {other:?}"
                ));
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
                Some(event) => self.sources.apply(event),
            }
        }
        self.cursor = events.cursor();
        self.sources.now
    }
}

/// Outputs: the rider's view, recorded for agents and for comparison.
fn seen(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Page(_)
            | Event::Toast(_)
            | Event::Sound(_)
            | Event::RideState(_)
            | Event::Shown(_)
            | Event::Log { .. }
    )
}

/// The devices as the trace reports them, one pass at a time.
pub struct Sources {
    pub ant: ReplayAnt,
    pub ble: ReplayBle,
    pub position: ReplayPosition,
    pub companion: ReplayCompanion,
    startup: Latest<sensors::Startup>,
    physical: Latest<sensors::Snapshot>,
    network_time: Latest<firmware_services::network_time::Snapshot>,
    input: InputDevice,
    now: u64,
    /// The first place this run asked for something the recording did not
    /// answer, or left an answer unused.
    pub divergence: Option<String>,
}

impl Sources {
    fn new(input: InputDevice) -> Self {
        Self {
            ant: ReplayAnt::default(),
            ble: ReplayBle::default(),
            position: ReplayPosition::default(),
            companion: ReplayCompanion::default(),
            startup: Latest::default(),
            physical: Latest::default(),
            network_time: Latest::default(),
            input,
            now: 0,
            divergence: None,
        }
    }

    fn begin(&mut self, now: u64) {
        self.now = now;
        self.ant.now = now;
        self.ble.now = now;
        self.position.now = now;
    }

    fn diverge(&mut self, what: String) {
        let now = self.now;
        self.divergence
            .get_or_insert_with(|| format!("pass at {now} ms: {what}"));
    }

    /// Every answer recorded for the pass that ended was used.
    fn drained(&mut self) {
        let left = [
            ("ANT packets", self.ant.packets.len()),
            ("ANT admissions", self.ant.admissions.len()),
            ("ANT send states", self.ant.sends.borrow().len()),
            ("ANT requests unanswered", self.ant.unanswered.get()),
            ("BLE packets", self.ble.packets.len()),
            ("BLE battery levels", self.ble.battery.len()),
            ("input edges", self.input.replayed.borrow().len()),
        ];
        for (what, count) in left {
            if count > 0 {
                self.diverge(format!("{count} {what} left over"));
            }
        }
    }

    fn apply(&mut self, event: Event<'_>) {
        let now = self.now;
        match event {
            Event::Input(edge) => self.input.replayed.borrow_mut().push_back(edge),
            Event::AntRadio(radio) => {
                let types = intern(radio.capabilities.supported_types);
                let capabilities = radio.capabilities.with_types(types);
                self.ant.change(AntChange::Radio((
                    (radio.availability, capabilities),
                    (radio.scanning, radio.scan),
                )));
            }
            Event::AntDiscoveries(found) => self.ant.change(AntChange::Discoveries(found)),
            Event::AntLink { channel, snapshot } => {
                if usize::from(channel) < CHANNEL_CAPACITY {
                    self.ant.change(AntChange::Link(channel.into(), snapshot));
                } else {
                    self.diverge(format!("ANT channel {channel}"));
                }
            }
            Event::AntPacket(packet) => self.ant.packets.push_back(packet),
            Event::AntAdmission(result) => self.ant.admissions.push_back(Answer {
                result,
                after: Vec::new(),
            }),
            Event::AntSend(status) => self.ant.sends.borrow_mut().push_back(status),
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
            event if seen(&event) => {}
            event => self.diverge(format!("{event:?} is not replayed in the simulator")),
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

type Radio = ((Availability, ant::Capabilities), (bool, ant::ScanSnapshot));

/// A recorded change to what the radio reports.
enum AntChange {
    Radio(Radio),
    Discoveries([Option<ant::Discovery>; DISCOVERY_CAPACITY]),
    Link(usize, Option<ant::Snapshot>),
}

/// The answer to one request, and what the request changed.
struct Answer {
    result: Result<ant::Admission, ant::Error>,
    after: Vec<AntChange>,
}

#[derive(Default)]
pub struct ReplayAnt {
    radio: Latest<Radio>,
    discoveries: Latest<[Option<ant::Discovery>; DISCOVERY_CAPACITY]>,
    channels: [Latest<Option<ant::Snapshot>>; CHANNEL_CAPACITY],
    packets: VecDeque<ant::Packet>,
    admissions: VecDeque<Answer>,
    sends: RefCell<VecDeque<Option<ant::SendObservation>>>,
    unanswered: Cell<usize>,
    now: u64,
}

impl ReplayAnt {
    /// A change recorded after a request belongs to that request; one
    /// before any request is how the pass begins.
    fn change(&mut self, change: AntChange) {
        match self.admissions.back_mut() {
            Some(answer) => answer.after.push(change),
            None => self.apply(change),
        }
    }

    fn apply(&mut self, change: AntChange) {
        let now = self.now;
        match change {
            AntChange::Radio(radio) => self.radio.set(radio, now),
            AntChange::Discoveries(found) => self.discoveries.set(found, now),
            AntChange::Link(channel, snapshot) => self.channels[channel].set(snapshot, now),
        }
    }

    fn radio(&self) -> ((Availability, ant::Capabilities), (bool, ant::ScanSnapshot)) {
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

impl Ant for ReplayAnt {
    fn scanning(&self) -> bool {
        self.radio().1.0
    }
    fn discoveries(&self) -> [Option<ant::Discovery>; 8] {
        self.discoveries
            .at(self.now)
            .unwrap_or([None; DISCOVERY_CAPACITY])
    }
    fn request(&mut self, _: AntOperation, _: u64) -> Result<ant::Admission, ant::Error> {
        let Some(answer) = self.admissions.pop_front() else {
            self.unanswered.set(self.unanswered.get() + 1);
            return Err(ant::Error::Unavailable);
        };
        for change in answer.after {
            self.apply(change);
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
