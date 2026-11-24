//! Ride done, full screen after a ride is saved: a green tick, "Ride saved"
//! and the day across the top, the picture (`screens::done`) taking most of
//! the screen, three numbers under it and the time in each power zone along
//! the foot. Any key or a tap goes Home.
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons};
use crate::compose::kit::bar::Gauge;
use crate::compose::kit::picture::Frame;
use crate::compose::kit::space;
use crate::compose::kit::stat::Slot;
use crate::compose::kit::symbol::Mark;
use crate::compose::kit::text::Say;
use crate::compose::node::{Node, Size};
use crate::compose::registry::{BAR, PICTURE, SPACE, STAT, SYMBOL, TEXT};
use crate::screens::theme::LAYOUT;
use device_api::input::Role;

const fn home(role: Role) -> Option<Button> {
    Some(Button {
        action: Action::Ride(role),
        cue: None,
    })
}

pub const BUILTIN: &Builtin = &Builtin {
    tree: || tree(),
    buttons: Buttons {
        act: home(Role::Act),
        back: home(Role::Back),
        move_: home(Role::Move),
        held: None,
    },
};

fn tree() -> Nodes {
    let space = &LAYOUT.space;
    let (content, grow) = (Size::CONTENT, Size::grow(1));
    let side = space.gutter + space.tight;
    [
        Node::col(9, grow),
        self::space(2 * space.pad),
        Node::row(7, content),
        self::space(side),
        Node::leaf(SYMBOL, Mark::Saved as u8, content),
        self::space(space.pad),
        Node::leaf(TEXT, Say::RideSaved as u8, content),
        Node::leaf(SPACE, 0, grow),
        Node::leaf(TEXT, Say::SavedOn as u8, content),
        self::space(side),
        self::space(space.gutter),
        Node::row(3, Size::grow(3)),
        self::space(space.tight),
        Node::leaf(PICTURE, Frame::Done as u8, grow),
        self::space(space.tight),
        self::space(space.pad),
        Node::row(5, content),
        self::space(side),
        Node::leaf(STAT, Slot::First as u8, Size::grow(5)),
        Node::leaf(STAT, Slot::Second as u8, Size::grow(4)),
        Node::leaf(STAT, Slot::Third as u8, Size::grow(4)),
        self::space(side),
        Node::leaf(SPACE, 0, grow),
        Node::row(3, content),
        self::space(side),
        Node::leaf(BAR, Gauge::PowerTime as u8, grow),
        self::space(side),
        self::space(2 * space.pad),
    ]
    .into_iter()
    .collect()
}
