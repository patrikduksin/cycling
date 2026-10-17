//! Structured training: workouts, the library riders pick them from, their
//! upload format and the player that runs one during a ride.
//!
//! `model` owns steps, targets and repeats; `library` the built-ins and the
//! uploads on the FILES volume; `codec` the upload bytes; `player` the
//! ride-time state, including the 20-minute and ramp FTP tests; `history`
//! the record of how a saved ride's workout went. Screens live in
//! `screens::workouts` (the picker) and `screens::ride::workout` (the page).
pub mod codec;
pub mod history;
pub mod library;
pub mod model;
pub mod player;
