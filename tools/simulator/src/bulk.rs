//! The C606 owned MMC data region, simulated sparsely. Written sectors outlive
//! a power loss; never-written sectors return stale bytes, as the partition
//! reused an old filesystem. Like the device, a failed write leaves the
//! controller failed until the next boot, and every write is verified.
use device_api::bulk::{Error, OwnedInfo, ReadWrite, SECTOR_SIZE};
use firmware_services::trace::{Medium, StorageError, StorageFault, StorageOp};
use std::{
    cell::RefCell,
    collections::{HashMap, VecDeque},
    rc::Rc,
};

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

#[derive(Clone)]
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
    log: Vec<StorageFault>,
    script: Option<VecDeque<StorageFault>>,
    attempts: u32,
}

/// The region, and whether this handle is a host's: a host's operations
/// are neither counted nor logged as the device's, and never meet a fault
/// a replay scripts for the device's.
#[derive(Clone)]
pub struct SimBulk(Rc<RefCell<Region>>, bool);

impl Default for SimBulk {
    fn default() -> Self {
        Self(
            Rc::new(RefCell::new(Region {
                presence: Presence::Absent,
                sectors: C606_SECTORS,
                written: HashMap::new(),
                fault: None,
                failed: false,
                reads: 0,
                writes: 0,
                entropy: 0x5eed,
                log: Vec::new(),
                script: None,
                attempts: 0,
            })),
            false,
        )
    }
}

/// What a region holds, to save it: the sectors written and the state a
/// boot keeps.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub presence: Presence,
    pub sectors: u64,
    pub entropy: u64,
    pub written: Vec<(u64, [u8; SECTOR_SIZE])>,
}

impl SimBulk {
    pub fn image(&self) -> Image {
        let region = self.0.borrow();
        let mut written: Vec<_> = region
            .written
            .iter()
            .map(|(&sector, bytes)| (sector, **bytes))
            .collect();
        written.sort_unstable_by_key(|&(sector, _)| sector);
        Image {
            presence: region.presence,
            sectors: region.sectors,
            entropy: region.entropy,
            written,
        }
    }

    pub fn from_image(image: Image) -> Self {
        let bulk = Self::default();
        {
            let mut region = bulk.0.borrow_mut();
            region.presence = image.presence;
            region.sectors = image.sectors;
            region.entropy = image.entropy;
            region.written = image
                .written
                .into_iter()
                .map(|(sector, bytes)| (sector, Box::new(bytes)))
                .collect();
        }
        bulk
    }

    /// A separate region holding the same sectors and state.
    pub fn copy(&self) -> Self {
        Self(Rc::new(RefCell::new(self.0.borrow().clone())), false)
    }

    /// The same region as a host reaches it over USB.
    pub fn host(&self) -> Self {
        Self(self.0.clone(), true)
    }

    pub fn begin_pass(&self) {
        self.0.borrow_mut().attempts = 0;
    }

    /// The same sectors, failing only as a trace says: presence, faults
    /// and failure come from the trace from now on.
    pub fn scripted(&self) -> Self {
        let copy = self.copy();
        let mut region = copy.0.borrow_mut();
        region.script = Some(VecDeque::new());
        region.fault = None;
        region.failed = false;
        drop(region);
        copy
    }

    /// A failure the trace recorded, for the operation that met it.
    pub fn script(&self, fault: StorageFault) {
        if let Some(script) = &mut self.0.borrow_mut().script {
            script.push_back(fault);
        }
    }

    /// Scripted failures no operation met; they are dropped.
    pub fn unmet(&self) -> Vec<StorageFault> {
        let mut region = self.0.borrow_mut();
        region
            .script
            .as_mut()
            .map_or_else(Vec::new, |script| script.drain(..).collect())
    }

    /// The failures answered since the last call.
    pub fn take_log(&self) -> Vec<StorageFault> {
        core::mem::take(&mut self.0.borrow_mut().log)
    }

