#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]
#[cfg(test)]
extern crate std;
#[cfg(any(feature = "hardware", test))]
pub mod activity;
#[cfg(feature = "hardware")]
pub mod board;
#[cfg(feature = "hardware")]
pub mod capabilities;
pub mod drivers;
pub mod fitted;
#[cfg(any(feature = "scripted-gnss", test))]
mod scripted;
#[cfg(feature = "hardware")]
pub mod stack;
