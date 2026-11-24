//! The Workout ride page: the top line, then the target range left of the
//! reading it checks, the target band, the time left in the step, the next
//! step, and the whole workout as a chart that takes the rest. The radar
//! strip, while one is paired, takes its room at the left. Holding MOVE, or
//! a tap on the time left, adds to the step.
use super::{Builtin, Nodes};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::kit::bar::Gauge;
use crate::compose::kit::chart::Plot;
use crate::compose::kit::step::Step;
use crate::compose::node::{Flags, Node, Size, When};
use crate::compose::registry::{BAR, CHART, SPACE, STEP, TOP_LINE};
use crate::screens::theme::LAYOUT;
use device_api::input::Role;

pub const BUILTIN: &Builtin = &Builtin {
    tree: |_| tree(),
    buttons: Buttons {
        act: Some(Button {
            action: Action::Ride(Role::Act),
            cue: None,
        }),
        back: Some(Button {
            action: Action::Ride(Role::Back),
            cue: None,
        }),
        move_: Some(Button {
            action: Action::Go(Go::Next),
            cue: None,
        }),
        held: Some(Action::ExtendStep),
    },
};

const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;
const PAD: u8 = LAYOUT.space.pad;

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

const fn step(step: Step) -> Node {
    Node::leaf(STEP, step as u8, Size::CONTENT)
}

pub fn tree() -> Nodes {
    let grow = Size::grow(1);
    let content = Size::CONTENT;
    let radar = When::new(Flags::RADAR, Flags::NONE);
    [
        Node::col(2, grow),
        Node::leaf(TOP_LINE, 0, content),
        Node::row(2, grow),
        space(RADAR_UNITS).when(radar),
        Node::col(11, grow),
        space(2 * PAD),
        Node::row(3, content),
        step(Step::Range),
        Node::leaf(SPACE, 0, grow),
        step(Step::Reading).hero(),
        space(2 * PAD),
        Node::leaf(BAR, Gauge::Step as u8, content),
        space(PAD),
        step(Step::Left),
        space(2 * PAD),
        step(Step::Next),
        space(2 * PAD),
        Node::leaf(CHART, Plot::Workout as u8, grow),
        space(PAD),
    ]
    .into_iter()
    .collect()
}
