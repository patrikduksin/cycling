//! The layout check (#398 D5). A frame's regions lie inside the panel and
//! apart; every label stays inside the region it was laid out in; hit
//! targets stay inside the panel and apart. A frame that breaks it fails by
//! panicking.
//!
//! The simulator checks every frame; the render benchmarks turn it off so
//! they time painting alone.
use super::chrome::Rect;
use std::cell::Cell;
use std::format;
use std::string::String;
use std::vec::Vec;

std::thread_local! {
    static ENABLED: Cell<bool> = const { Cell::new(true) };
}

/// Check frames on this thread, or stop checking them.
pub fn enable(on: bool) {
    ENABLED.set(on);
}

pub(super) fn enabled() -> bool {
    ENABLED.get()
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

pub(super) const fn inside(rect: Rect, bounds: Rect) -> bool {
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

/// Text that overlaps other text: runs only meet by design within
/// `font::SPILL` beside each other, never over each other's ink.
pub(super) fn overlaps(inks: &[(String, Rect)]) -> Vec<String> {
    let mut breaks = Vec::new();
    for (i, (name, rect)) in inks.iter().enumerate() {
        for (other, against) in &inks[i + 1..] {
            if overlap(*rect, *against) {
                breaks.push(format!("{name:?} {rect:?} overlaps {other:?} {against:?}"));
            }
        }
    }
    breaks
}

pub(super) fn canvas_breaks(
    canvas: &super::canvas::Canvas,
    bounds: Rect,
    regions: &[(&str, Rect)],
    page_first: usize,
    targets: &[(&str, Rect)],
) -> Vec<String> {
    let region = |rect: Rect| {
        (page_first..regions.len())
            .find(|&i| inside(rect, regions[i].1))
            .unwrap_or(page_first)
    };
    let inks: Vec<_> = canvas
        .runs
        .iter()
        .filter_map(super::chrome::layout::text)
        .collect();
    let shapes = canvas.shapes.iter().map(|shape| {
        let rect = shape.bounds().clipped(bounds);
        Placed {
            name: format!("{shape:?}"),
            rect,
            region: region(rect),
        }
    });
    let placed: Vec<Placed> = inks
        .iter()
        .map(|(name, rect)| Placed {
            name: name.clone(),
            rect: *rect,
            region: region(*rect),
        })
        .chain(shapes)
        .collect();
    let mut breaks = region_breaks(bounds, regions, &placed, targets);
    breaks.extend(overlaps(&inks));
    breaks
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
