use device_api::bulk::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout {
    pub total: u64,
    pub stock_start: u64,
    pub stock_sectors: u64,
    pub custom_start: u64,
    pub custom_sectors: u64,
}

pub const OWNED_MAGIC: &[u8; 16] = b"CYCLING-BULK\0\0\0\0";

fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

/// IEEE CRC32, shared by the on-media ownership marker and maintenance tools.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// Accept only the agreed two-primary-partition layout: FAT32 followed by an
/// application reservation (0xDA). MBR, unused entries and marker stay outside
/// application data. Extended/GPT/overlapping/zero-sized layouts are rejected.
pub fn parse_mbr(mbr: &[u8; SECTOR_SIZE], total: u64) -> Result<Layout, Error> {
    if mbr[510..] != [0x55, 0xaa] || mbr[478..510].iter().any(|v| *v != 0) {
        return Err(Error::Unsupported);
    }
    let stock = &mbr[446..462];
    let custom = &mbr[462..478];
    if !matches!(stock[0], 0 | 0x80)
        || custom[0] != 0
        || !matches!(stock[4], 0x0b | 0x0c)
        || custom[4] != 0xda
    {
        return Err(Error::Unsupported);
    }
    let layout = Layout {
        total,
        stock_start: u64::from(u32_at(stock, 8)),
        stock_sectors: u64::from(u32_at(stock, 12)),
        custom_start: u64::from(u32_at(custom, 8)),
        custom_sectors: u64::from(u32_at(custom, 12)),
    };
    if layout.stock_start == 0
        || layout.stock_sectors == 0
        || layout.custom_sectors < 2
        || layout.stock_start + layout.stock_sectors > layout.custom_start
        || layout.custom_start + layout.custom_sectors > total
    {
        return Err(Error::Range);
    }
    Ok(layout)
}

/// Construct the explicit reservation marker for offline maintenance tools.
/// This only produces bytes; it never writes or initializes media.
pub fn ownership_marker(mbr: &[u8; SECTOR_SIZE], total: u64) -> Result<[u8; SECTOR_SIZE], Error> {
    let layout = parse_mbr(mbr, total)?;
    let mut marker = [0; SECTOR_SIZE];
    marker[..16].copy_from_slice(OWNED_MAGIC);
    marker[16..20].copy_from_slice(&1u32.to_le_bytes());
    marker[20..24].copy_from_slice(&(SECTOR_SIZE as u32).to_le_bytes());
    for (i, value) in [
        layout.total,
        layout.stock_start,
        layout.stock_sectors,
        layout.custom_start,
        layout.custom_sectors,
    ]
    .iter()
    .enumerate()
    {
        marker[24 + i * 8..32 + i * 8].copy_from_slice(&value.to_le_bytes());
    }
    marker[64..68].copy_from_slice(&crc32(mbr).to_le_bytes());
    let checksum = crc32(&marker[..508]);
    marker[508..].copy_from_slice(&checksum.to_le_bytes());
    Ok(marker)
}

pub fn parse_owned(
    mbr: &[u8; SECTOR_SIZE],
    marker: &[u8; SECTOR_SIZE],
    total: u64,
) -> Result<OwnedInfo, Error> {
    let layout = parse_mbr(mbr, total)?;
    if marker[..16] != OWNED_MAGIC[..]
        || u32_at(marker, 16) != 1
        || u32_at(marker, 20) != SECTOR_SIZE as u32
        || u64_at(marker, 24) != layout.total
        || u64_at(marker, 32) != layout.stock_start
        || u64_at(marker, 40) != layout.stock_sectors
        || u64_at(marker, 48) != layout.custom_start
        || u64_at(marker, 56) != layout.custom_sectors
        || u32_at(marker, 64) != crc32(mbr)
        || marker[68..508].iter().any(|v| *v != 0)
        || u32_at(marker, 508) != crc32(&marker[..508])
    {
        return Err(Error::Unsupported);
    }
    Ok(OwnedInfo {
        start_sector: layout.custom_start + 1,
        sectors: layout.custom_sectors - 1,
        sector_size: SECTOR_SIZE as u16,
    })
}

pub fn discover(media: &mut impl Read) -> Result<OwnedInfo, Error> {
    let total = media.info()?.sectors;
    let mut mbr = [0; SECTOR_SIZE];
    media.read(0, &mut mbr)?;
    let layout = parse_mbr(&mbr, total)?;
    let mut marker = [0; SECTOR_SIZE];
    media.read(layout.custom_start, &mut marker)?;
    parse_owned(&mbr, &marker, total)
}

pub fn address(sector: u64, sectors: u64, high_capacity: bool) -> Result<u32, Error> {
    if sector >= sectors {
        return Err(Error::Range);
    }
    let address = if high_capacity {
        sector
    } else {
        sector.checked_mul(SECTOR_SIZE as u64).ok_or(Error::Range)?
    };
    address.try_into().map_err(|_| Error::Range)
}

