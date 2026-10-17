#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]
#[cfg(test)]
extern crate std;
#[cfg(feature = "hardware")]
pub mod board;
#[cfg(feature = "hardware")]
pub mod capabilities;
pub mod drivers;
#[cfg(feature = "hardware")]
pub mod stack;
