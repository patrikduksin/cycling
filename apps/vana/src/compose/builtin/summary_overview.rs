//! The summary's first page: its heading (RIDE SAVED under the badge, SYNC
//! TO SAVE or NOT ALL SAVED over its cause), then the ride's time, distance,
//! average speed, ascent, average power and heart rate. ACT (check) or BACK
//! finishes the ride; MOVE steps through the pages.
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{CELL, SPACE, WORDS};
use crate::screens::cue::Icon;
use crate::screens::ride::summary::{Stat, Words};
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_| tree(),
    buttons: FINISH,
});

pub(super) fn tree() -> Nodes {
    let mut body = Nodes::new();
    let _ = body.push(Node::leaf(WORDS, Words::Heading as u8, Size::CONTENT));
    for (left, right) in [
        (Stat::Time, Stat::Distance),
        (Stat::AvgSpeed, Stat::Ascent),
        (Stat::AvgPower, Stat::AvgHeart),
    ] {
        let _ = body.extend_from_slice(&inset(left, right));
    }
    let keep = Keep {
        top: HEADING_TOP,
        bottom: CELLS_BOTTOM,
    };
    page(WIDE_UNITS, keep, 4, &body)
}

/// ACT (check) or BACK finishes a ride just saved; MOVE steps its pages.
pub(super) const FINISH: Buttons = Buttons {
    act: Some(Button {
        action: Action::Ride(Role::Act),
        cue: Some(Icon::Check),
    }),
    back: Some(Button {
        action: Action::Ride(Role::Back),
        cue: None,
    }),
    move_: Some(Button {
        action: Action::Go(Go::Next),
        cue: None,
    }),
    held: None,
};

/// The status bar and the black under it.
const BAR_UNITS: u8 = 24;
/// The text's margins at either side.
pub(super) const MARGIN_UNITS: u8 = 16;
/// The first page's margins, which its widest heading reaches.
const WIDE_UNITS: u8 = 8;
const BETWEEN_UNITS: u8 = 8;

pub(super) const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

/// Two cells side by side; the column between them is the only room
/// beside the page's margins.
pub(super) const fn cells(left: Stat, right: Stat) -> [Node; 4] {
    [
        Node::row(3, Size::CONTENT),
        Node::leaf(CELL, left as u8, Size::grow(1)),
        space(BETWEEN_UNITS),
        Node::leaf(CELL, right as u8, Size::grow(1)),
    ]
}

/// Two cells inside the first page's narrower margins, so they line up
/// with every other page's.
const fn inset(left: Stat, right: Stat) -> [Node; 6] {
    let inset = MARGIN_UNITS - WIDE_UNITS;
    [
        Node::row(5, Size::CONTENT),
        space(inset),
        Node::leaf(CELL, left as u8, Size::grow(1)),
        space(BETWEEN_UNITS),
        Node::leaf(CELL, right as u8, Size::grow(1)),
        space(inset),
    ]
}

/// The rows a page keeps below the bar's room and above the bottom edge, so
/// its first and last tiles stay clear of a cue in any corner. A cue's disc
/// reaches less far in the further a tile starts from the edge, so each
/// page keeps what its narrowest end tile needs.
#[derive(Clone, Copy)]
pub(super) struct Keep {
    pub top: u8,
    pub bottom: u8,
}

/// A lone block in the middle of the page, clear of every corner.
pub(super) const CENTRED: Keep = Keep { top: 0, bottom: 0 };
/// Below the bar's room: a row of cells first, or the first page's heading.
pub(super) const CELLS_TOP: u8 = 22;
const HEADING_TOP: u8 = 27;
/// A row of cells last, which a bottom corner's cue would narrow.
pub(super) const CELLS_BOTTOM: u8 = 46;

/// A summary page: the room of the status bar, which a top corner's cue
/// hides, and what `keep` keeps, then `body`, `children` tiles in a column
/// inside `margin` units at either side. The spare rows become even gaps.
pub(super) fn page(margin: u8, keep: Keep, children: u8, body: &[Node]) -> Nodes {
    let mut nodes = Nodes::new();
    let _ = nodes.extend_from_slice(&[
        Node::col(4, Size::grow(1)),
        space(BAR_UNITS),
        space(keep.top),
        Node::row(3, Size::grow(1)),
        space(margin),
        Node::col(children, Size::grow(1)),
    ]);
    let _ = nodes.extend_from_slice(body);
    let _ = nodes.extend_from_slice(&[space(margin), space(keep.bottom)]);
    nodes
}
