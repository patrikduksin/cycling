//! Where two traces of one session part: in what the rider saw, in the
//! panel's frames, and in the world records.
//!
//! The frames are the panel digests, pass by pass. When either trace
//! replaced the credentials the phone page draws, that page's frames are
//! left out, and the diff says how many.
use crate::twin::script::{Hex, Named};
use firmware_services::trace::{self, Event};
use serde::Serialize;
use std::fmt;

/// The first entry two traces hold otherwise, in the pass of whichever
/// comes first; `None` on a side that has no more entries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Difference {
    pub at_ms: u64,
    pub a: Option<String>,
    pub b: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Diff {
    /// A page, toast, sound, ride state, mark or shown reading.
    pub seen: Option<Difference>,
    /// A frame: the digest of what the panel drew.
    pub panel: Option<Difference>,
    pub world: Option<Difference>,
    /// Frames the panel comparison left out, and why.
    pub exclusions: Vec<String>,
}

fn seen(event: &Event<'_>) -> Option<String> {
    match event {
        Event::Page(page) => Some(format!("page {page}")),
        Event::Toast(text) => Some(format!("toast {text}")),
        Event::RideState(state) => Some(format!("ride {state}")),
        Event::Shown(fields) => Some(format!("shown {fields}")),
        Event::Sound { cue, .. } => Some(format!("sound {cue}")),
        Event::Mark { count, page, .. } => Some(format!("mark {count} on {page}")),
        _ => None,
    }
}

fn world(event: &Event<'_>) -> Option<String> {
    match event {
        Event::Call(call) => Some(format!("{} {}", Named(call.method), Hex(call.body))),
        _ => None,
    }
}

fn entries(
    bytes: &[u8],
    of: fn(&Event<'_>) -> Option<String>,
) -> Result<Vec<(u64, String)>, String> {
    let (_, events) = trace::decode(bytes).map_err(|error| error.to_string())?;
    let mut pass_ms = 0;
    Ok(events
        .filter_map(|event| {
            if let Event::Pass { now_ms } = event {
                pass_ms = now_ms;
            }
            Some((pass_ms, of(&event)?))
        })
        .collect())
}

/// Each frame by its pass, but the phone page's when `redacted`, and how
/// many of those there were.
fn frames(bytes: &[u8], redacted: bool) -> Result<(Vec<(u64, String)>, u32), String> {
    let (_, events) = trace::decode(bytes).map_err(|error| error.to_string())?;
    let (mut pass_ms, mut phone, mut left_out, mut frames) = (0, false, 0, Vec::new());
    for event in events {
        match event {
            Event::Pass { now_ms } => pass_ms = now_ms,
            Event::Page(page) => phone = page == "phone",
            Event::Panel(_) if redacted && phone => left_out += 1,
            Event::Panel(digest) => frames.push((pass_ms, format!("frame {digest:08x}"))),
            _ => {}
        }
    }
    Ok((frames, left_out))
}

fn first(a: &[(u64, String)], b: &[(u64, String)]) -> Option<Difference> {
    let at = (0..a.len().max(b.len())).find(|&at| a.get(at) != b.get(at))?;
    let (was, now) = (a.get(at), b.get(at));
    let at_ms = match (was, now) {
        (Some((x, _)), Some((y, _))) => (*x).min(*y),
        (Some((at, _)), None) | (None, Some((at, _))) => *at,
        (None, None) => unreachable!("one of them holds an entry"),
    };
    Some(Difference {
        at_ms,
        a: was.map(|(_, text)| text.clone()),
        b: now.map(|(_, text)| text.clone()),
    })
}

/// Where `a` and `b` part.
pub fn diff(a: &[u8], b: &[u8]) -> Result<Diff, String> {
    let redacted =
        crate::trace::world::opened(a).redacted || crate::trace::world::opened(b).redacted;
    let ((frames_a, left_a), (frames_b, left_b)) = (frames(a, redacted)?, frames(b, redacted)?);
    let exclusions = [("a", left_a), ("b", left_b)]
        .into_iter()
        .filter(|(_, left_out)| *left_out > 0)
        .map(|(side, left_out)| {
            format!(
                "{left_out} phone-page frames of {side}: a trace replaced the credentials they draw"
            )
        })
        .collect();
    Ok(Diff {
        seen: first(&entries(a, seen)?, &entries(b, seen)?),
        panel: first(&frames_a, &frames_b),
        world: first(&entries(a, world)?, &entries(b, world)?),
        exclusions,
    })
}

impl fmt::Display for Diff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let parts = [
            ("seen", &self.seen),
            ("panel", &self.panel),
            ("world", &self.world),
        ];
        for (what, difference) in parts {
            let Some(difference) = difference else {
                writeln!(f, "{what}: the same")?;
                continue;
            };
            let side =
                |side: &Option<String>| side.clone().unwrap_or_else(|| "nothing more".into());
            writeln!(
                f,
                "{what}: first differs at {}: {} | {}",
                crate::trace::timeline::session_clock(difference.at_ms),
                side(&difference.a),
                side(&difference.b)
            )?;
        }
        for exclusion in &self.exclusions {
            writeln!(f, "left out: {exclusion}")?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
