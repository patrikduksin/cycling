//! A recorded trace standing in for the world: every device VANA and the
//! shell read reports what it reported when it was recorded, and the session
//! runs one pass at each recorded uptime. A read the trace holds no answer
//! for is a divergence, never a guess from a model.
//!
//! A strict replay runs the build that recorded the trace and must answer
//! every call from it. A stimulus replay runs another build against the
//! same world: inputs arrive at their recorded pass whatever VANA asks, and
//! the ANT radio is a model that hears the recorded sensors. Either way the
//! replay keeps a [`Report`] of where it parted from the recording.
use crate::ant::{AntSensor, SimulatedAnt, ZeroReply};
use crate::bulk::SimBulk;
use crate::devices::{DisplayDevice, InputDevice, PowerDevice};
use crate::storage::FileFlash;
use device_api::ant::{
    self, Admission, Ant, AntOperation, CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Identity, LinkState,
};
use device_api::ble_transport::{self, Ble};
use device_api::bulk::{self, ReadWrite};
use device_api::input::{InputObservation, InputSnapshot};
use device_api::observation::{Availability, Error};
use device_api::positioning::{self, Positioning};
use device_api::power::{self, PowerControl};
use device_api::sensors;
use device_api::server::{self, AccessPoint, AccessPointConfig, AccessPointState, Reach, Server};
use device_api::sound::{self, Sound};
use firmware_services::trace::frozen::{Aging, Latest, Sink};
use firmware_services::trace::{self, Cursor, End, Event, Events, Medium, Output};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
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
    /// VANA or the shell read an input the trace holds no value for.
    Unrecorded,
    /// A stimulus replay's ANT model answered a request the recording
    /// never made.
    Modelled,
    /// A recorded answer, input or entropy value the pass did not use.
    LeftOver,
    /// VANA made another request than the one recorded next.
    OtherRequest,
    /// A recorded request the replay did not make in time.
    NotRequested,
    /// The model refused an ANT request the recording's radio accepted.
    Refused,
    /// A recorded sensor page reached no open channel.
    NotHeard,
    /// Recorded input VANA had not taken by the end of its pass; it waits.
    Carried,
    /// The rider saw another page or heard another sound.
    Seen,
    /// What the replay handed VANA, as it recorded it again, differs from
    /// the trace it replayed.
    Rerecorded,
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
#[derive(Clone, Debug)]
pub struct Report {
    pub mode: Mode,
    pub first: Option<Divergence>,
    pub counts: BTreeMap<Kind, u32>,
    now: u64,
}

impl Report {
    fn new(mode: Mode) -> Self {
        Self {
            mode,
            first: None,
            counts: BTreeMap::new(),
            now: 0,
        }
    }

    /// What answered VANA's reads.
    pub fn header(&self) -> &'static str {
        match self.mode {
            Mode::Strict => {
                "strict replay: every input VANA and the shell read came from the trace"
            }
            Mode::Stimulus => {
                "stimulus replay: every input VANA and the shell read came from the trace, except \
                 the ANT radio, a model that hears the sensors the recording heard"
            }
        }
    }

    pub fn count(&self, kind: Kind) -> u32 {
        self.counts.get(&kind).copied().unwrap_or(0)
    }

    fn note(&mut self, kind: Kind, count: usize, detail: impl FnOnce() -> String) {
        if count == 0 {
            return;
        }
        *self.counts.entry(kind).or_default() += count as u32;
        let at_ms = self.now;
        self.first.get_or_insert_with(|| Divergence {
            at_ms,
            kind,
            detail: detail(),
        });
    }
}

impl fmt::Display for Report {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.header())?;
        match &self.first {
            None => writeln!(f, "no divergence"),
            Some(first) => {
                writeln!(
                    f,
                    "first at {} ms, {:?}: {}",
                    first.at_ms, first.kind, first.detail
                )?;
                for (kind, count) in &self.counts {
                    writeln!(f, "{kind:?} {count}")?;
                }
                Ok(())
            }
        }
    }
}

#[derive(Clone)]
struct Ledger(Rc<RefCell<Report>>);

impl Ledger {
    fn note(&self, kind: Kind, count: usize, detail: impl FnOnce() -> String) {
        self.0.borrow_mut().note(kind, count, detail);
    }

    fn unrecorded(&self, input: &str) {
        self.note(Kind::Unrecorded, 1, || format!("unrecorded input {input}"));
    }

    fn now(&self) -> u64 {
        self.0.borrow().now
    }

    fn mode(&self) -> Mode {
        self.0.borrow().mode
    }
}

struct Served<T> {
    latest: Latest<T>,
    input: &'static str,
    ledger: Ledger,
}

impl<T: Aging> Served<T> {
    fn new(input: &'static str, ledger: &Ledger) -> Self {
        Self {
            latest: Latest::default(),
            input,
            ledger: ledger.clone(),
        }
    }

