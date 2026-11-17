//! A recorded trace standing in for the world, and how a replay parted
//! from it.
//!
//! A strict replay runs the build that recorded the trace and must answer
//! every call from it: `crate::twin` serves it. A stimulus replay, here,
//! runs another build against the same world: the trace's world records
//! arrive at their recorded pass whatever VANA asks, the session runs one
//! pass at each recorded uptime, and the ANT radio is a model that hears
//! the recorded sensors. Either way the replay keeps a [`Report`] of where
//! it parted from the recording.
use crate::ant::{AntSensor, SimulatedAnt, ZeroReply};
use crate::bulk::SimBulk;
use crate::devices::{DisplayDevice, InputDevice, PowerDevice};
use crate::storage::FileFlash;
use crate::storage::StorageFault;
use crate::trace::world::Output;
use crate::trace::world::{self, World};
use device_api::ant::{
    self, Admission, Ant, AntOperation, CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Identity, LinkState,
};
use device_api::ble_transport::{self, Ble};
use device_api::bulk::{self, ReadWrite};
use device_api::input::{InputObservation, InputSnapshot};
use device_api::journal::{Aging, Latest};
use device_api::observation::{Availability, Error, Observation};
use device_api::positioning::{self, Positioning};
use device_api::power::{self, PowerControl};
use device_api::sensors;
use device_api::server::{self, AccessPoint, AccessPointConfig, AccessPointState, Reach, Server};
use device_api::sound::{self, Sound};
use firmware_services::trace::recorder::Sink;
use firmware_services::trace::{self, Cursor, End, Event, Events, Medium, StorageOp};
use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::fmt;
use std::rc::Rc;

/// Sealed trace blocks, back to back.
#[derive(Default)]
pub struct Blocks(pub Vec<u8>);

impl Sink for Blocks {
    fn block(&mut self, bytes: &[u8]) -> bool {
        self.0.extend_from_slice(bytes);
        true
    }
}

/// The media as a session found it before its first pass.
pub struct Origin {
    pub(crate) flash: FileFlash,
    pub bulk: SimBulk,
    pub(crate) boot: u32,
}

impl Origin {
    pub fn untapped(&self) -> Self {
        Self {
            flash: self.flash.untapped(),
            bulk: self.bulk.untapped(),
            boot: self.boot,
        }
    }
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
    /// The build that recorded it: every call VANA makes is answered from
    /// the trace, and one it holds no answer for ends the replay.
    Strict,
    /// Another build: the recording is the world. Queued inputs arrive at
    /// their recorded pass and wait until VANA takes them; latest values
    /// hold from their pass; the ANT radio is a model hearing the recorded
    /// sensors, and answers a request as the recording did when it made the
    /// same one. Differences are counted, not failures.
    Stimulus,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Strict => "strict",
            Self::Stimulus => "stimulus",
        }
    }
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
    /// The panel showed the phone page in other pixels, all else the same.
    /// The page draws the session's credentials, which a redacted trace
    /// replaced, so this is expected and never the first divergence.
    Redacted,
    /// What the replay handed VANA, as it recorded it again, differs from
    /// the trace it replayed.
    Rerecorded,
    /// An event the simulator cannot replay.
    Unreplayable,
    /// The trace is damaged here: the rest of it does not read.
    Truncated,
    /// The recording lost events here: its sink had no room for them.
    Lost,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unrecorded => "Unrecorded",
            Self::Modelled => "Modelled",
            Self::LeftOver => "LeftOver",
            Self::OtherRequest => "OtherRequest",
            Self::NotRequested => "NotRequested",
            Self::Refused => "Refused",
            Self::NotHeard => "NotHeard",
            Self::Carried => "Carried",
            Self::Seen => "Seen",
            Self::Redacted => "Redacted",
            Self::Rerecorded => "Rerecorded",
            Self::Unreplayable => "Unreplayable",
            Self::Truncated => "Truncated",
            Self::Lost => "Lost",
        }
    }
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
    /// What a strict replay proved, once it ended.
    pub faithful: Option<Faithful>,
    pub(crate) now: u64,
}

/// Counts of matching media and settings records from the replayed prefix. Media checks compare
/// inline bytes or CRC32 digests; settings checks compare decoded records. `ended` identifies the
/// first mismatch or early stop, and `exclusions` lists gaps in coverage.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Faithful {
    /// Media records that matched.
    pub physical: u64,
    /// Settings records that matched.
    pub semantic: u64,
    /// The pass the replay ended in before the trace did, and why: the
    /// guarantees hold up to it.
    pub ended: Option<(u64, String)>,
    pub exclusions: Vec<Exclusion>,
}

/// What a guarantee does not cover.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Exclusion {
    /// Frames of the phone page, whose credentials the trace replaced.
    RedactedFrames(u32),
    /// Frames the panel refused: replayed as refused, so no panel digest
    /// stands for them.
    RefusedFrames(u32),
    /// The settings journal did not load whole. The replay loads the
    /// decoded record the device made of it, not its sectors.
    SettingsJournal(String),
    /// TERRAIN reads the trace holds a digest of: served from the pack.
    TerrainByDigest(u32),
    /// Accesses the trace does not record: STORE, which holds the trace
    /// itself, and the settings region.
    Outside(u32),
    /// The recording lost world records here: nothing after is replayed.
    Lost { pass_ms: u64 },
}

