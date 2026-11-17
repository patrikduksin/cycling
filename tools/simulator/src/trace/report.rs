//! What each `cycling trace` command prints: one value per command, which
//! writes itself as text (`Display`) and as JSON (`Serialize`) from the same
//! fields, so the two cannot tell different stories.
use crate::replay::Mode;
use crate::trace::battery;
use crate::trace::divergence::Divergences;
use crate::trace::file::Media;
use crate::trace::find::Found;
use crate::trace::story::{Point, Span, tell};
use crate::trace::timeline::{
    Before, Entry, Kind, Moment, SILENCE_MS, Silence, Timeline, duration, session_clock,
};
use firmware_services::trace::{self, End};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};

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

#[derive(Serialize)]
pub struct Header<'a> {
    trace: String,
    build: Option<&'a str>,
    boot: u32,
    format: u16,
    end_ms: u64,
    passes: u64,
    #[serde(serialize_with = "end_name")]
    end: End,
    #[serde(serialize_with = "media_name")]
    media: &'a Media,
}

fn end_name<S: Serializer>(end: &End, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(end.name())
}

fn mode_name<S: Serializer>(mode: &Mode, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(mode.name())
}

fn media_name<S: Serializer>(media: &&Media, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(media.name())
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
        }
    }
}

impl fmt::Display for Header<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "trace {}", self.trace)?;
        write!(
            f,
            "build {}, boot {}, format {}, {} over {} passes, {}{}; media ",
            self.build.unwrap_or("unknown"),
            self.boot,
            self.format,
            duration(self.end_ms),
            self.passes,
            self.end.name(),
            match self.end {
                End::Complete => "",
                End::Torn => ": the last block was cut short",
                End::Corrupt => ": a damaged block and everything after it were dropped",
            },
        )?;
        match self.media {
            Media::Saved => writeln!(f, "saved beside it (.origin)"),
            Media::Blank => writeln!(
                f,
                "not saved: a strict replay reads them from the trace, a stimulus replay starts on blank media"
            ),
        }
    }
}

#[derive(Serialize)]
pub struct Summary<'a> {
    header: Header<'a>,
    before: &'a Before,
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
        let mut counts = [(0, 0); Kind::ALL.len()];
        for entry in &timeline.entries {
            let (new, repeats) = &mut counts[entry.kind as usize];
            *if entry.repeat { repeats } else { new } += 1;
        }
        let entries = Kind::ALL
            .into_iter()
            .zip(counts)
            .filter(|(_, (new, repeats))| new + repeats > 0)
            .map(|(kind, (new, repeats))| (kind, Count { new, repeats }))
            .collect();
        Self {
            header: Header::of(path, timeline, media),
            before: &timeline.before,
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
        if self.before.horizon.is_some() || self.before.salvaged.is_some() {
            writeln!(f, "{}", self.before)?;
        }
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
        let mut hidden = [0; Kind::ALL.len()];
        let mut repeats = 0;
        for entry in in_window(timeline, from, to) {
            if !shown[entry.kind as usize] {
                hidden[entry.kind as usize] += 1;
            } else if entry.repeat && !all {
                repeats += 1;
            } else {
                entries.push(entry);
            }
        }
        Self {
            from_ms: from,
            to_ms: (to != u64::MAX).then_some(to),
            entries,
            hidden: Kind::ALL
                .iter()
                .map(|kind| (kind.name(), hidden[*kind as usize]))
                .chain([("repeats", repeats)])
                .filter(|(_, n)| *n > 0)
                .collect(),
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

#[derive(Serialize)]
pub struct Written {
    pub out: PathBuf,
}

impl fmt::Display for Written {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.out.display())
    }
}

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

#[derive(Serialize)]
pub struct Replayed<'a> {
    pub header: Header<'a>,
    #[serde(serialize_with = "mode_name")]
    pub mode: Mode,
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
        writeln!(f, "mode {}: {}", self.mode.name(), self.why)?;
        writeln!(
            f,
            "replayed {} from {} to {}; reached {}{}",
            self.mode.name(),
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
