//! Little-endian fields at fixed offsets of persisted and on-media layouts.
//! Offsets come from the owning layout; out-of-range access is a layout bug.

fn field<const N: usize>(bytes: &[u8], offset: usize) -> [u8; N] {
    let mut value = [0; N];
    value.copy_from_slice(&bytes[offset..offset + N]);
    value
}

pub fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(field(bytes, offset))
}

pub fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(field(bytes, offset))
}

pub fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(field(bytes, offset))
}

pub fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

pub fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_at_offsets() {
        let mut bytes = [0; 16];
        put_u16(&mut bytes, 1, 0x0102);
        put_u32(&mut bytes, 3, 0x0304_0506);
        put_u64(&mut bytes, 7, 0x0708_090a_0b0c_0d0e);
        assert_eq!(bytes[1..3], [2, 1]);
        assert_eq!(get_u16(&bytes, 1), 0x0102);
        assert_eq!(get_u32(&bytes, 3), 0x0304_0506);
        assert_eq!(get_u64(&bytes, 7), 0x0708_090a_0b0c_0d0e);
    }
}
