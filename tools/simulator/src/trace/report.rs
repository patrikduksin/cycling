//! What each `cycling trace` command prints: one value per command, which
//! writes itself as text (`Display`) and as JSON (`Serialize`) from the same
//! fields, so the two cannot tell different stories.
use crate::trace::battery;
use crate::trace::divergence::Divergences;
use crate::trace::file::Media;
use crate::trace::find::Found;
use crate::trace::rebuild::{self, Gap};
use crate::trace::story::{Point, Span, tell};
use crate::trace::timeline::{
    Entry, Kind, Moment, SILENCE_MS, Silence, Timeline, duration, session_clock,
};
use firmware_services::trace::{self, End};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

/// `value` as one JSON document on a line, or as its text.
pub fn render<T: Serialize + fmt::Display>(json: bool, value: &T) -> Result<String, String> {
    if !json {
        return Ok(value.to_string());
    }
    let mut out = serde_json::to_string(value).map_err(|e| e.to_string())?;
    out.push('\n');
    Ok(out)
}

fn as_lines<S: Serializer>(text: &str, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(text.lines())
}

fn as_strings<T: fmt::Display, S: Serializer>(
    items: &[T],
    serializer: S,
) -> Result<S::Ok, S::Error> {
    serializer.collect_seq(items.iter().map(ToString::to_string))
}

/// The trace a command read: its file, its build, how it ended and the
/// media a replay starts from.
#[derive(Serialize)]
pub struct Header<'a> {
    trace: String,
    build: Option<&'a str>,
    boot: u32,
    format: u16,
    end_ms: u64,
    passes: u64,
    #[serde(serialize_with = "end_name")]
    end: Option<End>,
    #[serde(serialize_with = "media_name")]
    media: &'a Media,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    media_report: Vec<&'a str>,
}

fn end_name<S: Serializer>(end: &Option<End>, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match end {
        Some(End::Torn) => "torn",
        Some(End::Corrupt) => "corrupt",
        _ => "complete",
    })
}

fn media_name<S: Serializer>(media: &&Media, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(match media {
        Media::Saved => "saved",
        Media::Rebuilt(_) => "rebuilt",
        Media::Blank => "blank",
    })
}

impl<'a> Header<'a> {
    pub fn of(path: &Path, timeline: &'a Timeline, media: &'a Media) -> Self {
        Self {
            trace: path.display().to_string(),
            build: (!timeline.build.is_empty()).then_some(timeline.build.as_str()),
            boot: timeline.boot,
            format: trace::VERSION.into(),
            end_ms: timeline.end_ms,
            passes: timeline.passes,
            end: timeline.end,
            media,
            media_report: match media {
                Media::Rebuilt(report) => report.lines().collect(),
                _ => Vec::new(),
            },
        }
    }
}

impl fmt::Display for Header<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "trace {}", self.trace)?;
        write!(
            f,
            "build {}, boot {}, format {}, {} over {} passes, {}; media ",
            self.build.unwrap_or("unknown"),
            self.boot,
            self.format,
            duration(self.end_ms),
            self.passes,
            match self.end {
                Some(End::Torn) => "torn: the last block was cut short",
                Some(End::Corrupt) => {
                    "corrupt: a damaged block and everything after it were dropped"
                }
                _ => "complete",
            },
        )?;
        match self.media {
            Media::Saved => writeln!(f, "saved beside it (.origin)"),
            Media::Rebuilt(_) => writeln!(
                f,
                "rebuilt from its header and the pulled media (.origin); {}",
                self.media_report.join("; ")
            ),
            Media::Blank => writeln!(f, "not saved: replays start on blank media"),
        }
    }
}

/// `cycling trace summary`.
#[derive(Serialize)]
pub struct Summary<'a> {
    header: Header<'a>,
    silences: &'a [Silence],
    spans: Vec<Span>,
    events: Vec<Point>,
    /// The activity, flags and health lines: prose.
    #[serde(serialize_with = "as_lines")]
    battery: String,
    entries: BTreeMap<Kind, Count>,
}

#[derive(Serialize)]
struct Count {
    new: usize,
    repeats: usize,
}