    fn set(&mut self, value: T) {
        self.latest.set(value, self.ledger.now());
    }

    fn read(&self) -> Option<T> {
        let value = self.latest.at(self.ledger.now());
        if value.is_none() {
            self.ledger.unrecorded(self.input);
        }
        value
    }
}

struct Calls<C, V> {
    queue: VecDeque<(C, Vec<V>)>,
    input: &'static str,
    ledger: Ledger,
}

impl<C: fmt::Debug, V> Calls<C, V> {
    fn new(input: &'static str, ledger: &Ledger) -> Self {
        Self {
            queue: VecDeque::new(),
            input,
            ledger: ledger.clone(),
        }
    }

    fn push(&mut self, call: C) {
        self.queue.push_back((call, Vec::new()));
    }

    fn last(&mut self) -> Option<&mut C> {
        self.queue.back_mut().map(|(call, _)| call)
    }

    fn change(&mut self, change: V) -> Option<V> {
        match self.queue.back_mut() {
            Some((_, after)) => {
                after.push(change);
                None
            }
            None => Some(change),
        }
    }

    fn take(
        &mut self,
        asked: impl fmt::Debug,
        matches: impl Fn(&C) -> bool,
    ) -> Option<(C, Vec<V>)> {
        let index = match self.ledger.mode() {
            Mode::Strict => (!self.queue.is_empty()).then_some(0),
            Mode::Stimulus => self.queue.iter().position(|(call, _)| matches(call)),
        };
        let Some(taken) = index.and_then(|index| self.queue.remove(index)) else {
            self.ledger.unrecorded(self.input);
            return None;
        };
        if !matches(&taken.0) {
            self.ledger.note(Kind::OtherRequest, 1, || {
                format!("VANA asked {asked:?}, the recording {:?}", taken.0)
            });
        }
        Some(taken)
    }

    fn take_if(&mut self, matches: impl Fn(&C) -> bool) -> Option<(C, Vec<V>)> {
        let front = self.queue.front().is_some_and(|(call, _)| matches(call));
        front.then(|| self.queue.pop_front()).flatten()
    }

    fn drained(&mut self) -> Vec<V> {
        let left = self.queue.len();
        let kind = match self.ledger.mode() {
            Mode::Strict => Kind::LeftOver,
            Mode::Stimulus => Kind::NotRequested,
        };
        let (input, queue) = (self.input, &self.queue);
        self.ledger.note(kind, left, || {
            let calls: Vec<_> = queue.iter().map(|(call, _)| call).collect();
            format!("{left} {input} answers VANA did not ask for: {calls:?}")
        });
        self.queue.drain(..).flat_map(|(_, after)| after).collect()
    }
}

/// What the rider saw change in a pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    Page(String),
    Sound(u8),
}

/// The shell's own devices, which a replay scripts from the trace: the
/// panel and backlight refuse, and the media fail, as recorded.
pub struct Shelled {
    pub input: InputDevice,
    pub display: DisplayDevice,
    pub power: PowerDevice,
    pub flash: FileFlash,
    pub bulk: SimBulk,
}

pub struct Replay {
    bytes: Vec<u8>,
    cursor: Cursor,
    pub sources: Sources,
    seen: Vec<Seen>,
}

impl Replay {
    pub fn new(bytes: Vec<u8>, shelled: Shelled, mode: Mode) -> Result<Self, trace::Error> {
        let (_, events) = trace::decode(&bytes)?;
        let cursor = events.cursor();
        let ledger = Ledger(Rc::new(RefCell::new(Report::new(mode))));
        let ant = match mode {
            Mode::Strict => AntSource::Recorded(Box::new(RecordedAnt::new(&ledger))),
            Mode::Stimulus => AntSource::Model(Box::new(ModelAnt::hearing(events, &ledger))),
        };
        Ok(Self {
            sources: Sources::new(ant, shelled, ledger),
            bytes,
            cursor,
            seen: Vec::new(),
        })
    }

    /// Move past what the rider saw to the next pass or command. Inputs
    /// recorded before a command, such as the power status its gate read,
    /// apply now. A command is taken: the caller executes it.
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
                event => self.sources.apply(event),
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
                self.sources.ledger.note(Kind::Unrecorded, 1, || {
                    format!(
                        "unrecorded input: a pass the recording did not run; it holds {other:?}"
                    )
                });
                return self.sources.ledger.now();
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
        self.sources.ledger.now()
    }

    /// What the rider saw in the pass just run, against the recording.
    pub fn saw(&mut self, seen: &[Seen]) {
        if seen != self.seen.as_slice() {
            let recorded = core::mem::take(&mut self.seen);
            self.sources.ledger.note(Kind::Seen, 1, || {
                format!("the rider saw {seen:?}, the recording {recorded:?}")
            });
        }
    }

    /// A strict replay hands VANA what the trace holds, so recording it
    /// again gives the trace back. Where it does not, VANA was handed a
    /// value from elsewhere or the trace held one VANA never took.
    pub fn rerecorded(&mut self, trace: &[u8]) {
        if self.sources.ledger.mode() != Mode::Strict {
            return;
        }
        if let Some(difference) = first_difference(&self.bytes, trace) {
            self.sources.ledger.note(Kind::Rerecorded, 1, || difference);
        }
    }

    /// The trace this replay runs.
    pub fn trace(&self) -> &[u8] {
        &self.bytes
    }
}

