//! Synchronous display submission and geometry.
use crate::observation::{Availability, Error};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Geometry {
    pub width: usize,
    pub height: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Rgb565,
}

/// Submission is synchronous. Return, including error, means all DMA access has
/// ended and the device owns its buffers again. No cancellable future is exposed.
pub trait Display {
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn geometry(&self) -> Geometry;
    fn format(&self) -> Format {
        Format::Rgb565
    }
    fn submit(&mut self, pixel: impl Fn(usize, usize) -> u16) -> Result<(), Error>;
}