impl fmt::Display for Faithful {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let until = match &self.ended {
            Some((pass_ms, _)) => format!(" up to the pass at {pass_ms} ms"),
            None => String::new(),
        };
        writeln!(
            f,
            "physical: {} media records gave the bytes the trace holds{until}",
            self.physical
        )?;
        writeln!(
            f,
            "semantic: {} settings records gave the values the trace holds{until}",
            self.semantic
        )?;
        if let Some((pass_ms, why)) = &self.ended {
            writeln!(f, "ended at {pass_ms} ms: {why}")?;
        }
        if self.exclusions.is_empty() {
            return writeln!(f, "excluded: nothing");
        }
        for exclusion in &self.exclusions {
            match exclusion {
                Exclusion::RedactedFrames(frames) => writeln!(
                    f,
                    "excluded: {frames} frames of the phone page, whose credentials the trace redacted"
                )?,
                Exclusion::RefusedFrames(frames) => writeln!(
                    f,
                    "excluded: {frames} frames the panel refused, replayed as refused"
                )?,
                Exclusion::SettingsJournal(how) => writeln!(
                    f,
                    "excluded: the settings journal ({how}), replayed as the record the device decoded"
                )?,
                Exclusion::TerrainByDigest(reads) => writeln!(
                    f,
                    "excluded: {reads} TERRAIN reads, served from the pack and checked by digest"
                )?,
                Exclusion::Outside(accesses) => writeln!(
                    f,
                    "excluded: {accesses} accesses outside the recorded world (the settings region)"
                )?,
                Exclusion::Lost { pass_ms } => writeln!(
                    f,
                    "excluded: everything from {pass_ms} ms, where the recording lost world records"
                )?,
            }
        }
        Ok(())
    }
}

impl Report {
    pub(crate) fn new(mode: Mode) -> Self {
        Self {
            mode,
            first: None,
            counts: BTreeMap::new(),
            faithful: None,
            now: 0,
        }
    }