/// The first event where two traces part, with the pass it belongs to.
pub fn first_difference(recorded: &[u8], replayed: &[u8]) -> Option<String> {
    let (header, mut events) = match trace::decode(recorded) {
        Ok(decoded) => decoded,
        Err(error) => return Some(format!("the recorded trace does not decode: {error:?}")),
    };
    let (replayed_header, mut replayed_events) = match trace::decode(replayed) {
        Ok(decoded) => decoded,
        Err(error) => return Some(format!("the replayed trace does not decode: {error:?}")),
    };
    if header != replayed_header {
        return Some(format!("header {header:?}, replayed {replayed_header:?}"));
    }
    let mut pass = 0;
    for (index, event) in events.by_ref().enumerate() {
        if let Event::Pass { now_ms } = event {
            pass = now_ms;
        }
        let other = replayed_events.next();
        if other != Some(event) {
            return Some(format!(
                "event {index}, pass at {pass} ms: recorded {event:?}, replayed {other:?}"
            ));
        }
    }
    if events.end() != Some(End::Complete) {
        return Some(format!("the recorded trace ends {:?}", events.end()));
    }
    replayed_events
        .next()
        .map(|extra| format!("the replay went on with {extra:?}"))
}

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
    pub access_point: ReplayAccessPoint,
    pub server: ReplayServer,
    pub sound: ReplaySound,
    pub power: ReplayPowerControl,
    entropy: Entropy,
    startup: Served<sensors::Startup>,
    physical: Served<sensors::Snapshot>,
    network_time: Served<firmware_services::network_time::Snapshot>,
    backlight: Served<Availability>,
    shelled: Shelled,
    ledger: Ledger,
}

