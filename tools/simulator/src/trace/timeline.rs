//! A trace decoded into readable entries, each stamped with the session
//! uptime and the ride's own clock: what every `cycling trace` subcommand
//! reads.
use crate::trace::ant_page::Pages;
use device_api::ant;
use device_api::input::Input;
use device_api::observation::Observation;
use firmware_services::trace::{self, End, Event, Level};
use serde::{Serialize, Serializer};
use std::fmt;
use std::fmt::Write as _;

const GAP_MS: u64 = 1_000;
/// An ANT+ sensor broadcasts four times a second; this long without a
/// packet from it is a silence.
pub const SILENCE_MS: u64 = 3_000;

/// When something happened: uptime since power-on, and the ride clock while
/// a ride is open (it stops while the ride is paused).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Moment {
    pub session_ms: u64,
    pub ride_ms: Option<u64>,
}

impl fmt::Display for Moment {
    /// `40:12.345 ride 38:05`, or `ride --:--` outside a ride.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = self.session_ms;
        write!(
            f,
            "{:02}:{:02}.{:03} ride ",
            ms / 60_000,
            ms / 1000 % 60,
            ms % 1000
        )?;
        match self.ride_ms {
            Some(ms) => write!(f, "{:02}:{:02}", ms / 60_000, ms / 1000 % 60),
            None => f.write_str("--:--"),
        }
    }
}

/// `mm:ss.mmm`, minutes unbounded.
pub fn session_clock(ms: u64) -> String {
    format!("{:02}:{:02}.{:03}", ms / 60_000, ms / 1000 % 60, ms % 1000)
}

/// `mm:ss`, minutes unbounded.
pub fn ride_clock(ms: u64) -> String {
    format!("{:02}:{:02}", ms / 60_000, ms / 1000 % 60)
}

/// `1:05:03`, `5:03` or `3.2s`: a length of time.
pub fn duration(ms: u64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else if s >= 60 {
        format!("{}:{:02}", s / 60, s % 60)
    } else {
        format!("{}.{}s", s, ms % 1000 / 100)
    }
}

/// A time asked for on the command line: on the session clock, or on the
/// ride clock with a leading `r` (`r38:00`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    Session(u64),
    Ride(u64),
}

impl At {
    /// `40:00`, `1:02:03`, `40:12.5`, `90s`, `40m`, `2500ms`; `r` first for
    /// the ride clock.
    pub fn parse(text: &str) -> Result<Self, String> {
        let (ride, rest) = match text.strip_prefix('r') {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let ms = parse_ms(rest).ok_or_else(|| {
            format!(
                "{text:?} is no time; use 40:00, 1:02:03, 90s, 40m or 2500ms, r40:00 for ride time"
            )
        })?;
        Ok(if ride {
            Self::Ride(ms)
        } else {
            Self::Session(ms)
        })
    }
}

fn parse_ms(text: &str) -> Option<u64> {
    let seconds = |s: &str| -> Option<f64> { s.parse::<f64>().ok().filter(|v| *v >= 0.0) };
    let ms = if let Some(v) = text.strip_suffix("ms") {
        v.parse::<u64>().ok()? as f64
    } else if let Some(v) = text.strip_suffix('s') {
        seconds(v)? * 1000.0
    } else if let Some(v) = text.strip_suffix('m') {
        seconds(v)? * 60_000.0
    } else if let Some(v) = text.strip_suffix('h') {
        seconds(v)? * 3_600_000.0
    } else {
        let parts: Vec<&str> = text.split(':').collect();
        if parts.len() > 3 || parts.iter().any(|p| p.is_empty()) {
            return None;
        }
        let (last, whole) = parts.split_last()?;
        let mut total = seconds(last)?;
        for (i, part) in whole.iter().rev().enumerate() {
            total += part.parse::<u64>().ok()? as f64 * 60f64.powi(i as i32 + 1);
        }
        total * 1000.0
    };
    Some(ms.round() as u64)
}

/// What an entry is about. Bulky kinds are hidden unless asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Kind {
    Gap,
    Input,
    Command,
    Startup,
    Ant,
    AntPacket,
    AntRequest,
    Ble,
    BlePacket,
    Gps,
    Sensors,
    Battery,
    Time,
    Storage,
    Entropy,
    Page,
    Toast,
    Sound,
    Ride,
    Mark,
    Shown,
    Log,
    Phone,
    Power,
    Display,
    Buzzer,
    Trace,
    Health,
    Activity,
}

