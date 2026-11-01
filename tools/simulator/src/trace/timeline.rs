//! A trace decoded into readable entries, each stamped with the session
//! uptime and the ride's own clock: what every `cycling trace` subcommand
//! reads.
use crate::trace::ant_page::Pages;
use device_api::ant;
use device_api::input::Input;
use device_api::observation::Observation;
use firmware_services::trace::{self, End, Event, Level};
use std::collections::HashMap;
use std::fmt;

const GAP_MS: u64 = 1_000;
/// An ANT+ sensor broadcasts four times a second; this long without a
/// packet from it is a silence.
pub const SILENCE_MS: u64 = 3_000;

/// When something happened: uptime since power-on, and the ride clock while
/// a ride is open (it stops while the ride is paused).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Moment {
    pub session_ms: u64,
    pub ride_ms: Option<u64>,
}

impl fmt::Display for Moment {
    /// `40:12.345 ride 38:05`, or `ride --:--` outside a ride.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ride ", session_clock(self.session_ms))?;
        match self.ride_ms {
            Some(ms) => write!(f, "{}", ride_clock(ms)),
            None => write!(f, "--:--"),
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
    Shown,
    Log,
    Phone,
    Power,
    Display,
    Buzzer,
    Trace,
}

impl Kind {
    pub const ALL: [Self; 26] = [
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
        Self::Shown,
        Self::Log,
        Self::Phone,
        Self::Power,
        Self::Display,
        Self::Buzzer,
        Self::Trace,
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
            Self::Shown => "shown",
            Self::Log => "log",
            Self::Phone => "phone",
            Self::Power => "power",
            Self::Display => "display",
            Self::Buzzer => "buzzer",
            Self::Trace => "trace",
        }
    }

    /// What the rider saw or heard, or VANA logged; the rest is input.
    /// Storage holds both: faults the media reported, and VANA's warnings
    /// about saving or reading, which `about_storage` sorts out of `log`.
    pub const fn seen(self) -> bool {
        matches!(
            self,
            Self::Page | Self::Toast | Self::Sound | Self::Ride | Self::Shown | Self::Log
        )
    }

    /// Each entry is something that happened (a press, a command, a note
    /// played), not a state reported again, so none is ever a repeat: two
    /// presses of one button are two presses.
    pub const fn edge(self) -> bool {
        matches!(self, Self::Input | Self::Command | Self::Sound)
    }

    /// Many per second; hidden unless asked for.
    pub const fn bulky(self) -> bool {
        matches!(
            self,
            Self::AntPacket | Self::AntRequest | Self::BlePacket | Self::Entropy
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
                "inputs" => Self::ALL.into_iter().filter(|k| !k.seen()).collect(),
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
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub at: Moment,
    pub kind: Kind,
    pub text: String,
    /// The same source said the same thing before: only counters or a
    /// reading moved. Hidden unless asked for.
    pub repeat: bool,
    /// The bytes behind a decoded packet, in hex.
    pub hex: Option<String>,
    /// For a page: what in its pass brought it up.
    pub cause: Option<Cause>,
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
            write!(f, "  ({cause})")?;
        }
        match &self.hex {
            Some(hex) if f.alternate() => write!(f, "  [{hex}]"),
            _ => Ok(()),
        }
    }
}

/// A whole trace, decoded.
pub struct Timeline {
    pub version: u16,
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
    /// The trace ends in a block that was cut short or failed its check.
    pub torn: bool,
    /// UTC in ms at uptime zero, from the first GPS or Wi-Fi time.
    pub utc_at_boot_ms: Option<u64>,
    /// Stretches in which a sensor that had sent packets sent none, in
    /// the order they began.
    pub silences: Vec<Silence>,
}

