use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::kit::facts::Card;
use crate::compose::kit::item::Button as Item;
use crate::compose::kit::space;
use crate::compose::kit::symbol::Mark;
use crate::compose::kit::text::Say;
use crate::compose::node::{Node, Size};
use crate::compose::registry::{CODE, FACTS, ITEM, SPACE, SYMBOL, TEXT};
use crate::screens::cue::Icon;
use crate::screens::theme::LAYOUT;

const fn page(act: OnPage, cue: Option<Icon>) -> Option<Button> {
    Some(Button {
        action: Action::Page(act),
        cue,
    })
}

pub const CODE_SCREEN: &Builtin = &Builtin {
    tree: || {
        let space = &LAYOUT.space;
        let (content, grow) = (Size::CONTENT, Size::grow(1));
        [
            Node::col(5, grow),
            self::space(space.top_line),
            Node::row(3, content),
            Node::leaf(SPACE, 0, grow),
            Node::leaf(CODE, 0, content),
            Node::leaf(SPACE, 0, grow),
            self::space(space.pad + space.gap),
            Node::row(4, content),
            Node::leaf(SPACE, 0, grow),
            Node::leaf(TEXT, Say::ScanWith as u8, content),
            Node::leaf(TEXT, Say::IphoneCamera as u8, content),
            Node::leaf(SPACE, 0, grow),
            Node::leaf(SPACE, 0, grow),
        ]
        .into_iter()
        .collect()
    },
    buttons: Buttons {
        act: None,
        back: page(OnPage::Back, None),
        move_: None,
        held: None,
    },
};

pub const DONE: &Builtin = &Builtin {
    tree: || {
        let gap = LAYOUT.space.gap;
        let (content, grow) = (Size::CONTENT, Size::grow(1));
        [
            Node::col(5, grow),
            Node::leaf(SPACE, 0, grow),
            Node::leaf(SYMBOL, Mark::PhonePaired as u8, content),
            space(gap),
            Node::leaf(TEXT, Say::PhonePaired as u8, content),
            Node::leaf(SPACE, 0, grow),
        ]
        .into_iter()
        .collect()
    },
    buttons: Buttons {
        act: page(OnPage::Back, None),
        back: page(OnPage::Back, None),
        move_: page(OnPage::Back, None),
        held: None,
    },
};

pub const PAIRED: &Builtin = &Builtin {
    tree: || {
        let space = &LAYOUT.space;
        let content = Size::CONTENT;
        let lines = [
            Node::leaf(SYMBOL, Mark::Phone as u8, content),
            self::space(space.gap),
            Node::leaf(TEXT, Say::PhonePaired as u8, content),
            self::space(space.pad + space.gap),
            Node::row(3, content),
            self::space(space.pad),
            Node::leaf(FACTS, Card::Phone as u8, Size::grow(1)),
            self::space(space.pad),
        ];
        stop_screen(&lines, 5, [Item::Forget, Item::PairNew])
    },
    buttons: Buttons {
        act: page(OnPage::Pair, Some(Icon::More)),
        back: page(OnPage::Back, None),
        move_: page(OnPage::Forget, Some(Icon::Trash)),
        held: None,
    },
};

pub const FORGET: &Builtin = &Builtin {
    tree: || {
        let space = &LAYOUT.space;
        let content = Size::CONTENT;
        let lines = [
            Node::leaf(SYMBOL, Mark::ForgetPhone as u8, content),
            self::space(space.gap),
            Node::leaf(TEXT, Say::ForgetPhone as u8, content),
            self::space(space.gap),
            Node::leaf(TEXT, Say::StopsConnecting as u8, content),
        ];
        stop_screen(&lines, 5, [Item::Keep, Item::ForgetNow])
    },
    buttons: Buttons {
        act: page(OnPage::Answer(true), Some(Icon::Trash)),
        back: page(OnPage::Back, None),
        move_: page(OnPage::Back, Some(Icon::Back)),
        held: None,
    },
};

fn stop_screen(lines: &[Node], children: u8, buttons: [Item; 2]) -> Nodes {
    let space = &LAYOUT.space;
    let foot = [
        Node::leaf(SPACE, 0, Size::grow(1)),
        Node::row(5, Size::CONTENT),
        self::space(space.gap),
        Node::leaf(ITEM, buttons[0] as u8, Size::grow(1)),
        self::space(space.gap),
        Node::leaf(ITEM, buttons[1] as u8, Size::grow(1)),
        self::space(space.gap),
        self::space(space.gap),
    ];
    [
        Node::col(children + 4, Size::grow(1)),
        self::space(space.lead),
    ]
    .into_iter()
    .chain(lines.iter().copied())
    .chain(foot)
    .collect()
}
