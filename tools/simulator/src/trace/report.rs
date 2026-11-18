//! What each `cycling trace` command prints: one value per command, which
//! writes itself as text (`Display`) and as JSON (`Serialize`) from the same
//! fields, so the two cannot tell different stories.
use crate::bundle::Bundled;
use crate::replay::Mode;
use crate::trace::battery;
use crate::trace::divergence::Divergences;
use crate::trace::find::Found;
use crate::trace::story::{Point, Span, tell};
use crate::trace::timeline::{
    Before, Entry, Kind, Moment, SILENCE_MS, Silence, Timeline, duration, session_clock,
};
use firmware_services::trace::{self, End};
use firmware_services::why;
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
    terrain: Option<Pack>,
}

#[derive(Serialize)]
struct Pack {
    sha256: String,
    length: u64,
    ready: bool,
}

fn end_name<S: Serializer>(end: &End, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(end.name())
}

fn mode_name<S: Serializer>(mode: &Mode, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(mode.name())
}

impl<'a> Header<'a> {
    pub fn of(path: &Path, timeline: &'a Timeline) -> Self {
        Self {
            trace: path.display().to_string(),
            build: (!timeline.build.is_empty()).then_some(timeline.build.as_str()),
            boot: timeline.boot,
            format: trace::VERSION.into(),
            end_ms: timeline.end_ms,
            passes: timeline.passes,
            end: timeline.end,
            terrain: match timeline.terrain {
                trace::origin::Found::Read(pack) => Some(Pack {
                    sha256: pack.digest.iter().map(|b| format!("{b:02x}")).collect(),
                    length: pack.length,
                    ready: pack.ready,
                }),
                trace::origin::Found::Absent | trace::origin::Found::Unreadable(_) => None,
            },
        }
    }
}

impl fmt::Display for Header<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "trace {}", self.trace)?;
        writeln!(
            f,
            "build {}, boot {}, format {}, {} over {} passes, {}{}",
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
        match &self.terrain {
            Some(pack) => writeln!(
                f,
                "terrain pack sha256 {}, {} bytes{}",
                pack.sha256,
                pack.length,
                if pack.ready { "" } else { ", not ready" }
            ),
            None => Ok(()),
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
    pub fn of(path: &Path, timeline: &'a Timeline) -> Self {
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
            header: Header::of(path, timeline),
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
    pub model: bool,
    pub reached: Moment,
    pub trace_ended: bool,
    pub divergence: Option<Divergences>,
    /// What the replay proved by recording the world again.
    pub faithful: Option<Proved>,
    pub artifacts: Artifacts,
    /// A faithful replay's first divergence from the start of the trace, in
    /// the window or before it: what a bundle of this failure expects.
    /// `None` under --model or in a changed replay.
    pub parted: Option<Parted>,
    pub bundle: Option<Bundled>,
}

#[derive(Serialize)]
pub struct Parted {
    pub kind: &'static str,
    pub at_ms: u64,
    pub detail: String,
}

/// [`crate::replay::Faithful`] for a script to read.
#[derive(Serialize)]
pub struct Proved {
    pub ok: bool,
    pub physical: u64,
    pub semantic: u64,
    /// Where the replay ended before its trace, or first recorded
    /// otherwise, and why.
    pub ended: Option<String>,
    pub exclusions: Vec<String>,
}

impl Proved {
    pub fn of(ok: bool, faithful: &crate::replay::Faithful) -> Self {
        Self {
            ok,
            physical: faithful.physical,
            semantic: faithful.semantic,
            ended: (faithful.ended.as_ref())
                .map(|(pass_ms, why)| format!("{}: {why}", session_clock(*pass_ms))),
            exclusions: faithful
                .exclusions
                .iter()
                .map(ToString::to_string)
                .collect(),
        }
    }
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
            "replayed {}{} from {} to {}; reached {}{}",
            self.mode.name(),
            if self.model { " --model" } else { "" },
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
        if let Some(bundle) = &self.bundle {
            writeln!(f, "{bundle}")?;
        }
        writeln!(
            f,
            "screens.txt has the screen text at each screenshot; why.txt what the glue decided and why; replayed.trace is this replay's own trace (diff it with timeline)"
        )
    }
}

/// `cycling trace diff`: where two traces part.
#[derive(Serialize)]
pub struct Diffed<'a> {
    pub a: &'a str,
    pub b: &'a str,
    #[serde(flatten)]
    pub diff: crate::trace::diff::Diff,
}

