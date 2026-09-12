//! Owned reservations in the ota_1 tail. Safe flashing stops before RIDE_BASE.
//! Stock ota_0, the bootloader, partition table and vendor storage are untouched.

pub const SETTINGS_BASE: u32 = 0x00e9_8000;
pub const RIDE_BASE: u32 = 0x00d9_8000;