impl Kind {
    pub const ALL: [Self; 29] = [
        Self::Gap,
        Self::Input,
        Self::Command,
        Self::Startup,
        Self::Ant,
        Self::AntPacket,
        Self::AntRequest,
        Self::Ble,
        Self::BlePacket,
        Self::Gps,
        Self::Sensors,
        Self::Battery,
        Self::Time,
        Self::Storage,
        Self::Entropy,
        Self::Page,
        Self::Toast,
        Self::Sound,
        Self::Ride,
        Self::Mark,
        Self::Shown,
        Self::Log,
        Self::Phone,
        Self::Power,
        Self::Display,
        Self::Buzzer,
        Self::Trace,
        Self::Health,
        Self::Activity,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Gap => "gap",
            Self::Input => "input",
            Self::Command => "command",
            Self::Startup => "startup",
            Self::Ant => "ant",
            Self::AntPacket => "ant.packet",
            Self::AntRequest => "ant.request",
            Self::Ble => "ble",
            Self::BlePacket => "ble.packet",
            Self::Gps => "gps",
            Self::Sensors => "sensors",
            Self::Battery => "battery",
            Self::Time => "time",
            Self::Storage => "storage",
            Self::Entropy => "entropy",
            Self::Page => "page",
            Self::Toast => "toast",
            Self::Sound => "sound",
            Self::Ride => "ride",
            Self::Mark => "mark",
            Self::Shown => "shown",
            Self::Log => "log",
            Self::Phone => "phone",
            Self::Power => "power",
            Self::Display => "display",
            Self::Buzzer => "buzzer",
            Self::Trace => "trace",
            Self::Health => "health",
            Self::Activity => "activity",
        }
    }

    /// What the rider saw or heard, or VANA logged; the rest is input.
    /// Storage holds both: faults the media reported, and VANA's warnings
    /// about saving or reading, which `about_storage` sorts out of `log`.
    pub const fn seen(self) -> bool {
        matches!(
            self,
            Self::Page
                | Self::Toast
                | Self::Sound
                | Self::Ride
                | Self::Mark
                | Self::Shown
                | Self::Log
        )
    }

    /// What the device measured of itself: neither an input nor seen.
    pub const fn measured(self) -> bool {
        matches!(self, Self::Health | Self::Activity)
    }

    /// Each entry is something that happened (a press, a command, a note
    /// played), not a state reported again, so none is ever a repeat: two
    /// presses of one button are two presses.
    pub const fn edge(self) -> bool {
        matches!(self, Self::Input | Self::Command | Self::Sound | Self::Mark)
    }

    /// Many per second, or every few seconds all session long; hidden
    /// unless asked for.
    pub const fn bulky(self) -> bool {
        matches!(
            self,
            Self::AntPacket
                | Self::AntRequest
                | Self::BlePacket
                | Self::Entropy
                | Self::Health
                | Self::Activity
        )
    }

    /// Kinds by name, or a group: `seen`, `inputs`, `all`, `default`.
    pub fn parse_list(text: &str) -> Result<Vec<Self>, String> {
        let mut kinds = Vec::new();
        for name in text.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let group: Vec<Self> = match name {
                "all" => Self::ALL.to_vec(),
                "default" => Self::ALL.into_iter().filter(|k| !k.bulky()).collect(),
                "seen" => Self::ALL.into_iter().filter(|k| k.seen()).collect(),
                "inputs" => Self::ALL
                    .into_iter()
                    .filter(|k| !k.seen() && !k.measured())
                    .collect(),
                name => match Self::ALL.into_iter().find(|k| k.name() == name) {
                    Some(kind) => vec![kind],
                    None => {
                        return Err(format!(
                            "unknown kind {name:?}; kinds: {}, or seen, inputs, default, all",
                            Self::ALL.map(Self::name).join(", ")
                        ));
                    }
                },
            };
            kinds.extend(group);
        }
        kinds.sort();
        kinds.dedup();
        Ok(kinds)
    }
}

/// One decoded event.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Entry {
    #[serde(flatten)]
    pub at: Moment,
    pub kind: Kind,
    pub text: String,
    /// The same source said the same thing before: only counters or a
    /// reading moved. Hidden unless asked for.
    pub repeat: bool,
    /// The bytes behind a decoded packet.
    #[serde(skip_serializing_if = "Option::is_none", serialize_with = "hex_bytes")]
    pub bytes: Option<[u8; 8]>,
    /// For a page: what in its pass brought it up.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cause: Option<Cause>,
    /// For a ride page: which shown readings it draws, `power,hr,grade`, or
    /// `none`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shows: Option<String>,
}

fn hex_bytes<S: Serializer>(bytes: &Option<[u8; 8]>, serializer: S) -> Result<S::Ok, S::Error> {
    match bytes {
        Some(bytes) => serializer.serialize_str(&hex(bytes)),
        None => serializer.serialize_none(),
    }
}

impl Serialize for Kind {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.name())
    }
}

/// As the summary writes it: `after button BottomLeft code 1`.
impl Serialize for Cause {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(self)
    }
}

/// What brought a page up, read from the pass it changed in.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cause {
    /// An input in the same pass, as the timeline writes it.
    Input(String),
    Command(String),
    /// The ride changed state in the same pass.
    Ride(String),
    /// A toast came up or cleared in the same pass.
    Toast(String),
    /// None of those: VANA's own rule, such as the climb page following the
    /// road, or a screen that moves on when its work is done.
    Itself,
    /// The first page of the session.
    PowerOn,
}

impl Cause {
    fn of(pass: &[Entry]) -> Self {
        let first = |kind: Kind| pass.iter().find(|e| e.kind == kind).map(|e| e.text.clone());
        first(Kind::Input)
            .map(Self::Input)
            .or_else(|| first(Kind::Command).map(Self::Command))
            .or_else(|| first(Kind::Ride).map(Self::Ride))
            .or_else(|| first(Kind::Toast).map(Self::Toast))
            .unwrap_or(Self::Itself)
    }
}

impl fmt::Display for Cause {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Input(input) => write!(f, "after {input}"),
            Self::Command(command) => write!(f, "after command {command}"),
            Self::Ride(state) => write!(f, "with ride {state}"),
            Self::Toast(text) => write!(f, "with toast {text}"),
            Self::Itself => write!(f, "by itself"),
            Self::PowerOn => write!(f, "at power-on"),
        }
    }
}

impl fmt::Display for Entry {
    /// `{:#}` adds the packet's bytes after its words.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  {:<11} {}", self.at, self.kind.name(), self.text)?;
        if let Some(cause) = &self.cause {
            write!(f, "  ({cause}")?;
            if let Some(shows) = &self.shows {
                write!(f, "; shows {shows}")?;
            }
            write!(f, ")")?;
        }
        match &self.bytes {
            Some(bytes) if f.alternate() => write!(f, "  [{}]", hex(bytes)),
            _ => Ok(()),
        }
    }
}

/// A whole trace, decoded.
pub struct Timeline {
    /// The recording build's commit in hex, trailing zero bytes dropped;
    /// empty when unknown.
    pub build: String,
    pub boot: u32,
    pub entries: Vec<Entry>,
    /// The ride clock at each ride state change.
    pub clock: Vec<Anchor>,
    pub passes: u64,
    /// The last pass's uptime.
    pub end_ms: u64,
    /// How the trace's events ended: whole, cut short or damaged.
    pub end: End,
    /// UTC in ms at uptime zero, from the first GPS or Wi-Fi time.
    pub utc_at_boot_ms: Option<u64>,
    /// Stretches in which a sensor that had sent packets sent none, in
    /// the order they began.
    pub silences: Vec<Silence>,
    /// Each power control state, from the session time it was reported.
    pub power: Vec<(u64, device_api::power::State)>,
    /// Each health record, with the session time it was recorded at.
    pub health: Vec<(u64, trace::Health)>,
    /// The frame time of each health window that reported one.
    pub frame_times: Vec<(u64, trace::FrameTime)>,
    /// Each activity window, in order.
    pub activity: Vec<Window>,
}

