//! End ride?, which MOVE or BACK opens from the paused screen, phone style:
//! the stop on a red disc, the question and that the ride will be saved,
//! the ride as a card, and BACK over MOVE and END over ACT. ACT saves and
//! ends the ride; MOVE or BACK returns to the paused ride. Two different
//! keys, so a double press cannot end a ride.
use super::Builtin;
use crate::compose::input::{Action, Button, Buttons};
use crate::compose::kit::facts::Card;
use crate::compose::kit::item::Button as Item;
use crate::compose::kit::phone;
use crate::compose::kit::symbol::Mark;
use crate::compose::kit::text::Say;
use crate::screens::cue::Icon;
use device_api::input::Role;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| {
        phone(
            Mark::Stopping,
            Say::EndRide,
            Say::WillSave,
            Card::Ride,
            [Item::Back, Item::Save],
        )
        .into_iter()
        .collect()
    },
    buttons: Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: Some(Icon::Stop),
        }),
        back: Some(Button {
            action: Action::Ride(Role::Back),
            cue: None,
        }),
        move_: Some(Button {
            action: Action::Ride(Role::Move),
            cue: Some(Icon::Back),
        }),
        held: None,
    },
};
