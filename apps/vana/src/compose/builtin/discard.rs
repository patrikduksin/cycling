//! DISCARD RIDE?: the ride time and distance, and that the ride will not be
//! saved. ACT (trash) discards it, BACK returns to END RIDE?. Once answered
//! it stays while the ride is saved to be hidden, and the buttons rest.
use super::question::{block, button, gap, panel, say};
use super::{Builtin, Choices, Nodes};
use crate::compose::input::{Action, Buttons};
use crate::compose::node::Node;
use crate::compose::tiles::line::Say;
use crate::screens::cue::Icon;
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree,
    buttons: Buttons {
        act: button(Action::Ride(Role::Act), Icon::Trash),
        back: button(Action::Ride(Role::Back), Icon::Back),
        move_: None,
        held: None,
    },
});

pub const SAVING: Option<&Builtin> = Some(&Builtin {
    tree,
    buttons: Buttons::FREE,
});

fn tree(_: &Choices) -> Nodes {
    QUESTION.into_iter().collect()
}

const QUESTION: [Node; 9] = [
    panel(1),
    block(7),
    say(Say::DiscardTitle),
    gap(20),
    say(Say::RideTime),
    gap(14),
    say(Say::RideDistance),
    gap(18),
    say(Say::DiscardLine),
];
