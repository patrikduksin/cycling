#![no_std]

#[cfg(test)]
extern crate std;

pub mod companion;
pub mod crash;
pub mod gps;
pub mod idle;
pub mod input;
pub mod network;
pub mod network_time;
pub mod preferences;
pub mod sdmmc_probe;
pub mod storage;
pub mod uart_ring;

pub mod log_record;

#[cfg(feature = "cycling")]
pub mod sdk;

pub mod ble_transport;
pub mod capabilities;
pub mod positioning;
pub mod terminal_protocol;