/// Where the CPU time went in the window that ended at `end_ms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Window {
    pub end_ms: u64,
    pub activity: trace::Activity,
    pub tasks: Vec<TaskTime>,
    /// The main-loop pass's stages, parts of the `console` task's time.
    pub stages: Vec<StageTime>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TaskTime {
    pub name: String,
    pub polls: u32,
    pub busy_us: Option<u32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StageTime {
    pub name: String,
    pub runs: u32,
    pub busy_us: u32,
    pub max_us: u32,
}

/// No packet from `sensor` after `last_ms` until `next_ms`, or to the end.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Silence {
    pub sensor: String,
    pub last_ms: u64,
    pub next_ms: Option<u64>,
}

impl Timeline {
    /// The session time `at` names: a ride time is the first moment the
    /// ride clock reached it.
    pub fn session_ms(&self, at: At) -> Result<u64, String> {
        let ride_ms = match at {
            At::Session(ms) => return Ok(ms),
            At::Ride(ms) => ms,
        };
        self.clock
            .iter()
            .zip(
                self.clock
                    .iter()
                    .skip(1)
                    .map(|a| a.session_ms)
                    .chain([self.end_ms]),
            )
            .find_map(|(anchor, until)| {
                let start = anchor.ride_ms?;
                if start >= ride_ms {
                    return Some(anchor.session_ms);
                }
                let ran = until - anchor.session_ms;
                (anchor.running && start + ran >= ride_ms)
                    .then(|| anchor.session_ms + (ride_ms - start))
            })
            .ok_or_else(|| format!("no ride reached {}", ride_clock(ride_ms)))
    }

    /// The moment at a session time, with the ride clock as it read then.
    pub fn moment(&self, session_ms: u64) -> Moment {
        let at = self.clock.partition_point(|a| a.session_ms <= session_ms);
        let ride_ms = at.checked_sub(1).and_then(|i| {
            let anchor = self.clock[i];
            let ran = if anchor.running {
                session_ms - anchor.session_ms
            } else {
                0
            };
            anchor.ride_ms.map(|ms| ms + ran)
        });
        Moment {
            session_ms,
            ride_ms,
        }
    }

    /// The last thing each seen kind showed at `session_ms`: page, toast,
    /// ride state and shown fields.
    pub fn seen_at(&self, session_ms: u64) -> SeenAt {
        let mut seen = SeenAt::default();
        for entry in self
            .entries
            .iter()
            .take_while(|e| e.at.session_ms <= session_ms)
        {
            let slot = match entry.kind {
                Kind::Page => &mut seen.page,
                Kind::Toast => &mut seen.toast,
                Kind::Ride => &mut seen.ride,
                Kind::Shown => &mut seen.shown,
                _ => continue,
            };
            *slot = Some((entry.at.session_ms, entry.text.clone()));
        }
        seen.toast.take_if(|(_, text)| text == TOAST_CLEARED);
        seen
    }
}

/// The ride clock from a session time on: its reading then, none outside a
/// ride, and whether it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Anchor {
    pub session_ms: u64,
    pub ride_ms: Option<u64>,
    pub running: bool,
}

/// What the rider saw at one moment, each with when it began.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SeenAt {
    pub page: Option<(u64, String)>,
    pub toast: Option<(u64, String)>,
    pub ride: Option<(u64, String)>,
    pub shown: Option<(u64, String)>,
}

/// The toast entry's text once the band clears.
pub const TOAST_CLEARED: &str = "(cleared)";

/// The value of `name` in a shown line, `power=--` giving `--`.
pub fn field<'a>(shown: &'a str, name: &str) -> Option<&'a str> {
    shown
        .split(' ')
        .find_map(|pair| pair.strip_prefix(name)?.strip_prefix('='))
}

/// Decode `bytes`; an error for a trace whose header does not decode.
pub fn read(bytes: &[u8]) -> Result<Timeline, String> {
    let (header, mut events) = trace::decode(bytes).map_err(|error| match error {
        trace::Error::Header => "no trace header: the file is no ride trace".to_string(),
        trace::Error::Version(_) => error.to_string(),
    })?;
    let mut reader = Reader {
        last: vec![None; Kind::ALL.len() << 8],
        // A device trace spends about 23 bytes an entry, so the entries
        // rarely move as they grow.
        entries: Vec::with_capacity(bytes.len() / 16),
        ..Reader::default()
    };
    for event in events.by_ref() {
        reader.event(&event);
    }
    reader.end_pass();
    reader.silent_at_end();
    let build_len = header
        .build
        .iter()
        .rposition(|&b| b != 0)
        .map_or(0, |i| i + 1);
    Ok(Timeline {
        build: header.build[..build_len]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        boot: header.boot,
        passes: reader.passes,
        end_ms: reader.now,
        end: events.end().unwrap_or(End::Complete),
        entries: reader.entries,
        clock: reader.clock,
        utc_at_boot_ms: reader.utc_at_boot_ms,
        silences: reader.silences,
        power: reader.power,
        health: reader.health,
        frame_times: reader.frame_times,
        activity: reader.activity,
    })
}

/// A sensor that sent packets: when it was last heard, and its name.
struct Heard {
    identity: ant::Identity,
    last_ms: u64,
    name: String,
}

#[derive(Default)]
struct Reader {
    entries: Vec<Entry>,
    now: u64,
    passes: u64,
    ride_ms: Option<u64>,
    recording: bool,
    clock: Vec<Anchor>,
    utc_at_boot_ms: Option<u64>,
    /// The last entry each source said that was not a repeat, indexed by
    /// kind, then source.
    last: Vec<Option<usize>>,
    /// The text of the entry being written, reused so that an entry costs
    /// one allocation of its exact length.
    line: String,
    pages: Pages,
    /// Each sensor heard from; a handful.
    heard: Vec<Heard>,
    silences: Vec<Silence>,
    /// The first entry of the pass being read.
    pass_start: usize,
    /// A page was reported before this pass.
    paged: bool,
    power: Vec<(u64, device_api::power::State)>,
    health: Vec<(u64, trace::Health)>,
    frame_times: Vec<(u64, trace::FrameTime)>,
    frame_time_awaiting_health: Option<trace::FrameTime>,
    activity: Vec<Window>,
    /// The activity window of this pass, while its tasks are read.
    window: Option<Window>,
}

