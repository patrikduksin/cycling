//! While a ride is paused the whole screen says so, phone style: the pause
//! on an amber disc, Paused and for how long, or Auto paused and that riding
//! on resumes, the ride so far as a card, and END over MOVE and RESUME over
//! ACT. MOVE and BACK ask End ride?, ACT resumes; a swipe shows the ride
//! pages.
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
            Mark::Paused,
            Say::Paused,
            Say::PausedFor,
            Card::Ride,
            [Item::End, Item::Resume],
        )
        .into_iter()
        .collect()
    },
    buttons: Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: Some(Icon::Play),
        }),
        back: Some(Button {
            action: Action::Ride(Role::Back),
            cue: None,
        }),
        move_: Some(Button {
            action: Action::Ride(Role::Move),
            cue: Some(Icon::Stop),
        }),
        held: None,
    },
};
