//! Random rides, faults and presses in the twin, each session a function of
//! its seed.
pub mod plan;

pub use plan::{FIXED, Plan, draw};