    /// What answered VANA's reads.
    pub fn header(&self) -> &'static str {
        match self.mode {
            Mode::Strict => {
                "strict replay: every input VANA and the shell read came from the trace's world records"
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

    pub(crate) fn note(&mut self, kind: Kind, count: usize, detail: impl FnOnce() -> String) {
        if count == 0 {
            return;
        }
        *self.counts.entry(kind).or_default() += count as u32;
        if kind == Kind::Redacted {
            return;
        }
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
            None => writeln!(f, "no divergence")?,
            Some(first) => writeln!(
                f,
                "first at {} ms, {:?}: {}",
                first.at_ms, first.kind, first.detail
            )?,
        }
        for (kind, count) in &self.counts {
            writeln!(f, "{kind:?} {count}")?;
        }
        match &self.faithful {
            Some(faithful) => write!(f, "{faithful}"),
            None => Ok(()),
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

    fn take(&mut self, matches: impl Fn(&C) -> bool) -> Option<(C, Vec<V>)> {
        let found = self.take_first(matches);
        if found.is_none() {
            self.ledger.unrecorded(self.input);
        }
        found
    }

    fn take_first(&mut self, matches: impl Fn(&C) -> bool) -> Option<(C, Vec<V>)> {
        let index = self.queue.iter().position(|(call, _)| matches(call))?;
        self.queue.remove(index)
    }

    fn drained(&mut self) {
        let left = self.queue.len();
        let (input, queue) = (self.input, &self.queue);
        self.ledger.note(Kind::NotRequested, left, || {
            let calls: Vec<_> = queue.iter().map(|(call, _)| call).collect();
            format!("{left} {input} answers VANA did not ask for: {calls:?}")
        });
        self.queue.clear();
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
}

/// What the rider saw change in a pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Seen {
    Page(String),
    Toast(String),
    RideState(String),
    Shown(String),
    Mark {
        count: u16,
        pressed_ms: u64,
        page: String,
    },
    Sound {
        pattern: u8,
        cue: String,
        note: u8,
        of: u8,
    },
    Panel(u32),
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
    pub fn new(bytes: Vec<u8>, shelled: Shelled) -> Result<Self, trace::Error> {
        let (_, events) = trace::decode(&bytes)?;
        let cursor = events.cursor();
        let ledger = Ledger(Rc::new(RefCell::new(Report::new(Mode::Stimulus))));
        let ant = ModelAnt::hearing(events, &ledger);
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
                if events.end() == Some(End::Corrupt) {
                    self.sources.ledger.note(Kind::Truncated, 1, || {
                        "the trace is damaged after this pass; the rest does not read".into()
                    });
                }
                return Next::End;
            };
            match event {
                Event::Pass { .. } => return Next::Pass,
                Event::Command(line) => {
                    let line = line.into();
                    self.cursor = events.cursor();
                    self.apply_until_pass();
                    return Next::Command(line);
                }
                event if seen(&event).is_some() => {}
                event => self.sources.apply(event),
            }
            self.cursor = events.cursor();
        }
    }

    /// The inputs recorded after a command and before its pass, such as the
    /// power status its gate read.
    fn apply_until_pass(&mut self) {
        loop {
            let mut events = Events::resume(&self.bytes, self.cursor);
            match events.next() {
                None | Some(Event::Pass { .. } | Event::Command(_)) => return,
                Some(event) if seen(&event).is_some() => {}
                Some(event) => self.sources.apply(event),
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
    /// Another build may draw the same things in other pixels, so the
    /// frames are left out.
    pub fn saw(&mut self, seen: &[Seen]) {
        let recorded = core::mem::take(&mut self.seen);
        let compared = |seen: &[Seen]| -> Vec<Seen> {
            (seen.iter().filter(|seen| !matches!(seen, Seen::Panel(_))))
                .cloned()
                .collect()
        };
        let (seen, recorded) = (compared(seen), compared(&recorded));
        if seen != recorded {
            self.sources.ledger.note(Kind::Seen, 1, || {
                format!("the rider saw {seen:?}, the recording {recorded:?}")
            });
        }
    }

    /// The trace this replay runs.
    pub fn trace(&self) -> &[u8] {
        &self.bytes
    }
}

/// The first event where two traces part, with the pass it belongs to.
/// What [`measured`] holds is left out.
pub fn first_difference(recorded: &[u8], replayed: &[u8]) -> Option<String> {
    first_difference_of(recorded, replayed, |event| !measured(event))
}

/// The first `compared` event where two traces part.
fn first_difference_of(
    recorded: &[u8],
    replayed: &[u8],
    compared: impl Fn(&Event<'_>) -> bool,
) -> Option<String> {
    let (header, mut events) = match trace::decode(recorded) {
        Ok(decoded) => decoded,
        Err(error) => return Some(format!("the recorded trace does not decode: {error}")),
    };
    let (replayed_header, mut replayed_events) = match trace::decode(replayed) {
        Ok(decoded) => decoded,
        Err(error) => return Some(format!("the replayed trace does not decode: {error}")),
    };
    let same_but_build = trace::Header {
        build: replayed_header.build,
        ..header
    };
    if same_but_build != replayed_header {
        return Some(format!("header {header:?}, replayed {replayed_header:?}"));
    }
    let (mut pass, mut read) = (0, 0);
    for (index, event) in events.by_ref().enumerate().filter(|(_, e)| compared(e)) {
        if let Event::Pass { now_ms } = event {
            pass = now_ms;
        }
        let other = replayed_events.by_ref().find(|e| compared(e));
        if other != Some(event) {
            return Some(format!(
                "event {index}, pass at {pass} ms: recorded {event:?}, replayed {other:?}"
            ));
        }
        read = index + 1;
    }
    let damaged = |events: &trace::Events<'_>, which: &str| {
        (events.end() == Some(End::Corrupt))
            .then(|| format!("the {which} trace is damaged after event {read}, pass at {pass} ms"))
    };
    if let Some(damaged) = damaged(&events, "recorded") {
        return Some(damaged);
    }
    if let Some(extra) = replayed_events.by_ref().find(|e| compared(e)) {
        return Some(format!("the replay went on with {extra:?}"));
    }
    damaged(&replayed_events, "replayed")
}

pub(crate) fn seen(event: &Event<'_>) -> Option<Option<Seen>> {
    match event {
        Event::Page(page) => Some(Some(Seen::Page((*page).into()))),
        Event::Toast(text) => Some(Some(Seen::Toast((*text).into()))),
        Event::RideState(state) => Some(Some(Seen::RideState((*state).into()))),
        Event::Shown(fields) => Some(Some(Seen::Shown((*fields).into()))),
        Event::Mark {
            count,
            pressed_ms,
            page,
        } => Some(Some(Seen::Mark {
            count: *count,
            pressed_ms: *pressed_ms,
            page: (*page).into(),
        })),
        Event::Sound {
            pattern,
            cue,
            note,
            of,
        } => Some(Some(Seen::Sound {
            pattern: *pattern,
            cue: (*cue).into(),
            note: *note,
            of: *of,
        })),
        Event::Panel(digest) => Some(Some(Seen::Panel(*digest))),
        event if measured(event) => Some(None),
        _ => None,
    }
}

/// Logged warnings and errors, health and where the CPU time went: no
/// input, and a device logs and measures its drivers, which a replay never
/// runs, so none of it is compared. Nor is a harness's synthetic input.
pub(crate) fn measured(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Log { .. }
            | Event::FrameTime(_)
            | Event::Health(_)
            | Event::Activity(_)
            | Event::Task { .. }
            | Event::Stage { .. }
            | Event::Synthetic(_)
    )
}

/// The devices as the trace reports them, one pass at a time.
pub struct Sources {
    pub ant: ModelAnt,
    pub ble: ReplayBle,
    pub position: ReplayPosition,
    pub companion: ReplayCompanion,
    pub bulk: ReplayBulk,
    pub access_point: ReplayAccessPoint,
    pub server: ReplayServer,
    pub sound: ReplaySound,
    pub power: ReplayPowerControl,
    startup: Served<sensors::Startup>,
    physical: Served<sensors::Snapshot>,
    light: Served<Observation<sensors::Light>>,
    network_time: Served<firmware_services::network_time::Snapshot>,
    backlight: Served<Availability>,
    /// Where the last failed access recorded ended: a failed read's bytes
    /// may follow in further records, which are no further faults.
    fault_end: Option<(Medium, u32)>,
    shelled: Shelled,
    ledger: Ledger,
}

impl Sources {
    fn new(ant: ModelAnt, shelled: Shelled, ledger: Ledger) -> Self {
        let entropy = |input| Entropy {
            values: VecDeque::new(),
            input,
            ledger: ledger.clone(),
        };
        Self {
            ant,
            ble: ReplayBle::new(&ledger),
            position: ReplayPosition {
                availability: Served::new("position", &ledger),
                snapshot: Served::new("position", &ledger),
            },
            companion: ReplayCompanion(Served::new("companion report", &ledger)),
            bulk: ReplayBulk {
                media: shelled.bulk.untapped(),
                entropy: entropy("entropy"),
            },
            access_point: ReplayAccessPoint {
                view: Served::new("access point", &ledger),
                latest: (AccessPointState::Off, Reach::default()),
                calls: Calls::new("access point", &ledger),
                entropy: entropy("access point entropy"),
            },
            server: ReplayServer {
                view: Served::new("phone server", &ledger),
                latest: (None, 0),
                calls: Calls::new("phone server", &ledger),
                token: None,
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
            startup: Served::new("companion startup", &ledger),
            physical: Served::new("barometer and motion", &ledger),
            light: Served::new("ambient light", &ledger),
            network_time: Served::new("network time", &ledger),
            backlight: Served::new("backlight", &ledger),
            fault_end: None,
            shelled,
            ledger,
        }
    }

    /// A changed build's ANT adapter, answering a request itself before
    /// the model sees it when it returns an answer.
    pub fn adapt_ant(
        &mut self,
        adapter: impl FnMut(&AntOperation, u64) -> Option<Result<Admission, ant::Error>> + 'static,
    ) {
        self.ant.adapter = Some(Box::new(adapter));
    }

    /// Something read an input the trace holds no value for.
    pub fn unrecorded(&self, input: &str) {
        self.ledger.unrecorded(input);
    }

    /// Where the replay parted from its recording so far.
    pub fn report(&self) -> Report {
        self.ledger.0.borrow().clone()
    }

    /// Open a window: the next divergence becomes the report's first.
    /// Returns the counts before the window.
    pub fn open_window(&mut self) -> BTreeMap<Kind, u32> {
        let mut report = self.ledger.0.borrow_mut();
        report.first = None;
        report.counts.clone()
    }

    fn begin(&mut self, now: u64) {
        self.ledger.0.borrow_mut().now = now;
        self.power.off = false;
        self.fault_end = None;
    }

    fn ready(&mut self) {
        if let Some(available) = self.backlight.latest.at(self.ledger.now()) {
            self.shelled.power.0.borrow_mut().available = available;
        }
        self.ant.step();
    }

    /// The pass ended: what it queued and VANA did not take waits, and what
    /// it scripted and nothing met is counted.
    fn drained(&mut self) {
        let queued = [
            ("BLE packets", self.ble.packets.len()),
            ("BLE battery levels", self.ble.battery.len()),
            ("input edges", self.shelled.input.replayed.borrow().len()),
        ];
        for (what, count) in queued {
            self.ledger
                .note(Kind::Carried, count, || format!("{count} {what}"));
        }
        let panel = core::mem::take(&mut self.shelled.display.0.borrow_mut().refusals.scripted);
        let backlight = core::mem::take(&mut self.shelled.power.0.borrow_mut().refusals.scripted);
        for (what, left) in [("panel", panel.len()), ("backlight", backlight.len())] {
            self.ledger.note(Kind::NotRequested, left, || {
                format!("{left} {what} refusals no attempt met")
            });
        }
        let flash = core::mem::take(&mut self.shelled.flash.fault.borrow_mut().script);
        let bulk = self.shelled.bulk.unmet();
        for fault in flash.iter().chain(&bulk) {
            self.ledger.note(Kind::NotRequested, 1, || {
                format!("a storage fault no operation met: {fault:?}")
            });
        }
        self.access_point.calls.drained();
        self.server.calls.drained();
        self.sound.calls.drained();
        self.power.calls.drained();
    }

    fn ended(&mut self) {
        self.ant.expire(u64::MAX);
    }

    fn apply(&mut self, event: Event<'_>) {
        match event {
            Event::Call(call) => match world::read(&call) {
                Some(world) => self.world(world),
                None => self.ledger.note(Kind::Unreplayable, 1, || {
                    format!(
                        "a record of {} that does not decode",
                        crate::twin::script::Named(call.method)
                    )
                }),
            },
            Event::NetworkTime(snapshot) => self.network_time.set(snapshot),
            Event::PoweredOff => self.power.off = true,
            Event::TooLarge {
                capability,
                operation,
            } => self.ledger.unrecorded(&format!(
                "a record of {capability}.{operation}, too large for a block"
            )),
            Event::Forget(medium) => self.ledger.note(Kind::Unreplayable, 1, || {
                format!("a host wrote {medium:?} here; the media after it are not the origin's")
            }),
            Event::Lost { blocks, bytes } => self.ledger.note(Kind::Lost, 1, || {
                format!("the recording lost {blocks} blocks, {bytes} bytes, of events here")
            }),
            Event::Cost(_)
            | Event::Repeated { .. }
            | Event::Session { .. }
            | Event::Horizon { .. }
            | Event::Salvaged { .. } => {}
            Event::Pass { .. }
            | Event::Command(_)
            | Event::Page(_)
            | Event::Toast(_)
            | Event::Sound { .. }
            | Event::RideState(_)
            | Event::Shown(_)
            | Event::Mark { .. }
            | Event::Panel(_)
            | Event::Log { .. }
            | Event::FrameTime(_)
            | Event::Health(_)
            | Event::Activity(_)
            | Event::Task { .. }
            | Event::Stage { .. }
            | Event::Synthetic(_) => self.ledger.note(Kind::Unreplayable, 1, || {
                format!("{event:?} where an input belongs")
            }),
        }
    }

    fn world(&mut self, world: World<'_>) {
        match world {
            World::Edge(edge) => self.shelled.input.replayed.borrow_mut().push_back(edge),
            World::AntAvailability(_)
            | World::AntPacket(_)
            | World::AntRequest { .. }
            | World::AntScanning(_)
            | World::AntDiscovery { .. }
            | World::AntLink { .. }
            | World::AntSend(_) => self.ant.apply(world),
            World::BleAvailability(availability) => self.ble.availability.set(availability),
            World::BleLink(snapshot) => self.ble.link.set(snapshot),
            World::BlePacket(packet) => self.ble.packets.push_back(packet),
            World::BleBattery(level) => self.ble.battery.push_back(level),
            World::PositionAvailability(availability) => {
                self.position.availability.set(availability)
            }
            World::Position(snapshot) => self.position.snapshot.set(snapshot),
            World::Physical(snapshot) => self.physical.set(snapshot),
            World::Light(light) => self.light.set(light),
            World::Companion(snapshot) => self.companion.0.set(snapshot),
            World::Startup(startup) => self.startup.set(startup),
            World::Entropy(value) => self.bulk.entropy.values.push_back(value),
            World::AccessPointEntropy(value) => self.access_point.entropy.values.push_back(value),
            World::Media(op, access) => {
                let Some(error) = access.fault else {
                    return;
                };
                let span = access.span;
                let continues = self.fault_end == Some((span.medium, span.start));
                self.fault_end = Some((span.medium, span.start.saturating_add(span.count)));
                if !continues {
                    self.fault(StorageFault {
                        medium: span.medium,
                        attempt: 0,
                        op,
                        at: span.start.into(),
                        landed: 0,
                        error,
                    });
                }
            }
            World::NoCard(error) => self.fault(StorageFault {
                medium: Medium::Bulk,
                attempt: 0,
                op: StorageOp::Info,
                at: 0,
                landed: 0,
                error,
            }),
            World::Refused { output, attempt } => {
                let refusals = match output {
                    Output::Panel => &mut self.shelled.display.0.borrow_mut().refusals.scripted,
                    Output::Backlight => &mut self.shelled.power.0.borrow_mut().refusals.scripted,
                };
                refusals.push(attempt);
            }
            World::Backlight(availability) => self.backlight.set(availability),
            World::AccessPointState(state) => {
                self.access_point.latest.0 = state;
                self.access_point.changed();
            }
            World::AccessPointReach(reach) => {
                self.access_point.latest.1 = reach;
                self.access_point.changed();
            }
            World::AccessPointRequest { start, result } => {
                self.access_point.calls.push((start, result))
            }
            World::ServerRequest(request) => {
                self.server.latest.0 = request.map(|request| request.to_request());
                self.server.view_changed();
            }
            World::ServerRoom(room) => {
                self.server.latest.1 = room;
                self.server.view_changed();
            }
            World::ServerListen(on) => self.server.calls.push(ServerCall::Listen(on)),
            World::ServerFinish(id) => self.server.calls.push(ServerCall::Finish(id)),
            World::ServerRead { id, part, more } => match self.server.calls.last() {
                Some(ServerCall::Read {
                    id: read,
                    bytes,
                    more: pending @ true,
                }) if *read == id => {
                    bytes.extend_from_slice(part);
                    *pending = more;
                }
                _ => self.server.calls.push(ServerCall::Read {
                    id,
                    bytes: part.to_vec(),
                    more,
                }),
            },
            World::ServerWrite { id, taken } => {
                self.server.calls.push(ServerCall::Write { id, taken })
            }
            World::SoundState(snapshot) => {
                if let Some(view) = self.sound.calls.change(snapshot) {
                    self.sound.view.set(view);
                }
            }
            World::SoundRequest { play, result } => self.sound.calls.push((play, result)),
            World::PowerControl(status) => {
                if let Some(view) = self.power.calls.change(status) {
                    self.power.view.set(view);
                }
            }
            World::PowerRequest {
                operation,
                delay_ms,
                result,
            } => self.power.calls.push((operation, delay_ms, result)),
            World::Other => {}
        }
    }

    fn fault(&mut self, fault: StorageFault) {
        match fault.medium {
            Medium::Bulk => self.shelled.bulk.script(fault),
            _ => (self.shelled.flash.fault.borrow_mut().script).push_back(fault),
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

    pub fn light(&self) -> Observation<sensors::Light> {
        self.light
            .latest
            .at(self.light.ledger.now())
            .unwrap_or(Observation::Unavailable)
    }

    pub fn network_time(&self) -> firmware_services::network_time::Snapshot {
        self.network_time.read().unwrap_or_default()
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
                Event::Call(call) => match world::read(&call) {
                    Some(World::AntDiscovery {
                        found: Some(discovery),
                        ..
                    }) => evidence(
                        &mut radio,
                        discovery.identity,
                        discovery.seen_ms,
                        Some(discovery.rssi),
                    ),
                    Some(World::AntPacket(packet)) => {
                        evidence(&mut radio, packet.identity, packet.received_ms, None)
                    }
                    Some(World::AntLink {
                        snapshot: Some(snapshot),
                        ..
                    }) if snapshot.link == LinkState::Connected && !snapshot.stale => {
                        if let Some(identity) = snapshot.selected {
                            evidence(&mut radio, identity, now, None);
                        }
                    }
                    _ => {}
                },
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

    fn apply(&mut self, world: World<'_>) {
        match world {
            World::AntAvailability(availability) => self.radio.availability = availability,
            World::AntPacket(packet) => self.pages.push((packet.identity, packet.data)),
            World::AntRequest { operation, result } => self.recorded.push_back(Recorded {
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

    fn answer(&mut self, operation: AntOperation, now: u64) -> Result<Admission, ant::Error> {
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

impl Ant for ModelAnt {
    fn take_diagnostic_packet(&mut self) -> Option<ant::Packet> {
        None
    }
    fn channel(&self, kind: u8, now_ms: u64) -> Option<ant::Snapshot> {
        ant::channel(self.channels(now_ms), kind)
    }
    fn scanning(&self) -> bool {
        self.radio.scanning()
    }
    fn discoveries(&self) -> [Option<ant::Discovery>; DISCOVERY_CAPACITY] {
        self.radio.discoveries()
    }
    fn request(&mut self, operation: AntOperation, now: u64) -> Result<Admission, ant::Error> {
        self.answer(operation, now)
    }
    fn capabilities(&self) -> ant::Capabilities {
        self.radio.capabilities()
    }
    fn scan(&self) -> ant::ScanSnapshot {
        self.radio.scan()
    }
    fn send_status(&self, id: ant::OperationId) -> Option<ant::SendObservation> {
        self.radio.send_status(id)
    }
    fn availability(&self) -> Availability {
        self.radio.availability()
    }
    fn channels(&self, now: u64) -> [Option<ant::Snapshot>; CHANNEL_CAPACITY] {
        self.radio.channels(now)
    }
    fn take_packet(&mut self) -> Option<ant::Packet> {
        self.radio.take_packet()
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
    availability: Served<Availability>,
    link: Served<ble_transport::Snapshot>,
    packets: VecDeque<ble_transport::Packet>,
    battery: VecDeque<ble_transport::BatteryLevel>,
    ledger: Ledger,
}

impl ReplayBle {
    fn new(ledger: &Ledger) -> Self {
        Self {
            availability: Served::new("BLE link", ledger),
            link: Served::new("BLE link", ledger),
            packets: VecDeque::new(),
            battery: VecDeque::new(),
            ledger: ledger.clone(),
        }
    }
}

impl Ble for ReplayBle {
    fn availability(&self) -> Availability {
        self.availability
            .read()
            .unwrap_or(Availability::Unsupported)
    }
    fn snapshot(&self) -> ble_transport::Snapshot {
        self.link.read().unwrap_or_default()
    }
    fn take_packet(&mut self) -> Option<ble_transport::Packet> {
        self.packets.pop_front()
    }
    fn take_battery(&mut self) -> Option<ble_transport::BatteryLevel> {
        self.battery.pop_front()
    }
    fn request(&mut self, _: ble_transport::Operation) -> Result<(), Error> {
        self.ledger.unrecorded("BLE request");
        Err(Error::Unsupported)
    }
    fn control(&self) -> device_api::connectivity::ControlStatus {
        self.ledger.unrecorded("BLE control status");
        device_api::connectivity::ControlStatus::new()
    }
    fn discoveries(&self) -> [Option<ble_transport::Discovery>; 8] {
        self.ledger.unrecorded("BLE discoveries");
        [None; 8]
    }
}

pub struct ReplayPosition {
    availability: Served<Availability>,
    snapshot: Served<Option<positioning::Snapshot>>,
}

impl Positioning for ReplayPosition {
    fn availability(&self) -> Availability {
        self.availability
            .read()
            .unwrap_or(Availability::Unsupported)
    }
    fn snapshot(&self, _: u64) -> Option<positioning::Snapshot> {
        self.snapshot.read().flatten()
    }
}

pub struct ReplayCompanion(Served<Option<InputSnapshot>>);

impl InputObservation for ReplayCompanion {
    fn snapshot(&self, _: u64) -> Option<InputSnapshot> {
        self.0.read().flatten()
    }
}

/// Unused entropy stays queued across passes so a changed build can request it later.
struct Entropy {
    values: VecDeque<u64>,
    input: &'static str,
    ledger: Ledger,
}

impl Entropy {
    fn take(&mut self) -> u64 {
        self.values.pop_front().unwrap_or_else(|| {
            self.ledger.unrecorded(self.input);
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
    /// The state and reach as last recorded: the trace holds each alone.
    latest: (AccessPointState, Reach),
    calls: Calls<(bool, Result<(), Error>), (AccessPointState, Reach)>,
    entropy: Entropy,
}

impl ReplayAccessPoint {
    fn changed(&mut self) {
        if let Some(view) = self.calls.change(self.latest) {
            self.view.set(view);
        }
    }

    fn answer(&mut self, start: bool) -> Result<(), Error> {
        let Some(((_, result), after)) = self.calls.take(|call| call.0 == start) else {
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
    Read { id: u32, bytes: Vec<u8>, more: bool },
    Write { id: u32, taken: usize },
}

type ServerView = (Option<server::Request>, usize);

/// The phone server as recorded: the request it held, the body bytes each
/// read gave and what each write took.
pub struct ReplayServer {
    view: Served<ServerView>,
    /// The request and room as last recorded: the trace holds each alone.
    latest: ServerView,
    calls: Calls<ServerCall, ServerView>,
    /// The open phone session's token, which a redacted recording's
    /// placeholder for its own session's token stands for.
    pub token: Option<device_api::connectivity::Text<32>>,
}

impl ReplayServer {
    fn view_changed(&mut self) {
        if let Some(view) = self.calls.change(self.latest.clone()) {
            self.view.set(view);
        }
    }

    fn changed(&mut self, after: Vec<ServerView>) {
        for view in after {
            self.view.set(view);
        }
    }

    /// The recorded listen or finish that answers VANA's: the first such
    /// call of the pass. A listen is recorded only when it differs from the
    /// one before or changed what the server holds, so VANA's call may meet
    /// none; one that would hide the held request and meets none parts
    /// from the recording.
    fn changes(&mut self, call: ServerCall, hides: bool) {
        match self.calls.take_first(|next| *next == call) {
            Some((_, after)) => self.changed(after),
            None if hides => self.calls.ledger.note(Kind::OtherRequest, 1, || {
                format!("VANA's {call:?} hides the request the recording kept")
            }),
            None => {}
        }
    }

    fn held(&self) -> Option<u32> {
        let now = self.calls.ledger.now();
        self.view.latest.at(now)?.0.map(|request| request.id)
    }
}

impl Server for ReplayServer {
    fn listen(&mut self, on: bool) {
        let hides = !on && self.held().is_some();
        self.changes(ServerCall::Listen(on), hides);
    }
    fn request(&self) -> Option<server::Request> {
        let mut request = self.view.read().and_then(|view| view.0)?;
        if let Some(token) = &self.token
            && firmware_services::trace::redact::is_token(&request.session)
        {
            request.session.clear();
            for byte in token.bytes() {
                let _ = request.session.push(char::from(*byte));
            }
        }
        Some(request)
    }
    fn read(&mut self, id: u32, out: &mut [u8]) -> usize {
        let read =
            |call: &ServerCall| matches!(call, ServerCall::Read { id: read, .. } if *read == id);
        let Some((call, after)) = self.calls.take(read) else {
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
        let Some((call, after)) = self.calls.take(write) else {
            return 0;
        };
        self.changed(after);
        match call {
            ServerCall::Write { taken, .. } => taken.min(bytes.len()),
            _ => 0,
        }
    }
    fn finish(&mut self, id: u32) {
        let hides = self.held() == Some(id);
        self.changes(ServerCall::Finish(id), hides);
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
        let Some(((_, result), after)) = self.calls.take(|call| call.0 == play) else {
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
        let Some(((_, _, result), after)) = self.calls.take(same) else {
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
    use device_api::journal::PART;
    use device_api::server::{Method, ServerObserved, ServerRecorder};
    use firmware_services::trace::Header;
    use firmware_services::trace::log::Log;
    use firmware_services::trace::media::Memory;
    use firmware_services::trace::recorder::Recorder;

    struct Transport {
        request: server::Request,
        body: Vec<u8>,
        listening: bool,
    }

    impl Server for Transport {
        fn listen(&mut self, on: bool) {
            self.listening = on;
        }
        fn request(&self) -> Option<server::Request> {
            self.listening.then(|| self.request.clone())
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

    type Journal = RefCell<Log<Box<[u8]>, Blocks, Box<Memory>>>;

    fn journal() -> Journal {
        recorded_by([0; 20])
    }

    fn recorded_by(build: [u8; 20]) -> Journal {
        let header = Header {
            build,
            device: "c606",
            boot: 1,
            terrain: trace::origin::Found::Absent,
        };
        let mut file = vec![0u8; 4096];
        let len = trace::encode_header(&header, &mut file).unwrap();
        file.truncate(len);
        let buffer = vec![0u8; 4096].into_boxed_slice();
        let recorder = Recorder::new(buffer, Blocks(file));
        RefCell::new(Log::new(Some(recorder), Box::default()))
    }

    fn bytes(journal: &Journal) -> Vec<u8> {
        let mut journal = journal.borrow_mut();
        let recorder = journal.recorder.as_mut().unwrap();
        let mut bytes = recorder.sink().0.clone();
        bytes.extend_from_slice(recorder.open_block());
        bytes
    }

    fn transport(body: &[u8]) -> Transport {
        let mut request = server::Request::new(Method::Put);
        request.id = 9;
        Transport {
            request,
            body: body.to_vec(),
            listening: true,
        }
    }

    fn replay(bytes: Vec<u8>) -> Replay {
        let shelled = Shelled {
            input: InputDevice::new(&[Button::TopLeft], Availability::Ready),
            display: DisplayDevice::new(16, 16),
            power: PowerDevice::new(Availability::Ready),
            flash: FileFlash::memory(),
            bulk: SimBulk::default().scripted(),
        };
        Replay::new(bytes, shelled).unwrap()
    }

    #[test]
    fn traces_that_part_only_in_their_build_do_not_differ() {
        let traced = |build: [u8; 20], page: &str| {
            let journal = recorded_by(build);
            journal.borrow_mut().record(&Event::Page(page));
            bytes(&journal)
        };
        assert_eq!(
            first_difference(&traced([1; 20], "home"), &traced([2; 20], "home")),
            None
        );
        assert!(first_difference(&traced([1; 20], "home"), &traced([1; 20], "ride")).is_some());
    }

    #[test]
    fn a_warning_only_the_device_logged_parts_no_replay_from_its_recording() {
        use firmware_services::trace::Level;
        let traced = |events: &[Event<'_>]| {
            let journal = journal();
            for event in events {
                journal.borrow_mut().record(event);
            }
            bytes(&journal)
        };
        let pass = Event::Pass { now_ms: 10 };
        let warned = Event::Log {
            level: Level::Warn,
            target: "mmc",
            text: "auto recover ok=true",
        };
        let home = Event::Page("home");
        let device = traced(&[pass, warned, home]);
        assert_eq!(first_difference(&device, &traced(&[pass, home])), None);
        let other = traced(&[pass, Event::Page("ride/main")]);
        assert!(first_difference(&device, &other).is_some());
    }

    #[test]
    fn a_read_recorded_in_parts_replays_whole() {
        let journal = journal();
        let observed = RefCell::new(ServerObserved::default());
        let body: Vec<u8> = (0..3 * PART + 100).map(|i| i as u8).collect();
        journal.borrow_mut().begin_pass(10, false);
        let mut server = ServerRecorder::new(transport(&body), &journal, &observed);
        let mut out = vec![0u8; 4096];
        assert_eq!(server.read(9, &mut out), body.len());
        let bytes = bytes(&journal);
        let parts = {
            let (_, events) = trace::decode(&bytes).unwrap();
            events
                .filter(|event| match event {
                    Event::Call(call) => {
                        matches!(world::read(call), Some(World::ServerRead { .. }))
                    }
                    _ => false,
                })
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

    #[test]
    fn a_request_appears_only_after_the_listen_that_revealed_it() {
        let journal = journal();
        let observed = RefCell::new(ServerObserved::default());
        let mut transport = transport(b"");
        transport.listening = false;
        journal.borrow_mut().begin_pass(10, false);
        {
            let mut server = ServerRecorder::new(&mut transport, &journal, &observed);
            assert_eq!(server.request(), None);
            server.listen(true);
            assert_eq!(server.request().map(|request| request.id), Some(9));
        }
        let mut replay = replay(bytes(&journal));
        assert!(matches!(replay.step(), Next::Pass));
        replay.load_pass();
        let server = &mut replay.sources.server;
        assert_eq!(server.request(), None);
        server.listen(true);
        assert_eq!(server.request().map(|request| request.id), Some(9));
        assert_eq!(replay.sources.report().first, None);
    }

    #[test]
    fn events_the_recording_lost_are_reported_where_they_were_lost() {
        let journal = journal();
        journal.borrow_mut().begin_pass(10, false);
        journal.borrow_mut().record(&Event::Lost {
            blocks: 2,
            bytes: 8_000,
        });
        journal.borrow_mut().begin_pass(20, false);
        let mut replay = replay(bytes(&journal));
        assert!(matches!(replay.step(), Next::Pass));
        assert_eq!(replay.load_pass(), 10);
        let report = replay.sources.report();
        assert_eq!(report.count(Kind::Lost), 1);
        assert_eq!(report.first.map(|first| first.at_ms), Some(10));
    }

    #[test]
    fn a_finish_or_listen_that_hides_the_held_request_unrecorded_is_reported() {
        let journal = journal();
        let observed = RefCell::new(ServerObserved::default());
        journal.borrow_mut().begin_pass(10, false);
        ServerRecorder::new(transport(b""), &journal, &observed).sample();
        let hides: [fn(&mut ReplayServer); 2] =
            [|server| server.finish(9), |server| server.listen(false)];
        for hide in hides {
            let mut replay = replay(bytes(&journal));
            assert!(matches!(replay.step(), Next::Pass));
            replay.load_pass();
            let server = &mut replay.sources.server;
            server.finish(8);
            server.listen(true);
            assert_eq!(replay.sources.report().first, None);
            hide(&mut replay.sources.server);
            assert_eq!(replay.sources.report().count(Kind::OtherRequest), 1);
        }
    }
}
