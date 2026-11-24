//! Ids 36–39: the workout page's tiles.
use crate::compose::tile::{Component, Erased};
use crate::compose::tiles::{step_left, target_band, workout_chart, workout_line};

pub const WORKOUT_LINE: u8 = 36;
pub const TARGET_BAND: u8 = 37;
pub const STEP_LEFT: u8 = 38;
pub const WORKOUT_CHART: u8 = 39;

pub(super) static TILES: &[&dyn Component] = &[
    &Erased::<workout_line::WorkoutLine>::NEW,
    &Erased::<target_band::TargetBand>::NEW,
    &Erased::<step_left::StepLeft>::NEW,
    &Erased::<workout_chart::WorkoutChart>::NEW,
];
