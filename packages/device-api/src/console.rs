//! Byte-oriented device console access.

pub trait Console {
    fn read(&mut self) -> Option<u8>;
    fn write(&mut self, byte: u8) -> bool;
    fn flush(&mut self);
}