impl Reader {
    fn at(&self) -> Moment {
        Moment {
            session_ms: self.now,
            ride_ms: self.ride_ms,
        }
    }

    fn learn_utc(&mut self, utc_ms: u64) {
        self.utc_at_boot_ms
            .get_or_insert(utc_ms.saturating_sub(self.now));
    }

    fn push(&mut self, kind: Kind, text: &str) {
        self.push_from(kind, 0, text);
    }

    fn push_fmt(&mut self, kind: Kind, text: fmt::Arguments<'_>) {
        let mut line = self.line();
        let _ = line.write_fmt(text);
        self.push_line(kind, 0, line);
    }

    /// Push `text` lowercased, as the Debug names of enums read.
    fn push_lower(&mut self, kind: Kind, text: fmt::Arguments<'_>) {
        let mut line = self.line();
        let _ = line.write_fmt(text);
        lowercase_from(&mut line, 0);
        self.push_line(kind, 0, line);
    }

    /// The reused line buffer, empty, to write an entry's text into.
    fn line(&mut self) -> String {
        let mut line = std::mem::take(&mut self.line);
        line.clear();
        line
    }

    fn push_line(&mut self, kind: Kind, source: u8, line: String) {
        self.push_from(kind, source, &line);
        self.line = line;
    }

    fn push_from(&mut self, kind: Kind, source: u8, text: &str) {
        let slot = &mut self.last[(kind as usize) << 8 | usize::from(source)];
        let repeat = !kind.edge() && slot.is_some_and(|i| self.entries[i].text == text);
        if !repeat {
            *slot = Some(self.entries.len());
        }
        self.entries.push(Entry {
            at: self.at(),
            kind,
            text: text.to_owned(),
            repeat,
            bytes: None,
            cause: None,
            shows: None,
        });
    }

    /// Give each page that changed in the pass just read its cause, and
    /// enter its activity window.
    fn end_pass(&mut self) {
        self.close_window();
        let pass = &mut self.entries[self.pass_start..];
        if pass.iter().any(|e| e.kind == Kind::Page) {
            let cause = if self.paged {
                Cause::of(pass)
            } else {
                Cause::PowerOn
            };
            self.paged = true;
            for page in pass.iter_mut().filter(|e| e.kind == Kind::Page) {
                page.cause = Some(cause.clone());
                {
                    page.shows = readings(&page.text);
                }
            }
        }
        self.pass_start = self.entries.len();
    }

    /// Set the ride clock to the ride time VANA showed when they part: a
    /// storage fallback closes and reopens the recorder, which would start
    /// the clock again, while the rider's ride time carries on.
    fn ride_time_shown(&mut self, fields: &str) {
        let (Some(ride_ms), Some(shown_ms)) =
            (self.ride_ms, field(fields, "time").and_then(parse_ms))
        else {
            return;
        };
        if ride_ms / 1000 != shown_ms / 1000 {
            self.ride_ms = Some(shown_ms);
            self.clock.push(Anchor {
                session_ms: self.now,
                ride_ms: self.ride_ms,
                running: self.recording,
            });
        }
    }

    fn close_window(&mut self) {
        if let Some(window) = self.window.take() {
            self.push(Kind::Activity, &self::window(&window));
            self.activity.push(window);
        }
    }

    /// Note a packet from `identity`, and the silence it ends; the sensor's
    /// name.
    fn heard_from(&mut self, identity: ant::Identity) -> &str {
        let at = self.heard.iter().position(|heard| {
            (heard.identity.device_type, heard.identity.device_number)
                == (identity.device_type, identity.device_number)
        });
        let Some(at) = at else {
            self.heard.push(Heard {
                identity,
                last_ms: self.now,
                name: sensor(identity),
            });
            return &self.heard[self.heard.len() - 1].name;
        };
        let heard = &mut self.heard[at];
        heard.identity = identity;
        let last_ms = std::mem::replace(&mut heard.last_ms, self.now);
        if self.now - last_ms > SILENCE_MS {
            self.silences.push(Silence {
                sensor: heard.name.clone(),
                last_ms,
                next_ms: Some(self.now),
            });
        }
        &self.heard[at].name
    }

    fn silent_at_end(&mut self) {
        let mut open: Vec<Silence> = self
            .heard
            .iter()
            .filter(|heard| self.now - heard.last_ms > SILENCE_MS)
            .map(|heard| Silence {
                sensor: heard.name.clone(),
                last_ms: heard.last_ms,
                next_ms: None,
            })
            .collect();
        self.silences.append(&mut open);
        self.silences
            .sort_by(|a, b| (a.last_ms, &a.sensor).cmp(&(b.last_ms, &b.sensor)));
    }

