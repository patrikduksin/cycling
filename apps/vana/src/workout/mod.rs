//! Structured training: workouts, today's workout, their upload format and
//! the player that runs one during a ride.
//!
//! `model` owns steps, targets and repeats; `today` the one workout stored on
//! the FILES volume; `codec` the upload bytes; `player` the ride-time state,
//! including the 20-minute and ramp FTP tests; `history` the record of how a
//! saved ride's workout went. `screens::ride::workout` draws the page.
pub mod codec;
#[cfg(any(test, feature = "invariants"))]
pub mod fixtures;
pub mod history;
pub mod model;
pub mod player;
pub mod shape;
pub mod today;
