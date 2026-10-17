//! IEEE CRC32 (reflected 0xEDB88320) shared by persisted records, media markers
//! and host transfer checks.

const fn table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = index as u32;
        let mut bit = 0;
        while bit < 8 {
            value = (value >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(value & 1));
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}

/// Byte-at-a-time table: whole-volume checks over hundreds of MB stay fast.
static TABLE: [u32; 256] = table();

/// Incremental CRC32 for records whose checked bytes are not contiguous.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crc32(u32);

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    pub const fn new() -> Self {
        Self(u32::MAX)
    }

    pub fn update(mut self, bytes: &[u8]) -> Self {
        for &byte in bytes {
            self.0 = (self.0 >> 8) ^ TABLE[usize::from(self.0 as u8 ^ byte)];
        }
        self
    }

    pub const fn finish(self) -> u32 {
        !self.0
    }
}

pub fn crc32(bytes: &[u8]) -> u32 {
    Crc32::new().update(bytes).finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_ieee_check_value_and_is_incremental() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
        assert_eq!(
            Crc32::new().update(b"1234").update(b"56789").finish(),
            0xcbf4_3926
        );
        assert_eq!(crc32(&[]), 0);
    }
}
