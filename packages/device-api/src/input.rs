//! Physical input observations, without gesture policy.
use crate::observation::{Availability, Observation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Point {
    pub x: u16,
    pub y: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    TopLeft,
    BottomLeft,
    BottomRight,
    Center,
}

impl Button {
    pub fn index(self) -> usize {
        match self {
            Self::TopLeft => 0,
            Self::BottomLeft => 1,
            Self::BottomRight => 2,
            Self::Center => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Controls {
    pub touch: Availability,
    pub buttons: &'static [Button],
}

pub trait InputSource {
    fn controls(&self) -> Controls;
    fn take_edge(&mut self) -> Option<Edge>;
}

#[derive(Clone, Copy)]
pub struct InputSnapshot {
    pub battery: Observation<(u8, u16)>,
    pub power: Observation<u8>,
    pub button_counts: [u32; 3],
    pub touch_available: bool,
    pub touch_errors: u32,
    pub companion_valid: u32,
    pub companion_bad_crc: u32,
    pub uart_errors: u32,
    pub input_lost: u32,
}

pub trait InputObservation {
    fn snapshot(&self, now_ms: u64) -> Option<InputSnapshot>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Touch(Point),
    Release,
    /// Cancel all consumer-held input. Caused by invalid/lost touch or edge loss.
    Cancel,
    /// Known physical button with uninterpreted companion report code. Repeated
    /// code 1 reports do not establish a physical release; clients choose policy.
    Button {
        button: Button,
        code: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub sequence: u32,
    pub received_ms: u64,
    pub input: Input,
}
