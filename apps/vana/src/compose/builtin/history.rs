//! Rides: this week at a glance, with the focus's place among the rides,
//! over the saved rides, newest first, one card each. Each card names the
//! ride by its start and says how long, how far and how hard it was. MOVE
//! focuses the next ride, ACT opens it, a tap opens the ride under the
//! finger, and BACK goes Home. Without a ride the list says how one gets
//! here.
use super::workouts::list;
use super::{Builtin, Choices, Nodes};
use crate::compose::data::ListKind;
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Node, Size};
use crate::compose::registry::GLANCE;
use crate::compose::tiles::entry::Line;
use crate::screens::workout::Page;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| tree(),
    buttons: Buttons {
        act: None,
        back: Some(Button {
            action: Action::Go(Go::Open(Page::Home)),
            cue: None,
        }),
        move_: None,
        held: None,
    },
});

/// A ride's card: its start over its time and distance.
const CARD_UNITS: u8 = 54;

fn tree() -> Nodes {
    let glance = Node::leaf(GLANCE, 0, Size::CONTENT);
    list(
        glance,
        ListKind::Rides,
        CARD_UNITS,
        &[Line::Title, Line::Detail],
        &[],
    )
}
