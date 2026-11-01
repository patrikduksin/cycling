//! Ride traces for agents: decode one into a timestamped timeline, tell it
//! as spans and events, find moments, export OTLP, and write a regression
//! test that replays it. `src/bin/trace.rs` is the command line.
pub mod ant_page;
pub mod battery;
#[cfg(feature = "cycling")]
pub mod divergence;
pub mod file;
pub mod find;
#[cfg(feature = "cycling")]
pub mod origin;
pub mod otlp;
#[cfg(feature = "cycling")]
pub mod rebuild;
pub mod secret;
pub mod story;
pub mod stub;
pub mod timeline;