    fn event(&mut self, event: &Event<'_>) {
        if !matches!(event, Event::FrameTime(_) | Event::Health(_)) {
            self.frame_time_awaiting_health = None;
        }
        match *event {
            Event::Pass { now_ms } => {
                self.end_pass();
                if self.passes > 0 && now_ms.saturating_sub(self.now) >= GAP_MS {
                    self.push_fmt(
                        Kind::Gap,
                        format_args!(
                            "no pass for {} until {}",
                            duration(now_ms - self.now),
                            session_clock(now_ms)
                        ),
                    );
                }
                if self.recording {
                    self.ride_ms = self.ride_ms.map(|ms| ms + now_ms.saturating_sub(self.now));
                }
                self.now = now_ms;
                self.passes += 1;
            }
            Event::Input(edge) => self.push(Kind::Input, &input(edge.input)),
            Event::Synthetic(pressed) => {
                self.push_fmt(Kind::Input, format_args!("synthetic {}", input(pressed)))
            }
            Event::Panel(_) => {}
            Event::Command(line) => self.push(Kind::Command, &super::secret::shown_command(line)),
            Event::Startup(startup) => {
                self.push_fmt(Kind::Startup, format_args!("companion {}", startup.name()))
            }
            Event::AntRadio(radio) => self.push_lower(
                Kind::Ant,
                format_args!(
                    "radio {:?}{}",
                    radio.availability,
                    if radio.scanning { " scanning" } else { "" }
                ),
            ),
            Event::AntDiscoveries(found) => {
                let mut line = self.line();
                for (i, d) in found.iter().flatten().enumerate() {
                    let _ = write!(
                        line,
                        "{}{} rssi {}",
                        if i == 0 { "found " } else { ", " },
                        Sensor(d.identity),
                        d.rssi
                    );
                }
                if line.is_empty() {
                    line.push_str("found nothing");
                }
                self.push_line(Kind::Ant, 0, line);
            }
            Event::AntLink { channel, snapshot } => {
                let mut line = self.line();
                ant_link(&mut line, channel, snapshot);
                self.push_line(Kind::Ant, channel, line);
            }
            Event::AntPacket(packet) => {
                let mut line = self.line();
                line.push_str(self.heard_from(packet.identity));
                line.push(' ');
                self.pages
                    .describe_into(&mut line, packet.identity, packet.data);
                self.push_line(Kind::AntPacket, 0, line);
                if let Some(entry) = self.entries.last_mut() {
                    entry.bytes = Some(packet.data);
                }
            }
            Event::AntRequest { operation, result } => self.push_fmt(
                Kind::AntRequest,
                format_args!("{} -> {}", ant_operation(operation), ant_result(result)),
            ),
            Event::AntSend(send) => {
                self.push_fmt(Kind::AntRequest, format_args!("send status {send:?}"))
            }
            Event::BleLink {
                availability,
                snapshot,
            } => {
                let mut line = self.line();
                let _ = write!(line, "link {}", snapshot.link.name());
                if availability != device_api::observation::Availability::Ready {
                    let start = line.len();
                    let _ = write!(line, " ({availability:?})");
                    lowercase_from(&mut line, start);
                }
                self.push_line(Kind::Ble, 0, line);
            }
            Event::BlePacket(packet) => self.push_fmt(
                Kind::BlePacket,
                format_args!(
                    "connection {} seq {} data {}",
                    packet.connection,
                    packet.sequence,
                    hex(packet.data().unwrap_or(&[]))
                ),
            ),
            Event::BleBattery(level) => {
                self.push_fmt(Kind::Ble, format_args!("sensor battery raw {}", level.raw))
            }
            Event::Position {
                availability: _,
                snapshot,
            } => {
                if let Some(utc) = snapshot.and_then(|s| s.gps.utc_ms) {
                    self.learn_utc(utc);
                }
                let mut line = self.line();
                gps(&mut line, snapshot);
                self.push_line(Kind::Gps, 0, line);
            }
            Event::Physical(snapshot) => self.push(
                Kind::Sensors,
                match snapshot.pressure {
                    Observation::Unavailable => "barometer unavailable",
                    Observation::Fresh { .. } => "barometer fresh",
                    Observation::Stale { .. } => "barometer stale",
                },
            ),
            Event::Light(light) => match light {
                Observation::Unavailable => self.push(Kind::Sensors, "ambient light unavailable"),
                Observation::Fresh { value, .. } => {
                    self.push_fmt(Kind::Sensors, format_args!("ambient light {}", value.0))
                }
                Observation::Stale { value, .. } => self.push_fmt(
                    Kind::Sensors,
                    format_args!("ambient light {} stale", value.0),
                ),
            },
            Event::Power(snapshot) => {
                let mut line = self.line();
                battery(&mut line, snapshot);
                self.push_line(Kind::Battery, 0, line);
            }
            Event::NetworkTime(snapshot) => {
                if let Some(unix) = snapshot.unix_seconds {
                    self.learn_utc(unix * 1000 + u64::from(snapshot.millis));
                }
                match snapshot.unix_seconds {
                    Some(unix) => self.push_lower(
                        Kind::Time,
                        format_args!("wifi utc {unix} ({:?})", snapshot.status),
                    ),
                    None => self.push_lower(
                        Kind::Time,
                        format_args!("wifi utc none ({:?})", snapshot.status),
                    ),
                }
            }
            Event::StorageFault(fault) => self.push_lower(
                Kind::Storage,
                format_args!(
                    "{:?} {:?} failed at {}: {:?}",
                    fault.medium, fault.op, fault.at, fault.error
                ),
            ),
            Event::Entropy(value) => self.push_fmt(Kind::Entropy, format_args!("{value:#018x}")),
            Event::Page(page) => self.push(Kind::Page, page),
            Event::Toast(text) => self.push(
                Kind::Toast,
                if text.is_empty() { TOAST_CLEARED } else { text },
            ),
            Event::Sound {
                pattern,
                cue,
                note,
                of,
            } => self.push(
                Kind::Sound,
                &match (cue.replace('_', " "), of) {
                    (cue, _) if cue.is_empty() => format!("pattern {pattern}"),
                    (cue, 1) => format!("pattern {pattern}: {cue}"),
                    (cue, of) => format!("pattern {pattern}: {cue} ({note} of {of})"),
                },
            ),
            Event::RideState(state) => {
                let open = matches!(state, "recording" | "paused" | "auto_paused");
                self.ride_ms = match (self.ride_ms, open) {
                    (_, false) => None,
                    (None, true) => Some(0),
                    (Some(ms), true) => Some(ms),
                };
                self.recording = state == "recording";
                self.clock.push(Anchor {
                    session_ms: self.now,
                    ride_ms: self.ride_ms,
                    running: self.recording,
                });
                self.push(Kind::Ride, state);
            }
            Event::Mark {
                count,
                pressed_ms,
                page,
            } => self.push_fmt(
                Kind::Mark,
                format_args!("{count} pressed {} on {page}", session_clock(pressed_ms)),
            ),
            Event::Shown(fields) => {
                self.ride_time_shown(fields);
                self.push(Kind::Shown, fields)
            }
            Event::Log {
                level,
                target,
                text,
            } => self.push_fmt(
                if about_storage(text) {
                    Kind::Storage
                } else {
                    Kind::Log
                },
                format_args!(
                    "{} {target} {text}",
                    match level {
                        Level::Error => "error",
                        Level::Warn => "warn",
                    }
                ),
            ),
            Event::AccessPoint { state, reach } => self.push_lower(
                Kind::Phone,
                format_args!("access point {state:?}, {reach:?}"),
            ),
            Event::AccessPointRequest { start, result } => self.push_fmt(
                Kind::Phone,
                format_args!(
                    "access point {} -> {}",
                    if start { "start" } else { "stop" },
                    answer(result)
                ),
            ),
            Event::Server { request, room } => self.push(
                Kind::Phone,
                &match request {
                    Some(request) => format!(
                        "server holds #{} {:?} {}{}{} body {} B, room {room} B",
                        request.id,
                        request.method,
                        request.path,
                        if request.query.is_empty() { "" } else { "?" },
                        super::secret::shown_query(request.query),
                        request.body
                    ),
                    None => format!("server holds nothing, room {room} B"),
                },
            ),
            Event::ServerListen(listen) => self.push(
                Kind::Phone,
                if listen {
                    "server listen"
                } else {
                    "server stop"
                },
            ),
            Event::ServerFinish(id) => {
                self.push_fmt(Kind::Phone, format_args!("server finish #{id}"))
            }
            Event::ServerRead { id, part, more } => self.push_fmt(
                Kind::Phone,
                format_args!(
                    "server read #{id} {} B{}",
                    part.len(),
                    if more { ", more" } else { "" }
                ),
            ),
            Event::ServerWrite { id, taken } => self.push_fmt(
                Kind::Phone,
                format_args!("server write #{id} took {taken} B"),
            ),
            Event::PowerControl(status) => {
                self.power.push((self.now, status.state));
                self.push_lower(
                    Kind::Power,
                    format_args!("{:?} {:?}", status.state, status.operation),
                )
            }
            Event::PowerRequest {
                operation,
                delay_ms,
                result,
            } => self.push_fmt(
                Kind::Power,
                format_args!(
                    "{} in {delay_ms} ms -> {}",
                    format!("{operation:?}").to_lowercase(),
                    answer(result)
                ),
            ),
            Event::PoweredOff => self.push(Kind::Power, "powered off"),
            Event::Backlight(availability) => {
                self.push_lower(Kind::Display, format_args!("backlight {availability:?}"))
            }
            Event::Refused { output, attempt } => self.push_lower(
                Kind::Display,
                format_args!("{output:?} refused, attempt {attempt}"),
            ),
            Event::SoundState(snapshot) => self.push_lower(
                Kind::Buzzer,
                format_args!("{:?} {:?}", snapshot.state, snapshot.pattern),
            ),
            Event::SoundRequest { play, result } => self.push_fmt(
                Kind::Buzzer,
                format_args!(
                    "{} -> {}",
                    match play {
                        Some(pattern) => format!("play {pattern}"),
                        None => "stop".into(),
                    },
                    answer(result)
                ),
            ),
            Event::TooLarge(tag) => self.push_fmt(
                Kind::Trace,
                format_args!("a record tagged {tag} was too large and left out"),
            ),
            Event::HostWrote => self.push(
                Kind::Trace,
                "a host wrote FILES or TERRAIN; the trace ends here",
            ),
            Event::Lost { blocks, bytes } => self.push_fmt(
                Kind::Trace,
                format_args!(
                    "{blocks} blocks, {bytes} B, lost here: the recorder had no room for them"
                ),
            ),
            Event::FrameTime(frame_time) => self.frame_time_awaiting_health = Some(frame_time),
            Event::Health(health) => {
                let frame_time = self.frame_time_awaiting_health.take();
                self.push(Kind::Health, &self::health(&health, frame_time));
                self.health.push((self.now, health));
                self.frame_times.extend(frame_time.map(|f| (self.now, f)));
            }
            Event::Activity(activity) => {
                self.close_window();
                self.window = Some(Window {
                    end_ms: self.now,
                    activity,
                    tasks: Vec::new(),
                    stages: Vec::new(),
                });
            }
            Event::Task {
                name,
                polls,
                busy_us,
            } => {
                if let Some(window) = &mut self.window {
                    window.tasks.push(TaskTime {
                        name: name.into(),
                        polls,
                        busy_us,
                    });
                }
            }
            Event::Stage {
                name,
                runs,
                busy_us,
                max_us,
            } => {
                if let Some(window) = &mut self.window {
                    window.stages.push(StageTime {
                        name: name.into(),
                        runs,
                        busy_us,
                        max_us,
                    });
                }
            }
        }
    }
}

