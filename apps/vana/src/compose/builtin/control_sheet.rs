use super::{Builtin, Choices, Nodes};
use crate::compose::input::{Action, Button, Buttons, SheetAct};
use crate::compose::node::{Node, Size};
use crate::compose::registry::{DAY_NIGHT, LEVEL, SHEET_SENSORS, SPACE};
use crate::screens::cue::Icon;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_: &Choices| TREE.into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Sheet(SheetAct::Brighter),
            cue: Some(Icon::More),
        }),
        back: Some(Button {
            action: Action::Sheet(SheetAct::Close),
            cue: None,
        }),
        move_: Some(Button {
            action: Action::Sheet(SheetAct::Dimmer),
            cue: Some(Icon::Less),
        }),
        held: None,
    },
};

const fn space(size: Size) -> Node {
    Node::leaf(SPACE, 0, size)
}

const TREE: [Node; 13] = [
    Node::col(3, Size::grow(1)),
    space(Size::units(30)),
    Node::row(3, Size::units(108)),
    space(Size::units(10)),
    Node::col(3, Size::grow(1)),
    Node::leaf(LEVEL, 0, Size::units(48)),
    space(Size::units(8)),
    Node::row(3, Size::units(52)),
    Node::leaf(SHEET_SENSORS, 0, Size::grow(1)),
    space(Size::units(8)),
    Node::leaf(DAY_NIGHT, 0, Size::units(58)),
    space(Size::units(10)),
    space(Size::grow(1)),
];