/// No packet from `sensor` after `last_ms` until `next_ms`, or to the end.
#[derive(Clone, Debug, PartialEq, Eq)]
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
    let (header, mut events) = trace::decode(bytes)
        .map_err(|_| "no trace header: the file is no ride trace".to_string())?;
    let mut reader = Reader::default();
    for event in events.by_ref() {
        reader.event(&event);
    }
    reader.end_pass();
    #[cfg(feature = "cycling")]
    super::cue::name(&mut reader.entries, reader.now);
    reader.silent_at_end();
    let build_len = header
        .build
        .iter()
        .rposition(|&b| b != 0)
        .map_or(0, |i| i + 1);
    Ok(Timeline {
        version: header.version,
        build: header.build[..build_len]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect(),
        boot: header.boot,
        passes: reader.passes,
        end_ms: reader.now,
        torn: events.end() == Some(End::Torn),
        entries: reader.entries,
        clock: reader.clock,
        utc_at_boot_ms: reader.utc_at_boot_ms,
        silences: reader.silences,
    })
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
    last: HashMap<(Kind, u8), String>,
    pages: Pages,
    heard: HashMap<String, u64>,
    silences: Vec<Silence>,
    /// The first entry of the pass being read.
    pass_start: usize,
    /// A page was reported before this pass.
    paged: bool,
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

    fn push(&mut self, kind: Kind, text: String) {
        self.push_from(kind, 0, text);
    }

    fn push_from(&mut self, kind: Kind, source: u8, text: String) {
        let repeat = !kind.edge() && self.last.get(&(kind, source)) == Some(&text);
        if !repeat {
            self.last.insert((kind, source), text.clone());
        }
        self.entries.push(Entry {
            at: self.at(),
            kind,
            text,
            repeat,
            hex: None,
            cause: None,
        });
    }

    /// Give each page that changed in the pass just read its cause.
    fn end_pass(&mut self) {
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
            }
        }
        self.pass_start = self.entries.len();
    }

    fn heard_from(&mut self, sensor: &str) {
        let Some(last_ms) = self.heard.insert(sensor.to_string(), self.now) else {
            return;
        };
        if self.now - last_ms > SILENCE_MS {
            self.silences.push(Silence {
                sensor: sensor.to_string(),
                last_ms,
                next_ms: Some(self.now),
            });
        }
    }

    fn silent_at_end(&mut self) {
        let mut open: Vec<Silence> = self
            .heard
            .iter()
            .filter(|(_, last_ms)| self.now - **last_ms > SILENCE_MS)
            .map(|(sensor, last_ms)| Silence {
                sensor: sensor.clone(),
                last_ms: *last_ms,
                next_ms: None,
            })
            .collect();
        self.silences.append(&mut open);
        self.silences
            .sort_by(|a, b| (a.last_ms, &a.sensor).cmp(&(b.last_ms, &b.sensor)));
    }

    fn event(&mut self, event: &Event<'_>) {
        match *event {
            Event::Pass { now_ms } => {
                self.end_pass();
                if self.passes > 0 && now_ms.saturating_sub(self.now) >= GAP_MS {
                    let text = format!(
                        "no pass for {} until {}",
                        duration(now_ms - self.now),
                        session_clock(now_ms)
                    );
                    self.push(Kind::Gap, text);
                }
                if self.recording {
                    self.ride_ms = self.ride_ms.map(|ms| ms + now_ms.saturating_sub(self.now));
                }
                self.now = now_ms;
                self.passes += 1;
            }
            Event::Input(edge) => self.push(Kind::Input, input(edge.input)),
            Event::Command(line) => self.push(Kind::Command, super::secret::shown_command(line)),
            Event::Startup(startup) => {
                self.push(Kind::Startup, format!("companion {}", startup.name()))
            }
            Event::AntRadio(radio) => self.push(
                Kind::Ant,
                format!(
                    "radio {:?}{}",
                    radio.availability,
                    if radio.scanning { " scanning" } else { "" }
                )
                .to_lowercase(),
            ),
            Event::AntDiscoveries(found) => {
                let found: Vec<String> = found
                    .iter()
                    .flatten()
                    .map(|d| format!("{} rssi {}", sensor(d.identity), d.rssi))
                    .collect();
                let text = if found.is_empty() {
                    "found nothing".into()
                } else {
                    format!("found {}", found.join(", "))
                };
                self.push(Kind::Ant, text);
            }
            Event::AntLink { channel, snapshot } => {
                self.push_from(Kind::Ant, channel, ant_link(channel, snapshot))
            }
            Event::AntPacket(packet) => {
                let words = self.pages.describe(packet.identity, packet.data);
                let sensor = sensor(packet.identity);
                self.heard_from(&sensor);
                self.push(Kind::AntPacket, format!("{sensor} {words}"));
                if let Some(entry) = self.entries.last_mut() {
                    entry.hex = Some(hex(&packet.data));
                }
            }
            Event::AntRequest { operation, result } => self.push(
                Kind::AntRequest,
                format!("{} -> {}", ant_operation(operation), ant_result(result)),
            ),
            Event::AntSend(send) => self.push(Kind::AntRequest, format!("send status {send:?}")),
            Event::BleLink {
                availability,
                snapshot,
            } => self.push(
                Kind::Ble,
                format!(
                    "link {}{}",
                    snapshot.link.name(),
                    match availability {
                        device_api::observation::Availability::Ready => String::new(),
                        other => format!(" ({other:?})").to_lowercase(),
                    }
                ),
            ),
            Event::BlePacket(packet) => self.push(
                Kind::BlePacket,
                format!(
                    "connection {} seq {} data {}",
                    packet.connection,
                    packet.sequence,
                    hex(packet.data().unwrap_or(&[]))
                ),
            ),
            Event::BleBattery(level) => {
                self.push(Kind::Ble, format!("sensor battery raw {}", level.raw))
            }
            Event::Position {
                availability: _,
                snapshot,
            } => {
                if let Some(utc) = snapshot.and_then(|s| s.gps.utc_ms) {
                    self.learn_utc(utc);
                }
                self.push(Kind::Gps, gps(snapshot))
            }
            Event::Physical(snapshot) => self.push(
                Kind::Sensors,
                match snapshot.pressure {
                    Observation::Unavailable => "barometer unavailable".into(),
                    Observation::Fresh { .. } => "barometer fresh".into(),
                    Observation::Stale { .. } => "barometer stale".into(),
                },
            ),
            Event::Power(snapshot) => self.push(Kind::Battery, battery(snapshot)),
            Event::NetworkTime(snapshot) => {
                if let Some(unix) = snapshot.unix_seconds {
                    self.learn_utc(unix * 1000 + u64::from(snapshot.millis));
                }
                self.push(
                    Kind::Time,
                    match snapshot.unix_seconds {
                        Some(unix) => {
                            format!("wifi utc {unix} ({:?})", snapshot.status).to_lowercase()
                        }
                        None => format!("wifi utc none ({:?})", snapshot.status).to_lowercase(),
                    },
                )
            }
            Event::StorageFault(fault) => self.push(
                Kind::Storage,
                format!(
                    "{:?} {:?} failed at {}: {:?}",
                    fault.medium, fault.op, fault.at, fault.error
                )
                .to_lowercase(),
            ),
            Event::Entropy(value) => self.push(Kind::Entropy, format!("{value:#018x}")),
            Event::Page(page) => self.push(Kind::Page, page.into()),
            Event::Toast(text) => self.push(
                Kind::Toast,
                if text.is_empty() {
                    TOAST_CLEARED.into()
                } else {
                    text.into()
                },
            ),
            Event::Sound(pattern) => self.push(Kind::Sound, format!("pattern {pattern}")),
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
                self.push(Kind::Ride, state.into());
            }
            Event::Shown(fields) => self.push(Kind::Shown, fields.into()),
            Event::Log {
                level,
                target,
                text,
            } => self.push(
                if about_storage(text) {
                    Kind::Storage
                } else {
                    Kind::Log
                },
                format!(
                    "{} {target} {text}",
                    match level {
                        Level::Error => "error",
                        Level::Warn => "warn",
                    }
                ),
            ),
            Event::AccessPoint { state, reach } => self.push(
                Kind::Phone,
                format!("access point {state:?}, {reach:?}").to_lowercase(),
            ),
            Event::AccessPointRequest { start, result } => self.push(
                Kind::Phone,
                format!(
                    "access point {} -> {}",
                    if start { "start" } else { "stop" },
                    answer(result)
                ),
            ),
            Event::Server { request, room } => self.push(
                Kind::Phone,
                match request {
                    Some(request) => format!(
                        "server holds #{} {:?} {}{}{} body {} B, room {room} B",
                        request.id,
                        request.method,
                        request.path,
                        if request.query.is_empty() { "" } else { "?" },
                        request.query,
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
                }
                .into(),
            ),
            Event::ServerFinish(id) => self.push(Kind::Phone, format!("server finish #{id}")),
            Event::ServerRead { id, part, more } => self.push(
                Kind::Phone,
                format!(
                    "server read #{id} {} B{}",
                    part.len(),
                    if more { ", more" } else { "" }
                ),
            ),
            Event::ServerWrite { id, taken } => {
                self.push(Kind::Phone, format!("server write #{id} took {taken} B"))
            }
            Event::PowerControl(status) => self.push(
                Kind::Power,
                format!("{:?} {:?}", status.state, status.operation).to_lowercase(),
            ),
            Event::PowerRequest {
                operation,
                delay_ms,
                result,
            } => self.push(
                Kind::Power,
                format!(
                    "{} in {delay_ms} ms -> {}",
                    format!("{operation:?}").to_lowercase(),
                    answer(result)
                ),
            ),
            Event::PoweredOff => self.push(Kind::Power, "powered off".into()),
            Event::Backlight(availability) => self.push(
                Kind::Display,
                format!("backlight {availability:?}").to_lowercase(),
            ),
            Event::Refused { output, attempt } => self.push(
                Kind::Display,
                format!("{output:?} refused, attempt {attempt}").to_lowercase(),
            ),
            Event::SoundState(snapshot) => self.push(
                Kind::Buzzer,
                format!("{:?} {:?}", snapshot.state, snapshot.pattern).to_lowercase(),
            ),
            Event::SoundRequest { play, result } => self.push(
                Kind::Buzzer,
                format!(
                    "{} -> {}",
                    match play {
                        Some(pattern) => format!("play {pattern}"),
                        None => "stop".into(),
                    },
                    answer(result)
                ),
            ),
            Event::TooLarge(tag) => self.push(
                Kind::Trace,
                format!("a record tagged {tag} was too large and left out"),
            ),
            Event::Unknown { tag, body } => self.push(
                Kind::Trace,
                format!(
                    "a record tagged {tag} this build cannot read, {} B",
                    body.len()
                ),
            ),
        }
    }
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

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// An ANT+ sensor as `power#12345`.
pub fn sensor(identity: ant::Identity) -> String {
    format!(
        "{}#{}",
        sensor_type(identity.device_type),
        identity.device_number
    )
}