impl Sources {
    fn new(ant: AntSource, shelled: Shelled, ledger: Ledger) -> Self {
        let entropy = Entropy {
            values: Rc::default(),
            ledger: ledger.clone(),
        };
        Self {
            ant,
            ble: ReplayBle::new(&ledger),
            position: ReplayPosition(Served::new("position", &ledger)),
            companion: ReplayCompanion(Served::new("companion report", &ledger)),
            bulk: ReplayBulk {
                media: shelled.bulk.clone(),
                entropy: entropy.clone(),
            },
            access_point: ReplayAccessPoint {
                view: Served::new("access point", &ledger),
                calls: Calls::new("access point", &ledger),
                entropy: entropy.clone(),
            },
            server: ReplayServer {
                view: Served::new("phone server", &ledger),
                calls: Calls::new("phone server", &ledger),
            },
            sound: ReplaySound {
                view: Served::new("buzzer", &ledger),
                calls: Calls::new("buzzer", &ledger),
            },
            power: ReplayPowerControl {
                view: Served::new("power control", &ledger),
                calls: Calls::new("power control", &ledger),
                off: false,
                ledger: ledger.clone(),
            },
            entropy,
            startup: Served::new("companion startup", &ledger),
            physical: Served::new("barometer and motion", &ledger),
            network_time: Served::new("network time", &ledger),
            backlight: Served::new("backlight", &ledger),
            shelled,
            ledger,
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

    /// Where the replay parted from its recording so far.
    pub fn report(&self) -> Report {
        self.ledger.0.borrow().clone()
    }

    fn begin(&mut self, now: u64) {
        self.ledger.0.borrow_mut().now = now;
        self.power.off = false;
    }

    fn ready(&mut self) {
        if let Some(available) = self.backlight.latest.at(self.ledger.now()) {
            self.shelled.power.0.borrow_mut().available = available;
        }
        if let AntSource::Model(model) = &mut self.ant {
            model.step();
        }
    }

    fn drained(&mut self) {
        let mode = self.ledger.mode();
        let queued = [
            ("BLE packets", self.ble.packets.len()),
            ("BLE battery levels", self.ble.battery.len()),
            ("input edges", self.shelled.input.replayed.borrow().len()),
        ];
        let kind = match mode {
            Mode::Strict => Kind::LeftOver,
            Mode::Stimulus => Kind::Carried,
        };
        for (what, count) in queued {
            self.ledger.note(kind, count, || format!("{count} {what}"));
        }
        let unused = match mode {
            Mode::Strict => Kind::LeftOver,
            Mode::Stimulus => Kind::NotRequested,
        };
        if mode == Mode::Strict {
            let left = self.entropy.values.borrow().len();
            self.ledger
                .note(Kind::LeftOver, left, || format!("{left} entropy values"));
        }
        let panel = core::mem::take(&mut self.shelled.display.0.borrow_mut().refusals.scripted);
        let backlight = core::mem::take(&mut self.shelled.power.0.borrow_mut().refusals.scripted);
        for (what, left) in [("panel", panel.len()), ("backlight", backlight.len())] {
            self.ledger.note(unused, left, || {
                format!("{left} {what} refusals no attempt met")
            });
        }
        let flash = core::mem::take(&mut self.shelled.flash.fault.borrow_mut().script);
        let bulk = self.shelled.bulk.unmet();
        for fault in flash.iter().chain(&bulk) {
            self.ledger.note(unused, 1, || {
                format!("a storage fault no operation met: {fault:?}")
            });
        }
        for view in self.access_point.calls.drained() {
            self.access_point.view.set(view);
        }
        for view in self.server.calls.drained() {
            self.server.view.set(view);
        }
        for view in self.sound.calls.drained() {
            self.sound.view.set(view);
        }
        for view in self.power.calls.drained() {
            self.power.view.set(view);
        }
        if let AntSource::Recorded(ant) = &mut self.ant {
            ant.drained();
        }
    }

    fn ended(&mut self) {
        if let AntSource::Model(model) = &mut self.ant {
            model.expire(u64::MAX);
        }
    }

    fn apply(&mut self, event: Event<'_>) {
        match event {
            Event::Input(edge) => self.shelled.input.replayed.borrow_mut().push_back(edge),
            Event::AntRadio(_)
            | Event::AntDiscoveries(_)
            | Event::AntLink { .. }
            | Event::AntPacket(_)
            | Event::AntRequest { .. }
            | Event::AntSend(_) => self.ant.apply(event),
            Event::BleLink {
                availability,
                snapshot,
            } => self.ble.link.set((availability, snapshot)),
            Event::BlePacket(packet) => self.ble.packets.push_back(packet),
            Event::BleBattery(level) => self.ble.battery.push_back(level),
            Event::Position {
                availability,
                snapshot,
            } => self.position.0.set((availability, snapshot)),
            Event::Physical(snapshot) => self.physical.set(snapshot),
            Event::Power(snapshot) => self.companion.0.set(snapshot),
            Event::NetworkTime(snapshot) => self.network_time.set(snapshot),
            Event::Startup(startup) => self.startup.set(startup),
            Event::Entropy(value) => self.entropy.values.borrow_mut().push_back(value),
            Event::StorageFault(fault) => match fault.medium {
                Medium::Bulk => self.shelled.bulk.script(fault),
                _ => self
                    .shelled
                    .flash
                    .fault
                    .borrow_mut()
                    .script
                    .push_back(fault),
            },
            Event::Refused { output, attempt } => match output {
                Output::Panel => self
                    .shelled
                    .display
                    .0
                    .borrow_mut()
                    .refusals
                    .scripted
                    .push(attempt),
                Output::Backlight => self
                    .shelled
                    .power
                    .0
                    .borrow_mut()
                    .refusals
                    .scripted
                    .push(attempt),
            },
            Event::Backlight(availability) => self.backlight.set(availability),
            Event::AccessPoint { state, reach } => {
                if let Some(view) = self.access_point.calls.change((state, reach)) {
                    self.access_point.view.set(view);
                }
            }
            Event::AccessPointRequest { start, result } => {
                self.access_point.calls.push((start, result))
            }
            Event::Server { request, room } => {
                let view = (request.map(|request| request.to_request()), room as usize);
                if let Some(view) = self.server.calls.change(view) {
                    self.server.view.set(view);
                }
            }
            Event::ServerListen(on) => self.server.calls.push(ServerCall::Listen(on)),
            Event::ServerFinish(id) => self.server.calls.push(ServerCall::Finish(id)),
            Event::ServerRead { id, part, more } => match self.server.calls.last() {
                Some(ServerCall::Read {
                    id: open_id,
                    bytes,
                    open: open @ true,
                }) if *open_id == id => {
                    bytes.extend_from_slice(part);
                    *open = more;
                }
                _ => self.server.calls.push(ServerCall::Read {
                    id,
                    bytes: part.to_vec(),
                    open: more,
                }),
            },
            Event::ServerWrite { id, taken } => {
                self.server.calls.push(ServerCall::Write { id, taken })
            }
            Event::SoundState(snapshot) => {
                if let Some(view) = self.sound.calls.change(snapshot) {
                    self.sound.view.set(view);
                }
            }
            Event::SoundRequest { play, result } => self.sound.calls.push((play, result)),
            Event::PowerControl(status) => {
                if let Some(view) = self.power.calls.change(status) {
                    self.power.view.set(view);
                }
            }
            Event::PowerRequest {
                operation,
                delay_ms,
                result,
            } => self.power.calls.push((operation, delay_ms, result)),
            Event::PoweredOff => self.power.off = true,
            Event::TooLarge(tag) => self
                .ledger
                .unrecorded(&format!("a record with tag {tag}, too large for a block")),
            event => self.ledger.note(Kind::Unreplayable, 1, || {
                format!("{event:?} is not replayed in the simulator")
            }),
        }
    }

    pub fn startup(&self) -> sensors::Startup {
        self.startup.read().unwrap_or(sensors::Startup::Unsupported)
    }

    pub fn physical(&self) -> sensors::Snapshot {
        self.physical.read().unwrap_or(sensors::Snapshot {
            pressure: device_api::observation::Observation::Unavailable,
            motion: [device_api::observation::Observation::Unavailable; 2],
            identity: device_api::observation::Observation::Unavailable,
            reports: 0,
            invalid_reports: 0,
            losses: 0,
        })
    }

    pub fn network_time(&self) -> firmware_services::network_time::Snapshot {
        self.network_time.read().unwrap_or_default()
    }
}

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
    fn apply(&mut self, event: Event<'_>) {
        match self {
            Self::Recorded(ant) => ant.apply(event),
            Self::Model(model) => model.apply(event),
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

enum AntChange {
    Radio(Radio),
    Discoveries([Option<ant::Discovery>; DISCOVERY_CAPACITY]),
    Link(usize, Option<ant::Snapshot>),
}

struct Answer {
    operation: AntOperation,
    result: Result<Admission, ant::Error>,
    after: Vec<AntChange>,
}

/// The radio exactly as recorded: each request takes the next recorded
/// answer, and the changes recorded after it.
pub struct RecordedAnt {
    radio: Served<Radio>,
    discoveries: Served<[Option<ant::Discovery>; DISCOVERY_CAPACITY]>,
    channels: [Served<Option<ant::Snapshot>>; CHANNEL_CAPACITY],
    packets: VecDeque<ant::Packet>,
    admissions: VecDeque<Answer>,
    sends: RefCell<VecDeque<Option<ant::SendObservation>>>,
    ledger: Ledger,
}

impl RecordedAnt {
    fn new(ledger: &Ledger) -> Self {
        Self {
            radio: Served::new("ANT radio", ledger),
            discoveries: Served::new("ANT discoveries", ledger),
            channels: core::array::from_fn(|_| Served::new("ANT channel", ledger)),
            packets: VecDeque::new(),
            admissions: VecDeque::new(),
            sends: RefCell::default(),
            ledger: ledger.clone(),
        }
    }

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
                .ledger
                .note(Kind::Unreplayable, 1, || format!("{event:?}")),
        }
    }

    fn change(&mut self, change: AntChange) {
        match self.admissions.back_mut() {
            Some(answer) => answer.after.push(change),
            None => self.apply_change(change),
        }
    }

    fn apply_change(&mut self, change: AntChange) {
        match change {
            AntChange::Radio(radio) => self.radio.set(radio),
            AntChange::Discoveries(found) => self.discoveries.set(found),
            AntChange::Link(channel, snapshot) => self.channels[channel].set(snapshot),
        }
    }

    fn drained(&mut self) {
        let left = [
            ("ANT packets", self.packets.len()),
            ("ANT answers", self.admissions.len()),
            ("ANT send states", self.sends.borrow().len()),
        ];
        for (what, count) in left {
            self.ledger
                .note(Kind::LeftOver, count, || format!("{count} {what}"));
        }
        self.packets.clear();
        self.sends.borrow_mut().clear();
        for answer in core::mem::take(&mut self.admissions) {
            for change in answer.after {
                self.apply_change(change);
            }
        }
    }

    fn radio(&self) -> Radio {
        self.radio.read().unwrap_or((
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
            .read()
            .unwrap_or([None; DISCOVERY_CAPACITY])
    }
    fn request(&mut self, operation: AntOperation, _: u64) -> Result<Admission, ant::Error> {
        let Some(answer) = self.admissions.pop_front() else {
            self.ledger.unrecorded("ANT request");
            return Err(ant::Error::Unavailable);
        };
        if answer.operation != operation {
            self.ledger.note(Kind::OtherRequest, 1, || {
                format!(
                    "VANA asked {operation:?}, the recording {:?}",
                    answer.operation
                )
            });
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
            self.ledger.unrecorded("ANT send status");
            None
        })
    }
    fn availability(&self) -> Availability {
        self.radio().0.0
    }
    fn channels(&self, _: u64) -> [Option<ant::Snapshot>; CHANNEL_CAPACITY] {
        core::array::from_fn(|channel| self.channels[channel].read().flatten())
    }
    fn take_packet(&mut self) -> Option<ant::Packet> {
        self.packets.pop_front()
    }
}

const MATCH_MS: u64 = 1_000;
const EVIDENCE_MS: u64 = 3_000;

type Adapter = dyn FnMut(&AntOperation, u64) -> Option<Result<Admission, ant::Error>>;

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
    heard: Vec<Vec<u64>>,
    pages: Vec<(Identity, [u8; 8])>,
    recorded: VecDeque<Recorded>,
    adapter: Option<Box<Adapter>>,
    ledger: Ledger,
}

impl ModelAnt {
    fn hearing(events: Events<'_>, ledger: &Ledger) -> Self {
        let mut radio = SimulatedAnt::new(Availability::Unsupported);
        let mut heard: Vec<Vec<u64>> = Vec::new();
        let mut evidence = |radio: &mut SimulatedAnt, identity: Identity, at: u64, rssi| {
            let index = match radio.sensors.iter().position(|s| s.identity == identity) {
                Some(index) => index,
                None => {
                    let mut sensor = AntSensor::new(identity.device_type, identity.device_number);
                    sensor.identity = identity;
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
            ledger: ledger.clone(),
        }
    }

    fn apply(&mut self, event: Event<'_>) {
        match event {
            Event::AntRadio(radio) => self.radio.availability = radio.availability,
            Event::AntPacket(packet) => self.pages.push((packet.identity, packet.data)),
            Event::AntRequest { operation, result } => self.recorded.push_back(Recorded {
                at_ms: self.ledger.now(),
                operation,
                result,
            }),
            _ => {}
        }
    }

    fn step(&mut self) {
        let now = self.ledger.now();
        self.expire(now);
        for (sensor, times) in self.radio.sensors.iter_mut().zip(&self.heard) {
            let first = times.partition_point(|&at| at + EVIDENCE_MS < now);
            sensor.present = times.get(first).is_some_and(|&at| at <= now + EVIDENCE_MS);
        }
        let pages = core::mem::take(&mut self.pages);
        for identity in self.radio.tick_hearing(now, pages) {
            self.ledger.note(Kind::NotHeard, 1, || {
                format!("a page from {identity:?} with no channel open to it")
            });
        }
    }

    fn expire(&mut self, now: u64) {
        while let Some(recorded) = self.recorded.front() {
            if recorded.at_ms.saturating_add(MATCH_MS) >= now {
                break;
            }
            let operation = recorded.operation;
            self.recorded.pop_front();
            self.ledger.note(Kind::NotRequested, 1, || {
                format!("the recording asked {operation:?}")
            });
        }
    }

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
            self.ledger.note(Kind::Modelled, 1, || {
                format!("{operation:?} answered by the model")
            });
            return self.radio.request(operation, now);
        };
        recorded.result?;
        let answer = self.radio.request(operation, now);
        if let Err(error) = answer {
            self.ledger.note(Kind::Refused, 1, || {
                format!("{operation:?}: the model answered {error:?}")
            });
        }
        answer
    }
}

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

pub struct ReplayBle {
    link: Served<(Availability, ble_transport::Snapshot)>,
    packets: VecDeque<ble_transport::Packet>,
    battery: VecDeque<ble_transport::BatteryLevel>,
    ledger: Ledger,
}

impl ReplayBle {
    fn new(ledger: &Ledger) -> Self {
        Self {
            link: Served::new("BLE link", ledger),
            packets: VecDeque::new(),
            battery: VecDeque::new(),
            ledger: ledger.clone(),
        }
    }
}

impl Ble for ReplayBle {
    fn availability(&self) -> Availability {
        self.link
            .read()
            .map_or(Availability::Unsupported, |link| link.0)
    }
    fn snapshot(&self) -> ble_transport::Snapshot {
        self.link.read().map(|link| link.1).unwrap_or_default()
    }
    fn take_packet(&mut self) -> Option<ble_transport::Packet> {
        self.packets.pop_front()
    }
    fn take_battery(&mut self) -> Option<ble_transport::BatteryLevel> {
        self.battery.pop_front()
    }
    fn reconnect(&mut self) -> Result<(), Error> {
        self.ledger.unrecorded("BLE reconnect");
        Err(Error::Unsupported)
    }
}

pub struct ReplayPosition(Served<(Availability, Option<positioning::Snapshot>)>);

impl Positioning for ReplayPosition {
    fn availability(&self) -> Availability {
        self.0
            .read()
            .map_or(Availability::Unsupported, |position| position.0)
    }
    fn snapshot(&self, _: u64) -> Option<positioning::Snapshot> {
        self.0.read().and_then(|position| position.1)
    }
}

pub struct ReplayCompanion(Served<Option<InputSnapshot>>);

impl InputObservation for ReplayCompanion {
    fn snapshot(&self, _: u64) -> Option<InputSnapshot> {
        self.0.read().flatten()
    }
}

#[derive(Clone)]
struct Entropy {
    values: Rc<RefCell<VecDeque<u64>>>,
    ledger: Ledger,
}

impl Entropy {
    fn take(&self) -> u64 {
        let value = self.values.borrow_mut().pop_front();
        value.unwrap_or_else(|| {
            self.ledger.unrecorded("entropy");
            0
        })
    }
}

/// The replaying device's medium: its sectors as they stood, failing only
/// where the trace says, with the recorded entropy.
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
        self.entropy.take()
    }
}

