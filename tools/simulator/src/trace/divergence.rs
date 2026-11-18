//! A replay's divergences in the window an agent asked for, sorted by what
//! they mean for the rider, so a clean window does not read as a replay
//! fault.
use crate::replay::{Kind, Mode, Report};
use crate::trace::timeline::{Moment, Timeline, session_clock};
use serde::{Serialize, Serializer};
use std::collections::BTreeMap;
use std::fmt;

/// What a divergence says about the replay.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Meaning {
    /// The rider saw or heard something else than in the recording.
    RiderVisible,
    /// VANA panicked or broke an invariant.
    Broken,
    /// The simulator could not replay a recorded event, or the build left
    /// the recorded world.
    ReplayFault,
    /// `--model` answered where the recording held nothing.
    Modelled,
    /// VANA asked other than the recording did.
    CallLevel,
    /// The phone page's pixels, where the trace redacted the credentials
    /// it draws.
    Redacted,
}

impl Meaning {
    pub fn of(kind: Kind) -> Self {
        match kind {
            Kind::Seen => Self::RiderVisible,
            Kind::Panicked => Self::Broken,
            Kind::Redacted => Self::Redacted,
            Kind::Unrecorded | Kind::Unreplayable | Kind::Truncated | Kind::Lost => {
                Self::ReplayFault
            }
            Kind::Modelled | Kind::NotRequested => Self::Modelled,
            Kind::LeftOver | Kind::Rerecorded => Self::CallLevel,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RiderVisible => "rider-visible",
            Self::Broken => "broken",
            Self::ReplayFault => "replay fault",
            Self::Modelled => "modelled",
            Self::CallLevel => "call-level",
            Self::Redacted => "redacted",
        }
    }

    fn explain(self) -> &'static str {
        match self {
            Self::RiderVisible => {
                "the rider saw or heard something else; compare screens.txt with timeline.txt"
            }
            Self::Broken => "VANA panicked or broke an invariant",
            Self::ReplayFault => {
                "the trace held no value for an input VANA read, or an event the simulator cannot replay"
            }
            Self::Modelled => {
                "--model answered with the nearest recorded value where the trace held none; the replay is on that branch"
            }
            Self::CallLevel => {
                "VANA asked other than the recording: a VANA change, or a recording bug on the same build"
            }
            Self::Redacted => {
                "the phone page drew other credentials than the redacted trace holds; expected"
            }
        }
    }
}

/// The report's counts before the window opened.
pub type Counts = BTreeMap<Kind, u32>;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Divergences {
    /// The replay document names the mode once, at its top.
    #[serde(skip)]
    pub mode: Mode,
    pub from_ms: u64,
    pub to_ms: u64,
    pub meanings: Vec<Meant>,
    pub first: Option<First>,
    #[serde(serialize_with = "by_kind")]
    pub before: Counts,
    /// What the rider saw, by the window's divergences.
    pub seen: &'static str,
    /// The mode's verdict over the whole replay.
    pub ok: bool,
    pub verdict: String,
    /// What a replay proved by recording the world again, and what
    /// that leaves out, a line each.
    pub guarantees: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Meant {
    pub meaning: Meaning,
    pub total: u32,
    #[serde(serialize_with = "by_kind")]
    pub kinds: Counts,
    pub explain: &'static str,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct First {
    pub at: Moment,
    pub meaning: Meaning,
    #[serde(serialize_with = "kind_name")]
    pub kind: Kind,
    pub detail: String,
}

impl Serialize for Meaning {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.label())
    }
}

fn by_kind<S: Serializer>(counts: &Counts, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.collect_map(
        counts
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(kind, n)| (kind.name(), n)),
    )
}

fn kind_name<S: Serializer>(kind: &Kind, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(kind.name())
}

/// The divergences from `from` to `to`, by meaning, the first of them, and
/// how many came before `from`. `before` is the report's counts at `from`,
/// taken when its `first` was cleared.
pub fn summarize(
    timeline: &Timeline,
    (from, to): (u64, u64),
    before: &Counts,
    report: &Report,
) -> Divergences {
    let mode = report.mode;
    let mut by_meaning: BTreeMap<Meaning, Counts> = BTreeMap::new();
    for (&kind, &count) in &report.counts {
        let inside = count - before.get(&kind).copied().unwrap_or(0);
        if inside > 0 {
            by_meaning
                .entry(Meaning::of(kind))
                .or_default()
                .insert(kind, inside);
        }
    }
    let seen = if by_meaning.contains_key(&Meaning::Broken) {
        "VANA panicked or broke an invariant"
    } else if by_meaning.contains_key(&Meaning::RiderVisible) {
        "the rider saw something else than the recording in this window"
    } else if by_meaning.contains_key(&Meaning::ReplayFault) {
        "the rider saw what the recording saw, but the replay lacked recorded input or skipped events"
    } else if by_meaning.contains_key(&Meaning::CallLevel) {
        "the rider saw what the recording saw; VANA asked differently"
    } else if by_meaning.is_empty() {
        "the replay matched the recording"
    } else {
        "the rider saw what the recording saw; nothing here is a replay fault"
    };
    let (ok, verdict) = match report.verdict() {
        Ok(verdict) => (true, verdict),
        Err(verdict) => (false, verdict),
    };
    Divergences {
        mode,
        from_ms: from,
        to_ms: to,
        meanings: by_meaning
            .into_iter()
            .map(|(meaning, kinds)| Meant {
                meaning,
                total: kinds.values().sum(),
                kinds,
                explain: meaning.explain(),
            })
            .collect(),
        first: report.first.as_ref().map(|first| First {
            at: timeline.moment(first.at_ms),
            meaning: Meaning::of(first.kind),
            kind: first.kind,
            detail: first.detail.clone(),
        }),
        before: before.clone(),
        seen,
        ok,
        verdict,
        guarantees: (report.faithful.iter())
            .flat_map(|faithful| {
                faithful
                    .to_string()
                    .lines()
                    .map(str::to_string)
                    .collect::<Vec<_>>()
            })
            .collect(),
    }
}

