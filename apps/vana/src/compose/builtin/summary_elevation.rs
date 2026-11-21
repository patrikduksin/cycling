//! The summary's elevation page, for a ride that recorded altitude: ascent,
//! descent, the lowest and highest altitude, over the ride's profile, which
//! grows into the rows the cells leave.
use super::summary_overview::{
    CELLS_BOTTOM, CELLS_TOP, FINISH, Keep, MARGIN_UNITS, cells, page, space,
};
use super::{Builtin, Nodes};
use crate::compose::node::{Node, Size};
use crate::compose::registry::PROFILE;
use crate::screens::ride::summary::Stat;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| tree(),
    buttons: FINISH,
};

/// The profile keeps clear of the done cue as a row of cells does.
const KEEP: Keep = Keep {
    top: CELLS_TOP,
    bottom: CELLS_BOTTOM,
};
/// Between the two rows of cells, and between them and the profile: the
/// profile takes every spare row, so these are the page's only gaps.
const BETWEEN_UNITS: u8 = 7;
const ABOVE_PROFILE_UNITS: u8 = 16;

pub(super) fn tree() -> Nodes {
    let mut body = Nodes::new();
    let _ = body.extend_from_slice(&cells(Stat::Ascent, Stat::Descent));
    let _ = body.push(space(BETWEEN_UNITS));
    let _ = body.extend_from_slice(&cells(Stat::Lowest, Stat::Highest));
    let _ = body.extend_from_slice(&[
        space(ABOVE_PROFILE_UNITS),
        Node::leaf(PROFILE, 0, Size::grow(1)),
    ]);
    page(MARGIN_UNITS, KEEP, 5, &body)
}