/// Which shown readings a ride page draws, from VANA's own pages; the main
/// page's lower line as the rider left it by default.
fn readings(page: &str) -> Option<String> {
    use vana::screens::ride::{PAUSED_READINGS, RidePage, slots::Slots};
    let readings = match page {
        "ride/paused" => PAUSED_READINGS.to_vec(),
        name => match RidePage::named(name)?.readings(Slots::default()) {
            Some(readings) => readings.to_vec(),
            None => return Some("the workout step's target".into()),
        },
    };
    Some(if readings.is_empty() {
        "none".into()
    } else {
        readings.join(",")
    })
}

fn answer(result: Result<(), device_api::observation::Error>) -> String {
    match result {
        Ok(()) => "ok".into(),
        Err(error) => format!("{error:?}").to_lowercase(),
    }
}

const STORAGE_WORDS: [&str; 11] = [
    "save",
    "load",
    "forget",
    "fold",
    "clear",
    "history",
    "storage",
    "bulk",
    "media",
    "volume",
    "turns_not",
];

fn about_storage(text: &str) -> bool {
    let text = text.to_lowercase();
    STORAGE_WORDS.iter().any(|word| text.contains(word))
}

/// Lowercase `line` from byte `start`, as `str::to_lowercase` does, without
/// a copy when that part is ASCII.
fn lowercase_from(line: &mut String, start: usize) {
    if line[start..].is_ascii() {
        line[start..].make_ascii_lowercase();
    } else {
        let lower = line[start..].to_lowercase();
        line.truncate(start);
        line.push_str(&lower);
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// An ANT+ sensor as `power#12345`.
pub fn sensor(identity: ant::Identity) -> String {
    Sensor(identity).to_string()
}

/// Writes an ANT+ sensor as `power#12345`.
pub struct Sensor(pub ant::Identity);

impl fmt::Display for Sensor {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0.device_type {
            11 => f.write_str("power"),
            40 => f.write_str("radar"),
            120 => f.write_str("hr"),
            121 => f.write_str("speed_cadence"),
            122 => f.write_str("cadence"),
            123 => f.write_str("speed"),
            other => write!(f, "type{other}"),
        }?;
        write!(f, "#{}", self.0.device_number)
    }
}

fn ant_link(out: &mut String, channel: u8, snapshot: Option<ant::Snapshot>) {
    let _ = match snapshot {
        None => write!(out, "ch{channel} closed"),
        Some(s) => {
            let _ = match s.selected {
                Some(peer) => write!(out, "ch{channel} {}", Sensor(peer)),
                None => write!(out, "ch{channel} no sensor"),
            };
            write!(
                out,
                " {}{}{}",
                s.link.name(),
                if s.stale { " stale" } else { "" },
                if s.scanning { " scanning" } else { "" }
            )
        }
    };
}

fn ant_operation(operation: ant::AntOperation) -> String {
    match operation {
        ant::AntOperation::Scan(ms) => format!("scan {ms} ms"),
        ant::AntOperation::StopScan => "stop scan".into(),
        ant::AntOperation::Connect(peer) => format!("connect {}", sensor(peer)),
        ant::AntOperation::Disconnect(channel) => format!("disconnect ch{channel}"),
        ant::AntOperation::Send { target, data } => {
            format!("send {target:?} data {}", hex(&data))
        }
    }
}

fn ant_result(result: Result<ant::Admission, ant::Error>) -> String {
    match result {
        Ok(admission) => format!("{admission:?}"),
        Err(error) => format!("refused {error:?}"),
    }
    .to_lowercase()
}

fn input(input: Input) -> String {
    match input {
        Input::Button { button, code } => format!("button {button:?} code {code}"),
        Input::Touch(point) => format!("touch {},{}", point.x, point.y),
        Input::Release => "release".into(),
        Input::Cancel => "cancel".into(),
    }
}

fn gps(out: &mut String, snapshot: Option<device_api::positioning::Snapshot>) {
    let Some(snapshot) = snapshot else {
        out.push_str("no receiver");
        return;
    };
    let fix = snapshot.gps;
    let state = match fix.state {
        device_api::gps::FixState::NoData => "no data",
        device_api::gps::FixState::NoFix => "no fix",
        device_api::gps::FixState::Fresh => "fix",
        device_api::gps::FixState::Stale => "fix stale",
    };
    out.push_str(state);
    if let Some(satellites) = fix.satellites {
        let _ = write!(out, " sats {satellites}");
    }
}

fn battery(out: &mut String, snapshot: Option<device_api::input::InputSnapshot>) {
    let Some(snapshot) = snapshot else {
        out.push_str("companion silent");
        return;
    };
    let _ = match snapshot.battery {
        Observation::Fresh {
            value: (percent, _),
            ..
        } => write!(out, "{percent}%"),
        Observation::Stale {
            value: (percent, _),
            ..
        } => write!(out, "{percent}% stale"),
        Observation::Unavailable => out.write_str("level unknown"),
    };
    out.push_str(match snapshot.power {
        Observation::Fresh { value: 0, .. } | Observation::Stale { value: 0, .. } => " charging",
        Observation::Unavailable => "",
        _ => " on battery",
    });
}

pub fn rate(per_second: f64) -> String {
    Rate(per_second).to_string()
}

/// Writes a rate as `12/s`, or `1.5/s` under ten.
struct Rate(f64);

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            per_second if per_second >= 10.0 => write!(f, "{per_second:.0}/s"),
            per_second => write!(f, "{per_second:.1}/s"),
        }
    }
}

