#![no_std]
#[cfg(test)]
extern crate std;
pub mod app;
pub mod clock;
pub mod daylight;
pub mod ride;
pub mod screens;
pub mod sensors;
#[cfg(test)]
mod testing;
pub mod workout;
