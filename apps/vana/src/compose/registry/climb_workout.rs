//! Ids 32–39: the climb and workout pages' tiles.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{
    climb_effort, climb_profile, road, steepness, step_left, target_band, workout_chart,
    workout_line,
};

pub const CLIMB_ROAD: u8 = 32;
pub const CLIMB_EFFORT: u8 = 33;
pub const STEEPNESS: u8 = 34;
pub const CLIMB_PROFILE: u8 = 35;
pub const WORKOUT_LINE: u8 = 36;
pub const TARGET_BAND: u8 = 37;
pub const STEP_LEFT: u8 = 38;
pub const WORKOUT_CHART: u8 = 39;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<road::Road>::NEW,
    &Erased::<climb_effort::ClimbEffort>::NEW,
    &Erased::<steepness::Steepness>::NEW,
    &Erased::<climb_profile::ClimbProfile>::NEW,
    &Erased::<workout_line::WorkoutLine>::NEW,
    &Erased::<target_band::TargetBand>::NEW,
    &Erased::<step_left::StepLeft>::NEW,
    &Erased::<workout_chart::WorkoutChart>::NEW,
];