pub fn percent(share: f64) -> String {
    Percent(share).to_string()
}

/// Writes a share as `12.5%`.
struct Percent(f64);

impl fmt::Display for Percent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:.1}%", self.0 * 100.0)
    }
}

/// A health record as one timeline line.
fn health(h: &trace::Health, frame_time: Option<trace::FrameTime>) -> String {
    let mut parts: Vec<String> = Vec::new();
    if h.simulated {
        parts.push("simulated".into());
    }
    if let Some(free) = h.heap_free {
        parts.push(format!("heap {}", bytes(u64::from(free))));
    }
    if let Some(least) = h.heap_min {
        parts.push(format!("least {}", bytes(u64::from(least))));
    }
    if let Some(free) = h.psram_free {
        parts.push(format!("psram {}", bytes(u64::from(free))));
    }
    match (frame_time, h.frame_max_us) {
        (Some(frame), Some(max_us)) => parts.push(format!(
            "frame mean {:.2} ms, max {}",
            f64::from(frame.mean_us) / 1000.0,
            millis(max_us)
        )),
        (_, Some(max_us)) => parts.push(format!("frame {}", millis(max_us))),
        _ => {}
    }
    if let Some(us) = h.pass_max_us {
        parts.push(format!("pass {}", millis(us)));
    }
    parts.push(format!(
        "gps {} sentences, {} overflows, {} uart errors",
        h.gps_sentences, h.gps_overflows, h.gps_uart_errors
    ));
    parts.push(format!("companion {} errors", h.companion_errors));
    if let Some(us) = h.card_write_max_us {
        parts.push(format!("card write {}", millis(us)));
    }
    if let Some(recoveries) = h.card_recoveries {
        parts.push(format!("card {recoveries} recoveries"));
    }
    if h.spool_size > 0 {
        parts.push(format!(
            "spool {} of {}",
            bytes(u64::from(h.spool_used)),
            bytes(u64::from(h.spool_size))
        ));
    }
    if let Some(state) = h.wifi {
        parts.push(format!("wifi {}", wifi(state)));
    }
    if let Some(rssi) = h.wifi_rssi {
        parts.push(format!("wifi {rssi} dBm"));
    }
    if let Some(rssi) = h.ble_rssi {
        parts.push(format!("ble sensor {rssi} dBm"));
    }
    parts.join(", ")
}

