//! The power sheets over every page: TURN OFF? after BACK on Home, the idle
//! warning, the farewell before the device turns off, and the charge while
//! the device rests on USB power with VANA off. Each is one ring with its
//! lines under it, centred down the panel. On TURN OFF ACT (check) turns the
//! device off and anything else returns; with rides the phone has not
//! confirmed it says to sync them first and no button turns it off. Any
//! button keeps VANA on through the warning.
use super::question::{answer, block, gap, panel, say};
use super::{Builtin, Choices, Nodes, ScreenId};
use crate::compose::input::{Action, Button, Buttons, OnPage};
use crate::compose::node::{Node, Size};
use crate::compose::registry::POWER_RING;
use crate::compose::tiles::line::Say;
use crate::compose::tiles::power::Ring;
use crate::screens::cue::Icon;
use crate::screens::power::Sheet;

impl ScreenId {
    /// The screen `sheet` shows.
    pub const fn power(sheet: Sheet) -> Self {
        match sheet {
            Sheet::Confirm { unsaved: 0 } => Self::TurnOff,
            Sheet::Confirm { .. } => Self::SyncFirst,
            Sheet::Warning { .. } => Self::AutoOff,
            Sheet::TurningOff { .. } => Self::TurningOff,
            Sheet::Charging { .. } => Self::Charging,
        }
    }
}

const RETURN: Option<Button> = Some(Button {
    action: Action::Page(OnPage::Answer(false)),
    cue: None,
});

pub const TURN_OFF: &Builtin = &Builtin {
    tree: |_: &Choices| question(Say::RidesSaved).into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: answer(true, Icon::Check),
        back: answer(false, Icon::Back),
        move_: RETURN,
        held: None,
    },
};

pub const SYNC_FIRST: &Builtin = &Builtin {
    tree: |_: &Choices| question(Say::SyncFirst).into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: RETURN,
        back: answer(false, Icon::Back),
        move_: RETURN,
        held: None,
    },
};

pub const AUTO_OFF: &Builtin = &Builtin {
    tree: |_: &Choices| WARNING.into_iter().collect::<Nodes>(),
    buttons: Buttons {
        act: RETURN,
        back: RETURN,
        move_: RETURN,
        held: None,
    },
};

pub const TURNING_OFF: &Builtin = &Builtin {
    tree: |_: &Choices| {
        [panel(1), ring(Ring::Farewell)]
            .into_iter()
            .collect::<Nodes>()
    },
    buttons: Buttons::FREE,
};

pub const CHARGING: &Builtin = &Builtin {
    tree: |_: &Choices| CHARGE.into_iter().collect::<Nodes>(),
    buttons: Buttons::FREE,
};

const fn ring(ring: Ring) -> Node {
    Node::leaf(POWER_RING, ring as u8, Size::CONTENT)
}

const fn question(line: Say) -> [Node; 7] {
    [
        panel(1),
        block(5),
        ring(Ring::Symbol),
        gap(30),
        say(Say::TurnOff),
        gap(16),
        say(line),
    ]
}

const WARNING: [Node; 9] = [
    panel(1),
    block(7),
    say(Say::TurningOffIn),
    gap(26),
    ring(Ring::Countdown),
    gap(16),
    say(Say::PressAny),
    gap(10),
    say(Say::StayOn),
];

const CHARGE: [Node; 5] = [
    panel(1),
    block(3),
    ring(Ring::Charge),
    gap(30),
    say(Say::ChargeState),
];
