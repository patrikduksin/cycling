//! The C606 owned MMC data region, simulated sparsely. Written sectors outlive
//! a power loss; never-written sectors return stale bytes, as the partition
//! reused an old filesystem. Like the device, a failed write leaves the
//! controller failed until the next boot, and every write is verified.
use device_api::bulk::{Error, OwnedInfo, ReadWrite, SECTOR_SIZE};
use firmware_services::trace::{Medium, StorageFault, StorageOp};
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
    /// Every failure the medium answered, for the trace.
    log: Vec<StorageFault>,
    /// In a replay, the failures its trace holds: the medium then fails
    /// only these, whatever its presence or fault.
    script: Option<VecDeque<StorageFault>>,
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
            log: Vec::new(),
            script: None,
        })))
    }
}

impl SimBulk {
    /// A separate region holding the same sectors and state.
    pub fn copy(&self) -> Self {
        Self(Rc::new(RefCell::new(self.0.borrow().clone())))
    }

    /// The same sectors, failing only as a trace says: presence, faults
    /// and failure come from the trace from now on.
    pub fn scripted(&self) -> Self {
        let copy = self.copy();
        copy.0.borrow_mut().script = Some(VecDeque::new());
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

    /// Whether `op` on `sector` may go ahead, and the failure it meets
    /// otherwise, logged. Range is the caller's error, not the medium's.
    fn answer(&self, op: StorageOp, sector: u64) -> Result<(), StorageFault> {
        let mut region = self.0.borrow_mut();
        let failure = |error: Error| StorageFault {
            medium: Medium::Bulk,
            op,
            at: sector,
            landed: 0,
            error: error.into(),
        };
        let fault = match &mut region.script {
            Some(script) => script
                .iter()
                .position(|f| f.op == op && f.at == sector)
                .and_then(|index| script.remove(index)),
            None => match region.presence {
                Presence::Absent => Some(failure(Error::Unsupported)),
                Presence::Missing => Some(failure(Error::Unavailable)),
                Presence::Ready if region.failed => Some(failure(Error::Failed)),
                Presence::Ready => None,
            },
        };
        if let Some(fault) = fault {
            region.log.push(fault);
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
                region.log.push(StorageFault {
                    medium: Medium::Bulk,
                    op: StorageOp::Write,
                    at: sector,
                    landed: partial as u32,
                    error: Error::Failed.into(),
                });
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
    /// The first `partial` bytes of `input` reach the sector; the rest keep
    /// what it held.
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
