//! The open lap: its time and number, its distance and averages, the last
//! finished lap with how this one compares, and a bar per lap filling the
//! rest of the page. The lap's numbers share their room as even gaps,
//! inside the ride page's frame: the status bar, the radar's room while one
//! is paired, and the page's column between margins.
use super::{Builtin, Choices, Nodes};
use crate::compose::data::Metric;
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Flags, Node, Size, When};
use crate::compose::registry::{LAP_HEAD, LAPS, RIDE_CELL, SPACE, STATUS};
use crate::compose::tiles::ride_cell::Celled;
use device_api::input::Role;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_: &Choices| page(2, body()),
    buttons: Buttons {
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
    },
};

/// The status bar and the black under it.
const BAR_UNITS: u8 = 24;
const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;
const LEFT_UNITS: u8 = 10;
const RIGHT_UNITS: u8 = crate::screens::ride::TEXT_RIGHT as u8;
const BETWEEN_UNITS: u8 = 8;

const PAIRS: [[Metric; 2]; 4] = [
    [Metric::LapDistance, Metric::LapPower],
    [Metric::LapSpeed, Metric::LapHeart],
    [Metric::PrevLapTime, Metric::PrevLapPower],
    [Metric::SpeedDiff, Metric::PowerDiff],
];

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

/// The page around `body`, the `children` subtrees of its column.
fn page(children: u8, body: impl IntoIterator<Item = Node>) -> Nodes {
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
const fn pair([left, right]: [Metric; 2]) -> [Node; 4] {
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
    let grow = Size::grow(1);
    let numbers = [
        Node::col(1 + PAIRS.len() as u8, grow),
        Node::leaf(LAP_HEAD, 0, Size::CONTENT),
    ];
    let pairs = PAIRS.into_iter().flat_map(pair);
    numbers
        .into_iter()
        .chain(pairs)
        .chain([Node::leaf(LAPS, 0, grow)])
}
