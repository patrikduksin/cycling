//! Byte-oriented device console access.

use crate::capability::capability;

#[capability(
    id = 5,
    outside = "a byte pipe for the composition's terminal task; its lines enter the world as commands"
)]
pub trait Console {
    fn read(&mut self) -> Option<u8>;
    fn write(&mut self, byte: u8) -> bool;
    fn flush(&mut self);
}
