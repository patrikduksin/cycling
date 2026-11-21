//! Workouts: the list of what to ride under its heading. Each card names a
//! workout and what it asks, the chosen one with a dot. MOVE focuses the
//! next card, ACT chooses the focused one and opens Ready, a tap chooses
//! the card under the finger, and BACK returns Home.
use super::{Builtin, Choices, Nodes};
use crate::compose::data::ListKind;
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size, Sizing};
use crate::compose::registry::{ENTRY, HEADING, SPACE, STATUS};
use crate::compose::tiles::entry::Line;
use crate::screens::workout::Page;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_: &Choices| tree().into_iter().collect(),
    buttons: Buttons {
        act: None,
        back: Some(Button {
            action: Action::Go(Go::Open(Page::Home)),
            cue: None,
        }),
        move_: None,
        held: None,
    },
};

/// The status bar and the black under it.
pub(super) const BAR_UNITS: u8 = 24;
/// The columns either side of the cards.
pub(super) const SIDE_UNITS: u8 = 10;
/// The columns either side of the scroll, whose own gutter takes the rest
/// of the side.
const SCROLL_SIDE_UNITS: u8 = 3;
/// The heading's band, its capitals ending on the band's bottom, and the
/// black under any head where cards scroll out of view.
pub(super) const HEADING_UNITS: u8 = 30;
pub(super) const UNDER_HEADING_UNITS: u8 = 8;
/// A card, its name over the line under it with even gaps.
pub(super) const CARD_UNITS: u8 = 52;

/// A list screen: the bar, then `head` over the scroll, whose template is
/// a card of `lines` `card` units tall, and `foot` under it, all between
/// side margins.
pub(super) fn list(head: Node, list: ListKind, card: u8, lines: &[Line], foot: &[Node]) -> Nodes {
    let space = |units| Node::leaf(SPACE, 0, Size::units(units));
    let mut nodes = Nodes::new();
    let mut push = |node| {
        if nodes.push(node).is_err() {
            unreachable!("a list screen is a score of nodes");
        }
    };
    let foot_units = foot.iter().map(|node| match node.size().get() {
        Sizing::Units(units) => units,
        Sizing::Content | Sizing::Grow(_) => unreachable!("a list's foot is bands"),
    });
    let foot_units = foot_units.sum();
    let parts = 4 + u8::from(!foot.is_empty());
    let between_sides = |size, children| {
        [
            Node::row(3, size),
            space(SIDE_UNITS),
            Node::col(children, Size::grow(1)),
        ]
    };
    for node in [
        Node::col(parts, Size::grow(1)),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
    ]
    .into_iter()
    .chain(between_sides(head.size(), 1))
    .chain([head, space(SIDE_UNITS), space(UNDER_HEADING_UNITS)])
    .chain([
        Node::row(3, Size::grow(1)),
        space(SCROLL_SIDE_UNITS),
        Node::col(1, Size::grow(1)),
        Node::scroll(list, Size::grow(1)),
        Node::col(lines.len() as u8, Size::units(card)),
    ]) {
        push(node);
    }
    for &line in lines {
        push(Node::leaf(ENTRY, line as u8, Size::CONTENT));
    }
    push(space(SCROLL_SIDE_UNITS));
    if !foot.is_empty() {
        let sides = between_sides(Size::units(foot_units), foot.len() as u8);
        for node in sides.into_iter().chain(foot.iter().copied()) {
            push(node);
        }
        push(space(SIDE_UNITS));
    }
    nodes
}

fn tree() -> Nodes {
    let heading = Node::leaf(
        HEADING,
        ListKind::Workouts as u8,
        Size::units(HEADING_UNITS),
    );
    list(
        heading,
        ListKind::Workouts,
        CARD_UNITS,
        &[Line::Title, Line::Detail],
        &[],
    )
}