impl fmt::Display for Diffed<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "a {}\nb {}", self.a, self.b)?;
        write!(f, "{}", self.diff)
    }
}

/// A `--set` and the pass it resolved to.
#[derive(Serialize)]
pub struct Resolved {
    pub set: String,
    pub at: Moment,
}

/// `cycling replay --set`: the recorded and the altered future, and where
/// they part.
#[derive(Serialize)]
pub struct Futures<'a> {
    pub sets: Vec<Resolved>,
    pub recorded: Replayed<'a>,
    pub set: Replayed<'a>,
    pub diff: crate::trace::diff::Diff,
}

impl fmt::Display for Futures<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for resolved in &self.sets {
            writeln!(
                f,
                "--set {} resolved to the pass at {}",
                resolved.set, resolved.at
            )?;
        }
        for (name, future) in [("recorded", &self.recorded), ("set", &self.set)] {
            let verdict = match &future.divergence {
                Some(divergence) => format!(
                    "{}: {}",
                    if divergence.ok { "holds" } else { "fails" },
                    divergence.verdict
                ),
                None => "no report".into(),
            };
            writeln!(
                f,
                "{name}: reached {}; verdict {verdict}; artifacts in {}",
                future.reached,
                future.artifacts.dir.display()
            )?;
        }
        write!(f, "{}", self.diff)
    }
}

#[derive(Serialize)]
pub struct Explained<'a> {
    pub header: Header<'a>,
    #[serde(serialize_with = "mode_name")]
    pub mode: Mode,
    pub why: String,
    pub at_ms: u64,
    pub from_ms: u64,
    pub to_ms: u64,
    pub reached: Moment,
    pub trace_ended: bool,
    pub divergence: Option<Divergences>,
    pub entries: Vec<Said>,
    pub hidden: usize,
    pub in_force: Vec<Said>,
}

pub struct Asking<'a> {
    pub at_ms: u64,
    pub from_ms: u64,
    pub to_ms: u64,
    pub owners: Vec<&'a str>,
    pub limit: usize,
    pub every_delta: bool,
}

const MAX_CHANGES_LISTED_SINGLY: usize = 3;

pub struct Kept {
    pub kept: Vec<Said>,
    pub hidden: usize,
    pub in_force: Vec<Said>,
}

impl Asking<'_> {
    fn owns(&self, entry: &why::Entry) -> bool {
        let named = |owner: &str| match entry {
            why::Entry::Decision(decision) => decision
                .tag
                .strip_prefix(owner)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with('.')),
            why::Entry::Delta(delta) => delta.owner == owner,
        };
        self.owners.is_empty() || self.owners.iter().any(|owner| named(owner))
    }

    pub fn keep(&self, timeline: &Timeline, entries: &[why::Entry]) -> Kept {
        let owned = || entries.iter().filter(|entry| self.owns(entry));
        let window: Vec<&why::Entry> = owned()
            .filter(|entry| (self.from_ms..=self.to_ms).contains(&entry.at_ms()))
            .collect();
        let window = self.collapse(timeline, &window);
        let mut nearest: Vec<usize> = (0..window.len()).collect();
        nearest.sort_by_key(|&i| (window[i].at.session_ms.abs_diff(self.at_ms), i));
        nearest.truncate(self.limit);
        nearest.sort_unstable();
        let hidden = window.len() - nearest.len();
        let mut window: Vec<Option<Said>> = window.into_iter().map(Some).collect();
        let kept = nearest.iter().filter_map(|&i| window[i].take()).collect();
        let mut latest = BTreeMap::new();
        for (noted, entry) in owned().enumerate() {
            if let why::Entry::Decision(decision) = entry
                && decision.at_ms <= self.at_ms
            {
                latest.insert(decision.tag, (noted, entry));
            }
        }
        let mut in_force: Vec<(&str, (usize, &why::Entry))> = latest.into_iter().collect();
        in_force.sort_by_key(|(tag, (noted, _))| (tag.split('.').next(), *noted));
        let in_force = (in_force.into_iter())
            .map(|(_, (_, entry))| Said::of(timeline, entry))
            .collect();
        Kept {
            kept,
            hidden,
            in_force,
        }
    }

    fn collapse(&self, timeline: &Timeline, window: &[&why::Entry]) -> Vec<Said> {
        let mut changes: BTreeMap<(&str, &str), Vec<&why::Delta>> = BTreeMap::new();
        for entry in window {
            if let why::Entry::Delta(delta) = entry {
                changes
                    .entry((delta.owner, delta.field))
                    .or_default()
                    .push(delta);
            }
        }
        let churns = |delta: &why::Delta| {
            !self.every_delta
                && changes[&(delta.owner, delta.field)].len() > MAX_CHANGES_LISTED_SINGLY
        };
        let mut said = Vec::new();
        for entry in window {
            let why::Entry::Delta(delta) = entry else {
                said.push(Said::of(timeline, entry));
                continue;
            };
            if !churns(delta) {
                said.push(Said::of(timeline, entry));
                continue;
            }
            let all = &changes[&(delta.owner, delta.field)];
            if !core::ptr::eq(all[0], delta) {
                continue;
            }
            said.push(Said {
                at: timeline.moment(delta.at_ms),
                what: What::Churn {
                    owner: delta.owner,
                    field: delta.field,
                    first: delta.old.clone(),
                    last: all[all.len() - 1].new.clone(),
                    passes: all.len(),
                },
            });
        }
        said
    }
}

