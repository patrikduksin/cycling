//! Structured training: workouts, the library riders pick them from, their
//! upload format and the player that runs one during a ride.
//!
//! `model` owns steps, targets and repeats; `library` the built-ins and the
//! upload store; `codec` the upload bytes; `player` the ride-time state,
//! including the 20-minute and ramp FTP tests. Screens live in
//! `screens::workouts` (the picker) and `screens::ride::workout` (the page).
pub mod codec;
pub mod library;
pub mod model;
pub mod player;
