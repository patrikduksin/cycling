//! The layout check (#398 D5). A frame's regions lie inside the panel and
//! apart; every label stays inside the region it was laid out in; hit
//! targets stay inside the panel and apart. A screen that lays out from
//! regions fails the check by panicking. Screens that do not yet only have
//! their text checked against itself, and what that finds is recorded on
//! request rather than failed, since their runs may already overlap within
//! `font::SPILL`.
//!
//! The simulator checks every frame; the render benchmarks turn it off so
//! they time painting alone.
use super::chrome::Rect;
use std::cell::{Cell, RefCell};
use std::format;
use std::string::String;
use std::vec::Vec;

std::thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
    static FINDINGS: RefCell<Option<Vec<String>>> = const { RefCell::new(None) };
}

/// Check frames on this thread, or stop checking them.
pub fn enable(on: bool) {
    ENABLED.set(on);
}

pub(super) fn enabled() -> bool {
    ENABLED.get()
}

/// Collect what the check finds on screens not laid out from regions, from
/// now until `findings`.
pub fn record() {
    FINDINGS.with(|found| *found.borrow_mut() = Some(Vec::new()));
}

/// What was collected since `record`, each finding once.
pub fn findings() -> Vec<String> {
    let mut found = FINDINGS
        .with(|found| found.borrow_mut().take())
        .unwrap_or_default();
    found.sort();
    found.dedup();
    found
}

pub(super) fn recording() -> bool {
    FINDINGS.with(|found| found.borrow().is_some())
}

pub(super) fn note(finding: String) {
    FINDINGS.with(|found| {
        if let Some(found) = found.borrow_mut().as_mut() {
            found.push(finding);
        }
    });
}

pub(super) struct Placed {
    pub name: String,
    pub rect: Rect,
    pub region: usize,
}

pub(super) const fn scrolling_bounds(panel: Rect) -> Rect {
    Rect {
        bottom: usize::MAX,
        ..panel
    }
}

const fn inside(rect: Rect, bounds: Rect) -> bool {
    rect.left >= bounds.left
        && rect.top >= bounds.top
        && rect.right <= bounds.right
        && rect.bottom <= bounds.bottom
}

const fn overlap(a: Rect, b: Rect) -> bool {
    a.left < b.right && b.left < a.right && a.top < b.bottom && b.top < a.bottom
}

pub(super) fn region_breaks(
    panel: Rect,
    regions: &[(&str, Rect)],
    placed: &[Placed],
    targets: &[(&str, Rect)],
) -> Vec<String> {
    let mut breaks = Vec::new();
    for (i, &(name, rect)) in regions.iter().enumerate() {
        if !inside(rect, panel) {
            breaks.push(format!("region {name} {rect:?} leaves the panel {panel:?}"));
        }
        for &(other, against) in &regions[i + 1..] {
            if overlap(rect, against) {
                breaks.push(format!(
                    "regions {name} {rect:?} and {other} {against:?} overlap"
                ));
            }
        }
    }
    for item in placed {
        let Some(&(name, region)) = regions.get(item.region) else {
            breaks.push(format!("{:?} has no region", item.name));
            continue;
        };
        if !inside(item.rect, region) {
            breaks.push(format!(
                "{:?} {:?} leaves its region {name} {region:?}",
                item.name, item.rect
            ));
        }
    }
    for (i, &(name, rect)) in targets.iter().enumerate() {
        if !inside(rect, panel) {
            breaks.push(format!("target {name} {rect:?} leaves the panel"));
        }
        for &(other, against) in &targets[i + 1..] {
            if overlap(rect, against) {
                breaks.push(format!("targets {name} and {other} overlap"));
            }
        }
    }
    breaks
}

pub(super) fn canvas_findings(what: &str, bounds: Rect, inks: &[(String, Rect)]) {
    use super::chrome::layout::WORD_GAP;
    for (i, (name, rect)) in inks.iter().enumerate() {
        if !inside(*rect, bounds) {
            note(format!("{what}: {name:?} {rect:?} leaves the panel"));
        }
        for (other, against) in &inks[i + 1..] {
            let near = Rect {
                left: rect.left.saturating_sub(WORD_GAP),
                right: rect.right + WORD_GAP,
                ..*rect
            };
            if overlap(*rect, *against) {
                note(format!(
                    "{what}: {name:?} {rect:?} overlaps {other:?} {against:?}"
                ));
            } else if overlap(near, *against) {
                note(format!(
                    "{what}: {name:?} {rect:?} runs into {other:?} {against:?} (under {WORD_GAP} columns apart)"
                ));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(left: usize, top: usize, right: usize, bottom: usize) -> Rect {
        Rect {
            left,
            top,
            right,
            bottom,
        }
    }

    #[test]
    fn the_check_names_what_breaks() {
        let panel = rect(0, 0, 100, 100);
        let regions = [("a", rect(0, 0, 100, 50)), ("b", rect(0, 40, 100, 120))];
        let placed = [Placed {
            name: "X".into(),
            rect: rect(0, 45, 10, 55),
            region: 0,
        }];
        let targets = [("t", rect(90, 90, 110, 100))];
        let breaks = region_breaks(panel, &regions, &placed, &targets);
        assert_eq!(breaks.len(), 4, "{breaks:#?}");
    }
}
