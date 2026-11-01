//! The C606 owned MMC data region, simulated sparsely. Written sectors outlive
//! a power loss; never-written sectors return stale bytes, as the partition
//! reused an old filesystem. Like the device, a failed write leaves the
//! controller failed until the next boot, and every write is verified.
use device_api::bulk::{Error, OwnedInfo, ReadWrite, SECTOR_SIZE};
use std::{cell::RefCell, collections::HashMap, rc::Rc};

/// Sectors in the C606 owned data region (the ownership marker excluded).
pub const C606_SECTORS: u64 = 5_777_407;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Presence {
    /// The board has no bulk storage.
    #[default]
    Absent,
    /// The medium did not initialize this boot.
    Missing,
    Ready,
}

/// Fail the write after `after` succeed: `partial` bytes reach the sector,
/// then the controller stays failed until the next boot.
#[derive(Clone, Copy, Debug)]
pub struct BulkFault {
    pub after: u32,
    pub partial: usize,
}

struct Region {
    presence: Presence,
    sectors: u64,
    written: HashMap<u64, Box<[u8; SECTOR_SIZE]>>,
    fault: Option<BulkFault>,
    failed: bool,
    reads: u64,
    writes: u64,
    /// Entropy state; it survives reboots like a hardware source would not repeat.
    entropy: u64,
}

#[derive(Clone)]
pub struct SimBulk(Rc<RefCell<Region>>);

impl Default for SimBulk {
    fn default() -> Self {
        Self(Rc::new(RefCell::new(Region {
            presence: Presence::Absent,
            sectors: C606_SECTORS,
            written: HashMap::new(),
            fault: None,
            failed: false,
            reads: 0,
            writes: 0,
            entropy: 0x5eed,
        })))
    }
}

impl SimBulk {
    pub fn set_presence(&self, presence: Presence) {
        self.0.borrow_mut().presence = presence;
    }

    /// Shrink the region, to reach full media quickly.
    pub fn set_sectors(&self, sectors: u64) {
        self.0.borrow_mut().sectors = sectors;
    }

    pub fn fail(&self, fault: BulkFault) {
        self.0.borrow_mut().fault = Some(fault);
    }

    /// Power returns: the controller recovers and faults clear.
    pub(crate) fn begin_boot(&self) {
        let mut region = self.0.borrow_mut();
        region.fault = None;
        region.failed = false;
    }

    pub fn writes(&self) -> u64 {
        self.0.borrow().writes
    }

    pub fn reads(&self) -> u64 {
        self.0.borrow().reads
    }

    /// Damage a sector so it no longer verifies.
    pub fn corrupt(&self, sector: u64) {
        self.0
            .borrow_mut()
            .written
            .insert(sector, Box::new([0x5a; SECTOR_SIZE]));
    }

    pub fn sector(&self, sector: u64) -> [u8; SECTOR_SIZE] {
        let region = self.0.borrow();
        region
            .written
            .get(&sector)
            .map_or_else(|| stale(sector), |bytes| **bytes)
    }

    fn check(&self, sector: u64) -> Result<(), Error> {
        let region = self.0.borrow();
        match region.presence {
            Presence::Absent => return Err(Error::Unsupported),
            Presence::Missing => return Err(Error::Unavailable),
            Presence::Ready => {}
        }
        if region.failed {
            return Err(Error::Failed);
        }
        if sector >= region.sectors {
            return Err(Error::Range);
        }
        Ok(())
    }
}

/// Deterministic leftovers of the filesystem that used to be here.
fn stale(sector: u64) -> [u8; SECTOR_SIZE] {
    let mut bytes = [0u8; SECTOR_SIZE];
    let mut state = sector.wrapping_mul(0x2545_f491_4f6c_dd1d) | 1;
    for chunk in bytes.chunks_mut(8) {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        chunk.copy_from_slice(&state.to_le_bytes());
    }
    bytes
}

impl ReadWrite for SimBulk {
    fn entropy(&mut self) -> u64 {
        let mut region = self.0.borrow_mut();
        let mut state = region.entropy;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        region.entropy = state;
        state
    }
    fn owned_info(&mut self) -> Result<OwnedInfo, Error> {
        self.check(0)?;
        Ok(OwnedInfo {
            start_sector: 1_955_841,
            sectors: self.0.borrow().sectors,
            sector_size: SECTOR_SIZE as u16,
        })
    }

    fn owned_read(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error> {
        self.check(sector)?;
        *output = self.sector(sector);
        self.0.borrow_mut().reads += 1;
        Ok(())
    }

    fn owned_write(&mut self, sector: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), Error> {
        self.check(sector)?;
        let mut region = self.0.borrow_mut();
        if let Some(fault) = region.fault.as_mut() {
            if fault.after == 0 {
                let partial = fault.partial.min(SECTOR_SIZE);
                let mut torn = region
                    .written
                    .get(&sector)
                    .map_or_else(|| stale(sector), |bytes| **bytes);
                torn[..partial].copy_from_slice(&input[..partial]);
                region.written.insert(sector, Box::new(torn));
                region.fault = None;
                region.failed = true;
                return Err(Error::Failed);
            }
            fault.after -= 1;
        }
        region.written.insert(sector, Box::new(*input));
        region.writes += 1;
        Ok(())
    }
}