/// The access point as recorded: what it reported, and its answers to
/// VANA's start and stop.
pub struct ReplayAccessPoint {
    view: Served<(AccessPointState, Reach)>,
    calls: Calls<(bool, Result<(), Error>), (AccessPointState, Reach)>,
    entropy: Entropy,
}

impl ReplayAccessPoint {
    fn answer(&mut self, start: bool) -> Result<(), Error> {
        let asked = if start { "start" } else { "stop" };
        let Some(((_, result), after)) = self.calls.take(asked, |call| call.0 == start) else {
            return Err(Error::Unavailable);
        };
        for view in after {
            self.view.set(view);
        }
        result
    }
}

impl AccessPoint for ReplayAccessPoint {
    fn start(&mut self, _: AccessPointConfig) -> Result<(), Error> {
        self.answer(true)
    }
    fn stop(&mut self) -> Result<(), Error> {
        self.answer(false)
    }
    fn state(&self) -> AccessPointState {
        self.view
            .read()
            .map_or(AccessPointState::Off, |view| view.0)
    }
    fn reach(&self) -> Reach {
        self.view.read().map(|view| view.1).unwrap_or_default()
    }
    fn entropy(&mut self) -> u32 {
        self.entropy.take() as u32
    }
}

#[derive(Debug, PartialEq, Eq)]
enum ServerCall {
    Listen(bool),
    Finish(u32),
    Read { id: u32, bytes: Vec<u8>, open: bool },
    Write { id: u32, taken: u32 },
}