/// Validate a bounded USB chunk before any device read.
pub fn chunk(offset: usize, length: usize) -> Result<core::ops::Range<usize>, Error> {
    if length == 0 || length > 256 {
        return Err(Error::Range);
    }
    let end = offset.checked_add(length).ok_or(Error::Range)?;
    if end > SECTOR_SIZE {
        return Err(Error::Range);
    }
    Ok(offset..end)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn no_wrap_or_access_past_media() {
        assert_eq!(address(0, 0, true), Err(Error::Range));
        assert_eq!(address(7, 8, true), Ok(7));
        assert_eq!(address(8, 8, true), Err(Error::Range));
        assert_eq!(address(u64::MAX - 1, u64::MAX, false), Err(Error::Range));
        assert_eq!(
            address(u32::MAX as u64 + 1, u64::MAX, true),
            Err(Error::Range)
        );
        assert_eq!(address(7, 8, false), Ok(3584));
        assert_eq!(chunk(256, 256), Ok(256..512));
        for (offset, length) in [(0, 0), (0, 257), (511, 2), (usize::MAX, 2)] {
            assert_eq!(chunk(offset, length), Err(Error::Range));
        }
    }

    fn fixture() -> ([u8; 512], [u8; 512]) {
        let mut mbr = [0; 512];
        mbr[450] = 0x0c;
        mbr[454..458].copy_from_slice(&2048u32.to_le_bytes());
        mbr[458..462].copy_from_slice(&1_953_792u32.to_le_bytes());
        mbr[466] = 0xda;
        mbr[470..474].copy_from_slice(&1_955_840u32.to_le_bytes());
        mbr[474..478].copy_from_slice(&5_777_408u32.to_le_bytes());
        mbr[510..].copy_from_slice(&[0x55, 0xaa]);
        let marker = ownership_marker(&mbr, 7_733_248).unwrap();
        (mbr, marker)
    }

    #[test]
    fn ownership_excludes_both_partitions_metadata_and_media_end() {
        let (mbr, marker) = fixture();
        assert_eq!(crc32(b"123456789"), 0xcbf43926);
        let owned = parse_owned(&mbr, &marker, 7_733_248).unwrap();
        assert_eq!(owned.start_sector, 1_955_841);
        assert_eq!(owned.sectors, 5_777_407);
        assert_eq!(owned.address(0), Ok(1_955_841));
        assert_eq!(owned.address(owned.sectors - 1), Ok(7_733_247));
        assert_eq!(owned.address(owned.sectors), Err(Error::Range));
        assert_eq!(owned.address(u64::MAX), Err(Error::Range));
        assert!(parse_owned(&mbr, &marker, 7_733_247).is_err());
    }

    #[test]
    fn reject_ambiguous_partition_tables() {
        let (mbr, _) = fixture();
        for (offset, bytes) in [
            (510, &b"XX"[..]),                      // absent MBR
            (478, &[0x80][..]),                     // third nonempty entry
            (450, &[0x0f][..]),                     // extended rather than FAT32
            (466, &[0x0c][..]),                     // custom was reformatted/retyped
            (462, &[0x80][..]),                     // custom made bootable
            (454, &0u32.to_le_bytes()[..]),         // stock overlaps MBR
            (458, &0u32.to_le_bytes()[..]),         // empty stock
            (470, &1_955_839u32.to_le_bytes()[..]), // overlap by one
            (474, &1u32.to_le_bytes()[..]),         // marker without data
            (474, &5_777_409u32.to_le_bytes()[..]), // beyond medium
            (470, &u32::MAX.to_le_bytes()[..]),
        ] {
            let mut changed = mbr;
            changed[offset..offset + bytes.len()].copy_from_slice(bytes);
            assert!(parse_mbr(&changed, 7_733_248).is_err(), "offset {offset}");
        }
    }

    #[test]
    fn marker_requires_complete_geometry_version_and_checksums() {
        let (mbr, marker) = fixture();
        // Reseal each malformed field to show checks are semantic, not only CRC.
        for offset in [0, 16, 20, 24, 32, 40, 48, 56, 64, 68, 507] {
            let mut changed = marker;
            changed[offset] ^= 1;
            let checksum = crc32(&changed[..508]);
            changed[508..].copy_from_slice(&checksum.to_le_bytes());
            assert!(
                parse_owned(&mbr, &changed, 7_733_248).is_err(),
                "offset {offset}"
            );
        }
        let mut changed = marker;
        changed[508] ^= 1;
        assert!(parse_owned(&mbr, &changed, 7_733_248).is_err());
        let mut changed = mbr;
        changed[0] = 1; // even unchanged geometry cannot reuse an old MBR binding
        assert!(parse_owned(&changed, &marker, 7_733_248).is_err());
    }

    struct FakeMedia {
        mbr: [u8; 512],
        marker: [u8; 512],
        reads: std::vec::Vec<u64>,
        fail_marker: bool,
    }
    impl Read for FakeMedia {
        fn info(&self) -> Result<Info, Error> {
            Ok(Info {
                sectors: 7_733_248,
                sector_size: 512,
                clock_hz: 0,
                bus_width: 1,
                reads: 0,
                failures: 0,
                last_read_us: 0,
            })
        }
        fn read(&mut self, sector: u64, output: &mut [u8; 512]) -> Result<(), Error> {
            self.reads.push(sector);
            match sector {
                0 => *output = self.mbr,
                1_955_840 if !self.fail_marker => *output = self.marker,
                _ => return Err(Error::Failed),
            }
            Ok(())
        }
        fn clock(&mut self, _: u32) -> Result<(), Error> {
            Err(Error::Unsupported)
        }
        fn recover(&mut self) -> Result<(), Error> {
            Err(Error::Unsupported)
        }
    }
    #[test]
    fn discovery_is_read_only_and_never_assumes_a_missing_marker() {
        let (mbr, marker) = fixture();
        let mut media = FakeMedia {
            mbr,
            marker,
            reads: std::vec::Vec::new(),
            fail_marker: false,
        };
        assert_eq!(discover(&mut media).unwrap().sectors, 5_777_407);
        assert_eq!(media.reads, [0, 1_955_840]);
        media.fail_marker = true;
        assert_eq!(discover(&mut media), Err(Error::Failed));
        media.fail_marker = false;
        media.marker = [0; 512];
        assert_eq!(discover(&mut media), Err(Error::Unsupported));
        media.reads.clear();
        media.mbr[510] = 0;
        assert_eq!(discover(&mut media), Err(Error::Unsupported));
        assert_eq!(media.reads, [0]);
    }
}
