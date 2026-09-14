#![no_std]

#[cfg(any(test, feature = "simulator"))]
extern crate std;

pub mod ant;
pub mod companion;
pub mod crash;
pub mod gps;
pub mod input;
pub mod network;
pub mod network_time;
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

pub mod shell;

#[cfg(any(test, feature = "simulator"))]
pub mod simulator;

pub mod harness;

#[cfg(feature = "cycling")]
pub mod sdk_runtime;

pub mod connectivity;

pub mod bulk;

pub mod sound;

pub mod companion_sensors;

pub mod position_control;

pub mod peripheral_commands;