type ServerView = (Option<server::Request>, usize);

/// The phone server as recorded: the request it held, the body bytes each
/// read gave and what each write took.
pub struct ReplayServer {
    view: Served<ServerView>,
    calls: Calls<ServerCall, ServerView>,
}

impl ReplayServer {
    fn changed(&mut self, after: Vec<ServerView>) {
        for view in after {
            self.view.set(view);
        }
    }
}

impl Server for ReplayServer {
    fn listen(&mut self, on: bool) {
        if let Some((_, after)) = self.calls.take_if(|call| *call == ServerCall::Listen(on)) {
            self.changed(after);
        }
    }
    fn request(&self) -> Option<server::Request> {
        self.view.read().and_then(|view| view.0)
    }
    fn read(&mut self, id: u32, out: &mut [u8]) -> usize {
        let read =
            |call: &ServerCall| matches!(call, ServerCall::Read { id: read, .. } if *read == id);
        let Some((call, after)) = self.calls.take(("read", id), read) else {
            return 0;
        };
        self.changed(after);
        let ServerCall::Read { bytes, .. } = call else {
            return 0;
        };
        let n = bytes.len().min(out.len());
        out[..n].copy_from_slice(&bytes[..n]);
        if n < bytes.len() {
            self.calls.ledger.note(Kind::OtherRequest, 1, || {
                format!(
                    "a read of {} bytes into {} for request {id}",
                    bytes.len(),
                    out.len()
                )
            });
        }
        n
    }
    fn room(&self) -> usize {
        self.view.read().map_or(0, |view| view.1)
    }
    fn write(&mut self, id: u32, bytes: &[u8]) -> usize {
        let write =
            |call: &ServerCall| matches!(call, ServerCall::Write { id: write, .. } if *write == id);
        let Some((call, after)) = self.calls.take(("write", id, bytes.len()), write) else {
            return 0;
        };
        self.changed(after);
        match call {
            ServerCall::Write { taken, .. } => (taken as usize).min(bytes.len()),
            _ => 0,
        }
    }
    fn finish(&mut self, id: u32) {
        if let Some((_, after)) = self.calls.take_if(|call| *call == ServerCall::Finish(id)) {
            self.changed(after);
        }
    }
}