/// An activity window as one timeline line.
fn window(window: &Window) -> String {
    let a = &window.activity;
    let span_ms = u64::from(a.window_ms).max(1);
    let share = |us: u32| Percent(f64::from(us) / (span_ms * 1000) as f64);
    let per_s = |count: u32| Rate(f64::from(count) * 1000.0 / span_ms as f64);
    let mut text = String::new();
    let _ = write!(
        text,
        "{}{}: {} wakeups",
        if a.simulated { "simulated " } else { "" },
        duration(span_ms),
        per_s(a.wakes),
    );
    if let Some(ticks) = a.ticks {
        let _ = write!(text, ", {} ticks", per_s(ticks));
    }
    if let Some(switches) = a.switches {
        let _ = write!(text, ", {} switches", per_s(switches));
    }
    for (name, us) in [
        ("awake", a.awake_us),
        ("main", a.main_us),
        ("radio", a.radio_us),
        ("idle", a.idle_us),
        ("counting", a.hooks_us),
    ] {
        if let Some(us) = us {
            let _ = write!(text, ", {name} {}", share(us));
        }
    }
    for (i, task) in window.tasks.iter().enumerate() {
        let _ = write!(
            text,
            "{}{} {}",
            if i == 0 { "; " } else { ", " },
            task.name,
            per_s(task.polls)
        );
        if let Some(busy) = task.busy_us {
            let _ = write!(text, " {}", share(busy));
        }
    }
    for (i, stage) in window.stages.iter().enumerate() {
        let _ = write!(
            text,
            "{}{} {} ({} a pass, max {})",
            if i == 0 { "; pass stages " } else { ", " },
            stage.name,
            share(stage.busy_us),
            Micros(u64::from(stage.busy_us) / u64::from(stage.runs.max(1))),
            Micros(u64::from(stage.max_us))
        );
    }
    let c = &a.consumers;
    let _ = write!(
        text,
        "; backlight {}%, {} frames, gnss {}{}, ap {}, ble {}, ant {}{}{}",
        c.backlight,
        c.frames,
        match c.gnss {
            trace::Gnss::Off => "off",
            trace::Gnss::Receiving => "on",
            trace::Gnss::Paused => "paused",
            trace::Gnss::Failed => "failed",
        },
        c.wifi
            .map(|state| format!(", wifi {}", wifi(state)))
            .unwrap_or_default(),
        match c.access_point {
            device_api::server::AccessPointState::Off
            | device_api::server::AccessPointState::Unsupported => "off".to_string(),
            device_api::server::AccessPointState::Up { clients } => format!("up {clients}"),
            other => format!("{other:?}").to_lowercase(),
        },
        c.ble.name(),
        format!("{:?}", c.ant).to_lowercase(),
        if c.ant_scanning { " scanning" } else { "" },
        if c.ant_channels > 0 {
            format!(" {} linked", c.ant_channels)
        } else {
            String::new()
        }
    );
    if let Some(mhz) = c.cpu_mhz {
        let _ = write!(text, ", cpu {mhz} MHz");
    }
    if c.slept_ms > 0 {
        let _ = write!(text, ", slept {}", duration(u64::from(c.slept_ms)));
    }
    text
}

fn wifi(state: device_api::connectivity::WifiState) -> String {
    format!("{state:?}").to_lowercase()
}

pub fn bytes(n: u64) -> String {
    if n >= 1024 * 1024 {
        format!("{:.2} MiB", n as f64 / (1024.0 * 1024.0))
    } else {
        format!("{:.1} KiB", n as f64 / 1024.0)
    }
}

pub fn millis(us: u32) -> String {
    format!("{:.1} ms", f64::from(us) / 1000.0)
}

/// `850 µs`, or `3.15 ms` from a millisecond up.
pub fn micros(us: u64) -> String {
    Micros(us).to_string()
}

struct Micros(u64);

impl fmt::Display for Micros {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.0 {
            us if us >= 1000 => write!(f, "{:.2} ms", us as f64 / 1000.0),
            us => write!(f, "{us} µs"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_read_as_a_rider_or_an_agent_writes_them() {
        for (text, at) in [
            ("40:00", At::Session(2_400_000)),
            ("1:02:03", At::Session(3_723_000)),
            ("40:12.5", At::Session(2_412_500)),
            ("90s", At::Session(90_000)),
            ("40m", At::Session(2_400_000)),
            ("2500ms", At::Session(2_500)),
            ("r38:00", At::Ride(2_280_000)),
        ] {
            assert_eq!(At::parse(text), Ok(at), "{text}");
        }
        for text in ["", "r", "4:", "1:2:3:4", "-5s", "soon"] {
            assert!(At::parse(text).is_err(), "{text}");
        }
    }

    #[test]
    fn clocks_count_minutes_past_the_hour() {
        let moment = Moment {
            session_ms: 4_812_345,
            ride_ms: Some(4_685_000),
        };
        assert_eq!(moment.to_string(), "80:12.345 ride 78:05");
        assert_eq!(Moment::default().to_string(), "00:00.000 ride --:--");
    }

    #[test]
    fn health_and_activity_show_only_when_asked_for_by_name() {
        for group in ["default", "seen", "inputs"] {
            let kinds = Kind::parse_list(group).unwrap();
            assert!(
                !kinds.contains(&Kind::Health) && !kinds.contains(&Kind::Activity),
                "{group}: {kinds:?}"
            );
        }
        assert_eq!(
            Kind::parse_list("activity,health"),
            Ok(vec![Kind::Health, Kind::Activity])
        );
    }

    #[test]
    fn a_shown_field_reads_by_name() {
        let shown = "time=0:40:12 speed=31.2 power=-- hr=142";
        assert_eq!(field(shown, "power"), Some("--"));
        assert_eq!(field(shown, "speed"), Some("31.2"));
        assert_eq!(field(shown, "cad"), None);
    }
}
