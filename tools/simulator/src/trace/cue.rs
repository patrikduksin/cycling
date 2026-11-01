//! Names the cue behind each buzzer pattern a trace heard, from VANA's own
//! cue palette (`app::cues`) on the C606 buzzer's patterns.
//!
//! A trace holds the patterns, one per note, not the cues. A cue's notes
//! follow each other one guard apart, so notes that do are read as one run;
//! a change of page, toast or ride state between two notes may raise a new
//! cue, so it starts a new run. A run is split into the fewest whole cues it
//! spells; a run of one note repeated longer than any cue repeats it is
//! single-note cues ticking, as the radar does. Where several cues spell the
//! same notes, all are named, except that a run raised by such a change is
//! not a radar alarm when another cue fits. A later cue cuts an earlier one
//! short, and the trace can end mid-cue, so a run's last cue may be the
//! start of a longer one; those are named too.
use crate::devices::PATTERNS;
use crate::trace::timeline::{Entry, Kind};
use std::collections::BTreeSet;
use vana::app::{Cue, cue_patterns};

/// A pass's worth of lateness on top of a pattern's guard.
const SLACK_MS: u64 = 30;
/// VANA drops a cue not started this long after its event.
const STALE_MS: u64 = 1_000;

/// One note heard: the entry that printed it, when, and its pattern.
struct Note {
    entry: usize,
    at_ms: u64,
    pattern: u8,
}

/// Notes heard back to back, and how the run began and ended.
struct Run<'a> {
    notes: &'a [Note],
    /// A change of page, toast or ride state raised its first cue.
    raised: bool,
    /// What cut the sequence short when it would have gone on past its
    /// last note: a later cue or the end of the trace.
    cut: Option<&'static str>,
}

/// Append the cue to each `pattern N` entry: `pattern 21: radar clear (1 of 2)`.
/// `end_ms` is the trace's last pass.
pub fn name(entries: &mut [Entry], end_ms: u64) {
    let notes: Vec<Note> = entries
        .iter()
        .enumerate()
        .filter(|(_, e)| e.kind == Kind::Sound)
        .filter_map(|(entry, e)| {
            Some(Note {
                entry,
                at_ms: e.at.session_ms,
                pattern: e.text.strip_prefix("pattern ")?.parse().ok()?,
            })
        })
        .collect();
    let changes: BTreeSet<u64> = entries
        .iter()
        .filter(|e| matches!(e.kind, Kind::Page | Kind::Toast | Kind::Ride))
        .map(|e| e.at.session_ms)
        .collect();
    let palette: Vec<(Cue, Vec<u8>)> = Cue::ALL
        .iter()
        .map(|&cue| (cue, cue_patterns(PATTERNS, cue).to_vec()))
        .filter(|(_, patterns)| !patterns.is_empty())
        .collect();
    for run in runs(&notes, &changes, end_ms) {
        let patterns: Vec<u8> = run.notes.iter().map(|note| note.pattern).collect();
        let lengths = split(&palette, &patterns);
        let segments = lengths.len();
        let mut at = 0;
        for (segment, length) in lengths.into_iter().enumerate() {
            let spelled = &patterns[at..at + length];
            let mut cues: Vec<Cue> = palette
                .iter()
                .filter(|(_, cue)| cue.as_slice() == spelled)
                .map(|(cue, _)| *cue)
                .collect();
            if segment == 0 && run.raised && cues.iter().any(|cue| !cue.safety()) {
                cues.retain(|cue| !cue.safety());
            }
            let mut names = if cues.is_empty() {
                "no cue".to_string()
            } else {
                join(cues.into_iter())
            };
            if let Some(by) = run.cut.filter(|_| segment + 1 == segments) {
                let longer: Vec<Cue> = palette
                    .iter()
                    .filter(|(_, cue)| cue.len() > length && cue.starts_with(spelled))
                    .map(|(cue, _)| *cue)
                    .collect();
                if !longer.is_empty() {
                    names.push_str(&format!(
                        ", or the start of {} cut off by {by}",
                        join(longer.into_iter())
                    ));
                }
            }
            for (i, note) in run.notes[at..at + length].iter().enumerate() {
                let text = &mut entries[note.entry].text;
                if length == 1 {
                    text.push_str(&format!(": {names}"));
                } else {
                    text.push_str(&format!(": {names} ({} of {length})", i + 1));
                }
            }
            at += length;
        }
    }
}

fn guard(pattern: u8) -> u64 {
    PATTERNS
        .iter()
        .find(|p| p.id == pattern)
        .map_or(500, |p| u64::from(p.guard_ms))
}

fn runs<'a>(notes: &'a [Note], changes: &BTreeSet<u64>, end_ms: u64) -> Vec<Run<'a>> {
    let changed = |after: u64, to: u64| changes.range(after + 1..=to).next().is_some();
    let next_due = |note: &Note| note.at_ms + guard(note.pattern) + SLACK_MS;
    let mut runs: Vec<Run> = Vec::new();
    let mut start = 0;
    for (i, note) in notes.iter().enumerate() {
        let previous = i.checked_sub(1).map(|p| &notes[p]);
        let since = previous.map_or(0, |p| p.at_ms);
        let raised = changed(since.max(note.at_ms.saturating_sub(STALE_MS)), note.at_ms);
        let follows = previous.is_some_and(|p| note.at_ms <= next_due(p));
        if previous.is_some() && follows && !raised {
            continue;
        }
        if let Some(run) = runs.last_mut() {
            run.notes = &notes[start..i];
            run.cut = follows.then_some("the next cue");
        }
        start = i;
        runs.push(Run {
            notes: &notes[i..=i],
            raised,
            cut: None,
        });
    }
    if let Some(run) = runs.last_mut() {
        run.notes = &notes[start..];
        let last = run.notes.last().expect("a run has a note");
        run.cut = (end_ms < next_due(last)).then_some("the end of the trace");
    }
    runs
}

/// The lengths of the fewest whole cues that spell `patterns`; single notes
/// where none do.
fn split(palette: &[(Cue, Vec<u8>)], patterns: &[u8]) -> Vec<usize> {
    let spells = |part: &[u8]| palette.iter().any(|(_, cue)| cue.as_slice() == part);
    let ticking = patterns.windows(2).all(|pair| pair[0] == pair[1]) && !spells(patterns);
    if ticking {
        return vec![1; patterns.len()];
    }
    let mut best: Vec<Option<Vec<usize>>> = vec![None; patterns.len() + 1];
    best[0] = Some(Vec::new());
    for end in 1..=patterns.len() {
        best[end] = (0..end)
            .filter(|&start| spells(&patterns[start..end]))
            .filter_map(|start| {
                let mut lengths = best[start].clone()?;
                lengths.push(end - start);
                Some(lengths)
            })
            .min_by_key(Vec::len);
    }
    best[patterns.len()]
        .clone()
        .unwrap_or_else(|| vec![1; patterns.len()])
}

fn join(cues: impl Iterator<Item = Cue>) -> String {
    cues.map(words).collect::<Vec<_>>().join(" or ")
}

/// `RadarClear` as `radar clear`.
fn words(cue: Cue) -> String {
    let mut out = String::new();
    for c in format!("{cue:?}").chars() {
        if c.is_uppercase() && !out.is_empty() {
            out.push(' ');
        }
        out.push(c.to_ascii_lowercase());
    }
    out
}