/// The buzzer as recorded: what it reported, and its answers to VANA's
/// play and stop. Its patterns are the build's own.
pub struct ReplaySound {
    view: Served<sound::Snapshot>,
    calls: Calls<(Option<u8>, Result<(), Error>), sound::Snapshot>,
}

impl ReplaySound {
    fn answer(&mut self, play: Option<u8>) -> Result<(), Error> {
        let Some(((_, result), after)) = self.calls.take(play, |call| call.0 == play) else {
            return Err(Error::Unavailable);
        };
        for view in after {
            self.view.set(view);
        }
        result
    }
}

impl Sound for ReplaySound {
    fn patterns(&self) -> &'static [sound::Pattern] {
        crate::devices::PATTERNS
    }
    fn snapshot(&self) -> sound::Snapshot {
        self.view.read().unwrap_or(sound::Snapshot {
            state: sound::State::Failed,
            pattern: None,
            requested_ms: 0,
            submitted_ms: None,
            until_ms: 0,
        })
    }
    fn play(&mut self, id: u8, _: u64) -> Result<(), Error> {
        self.answer(Some(id))
    }
    fn stop(&mut self, _: u64) -> Result<(), Error> {
        self.answer(None)
    }
}

type PowerCall = (power::Operation, u32, Result<(), Error>);

