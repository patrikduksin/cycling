//! A recorded trace standing in for the world, and how a replay parted
//! from it.
//!
//! Every replay runs on the twin (`crate::twin`), which answers each call
//! from the trace and stops where the build leaves the recorded world. The
//! mode is only the verdict: a faithful replay runs the build that recorded
//! the trace, so any difference in what it records again is a recording
//! bug; a changed replay runs another build, and fails only where it
//! stops. Either way the replay keeps a [`Report`] of where it parted from
//! the recording.
use firmware_services::trace::recorder::Sink;
use firmware_services::trace::{self, End, Event};
use std::collections::BTreeMap;
use std::fmt;

/// Sealed trace blocks, back to back.
#[derive(Default)]
pub struct Blocks(pub Vec<u8>);

impl Sink for Blocks {
    fn block(&mut self, bytes: &[u8]) -> bool {
        self.0.extend_from_slice(bytes);
        true
    }
}

/// What the trace holds next.
pub enum Next {
    Pass,
    Command(String),
    End,
}

/// The verdict a replay gives.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Mode {
    /// The build that recorded the trace: anything it records otherwise
    /// than the trace did is a recording bug.
    #[default]
    Faithful,
    /// Another build: it fails only where it leaves the recorded world,
    /// panics or breaks an invariant. What it records otherwise is what the
    /// change did.
    Changed,
}

impl Mode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Faithful => "faithful",
            Self::Changed => "changed",
        }
    }
}

/// How a replay runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Options {
    pub mode: Mode,
    /// Go on past a stop, on a branch labelled with the stop's kind.
    pub model: bool,
}

/// A way a replay parted from its recording.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    /// VANA or the shell read an input the trace holds no value for.
    Unrecorded,
    /// `--model` answered a call with the nearest recorded answer.
    Modelled,
    /// A recorded call the pass did not make.
    LeftOver,
    /// `--model` dropped a recorded call the pass did not make, and kept
    /// what it observed.
    NotRequested,
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
    /// VANA panicked, or broke an invariant.
    Panicked,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unrecorded => "Unrecorded",
            Self::Modelled => "Modelled",
            Self::LeftOver => "LeftOver",
            Self::NotRequested => "NotRequested",
            Self::Seen => "Seen",
            Self::Redacted => "Redacted",
            Self::Rerecorded => "Rerecorded",
            Self::Unreplayable => "Unreplayable",
            Self::Truncated => "Truncated",
            Self::Lost => "Lost",
            Self::Panicked => "Panicked",
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
    /// What the replay proved by recording the world again.
    pub faithful: Option<Faithful>,
    /// Where it left the recorded world, or VANA panicked: the replay
    /// ended there.
    pub stopped: Option<Divergence>,
    /// Where `--model` first went on past a stop: every later pass is on
    /// that branch.
    pub branched: Option<Divergence>,
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
    /// Accesses the trace does not record: the settings region.
    Outside(u32),
    /// The recording lost world records here: nothing after is replayed.
    Lost { pass_ms: u64 },
}

impl fmt::Display for Exclusion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RedactedFrames(frames) => write!(
                f,
                "{frames} frames of the phone page, whose credentials the trace redacted"
            ),
            Self::RefusedFrames(frames) => {
                write!(f, "{frames} frames the panel refused, replayed as refused")
            }
            Self::SettingsJournal(how) => write!(
                f,
                "the settings journal ({how}), replayed as the record the device decoded"
            ),
            Self::TerrainByDigest(reads) => write!(
                f,
                "{reads} TERRAIN reads, served from the pack and checked by digest"
            ),
            Self::Outside(accesses) => write!(
                f,
                "{accesses} accesses outside the recorded world (the settings region)"
            ),
            Self::Lost { pass_ms } => write!(
                f,
                "everything from {pass_ms} ms, where the recording lost world records"
            ),
        }
    }
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
            writeln!(f, "excluded: {exclusion}")?;
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
            stopped: None,
            branched: None,
            now: 0,
        }
    }

    /// What answered VANA's reads.
    pub fn header(&self) -> &'static str {
        match self.mode {
            Mode::Faithful => {
                "faithful replay: every input came from the trace's world records, and anything \
                 recorded otherwise is a recording bug"
            }
            Mode::Changed => {
                "changed replay: every input came from the trace's world records; it fails only \
                 where this build leaves the recorded world"
            }
        }
    }

    /// The verdict: why it holds, or why it fails.
    pub fn verdict(&self) -> Result<String, String> {
        let at = |divergence: &Divergence| {
            format!(
                "at {}: {}",
                crate::trace::timeline::session_clock(divergence.at_ms),
                divergence.detail
            )
        };
        if let Some(stopped) = &self.stopped {
            return Err(at(stopped));
        }
        if let Some(branched) = &self.branched {
            return Err(format!("branched by --model {}", at(branched)));
        }
        let ended = self
            .faithful
            .as_ref()
            .and_then(|faithful| faithful.ended.as_ref());
        let diverged = (self.counts.iter()).any(|(kind, n)| *kind != Kind::Redacted && *n > 0);
        match self.mode {
            Mode::Faithful if diverged => Err(match &self.first {
                Some(first) => format!("a recording bug {}", at(first)),
                None => "a recording bug before the window".into(),
            }),
            Mode::Faithful => Ok("the replay re-recorded the trace byte for byte".into()),
            Mode::Changed => match ended {
                Some((pass_ms, why)) => Ok(format!(
                    "stayed in the recorded world; first difference at {}: {why}",
                    crate::trace::timeline::session_clock(*pass_ms)
                )),
                None => Ok("the replay re-recorded the trace byte for byte".into()),
            },
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

/// The first event where two traces part, with the pass it belongs to.
/// What [`measured`] holds is left out.
pub fn first_difference(recorded: &[u8], replayed: &[u8]) -> Option<String> {
    first_difference_of(recorded, replayed, |event| !uncompared(event))
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

pub(crate) fn uncompared(event: &Event<'_>) -> bool {
    matches!(
        event,
        Event::Horizon { .. }
            | Event::Log { .. }
            | Event::FrameTime(_)
            | Event::Health(_)
            | Event::Activity(_)
            | Event::Task { .. }
            | Event::Stage { .. }
            | Event::Synthetic(_)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use firmware_services::trace::Header;
    use firmware_services::trace::log::Log;
    use firmware_services::trace::media::Memory;
    use firmware_services::trace::recorder::Recorder;
    use std::cell::RefCell;

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
}
