//! Ride traces for agents: decode one into a timestamped timeline, tell it
//! as spans and events, find moments, export OTLP, and write a regression
//! test that replays it. `src/bin/trace.rs` is the command line.
pub mod ant_page;
pub mod battery;
pub mod card;
pub mod diff;
pub mod divergence;
pub mod dump;
pub mod find;
pub mod fixture;
pub mod otlp;
pub mod report;
pub mod scenario;
pub mod secret;
pub mod shrink;
pub mod story;
pub mod surgery;
pub mod timeline;
pub mod world;
