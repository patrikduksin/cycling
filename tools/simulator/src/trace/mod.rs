//! Ride traces for agents: decode one into a timestamped timeline, tell it
//! as spans and events, find moments, export OTLP, and write a regression
//! test that replays it. `src/bin/trace.rs` is the command line.
pub mod ant_page;
pub mod file;
pub mod find;
pub mod otlp;
pub mod story;
pub mod stub;
pub mod timeline;
