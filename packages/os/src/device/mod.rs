//! C606 hardware ownership. Protocol and application services remain separate.

pub mod ant_protocol;
pub mod c606;
pub mod companion_startup;
pub mod companion_uart;
pub mod crash_rtc;
pub mod display;
pub mod gps_uart;
pub mod psram;
pub mod sdmmc;
pub mod storage;
pub mod touch;

pub mod bluetooth;
pub mod services;
pub mod usb;
pub mod wifi;

pub mod sound_protocol;

pub mod power_transition;
pub mod sleep;
