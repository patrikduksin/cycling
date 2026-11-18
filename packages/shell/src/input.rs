//! Input as the application sees it: the device's edges, with each button
//! report turned into the role its fixture plays.
use device_api::input::{Point, Role};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Input {
    Touch(Point),
    Release,
    Cancel,
    /// A report from the button that plays `role`, with the device's code.
    Press {
        role: Role,
        code: u16,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge {
    pub received_ms: u64,
    pub input: Input,
}
