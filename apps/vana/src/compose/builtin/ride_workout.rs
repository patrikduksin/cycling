//! The workout page as a tree of tiles: the status bar, then the workout's
//! name and step count, the step and its target, the reading the target
//! checks as the hero, the target band, the verdict, the time left over the
//! step's progress, the next step, and the whole workout as a chart. The
//! words and the chart share the room by weight, and the words' share
//! becomes even gaps between them. Holding MOVE, or a tap on the time left,
//! adds to the step.
use super::{Builtin, Choices};
use crate::compose::input::{Action, Button, Buttons, Go};
use crate::compose::node::{Flags, MAX_NODES, Node, Size, When};
use crate::compose::registry::{
    SPACE, STATUS, STEP_LEFT, TARGET_BAND, WORKOUT_CHART, WORKOUT_LINE,
};
use crate::compose::tiles::workout_line::Line;
use device_api::input::Role;

pub const BUILTIN: Option<&Builtin> = Some(&Builtin {
    tree: |_: &Choices| tree().into_iter().collect(),
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
});

const BAR_UNITS: u8 = 24;
const RADAR_UNITS: u8 = crate::screens::ride::RADAR_WIDTH as u8;
const LEFT_UNITS: u8 = crate::screens::ride::TEXT_LEFT as u8;
const RIGHT_UNITS: u8 = crate::screens::ride::TEXT_RIGHT as u8;
/// How the words and the chart share the rows they leave.
const WORDS_WEIGHT: u8 = 1;
const CHART_WEIGHT: u8 = 1;
pub const NODES: usize = 16;
const _: () = assert!(NODES <= MAX_NODES);

const fn space(units: u8) -> Node {
    Node::leaf(SPACE, 0, Size::units(units))
}

const fn line(line: Line) -> Node {
    Node::leaf(WORKOUT_LINE, line as u8, Size::CONTENT)
}

pub const fn tree() -> [Node; NODES] {
    let content = Size::CONTENT;
    [
        Node::col(2, Size::grow(1)),
        Node::leaf(STATUS, 0, Size::units(BAR_UNITS)),
        Node::row(4, Size::grow(1)),
        space(RADAR_UNITS).when(When::new(Flags::RADAR, Flags::NONE)),
        space(LEFT_UNITS),
        Node::col(2, Size::grow(1)),
        Node::col(7, Size::grow(WORDS_WEIGHT)),
        line(Line::Title),
        line(Line::Step),
        line(Line::Reading).hero(),
        Node::leaf(TARGET_BAND, 0, content),
        line(Line::Verdict),
        Node::leaf(STEP_LEFT, 0, content),
        line(Line::Next),
        Node::leaf(WORKOUT_CHART, 0, Size::grow(CHART_WEIGHT)),
        space(RIGHT_UNITS),
    ]
}