impl<'a> Summary<'a> {
    pub fn of(path: &Path, timeline: &'a Timeline, media: &'a Media) -> Self {
        let story = tell(timeline);
        let mut entries: BTreeMap<Kind, Count> = BTreeMap::new();
        for entry in &timeline.entries {
            let count = entries
                .entry(entry.kind)
                .or_insert(Count { new: 0, repeats: 0 });
            if entry.repeat {
                count.repeats += 1;
            } else {
                count.new += 1;
            }
        }
        Self {
            header: Header::of(path, timeline, media),
            silences: &timeline.silences,
            spans: story.spans,
            events: story.events,
            battery: battery::summary(timeline),
            entries,
        }
    }
}

impl fmt::Display for Summary<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.header)?;
        write!(
            f,
            "sensor silences (no ANT+ packet for over {}): ",
            duration(SILENCE_MS)
        )?;
        if self.silences.is_empty() {
            f.write_str("none")?;
        }
        for (i, silence) in self.silences.iter().enumerate() {
            let until = silence.next_ms.unwrap_or(self.header.end_ms);
            write!(
                f,
                "{}{} silent {}-{} ({}{})",
                if i == 0 { "" } else { "; " },
                silence.sensor,
                session_clock(silence.last_ms),
                session_clock(until),
                duration(until - silence.last_ms),
                if silence.next_ms.is_none() {
                    ", to the end"
                } else {
                    ""
                }
            )?;
        }
        writeln!(f)?;
        let mut lines: Vec<(u64, String)> = self
            .spans
            .iter()
            .map(|span| (span.start.session_ms, span.to_string()))
            .chain(
                self.events
                    .iter()
                    .map(|event| (event.at.session_ms, event.to_string())),
            )
            .collect();
        lines.sort_by_key(|(at, _)| *at);
        for (_, line) in lines {
            writeln!(f, "{line}")?;
        }
        f.write_str(&self.battery)?;
        f.write_str("entries: ")?;
        for (i, (kind, count)) in self.entries.iter().enumerate() {
            write!(
                f,
                "{}{} {}",
                if i == 0 { "" } else { ", " },
                kind.name(),
                count.new
            )?;
            if count.repeats > 0 {
                write!(f, "+{}", count.repeats)?;
            }
        }
        writeln!(f)?;
        writeln!(
            f,
            "next: timeline --from T --to T, find <field=value|kind|text>, replay --from T"
        )
    }
}

/// Text a command writes as prose, its lines in JSON: `origin` and `media`.
#[derive(Serialize)]
pub struct Prose {
    #[serde(serialize_with = "as_lines")]
    pub lines: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin: Option<String>,
}

impl fmt::Display for Prose {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.lines)
    }
}

/// `cycling trace rebuild`.
#[derive(Serialize)]
pub struct Rebuilt {
    #[serde(serialize_with = "as_strings")]
    pub not_rebuilt: Vec<Gap>,
    pub origin: PathBuf,
}

impl fmt::Display for Rebuilt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "origin {}", self.origin.display())?;
        f.write_str(&rebuild::report(&self.not_rebuilt))
    }
}

/// `cycling trace timeline`: the entries in a window, and how many of each
/// kind it left out.
#[derive(Serialize)]
pub struct Listing<'a> {
    from_ms: u64,
    to_ms: Option<u64>,
    entries: Vec<&'a Entry>,
    hidden: BTreeMap<&'static str, usize>,
    #[serde(skip)]
    hex: bool,
}

impl<'a> Listing<'a> {
    /// The entries of `kinds` from `from` to `to`, repeats only with `all`.
    pub fn of(
        timeline: &'a Timeline,
        (from, to): (u64, u64),
        kinds: &[Kind],
        all: bool,
        hex: bool,
    ) -> Self {
        let mut shown = [false; Kind::ALL.len()];
        for kind in kinds {
            shown[*kind as usize] = true;
        }
        let mut entries = Vec::new();
        let mut hidden = BTreeMap::new();
        for entry in in_window(timeline, from, to) {
            let left_out = if !shown[entry.kind as usize] {
                entry.kind.name()
            } else if entry.repeat && !all {
                "repeats"
            } else {
                entries.push(entry);
                continue;
            };
            *hidden.entry(left_out).or_default() += 1;
        }
        Self {
            from_ms: from,
            to_ms: (to != u64::MAX).then_some(to),
            entries,
            hidden,
            hex,
        }
    }
}

