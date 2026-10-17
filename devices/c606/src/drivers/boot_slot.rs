//! Reports which application slot the bootloader mapped for this boot.
//!
//! The first MMU entry maps the start of the booted image (its first segment is
//! linked at the beginning of the data-cache window), as in
//! esp-bootloader-esp-idf's `booted_partition`. Reading it needs no partition
//! table or flash access.

const PAGE: u32 = 0x1_0000;
const PAGE_MASK: u32 = 0x3fff;
/// ESP32-S3 MMU entry flags: invalid entry, external RAM instead of flash.
const NOT_FLASH: u32 = (1 << 14) | (1 << 15);
pub const SLOT_A: u32 = 0x2_0000;
pub const SLOT_B: u32 = 0x76_0000;

/// Slot name for one MMU entry; anything but a slot start is `unknown`.
pub fn name(entry: u32) -> &'static str {
    if entry & NOT_FLASH != 0 {
        return "unknown";
    }
    match (entry & PAGE_MASK) * PAGE {
        SLOT_A => "A",
        SLOT_B => "B",
        _ => "unknown",
    }
}

#[cfg(feature = "hardware")]
pub fn running() -> &'static str {
    // ESP32-S3 DR_REG_MMU_TABLE, entry 0; a read-only register access.
    name(unsafe { (0x600c_5000 as *const u32).read_volatile() })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_starts_name_their_slot() {
        assert_eq!(name(SLOT_A / PAGE), "A");
        assert_eq!(name(SLOT_B / PAGE), "B");
    }

    #[test]
    fn other_pages_and_non_flash_entries_are_unknown() {
        assert_eq!(name(0), "unknown");
        assert_eq!(name(SLOT_A / PAGE + 1), "unknown");
        assert_eq!(name((SLOT_A / PAGE) | (1 << 14)), "unknown");
        assert_eq!(name((SLOT_B / PAGE) | (1 << 15)), "unknown");
    }
}