pub fn sensor_type(device_type: u8) -> String {
    match device_type {
        11 => "power".into(),
        40 => "radar".into(),
        120 => "hr".into(),
        121 => "speed_cadence".into(),
        122 => "cadence".into(),
        123 => "speed".into(),
        other => format!("type{other}"),
    }
}

fn ant_link(channel: u8, snapshot: Option<ant::Snapshot>) -> String {
    let Some(s) = snapshot else {
        return format!("ch{channel} closed");
    };
    let peer = s.selected.map_or("no sensor".into(), sensor);
    format!(
        "ch{channel} {peer} {}{}{}",
        s.link.name(),
        if s.stale { " stale" } else { "" },
        if s.scanning { " scanning" } else { "" }
    )
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

fn gps(snapshot: Option<device_api::positioning::Snapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "no receiver".into();
    };
    let fix = snapshot.gps;
    let state = match fix.state {
        device_api::gps::FixState::NoData => "no data",
        device_api::gps::FixState::NoFix => "no fix",
        device_api::gps::FixState::Fresh => "fix",
        device_api::gps::FixState::Stale => "fix stale",
    };
    match fix.satellites {
        Some(satellites) => format!("{state} sats {satellites}"),
        None => state.into(),
    }
}

fn battery(snapshot: Option<device_api::input::InputSnapshot>) -> String {
    let Some(snapshot) = snapshot else {
        return "companion silent".into();
    };
    let level = match snapshot.battery {
        Observation::Fresh {
            value: (percent, _),
            ..
        } => format!("{percent}%"),
        Observation::Stale {
            value: (percent, _),
            ..
        } => format!("{percent}% stale"),
        Observation::Unavailable => "level unknown".into(),
    };
    let supply = match snapshot.power {
        Observation::Fresh { value: 0, .. } | Observation::Stale { value: 0, .. } => " charging",
        Observation::Unavailable => "",
        _ => " on battery",
    };
    format!("{level}{supply}")
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
    fn a_shown_field_reads_by_name() {
        let shown = "time=0:40:12 speed=31.2 power=-- hr=142";
        assert_eq!(field(shown, "power"), Some("--"));
        assert_eq!(field(shown, "speed"), Some("31.2"));
        assert_eq!(field(shown, "cad"), None);
    }
}