#[derive(Serialize)]
pub struct Said {
    pub at: Moment,
    #[serde(flatten)]
    pub what: What,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum What {
    Decision {
        tag: &'static str,
        #[serde(serialize_with = "as_map")]
        operands: Vec<(&'static str, String)>,
    },
    Delta {
        owner: &'static str,
        field: &'static str,
        old: Option<String>,
        new: Option<String>,
    },
    Churn {
        owner: &'static str,
        field: &'static str,
        first: Option<String>,
        last: Option<String>,
        passes: usize,
    },
}

fn as_map<S: Serializer>(pairs: &[(&str, String)], serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_map(pairs.iter().map(|(name, value)| (name, value)))
}

impl Said {
    pub fn of(timeline: &Timeline, entry: &why::Entry) -> Self {
        let what = match entry {
            why::Entry::Decision(decision) => What::Decision {
                tag: decision.tag,
                operands: decision.operands.clone(),
            },
            why::Entry::Delta(delta) => What::Delta {
                owner: delta.owner,
                field: delta.field,
                old: delta.old.clone(),
                new: delta.new.clone(),
            },
        };
        Self {
            at: timeline.moment(entry.at_ms()),
            what,
        }
    }
}

const SHOWN_CHARS: usize = 96;

fn clipped(value: &str) -> std::borrow::Cow<'_, str> {
    match value.char_indices().nth(SHOWN_CHARS) {
        Some((end, _)) => format!("{}…", &value[..end]).into(),
        None => value.into(),
    }
}

fn shown(value: &Option<String>) -> std::borrow::Cow<'_, str> {
    match value {
        Some(value) => clipped(value),
        None => "(not shown)".into(),
    }
}

impl fmt::Display for Said {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}  ", self.at)?;
        match &self.what {
            What::Decision { tag, operands } => {
                f.write_str(tag)?;
                for (i, (name, value)) in operands.iter().enumerate() {
                    let gap = if i == 0 { "  " } else { " " };
                    write!(f, "{gap}{name}={}", clipped(value))?;
                }
                Ok(())
            }
            What::Delta {
                owner,
                field,
                old,
                new,
            } => write!(f, "Δ {owner}.{field}  {} -> {}", shown(old), shown(new)),
            What::Churn {
                owner,
                field,
                first,
                last,
                passes,
            } => write!(
                f,
                "Δ {owner}.{field}  changed in {passes} passes: {} -> {}",
                shown(first),
                shown(last)
            ),
        }
    }
}

impl fmt::Display for Explained<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.header)?;
        writeln!(f, "mode {}: {}", self.mode.name(), self.why)?;
        writeln!(
            f,
            "why at {}: replayed {} from power-on, decisions and deltas from {} to {}; reached {}{}",
            session_clock(self.at_ms),
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
        for said in &self.entries {
            writeln!(f, "{said}")?;
        }
        if self.hidden > 0 {
            writeln!(
                f,
                "{} entries farther from {} left out; --limit all shows them",
                self.hidden,
                session_clock(self.at_ms)
            )?;
        }
        writeln!(f, "in force at {}:", session_clock(self.at_ms))?;
        for said in &self.in_force {
            writeln!(f, "  {said}")?;
        }
        Ok(())
    }
}
