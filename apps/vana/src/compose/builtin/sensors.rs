//! The sensor pages. SENSORS lists the saved sensors and ADD SENSOR under
//! its title; NEARBY lists what a search found under its title and the
//! search line. A sensor's page pictures it over its name and state, its
//! BATTERY and ID, and on a power meter its calibration. FORGET SENSOR?
//! and REPLACE SENSOR ask with corner cues.
//!
//! The menu (`screens::sensors`) takes the presses itself: lost input makes
//! MOVE confirm the focus before ACT, and ACT waits a moment after a page
//! opens. The buttons here set the cues.
use super::workouts::{BAR_UNITS, HEADING_UNITS, SIDE_UNITS, UNDER_HEADING_UNITS};
use super::{Builtin, Choices, Nodes};
use crate::compose::data::ListKind;
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{
    SENSOR_CELLS, SENSOR_ICON, SENSOR_LINE, SENSOR_ROW, SENSOR_SEARCH, SENSOR_SWAP, SENSOR_TITLE,
    SPACE, STATUS,
};
use crate::compose::tiles::sensor_line::Says;
use crate::screens::cue::Icon;

/// A sensor's card on either list.
const CARD_UNITS: u8 = 56;
/// NEARBY's search line under its title.
const SEARCH_UNITS: u8 = 22;
/// Between a sensor's name and its state, and between two lines that
/// read as one.
const NEAR_UNITS: u8 = 6;
/// The rows a corner cue takes.
const CUE_UNITS: u8 = 42;

const LEAVE: Button = Button {
    action: Action::Page(OnPage::Back),
    cue: None,
};
const BACK: Button = Button {
    action: Action::Page(OnPage::Back),
    cue: Some(Icon::Back),
};

const fn lists() -> Buttons {
    Buttons {
        back: Some(LEAVE),
        ..Buttons::FREE
    }
}

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| list(ListKind::Sensors),
    buttons: lists(),
});

pub const NEARBY: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| list(ListKind::Nearby),
    buttons: lists(),
});

/// A saved sensor's page, unless it is a power meter.
pub const DETAIL: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| page(&detail(false)),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Page(OnPage::Forget),
            cue: Some(Icon::Trash),
        }),
        back: Some(BACK),
        move_: None,
        held: None,
    },
});

pub const POWER_METER: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| page(&detail(true)),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Page(OnPage::Calibrate),
            cue: Some(Icon::Calibrate),
        }),
        back: Some(BACK),
        move_: Some(Button {
            action: Action::Page(OnPage::Forget),
            cue: Some(Icon::Trash),
        }),
        held: None,
    },
});

pub const FORGET: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| {
        page(&[
            Node::leaf(SENSOR_ICON, 0, Size::CONTENT),
            Node::col(2, Size::CONTENT),
            line(Says::Forget),
            line(Says::Sensor),
            line(Says::Forgotten),
        ])
    },
    buttons: answer(Icon::Trash),
});

pub const REPLACE: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| {
        page(&[
            Node::leaf(SENSOR_ICON, 0, Size::CONTENT),
            Node::col(3, Size::CONTENT),
            line(Says::Name),
            space(NEAR_UNITS),
            line(Says::Replace),
            Node::leaf(SENSOR_SWAP, 0, Size::CONTENT),
        ])
    },
    buttons: answer(Icon::Check),
});

const fn answer(icon: Icon) -> Buttons {
    Buttons {
        act: Some(Button {
            action: Action::Page(OnPage::Answer(true)),
            cue: Some(icon),
        }),
        back: Some(BACK),
        move_: None,
        held: None,
    }
}

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

const fn line(says: Says) -> Node {
    Node::leaf(SENSOR_LINE, says as u8, Size::CONTENT)
}

fn collect(nodes: impl IntoIterator<Item = Node>) -> Nodes {
    let mut collected = Nodes::new();
    for node in nodes {
        if collected.push(node).is_err() {
            unreachable!("a sensor page is under twenty nodes");
        }
    }
    collected
}

fn list(list: ListKind) -> Nodes {
    let nearby = list == ListKind::Nearby;
    let head = [
        Node::col(2, Size::grow(1)),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
        Node::row(3, Size::grow(1)),
        space(SIDE_UNITS),
        Node::col(3 + u8::from(nearby), Size::grow(1)),
        Node::leaf(SENSOR_TITLE, list as u8, Size::units(HEADING_UNITS)),
    ];
    let search = nearby.then_some(Node::leaf(SENSOR_SEARCH, 0, Size::units(SEARCH_UNITS)));
    let rest = [
        space(UNDER_HEADING_UNITS),
        Node::scroll(list, Size::grow(1)),
        Node::col(1, Size::units(CARD_UNITS)),
        Node::leaf(SENSOR_ROW, 0, Size::grow(1)),
        space(SIDE_UNITS),
    ];
    collect(head.into_iter().chain(search).chain(rest))
}

fn detail(power: bool) -> Nodes {
    let named = [
        Node::leaf(SENSOR_ICON, 0, Size::CONTENT),
        Node::col(3, Size::CONTENT),
        line(Says::Name),
        space(NEAR_UNITS),
        line(Says::State),
        Node::leaf(SENSOR_CELLS, 0, Size::CONTENT),
    ];
    let calibration = [
        Node::col(3, Size::CONTENT),
        line(Says::Calibration),
        space(NEAR_UNITS),
        line(Says::Hint),
    ];
    let calibration = calibration.into_iter().filter(|_| power);
    collect(named.into_iter().chain(calibration))
}

/// A page over the whole panel: `body`, a column of things that keep
/// their size, between side margins and the cue rows at its top and
/// foot, so its room becomes even gaps.
fn page(body: &[Node]) -> Nodes {
    let frame = [
        Node::row(3, Size::grow(1)),
        space(SIDE_UNITS),
        Node::col(top_level(body) + 2, Size::grow(1)),
        space(CUE_UNITS),
    ];
    collect(
        frame
            .into_iter()
            .chain(body.iter().copied())
            .chain([space(CUE_UNITS), space(SIDE_UNITS)]),
    )
}

/// How many of `body`'s nodes are its column's own children: the rest
/// belong to the columns among them.
fn top_level(body: &[Node]) -> u8 {
    use crate::compose::node::What;
    let mut children = 0;
    let mut owed = 0usize;
    for node in body {
        if owed == 0 {
            children += 1;
        } else {
            owed -= 1;
        }
        if let Some(What::Col { children } | What::Row { children }) = node.what() {
            owed += usize::from(children);
        }
    }
    children
}
