//! The ride's totals: two columns of cells, what this ride has added up so
//! far, then the time spent in each power and heart rate zone. The lap page
//! shares its frame: the status bar, the radar's room while one is paired,
//! and the page's column between margins, whose spare room becomes even
//! gaps.
use super::{Builtin, Choices, Nodes};
use crate::compose::data::{Effort, Metric};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Flags, Node, Size, When};
use crate::compose::registry::{RIDE_CELL, SPACE, STATUS, ZONES};
use crate::compose::tiles::ride_cell::Celled;
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| page(CELLS.len() as u8 + 2, body()),
    buttons: RIDE_PAGE,
});

pub(super) const RIDE_PAGE: Buttons = Buttons {
    act: Some(Button {
        action: Action::Ride(Role::Act),
        cue: None,
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
const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;
const LEFT_UNITS: u8 = 10;
const RIGHT_UNITS: u8 = crate::screens::ride::TEXT_RIGHT as u8;
const BETWEEN_UNITS: u8 = 8;

const CELLS: [[Metric; 2]; 5] = [
    [Metric::Distance, Metric::Moving],
    [Metric::AvgSpeed, Metric::MaxSpeed],
    [Metric::AvgPower, Metric::NormalizedPower],
    [Metric::Intensity, Metric::Stress],
    [Metric::AvgHeart, Metric::Ascent],
];

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

/// A ride page around `body`, the `children` subtrees of its column.
pub(super) fn page(children: u8, body: impl IntoIterator<Item = Node>) -> Nodes {
    let grow = Size::grow(1);
    let frame = [
        Node::col(2, grow),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
        Node::row(4, grow),
        space(RADAR_UNITS).when(When::new(Flags::RADAR, Flags::NONE)),
        space(LEFT_UNITS),
        Node::col(children, grow),
    ];
    frame
        .into_iter()
        .chain(body)
        .chain([space(RIGHT_UNITS)])
        .collect()
}

/// Two cells side by side. The left one keeps half the text's width beside
/// a radar, so the right column starts at one column in every row.
pub(super) const fn pair([left, right]: [Metric; 2]) -> [Node; 4] {
    [
        Node::row(3, Size::CONTENT),
        cell(left, Size::units(LEFT_COLUMN_UNITS)),
        space(BETWEEN_UNITS),
        cell(right, Size::grow(1)),
    ]
}

const LEFT_COLUMN_UNITS: u8 = {
    let width = crate::screens::scaled::BASE.panel.width as u8;
    (width - RADAR_UNITS - LEFT_UNITS - RIGHT_UNITS - BETWEEN_UNITS) / 2
};

const fn cell(metric: Metric, size: Size) -> Node {
    let celled = Celled {
        metric,
        large: false,
        centre: false,
    };
    Node::leaf(RIDE_CELL, celled.byte(), size)
}

fn body() -> impl Iterator<Item = Node> {
    let cells = CELLS.into_iter().flat_map(pair);
    let zones =
        [Effort::Power, Effort::Heart].map(|effort| Node::leaf(ZONES, effort.id(), Size::CONTENT));
    cells.chain(zones)
}
