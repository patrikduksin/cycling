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
    /// The simulator could not replay a recorded event.
    ReplayFault,
    /// Strict: VANA asked other than the recording did.
    CallLevel,
    /// Stimulus: the ANT model or fresh entropy answered where the
    /// recording's radio did; nothing the rider sees.
    ModelOnly,
    /// Stimulus: recorded input waited for VANA's next pass.
    Timing,
    /// Strict: the phone page's pixels, where the trace redacted the
    /// credentials it draws.
    Redacted,
}

impl Meaning {
    pub fn of(kind: Kind, mode: Mode) -> Self {
        match kind {
            Kind::Seen => Self::RiderVisible,
            Kind::Redacted => Self::Redacted,
            Kind::Unrecorded | Kind::Unreplayable | Kind::Truncated | Kind::Lost => {
                Self::ReplayFault
            }
            Kind::Modelled
            | Kind::LeftOver
            | Kind::OtherRequest
            | Kind::NotRequested
            | Kind::Refused
            | Kind::NotHeard
            | Kind::Carried
            | Kind::Rerecorded => match (kind, mode) {
                (_, Mode::Strict) => Self::CallLevel,
                (Kind::Carried, Mode::Stimulus) => Self::Timing,
                (_, Mode::Stimulus) => Self::ModelOnly,
            },
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::RiderVisible => "rider-visible",
            Self::ReplayFault => "replay fault",
            Self::CallLevel => "call-level",
            Self::ModelOnly => "model-only",
            Self::Timing => "timing-only",
            Self::Redacted => "redacted",
        }
    }

    fn explain(self) -> &'static str {
        match self {
            Self::RiderVisible => {
                "the rider saw or heard something else; compare screens.txt with timeline.txt"
            }
            Self::ReplayFault => {
                "the trace held no value for an input VANA read, or an event the simulator cannot replay"
            }
            Self::CallLevel => {
                "VANA asked other than the recording: a VANA change, or a replay fault on the same build"
            }
            Self::ModelOnly => {
                "the stimulus ANT model or fresh entropy answered where the recording's radio did; the rider does not see it"
            }
            Self::Timing => "recorded input waited for VANA's next pass",
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
    pub verdict: &'static str,
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
                .entry(Meaning::of(kind, mode))
                .or_default()
                .insert(kind, inside);
        }
    }
    let verdict = if by_meaning.contains_key(&Meaning::RiderVisible) {
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
            meaning: Meaning::of(first.kind, mode),
            kind: first.kind,
            detail: first.detail.clone(),
        }),
        before: before.clone(),
        verdict,
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
        writeln!(f, "verdict: {}", self.verdict)
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
        }
    }

    #[test]
    fn only_the_window_counts_and_model_answers_are_not_faults() {
        let before = Counts::from([(Kind::NotRequested, 1)]);
        let mut report = Report::new(Mode::Stimulus);
        report.first = Some(Divergence {
            at_ms: 2_000,
            kind: Kind::NotRequested,
            detail: "the recording asked Scan".into(),
        });
        report.counts = Counts::from([(Kind::NotRequested, 3), (Kind::Carried, 1)]);
        let text = summarize(&timeline(), (1_000, 5_000), &before, &report).to_string();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines[1], "  rider-visible: none");
        assert!(
            lines[2].starts_with("  model-only: 2 (NotRequested 2): "),
            "{text}"
        );
        assert!(
            lines[3].starts_with("  timing-only: 1 (Carried 1): "),
            "{text}"
        );
        assert!(lines[4].starts_with("  first: 00:02.000 ride --:-- model-only, NotRequested"));
        assert_eq!(lines[5], "before 00:01.000: 1 (NotRequested 1)");
        assert!(
            lines[6].ends_with("nothing here is a replay fault"),
            "{text}"
        );
        let json =
            serde_json::to_value(summarize(&timeline(), (1_000, 5_000), &before, &report)).unwrap();
        assert_eq!(json["first"]["kind"], "NotRequested");
        assert_eq!(json["meanings"][0]["kinds"]["NotRequested"], 2);
        assert_eq!(json["before"]["NotRequested"], 1);

        let mut seen = Report::new(Mode::Strict);
        seen.counts = Counts::from([(Kind::Seen, 1)]);
        let text = summarize(&timeline(), (0, 5_000), &Counts::new(), &seen).to_string();
        assert!(text.contains("  rider-visible: 1 (Seen 1)"), "{text}");
        assert!(!text.contains("rider-visible: none"), "{text}");
    }
}