impl fmt::Display for Divergences {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "in {}-{} ({}):",
            session_clock(self.from_ms),
            session_clock(self.to_ms),
            self.mode.name()
        )?;
        if !self
            .meanings
            .iter()
            .any(|meant| meant.meaning == Meaning::RiderVisible)
        {
            f.write_str("  rider-visible: none\n")?;
        }
        for meant in &self.meanings {
            writeln!(
                f,
                "  {}: {} ({}): {}",
                meant.meaning.label(),
                meant.total,
                counts_text(&meant.kinds),
                meant.explain
            )?;
        }
        if let Some(first) = &self.first {
            writeln!(
                f,
                "  first: {} {}, {}: {}",
                first.at,
                first.meaning.label(),
                first.kind.name(),
                first.detail
            )?;
        }
        let earlier: u32 = self.before.values().sum();
        writeln!(
            f,
            "before {}: {}",
            session_clock(self.from_ms),
            match earlier {
                0 => "none".to_string(),
                _ => format!("{earlier} ({})", counts_text(&self.before)),
            }
        )?;
        writeln!(f, "seen: {}", self.seen)?;
        let held = if self.ok { "holds" } else { "fails" };
        writeln!(
            f,
            "verdict ({}): {held}: {}",
            self.mode.name(),
            self.verdict
        )?;
        self.guarantees
            .iter()
            .try_for_each(|line| writeln!(f, "{line}"))
    }
}

fn counts_text(counts: &Counts) -> String {
    counts
        .iter()
        .filter(|(_, n)| **n > 0)
        .map(|(kind, n)| format!("{} {n}", kind.name()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::replay::Divergence;

    fn timeline() -> Timeline {
        Timeline {
            build: String::new(),
            boot: 1,
            terrain: firmware_services::trace::origin::Found::Absent,
            entries: Vec::new(),
            clock: Vec::new(),
            passes: 0,
            end_ms: 0,
            end: firmware_services::trace::End::Complete,
            utc_at_boot_ms: None,
            silences: Vec::new(),
            power: Vec::new(),
            health: Vec::new(),
            frame_times: Vec::new(),
            activity: Vec::new(),
            before: Default::default(),
        }
    }

    #[test]
    fn only_the_window_counts_and_a_modelled_branch_fails_the_verdict() {
        let before = Counts::from([(Kind::NotRequested, 1)]);
        let mut report = Report::new(Mode::Changed);
        let first = Divergence {
            at_ms: 2_000,
            kind: Kind::NotRequested,
            detail: "the recording asked Scan".into(),
        };
        report.first = Some(first.clone());
        report.branched = Some(first);
        report.counts = Counts::from([(Kind::NotRequested, 3), (Kind::Rerecorded, 1)]);
        let summary = summarize(&timeline(), (1_000, 5_000), &before, &report);
        let text = summary.to_string();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "  rider-visible: none");
        assert!(
            lines[2].starts_with("  modelled: 2 (NotRequested 2): "),
            "{text}"
        );
        assert!(
            lines[3].starts_with("  call-level: 1 (Rerecorded 1): "),
            "{text}"
        );
        assert!(lines[4].starts_with("  first: 00:02.000 ride --:-- modelled, NotRequested"));
        assert_eq!(lines[5], "before 00:01.000: 1 (NotRequested 1)");
        assert_eq!(
            lines[7],
            "verdict (changed): fails: branched by --model at 00:02.000: the recording asked Scan"
        );
        assert!(!summary.ok);
        let json = serde_json::to_value(&summary).unwrap();
        assert_eq!(json["first"]["kind"], "NotRequested");
        assert_eq!(json["meanings"][0]["kinds"]["NotRequested"], 2);
        assert_eq!(json["before"]["NotRequested"], 1);
        assert_eq!(json["ok"], false);

        let mut seen = Report::new(Mode::Faithful);
        seen.counts = Counts::from([(Kind::Seen, 1)]);
        let summary = summarize(&timeline(), (0, 5_000), &Counts::new(), &seen);
        let text = summary.to_string();
        assert!(text.contains("  rider-visible: 1 (Seen 1)"), "{text}");
        assert!(!text.contains("rider-visible: none"), "{text}");
        assert!(
            !summary.ok,
            "a faithful replay that saw otherwise is a recording bug"
        );
    }

    #[test]
    fn a_changed_replay_that_records_otherwise_without_stopping_holds() {
        let mut report = Report::new(Mode::Changed);
        report.counts = Counts::from([(Kind::Rerecorded, 1)]);
        report.faithful = Some(crate::replay::Faithful {
            ended: Some((3_000, "pass at 3000 ms: recorded A, replayed B".into())),
            ..Default::default()
        });
        let summary = summarize(&timeline(), (0, 5_000), &Counts::new(), &report);
        assert!(summary.ok, "{summary}");
        assert_eq!(
            summary.verdict,
            "stayed in the recorded world; first difference at 00:03.000: pass at 3000 ms: recorded A, replayed B"
        );
    }
}
