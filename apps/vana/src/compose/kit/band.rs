//! The bar's target band: a wide track with the step's target range lit in
//! its zone's colour in the middle, and the reading the target checks as a
//! marker on it. Past either end of the track the marker
//! holds at that end.
use super::px;
use crate::compose::data::Data;
use crate::compose::tile::Form;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Rect;
use crate::screens::scaled::Scaled;
use crate::screens::theme::LAYOUT;

pub(super) const SHAPES: u8 = 4;

fn marker(at: &Scaled) -> (usize, usize) {
    let space = &LAYOUT.space;
    (
        px(at, space.track) / 2 + px(at, space.mark),
        px(at, space.mark),
    )
}

pub(super) fn form(at: &Scaled) -> Form {
    let (radius, ring) = marker(at);
    Form {
        width: (8 * (radius + ring)) as u16,
        height: (2 * (radius + ring)) as u16,
        shapes: SHAPES,
        runs: 0,
    }
}

pub(super) fn prepare(rect: Rect, at: &Scaled, data: &dyn Data, out: &mut Canvas) {
    let Some(training) = data.training().filter(|training| !training.done) else {
        return;
    };
    let theme = data.theme();
    let gutter = px(at, LAYOUT.space.gutter);
    let height = px(at, LAYOUT.space.track);
    let middle = (rect.top + rect.bottom) / 2;
    let track = Rect {
        left: rect.left + gutter,
        right: rect.right - gutter,
        top: middle - height / 2,
        bottom: middle - height / 2 + height,
    };
    let radius = height / 2;
    out.shape(Shape::Bar {
        rect: track,
        radius,
        color: theme.off,
    });
    let Some(band) = training.now.band else {
        return;
    };
    let (low, high) = (i32::from(band.low), i32::from(band.high));
    let half = ((high - low) / 2).max(low / 20).max(1);
    let (from, to) = (low - half, high + half);
    let x = |value: i32| {
        track.left
            + ((value.clamp(from, to) - from) as usize * track.width()) / (to - from) as usize
    };
    out.shape(Shape::Bar {
        rect: Rect {
            left: x(low),
            right: x(high).max(x(low) + height),
            ..track
        },
        radius,
        color: theme.zone(training.zone),
    });
    let Some(reading) = training.now.reading else {
        return;
    };
    let (disc, ring) = marker(at);
    let cx = x(i32::from(reading)).clamp(track.left + disc, track.right - disc);
    let (cx, cy) = (4 * cx as i32, 4 * middle as i32);
    out.shape(Shape::Disc {
        cx,
        cy,
        radius: 4 * (disc + ring) as i32,
        color: theme.bg,
    });
    out.shape(Shape::Disc {
        cx,
        cy,
        radius: 4 * disc as i32,
        color: theme.text,
    });
}
