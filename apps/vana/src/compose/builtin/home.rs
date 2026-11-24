use super::{BAR_UNITS, Builtin, Choices, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{HOME_CARD, SPACE};
use crate::compose::tiles::home::Card;

pub const BUILTIN: &Builtin = &Builtin {
    tree,
    buttons: Buttons {
        act: None,
        back: Some(Button {
            action: Action::Go(Go::TurnOff),
            cue: None,
        }),
        move_: None,
        held: None,
    },
};

const SIDE_UNITS: u8 = 10;
const GAP_UNITS: u8 = 8;

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

const fn card(card: Card, size: Size) -> Node {
    Node::leaf(HOME_CARD, card as u8, size)
}

fn tree(_: &Choices) -> Nodes {
    let (grow, content) = (Size::grow(1), Size::CONTENT);
    [
        Node::col(2, grow),
        space(BAR_UNITS),
        Node::row(3, grow),
        space(SIDE_UNITS),
        Node::col(7, grow),
        space(GAP_UNITS),
        card(Card::Ride, grow),
        space(GAP_UNITS),
        card(Card::Sensors, content),
        space(GAP_UNITS),
        Node::row(3, content),
        card(Card::Train, content),
        space(GAP_UNITS),
        card(Card::Settings, grow),
        space(GAP_UNITS),
        space(SIDE_UNITS),
    ]
    .into_iter()
    .collect()
}
