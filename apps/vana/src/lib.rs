#![no_std]

#[cfg(all(feature = "planted-bug", target_os = "none"))]
compile_error!("planted-bug is the fuzzer's proof and never reaches a device");

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
