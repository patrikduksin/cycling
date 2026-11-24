use super::{BAR_UNITS, Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, ListAct, OnPage};
use crate::compose::kit::item::Button as Item;
use crate::compose::node::{Node, Size};
use crate::compose::registry::{
    ITEM, SENSOR_BACK, SENSOR_FACTS, SENSOR_HEAD, SENSOR_HINT, SENSOR_LIST, SENSOR_SEARCH,
    SENSOR_TITLE, SPACE, STATUS,
};
use crate::screens::cue::Icon;
use crate::screens::sensors::{List, Part};
use crate::screens::theme::LAYOUT;

const BACK: Button = Button {
    action: Action::Page(OnPage::Back),
    cue: None,
};

const fn button(action: Action, cue: Icon) -> Option<Button> {
    Some(Button {
        action,
        cue: Some(cue),
    })
}

const fn list(act: Icon) -> Buttons {
    Buttons {
        act: button(Action::List(ListAct::Act), act),
        back: Some(BACK),
        move_: button(Action::List(ListAct::Step), Icon::Step),
        held: None,
    }
}

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| glance(List::Sensors),
    buttons: list(Icon::Open),
};

pub const NEARBY: &Builtin = &Builtin {
    tree: |_| glance(List::Nearby),
    buttons: list(Icon::Check),
};

pub const DETAIL: &Builtin = &Builtin {
    tree: |_| page(Part::Detail, [Some(Item::Forget), None]),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Page(OnPage::Forget),
            cue: None,
        }),
        back: Some(BACK),
        move_: button(Action::Page(OnPage::Forget), Icon::Trash),
        held: None,
    },
};

pub const POWER_METER: &Builtin = &Builtin {
    tree: |_| page(Part::Detail, [Some(Item::Forget), Some(Item::Zero)]),
    buttons: Buttons {
        act: button(Action::Page(OnPage::Calibrate), Icon::Calibrate),
        back: Some(BACK),
        move_: button(Action::Page(OnPage::Forget), Icon::Trash),
        held: None,
    },
};

pub const FORGET: &Builtin = &Builtin {
    tree: |_| page(Part::Forget, [Some(Item::Keep), Some(Item::ForgetNow)]),
    buttons: Buttons {
        act: button(Action::Page(OnPage::Answer(true)), Icon::Trash),
        back: Some(BACK),
        move_: button(Action::Page(OnPage::Back), Icon::Back),
        held: None,
    },
};

const fn space(size: Size) -> Node {
    Node::leaf(SPACE, 0, size)
}

fn glance(list: List) -> Nodes {
    let nearby = list == List::Nearby;
    let search = nearby.then_some(Node::leaf(SENSOR_SEARCH, 0, Size::CONTENT));
    let hint = nearby.then_some(Node::leaf(SENSOR_HINT, 0, Size::CONTENT));
    [
        Node::col(3 + 2 * u8::from(nearby), Size::grow(1)),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
        Node::leaf(SENSOR_TITLE, list as u8, Size::CONTENT),
    ]
    .into_iter()
    .chain(search)
    .chain([Node::leaf(SENSOR_LIST, list as u8, Size::grow(1))])
    .chain(hint)
    .collect()
}

fn page(part: Part, buttons: [Option<Item>; 2]) -> Nodes {
    let units = &LAYOUT.space;
    let top = match part {
        Part::Detail => Node::leaf(SENSOR_BACK, 0, Size::CONTENT),
        Part::Forget => space(Size::units(units.top_line)),
    };
    let margin = || space(Size::units(units.pad));
    let gap = || space(Size::units(units.gap));
    let half = |item: Option<Item>| match item {
        Some(item) => Node::leaf(ITEM, item as u8, Size::grow(1)),
        None => space(Size::grow(1)),
    };
    [
        Node::col(7, Size::grow(1)),
        top,
        Node::leaf(SENSOR_HEAD, part as u8, Size::CONTENT),
        space(Size::units(units.gutter)),
        Node::row(3, Size::CONTENT),
        margin(),
        Node::leaf(SENSOR_FACTS, part as u8, Size::grow(1)),
        margin(),
        space(Size::grow(1)),
        Node::row(5, Size::CONTENT),
        gap(),
        half(buttons[0]),
        gap(),
        half(buttons[1]),
        gap(),
        gap(),
    ]
    .into_iter()
    .collect()
}
