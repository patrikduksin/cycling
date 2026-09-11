//! Portable parsing and status validation for the read-only SD/MMC probe.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProtocolError {
    AppCommand,
    Capacity,
    CardStatus,
    UnsupportedCsd,
}

// SD Physical Layer status error bits, including SWITCH_ERROR and AKE_SEQ_ERROR.
const R1_ERROR_MASK: u32 = 0xfdff_e088;

pub fn check_r1(status: u32) -> Result<(), ProtocolError> {
    if status & R1_ERROR_MASK == 0 {
        Ok(())
    } else {
        Err(ProtocolError::CardStatus)
    }
}

pub fn check_app_command(status: u32) -> Result<(), ProtocolError> {
    check_r1(status)?;
    if status & (1 << 5) != 0 {
        Ok(())
    } else {
        Err(ProtocolError::AppCommand)
    }
}

pub fn check_r6(response: u32) -> Result<u16, ProtocolError> {
    // R6 publishes COM_CRC_ERROR, ILLEGAL_COMMAND and ERROR in bits 15..13.
    if response & 0xe000 != 0 {
        return Err(ProtocolError::CardStatus);
    }
    let rca = (response >> 16) as u16;
    if rca == 0 {
        Err(ProtocolError::CardStatus)
    } else {
        Ok(rca)
    }
}

pub fn sd_sector_count(csd: [u32; 4]) -> Result<u64, ProtocolError> {
    let value = response_u128(csd);
    match ((value >> 126) & 3) as u8 {
        0 => {
            let read_block_len = ((value >> 80) & 0xf) as u32;
            let block_len = 1u64
                .checked_shl(read_block_len)
                .ok_or(ProtocolError::Capacity)?;
            let c_size = ((value >> 62) & 0xfff) as u64;
            let c_size_mult = ((value >> 47) & 7) as u32;
            let native_blocks = (c_size + 1)
                .checked_mul(1u64 << (c_size_mult + 2))
                .ok_or(ProtocolError::Capacity)?;
            native_blocks
                .checked_mul(block_len)
                .map(|bytes| bytes / 512)
                .filter(|sectors| *sectors != 0)
                .ok_or(ProtocolError::Capacity)
        }
        1 => Ok((((value >> 48) & 0x3f_ffff) as u64 + 1) * 1024),
        _ => Err(ProtocolError::UnsupportedCsd),
    }
}

pub fn response_u128(words: [u32; 4]) -> u128 {
    (u128::from(words[3]) << 96)
        | (u128::from(words[2]) << 64)
        | (u128::from(words[1]) << 32)
        | u128::from(words[0])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_checks_require_app_command_and_reject_card_errors() {
        assert_eq!(check_r1(1 << 8), Ok(()));
        assert_eq!(check_app_command((1 << 8) | (1 << 5)), Ok(()));
        assert_eq!(check_app_command(1 << 8), Err(ProtocolError::AppCommand));
        assert_eq!(check_r1(1 << 22), Err(ProtocolError::CardStatus));
        assert_eq!(check_r6(0x1234_0000), Ok(0x1234));
        assert_eq!(check_r6(1 << 15), Err(ProtocolError::CardStatus));
    }

    #[test]
    fn parses_sd_hc_sector_count_and_rejects_sduc() {
        let c_size = 0x12_345u128;
        let high_capacity = (1u128 << 126) | (c_size << 48);
        let words = [
            high_capacity as u32,
            (high_capacity >> 32) as u32,
            (high_capacity >> 64) as u32,
            (high_capacity >> 96) as u32,
        ];
        assert_eq!(sd_sector_count(words), Ok((0x12_345 + 1) * 1024));

        let sduc = 2u128 << 126;
        let words = [sduc as u32, 0, 0, (sduc >> 96) as u32];
        assert_eq!(sd_sector_count(words), Err(ProtocolError::UnsupportedCsd));
    }

    #[test]
    fn sdsc_capacity_is_converted_from_native_blocks_to_sectors() {
        // 1024-byte blocks, C_SIZE=1023, multiplier=4 => 16,384 native
        // blocks = 32,768 512-byte sectors.
        let value = (10u128 << 80) | (1023u128 << 62) | (2u128 << 47);
        let words = [
            value as u32,
            (value >> 32) as u32,
            (value >> 64) as u32,
            (value >> 96) as u32,
        ];
        assert_eq!(sd_sector_count(words), Ok(32_768));
    }
}