impl fmt::Display for Listing<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for entry in &self.entries {
            if self.hex {
                writeln!(f, "{entry:#}")?;
            } else {
                writeln!(f, "{entry}")?;
            }
        }
        if self.hidden.is_empty() {
            return Ok(());
        }
        f.write_str("hidden: ")?;
        for (i, (kind, n)) in self.hidden.iter().enumerate() {
            write!(f, "{}{kind} {n}", if i == 0 { "" } else { ", " })?;
        }
        writeln!(f, " (--kinds all, --all)")
    }
}

pub fn in_window(timeline: &Timeline, from: u64, to: u64) -> impl Iterator<Item = &Entry> {
    timeline
        .entries
        .iter()
        .filter(move |e| (from..=to).contains(&e.at.session_ms))
}

/// `cycling trace find`: the first answers, and how many there were.
#[derive(Serialize)]
pub struct Answers<'a> {
    found: Vec<Found<'a>>,
    total: usize,
    #[serde(skip)]
    hex: bool,
}

impl<'a> Answers<'a> {
    pub fn of(mut found: Vec<Found<'a>>, limit: usize, hex: bool) -> Self {
        let total = found.len();
        found.truncate(limit);
        Self { found, total, hex }
    }
}

impl fmt::Display for Answers<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for found in &self.found {
            if self.hex {
                writeln!(f, "{found:#}")?;
            } else {
                writeln!(f, "{found}")?;
            }
        }
        match self.total - self.found.len() {
            _ if self.total == 0 => writeln!(f, "found nothing"),
            0 => Ok(()),
            more => writeln!(f, "... {more} more (--limit)"),
        }
    }
}

/// `cycling trace otlp`: two OTLP/JSON requests, one per line, or in JSON
/// an object holding both.
pub struct Otlp(pub String);

impl fmt::Display for Otlp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl Serialize for Otlp {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::{Error, SerializeStruct};
        let mut requests = self
            .0
            .lines()
            .map(serde_json::from_str::<serde_json::Value>);
        let mut next = || {
            requests
                .next()
                .ok_or_else(|| S::Error::custom("the export holds two requests"))?
                .map_err(|e| S::Error::custom(format!("the export is no JSON: {e}")))
        };
        let (traces, metrics) = (next()?, next()?);
        let mut both = serializer.serialize_struct("Otlp", 2)?;
        both.serialize_field("metrics", &metrics)?;
        both.serialize_field("traces", &traces)?;
        both.end()
    }
}

/// A file a command wrote: `otlp --out`.
#[derive(Serialize)]
pub struct Written {
    pub out: PathBuf,
}

impl fmt::Display for Written {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.out.display())
    }
}

/// `cycling trace test-stub`: the test, after the commands that make the
/// trace a fixture when it is none yet.
#[derive(Serialize)]
pub struct Stub {
    pub fixture_steps: Option<String>,
    pub test: String,
}

impl fmt::Display for Stub {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(steps) = &self.fixture_steps {
            writeln!(f, "{steps}")?;
        }
        f.write_str(&self.test)
    }
}

/// `cycling replay`.
#[derive(Serialize)]
pub struct Replayed<'a> {
    pub header: Header<'a>,
    pub mode: &'static str,
    pub why: String,
    pub from_ms: u64,
    pub to_ms: u64,
    pub reached: Moment,
    pub trace_ended: bool,
    pub divergence: Option<Divergences>,
    pub artifacts: Artifacts,
}

#[derive(Serialize)]
pub struct Artifacts {
    pub dir: PathBuf,
    pub files: Vec<PathBuf>,
}

impl fmt::Display for Replayed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.header)?;
        writeln!(f, "mode {}: {}", self.mode, self.why)?;
        writeln!(
            f,
            "replayed {} from {} to {}; reached {}{}",
            self.mode,
            session_clock(self.from_ms),
            session_clock(self.to_ms),
            self.reached,
            if self.trace_ended {
                " (the trace ended)"
            } else {
                ""
            }
        )?;
        f.write_str("divergence: ")?;
        if let Some(divergence) = &self.divergence {
            write!(f, "{divergence}")?;
        }
        writeln!(f, "artifacts in {}:", self.artifacts.dir.display())?;
        for file in &self.artifacts.files {
            writeln!(f, "  {}", file.display())?;
        }
        writeln!(
            f,
            "screens.txt has the screen text at each screenshot; replayed.trace is this replay's own trace (diff it with timeline)"
        )
    }
}
