#![no_std]

#[cfg(test)]
extern crate std;

extern crate self as device_api;

pub mod absent;
pub mod ant;
pub mod ble_transport;
pub mod bulk;
pub mod capability;
pub mod connectivity;
pub mod console;
pub mod crash;
pub mod display;
pub mod gps;
pub mod input;
pub mod journal;
pub mod observation;
pub mod position_control;
pub mod positioning;
pub mod power;
pub mod sensors;
pub mod server;
pub mod sound;
pub mod storage;

pub use absent::Absent;