/// Power control as recorded: its status, its answers to VANA's requests,
/// and whether the power went away this pass.
pub struct ReplayPowerControl {
    view: Served<power::Status>,
    calls: Calls<PowerCall, power::Status>,
    off: bool,
    ledger: Ledger,
}

impl ReplayPowerControl {
    /// The device lost power in this pass.
    pub fn powered_off(&self) -> bool {
        self.off
    }

    fn answer(&mut self, operation: power::Operation, delay_ms: u32) -> Result<(), Error> {
        let asked = (operation, delay_ms);
        let same = |call: &PowerCall| (call.0, call.1) == asked;
        let Some(((_, _, result), after)) = self.calls.take(asked, same) else {
            return Err(Error::Unavailable);
        };
        for view in after {
            self.view.set(view);
        }
        result
    }
}

impl PowerControl for ReplayPowerControl {
    fn capabilities(&self) -> power::Capabilities {
        self.ledger.unrecorded("power capabilities");
        power::Capabilities::UNSUPPORTED
    }
    fn status(&self) -> power::Status {
        self.view.read().unwrap_or(power::Status {
            state: power::State::Uncertain,
            ready: false,
            ..power::Status::IDLE
        })
    }
    fn request(&mut self, operation: power::Operation) -> Result<(), Error> {
        self.answer(operation, 0)
    }
    fn request_after(&mut self, operation: power::Operation, delay_ms: u32) -> Result<(), Error> {
        self.answer(operation, delay_ms)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_api::input::Button;
    use device_api::server::Method;
    use firmware_services::trace::frozen::{FrozenServer, READ_PART, Record, Recorder};
    use firmware_services::trace::{Header, VERSION};

    struct Transport {
        request: server::Request,
        body: Vec<u8>,
    }

    impl Server for Transport {
        fn listen(&mut self, _: bool) {}
        fn request(&self) -> Option<server::Request> {
            Some(self.request.clone())
        }
        fn read(&mut self, _: u32, out: &mut [u8]) -> usize {
            let n = out.len().min(self.body.len());
            out[..n].copy_from_slice(&self.body[..n]);
            self.body.drain(..n);
            n
        }
        fn room(&self) -> usize {
            0
        }
        fn write(&mut self, _: u32, _: &[u8]) -> usize {
            0
        }
        fn finish(&mut self, _: u32) {}
    }

    fn replay(bytes: Vec<u8>) -> Replay {
        let shelled = Shelled {
            input: InputDevice::new(&[Button::TopLeft], Availability::Ready),
            display: DisplayDevice::new(16, 16),
            power: PowerDevice::new(Availability::Ready),
            flash: FileFlash::memory(),
            bulk: SimBulk::default().scripted(),
        };
        Replay::new(bytes, shelled, Mode::Strict).unwrap()
    }

    #[test]
    fn a_read_recorded_in_parts_replays_whole() {
        let header = Header {
            version: VERSION,
            build: [0; 20],
            boot: 1,
            profile: 0,
            config: &[],
            media: &[],
        };
        let recorder = RefCell::new(
            Recorder::new(
                &header,
                vec![0u8; 4096].into_boxed_slice(),
                Blocks::default(),
            )
            .unwrap(),
        );
        let body: Vec<u8> = (0..3 * READ_PART + 100).map(|i| i as u8).collect();
        let mut request = server::Request::new(Method::Put);
        request.id = 9;
        let mut transport = Transport {
            request,
            body: body.clone(),
        };
        recorder.borrow_mut().record(&Event::Pass { now_ms: 10 });
        let mut frozen = FrozenServer::default();
        let mut out = vec![0u8; 4096];
        let n = frozen.bind(&mut transport, 10, &recorder).read(9, &mut out);
        assert_eq!(n, body.len());
        let mut bytes = recorder.borrow().sink().0.clone();
        bytes.extend_from_slice(recorder.borrow_mut().open_block());
        let parts = {
            let (_, events) = trace::decode(&bytes).unwrap();
            events
                .filter(|event| matches!(event, Event::ServerRead { .. }))
                .count()
        };
        assert_eq!(parts, 4);

        let mut replay = replay(bytes);
        assert!(matches!(replay.step(), Next::Pass));
        replay.load_pass();
        let mut again = vec![0u8; 4096];
        assert_eq!(replay.sources.server.read(9, &mut again), body.len());
        assert_eq!(&again[..body.len()], &body[..]);
        assert_eq!(replay.sources.report().first, None);
    }
}
