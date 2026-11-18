//! A trace decoded into readable entries, each stamped with the session
//! uptime and the ride's own clock: what every `cycling trace` subcommand
//! reads.
mod reader;
mod text;

pub use reader::{build_name, read};
use text::hex;
pub use text::{bytes, duration, micros, millis, percent, rate, ride_clock, sensor, session_clock};

use firmware_services::trace::{self, End};
use serde::{Serialize, Serializer};
use std::fmt;

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
        session_clock(self.session_ms).fmt(f)?;
        f.write_str(" ride ")?;
        match self.ride_ms {
            Some(ms) => ride_clock(ms).fmt(f),
            None => f.write_str("--:--"),
        }
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

pub(super) fn parse_ms(text: &str) -> Option<u64> {
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
    /// The recording build's content hash in hex; empty when unknown.
    pub build: String,
    pub boot: u32,
    /// The TERRAIN pack the session found at power-on.
    pub terrain: trace::origin::Found<trace::origin::Terrain>,
    pub entries: Vec<Entry>,
    /// The ride clock at each ride state change.
    pub clock: Vec<Anchor>,
    pub passes: u64,
    /// The last pass's uptime.
    pub end_ms: u64,
    /// Why the events stopped: the trace ended, or a block was cut short or
    /// failed its check.
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
    /// What this session's boot found of the trace before it.
    pub before: Before,
}

/// What a boot found of the trace before its own.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Before {
    pub horizon: Option<Horizon>,
    /// The blocks the boot appended to it after a panic or a watchdog
    /// reset.
    pub salvaged: Option<u32>,
}

/// How far the trace before survived on the card.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Horizon {
    pub trace: String,
    /// Its last pass whose records survived; none when it holds none.
    pub last_pass_ms: Option<u64>,
    /// Its whole blocks after its last synced record.
    pub blocks: u32,
}

impl fmt::Display for Before {
    /// `trace before: traces/000041-1a2b3c4d.trace survived to 12:00.000,
    /// 2 blocks after its last sync; 5 blocks salvaged into it after the
    /// crash`, or nothing when the boot found no trace before.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(horizon) = &self.horizon {
            write!(f, "trace before: {horizon}")?;
            if let Some(blocks) = self.salvaged {
                write!(f, "; {blocks} blocks salvaged into it after the crash")?;
            }
        } else if let Some(blocks) = self.salvaged {
            write!(
                f,
                "trace before: {blocks} blocks salvaged into it after the crash"
            )?;
        }
        Ok(())
    }
}

impl fmt::Display for Horizon {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self.last_pass_ms {
            Some(ms) => write!(f, "{} survived to {}", self.trace, session_clock(ms))?,
            None => write!(f, "{} holds no pass", self.trace)?,
        }
        write!(f, ", {} blocks after its last sync", self.blocks)
    }
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