    pub fn set_presence(&self, presence: Presence) {
        self.0.borrow_mut().presence = presence;
    }

    /// Another card's entropy source: it gives other values from now on.
    pub fn reseed_entropy(&self, seed: u64) {
        self.0.borrow_mut().entropy = seed | 1;
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

    /// A digest of every sector written, to compare two media.
    pub fn digest(&self) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let region = self.0.borrow();
        let mut sectors: Vec<_> = region.written.iter().collect();
        sectors.sort_unstable_by_key(|(sector, _)| **sector);
        let mut hasher = DefaultHasher::new();
        for (sector, bytes) in sectors {
            sector.hash(&mut hasher);
            bytes.hash(&mut hasher);
        }
        hasher.finish()
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

    fn answer(&self, op: StorageOp, sector: u64) -> Result<(), StorageFault> {
        let mut region = self.0.borrow_mut();
        let attempt = region.attempts;
        if !self.1 {
            region.attempts += 1;
        }
        let failure = |error: Error| StorageFault {
            medium: Medium::Bulk,
            attempt,
            op,
            at: sector,
            landed: 0,
            error: StorageError::from_bulk(error),
        };
        let fault = match &mut region.script {
            Some(_) if self.1 => None,
            Some(script) => script
                .iter()
                .position(|f| (f.attempt, f.op, f.at) == (attempt, op, sector))
                .and_then(|index| script.remove(index)),
            None => match region.presence {
                Presence::Absent => Some(failure(Error::Unsupported)),
                Presence::Missing => Some(failure(Error::Unavailable)),
                Presence::Ready if region.failed => Some(failure(Error::Failed)),
                Presence::Ready => None,
            },
        };
        if let Some(fault) = fault {
            if !self.1 {
                region.log.push(fault);
            }
            return Err(fault);
        }
        if op != StorageOp::Info && sector >= region.sectors {
            return Err(failure(Error::Range));
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
        self.answer(StorageOp::Info, 0)
            .map_err(|fault| fault.error.bulk())?;
        Ok(OwnedInfo {
            start_sector: 1_955_841,
            sectors: self.0.borrow().sectors,
            sector_size: SECTOR_SIZE as u16,
        })
    }

    fn owned_read(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error> {
        self.answer(StorageOp::Read, sector)
            .map_err(|fault| fault.error.bulk())?;
        *output = self.sector(sector);
        self.0.borrow_mut().reads += 1;
        Ok(())
    }

    fn owned_write(&mut self, sector: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), Error> {
        if let Err(fault) = self.answer(StorageOp::Write, sector) {
            self.tear(sector, input, fault.landed as usize);
            return Err(fault.error.bulk());
        }
        let mut region = self.0.borrow_mut();
        if let Some(fault) = region.fault.as_mut() {
            if fault.after == 0 {
                let partial = fault.partial.min(SECTOR_SIZE);
                region.fault = None;
                region.failed = true;
                if !self.1 {
                    let attempt = region.attempts - 1;
                    region.log.push(StorageFault {
                        medium: Medium::Bulk,
                        attempt,
                        op: StorageOp::Write,
                        at: sector,
                        landed: partial as u32,
                        error: StorageError::Failed,
                    });
                }
                drop(region);
                self.tear(sector, input, partial);
                return Err(Error::Failed);
            }
            fault.after -= 1;
        }
        region.written.insert(sector, Box::new(*input));
        region.writes += 1;
        Ok(())
    }
}

impl SimBulk {
    fn tear(&self, sector: u64, input: &[u8; SECTOR_SIZE], partial: usize) {
        if partial == 0 {
            return;
        }
        let partial = partial.min(SECTOR_SIZE);
        let mut torn = self.sector(sector);
        torn[..partial].copy_from_slice(&input[..partial]);
        self.0.borrow_mut().written.insert(sector, Box::new(torn));
    }
}
