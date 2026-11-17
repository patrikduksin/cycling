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

pub struct SimBulk {
    region: Rc<RefCell<Region>>,
    actor: Actor,
    tap: Option<crate::compare::Tap>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Actor {
    Device,
    Host,
}

impl Default for SimBulk {
    fn default() -> Self {
        Self::on(
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
            Actor::Device,
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
    fn on(region: Rc<RefCell<Region>>, actor: Actor) -> Self {
        Self {
            region,
            actor,
            tap: None,
        }
    }

    pub fn image(&self) -> Image {
        let region = self.region.borrow();
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
            let mut region = bulk.region.borrow_mut();
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
        Self::on(
            Rc::new(RefCell::new(self.region.borrow().clone())),
            Actor::Device,
        )
    }

    pub fn untapped(&self) -> Self {
        Self::on(self.region.clone(), self.actor)
    }

    /// The same region as a host reaches it over USB.
    pub fn host(&self) -> Self {
        Self::on(self.region.clone(), Actor::Host)
    }

    /// A host's copy of the sectors on a card that is present and working,
    /// whatever this boot's controller does.
    pub fn inspect(&self) -> Self {
        let copy = self.copy();
        {
            let mut region = copy.region.borrow_mut();
            region.presence = Presence::Ready;
            region.fault = None;
            region.failed = false;
        }
        Self::on(copy.region, Actor::Host)
    }

    /// A write fault is armed, or one failed the controller this boot.
    pub fn faulted(&self) -> bool {
        let region = self.region.borrow();
        region.fault.is_some() || region.failed
    }

    pub fn begin_pass(&self) {
        self.region.borrow_mut().attempts = 0;
    }

    /// The same sectors, failing only as a trace says: presence, faults
    /// and failure come from the trace from now on.
    pub fn scripted(&self) -> Self {
        let copy = self.copy();
        let mut region = copy.region.borrow_mut();
        region.script = Some(VecDeque::new());
        region.fault = None;
        region.failed = false;
        drop(region);
        copy
    }

    /// A failure the trace recorded, for the operation that met it.
    pub fn script(&self, fault: StorageFault) {
        if let Some(script) = &mut self.region.borrow_mut().script {
            script.push_back(fault);
        }
    }

    /// Scripted failures no operation met; they are dropped.
    pub fn unmet(&self) -> Vec<StorageFault> {
        let mut region = self.region.borrow_mut();
        region
            .script
            .as_mut()
            .map_or_else(Vec::new, |script| script.drain(..).collect())
    }

    /// The failures answered since the last call.
    pub fn take_log(&self) -> Vec<StorageFault> {
        core::mem::take(&mut self.region.borrow_mut().log)
    }

    pub fn set_presence(&self, presence: Presence) {
        self.region.borrow_mut().presence = presence;
    }

    /// Another card's entropy source: it gives other values from now on.
    pub fn reseed_entropy(&self, seed: u64) {
        self.region.borrow_mut().entropy = seed | 1;
    }

    /// Shrink the region, to reach full media quickly.
    pub fn set_sectors(&self, sectors: u64) {
        self.region.borrow_mut().sectors = sectors;
    }

    pub fn fail(&self, fault: BulkFault) {
        self.region.borrow_mut().fault = Some(fault);
    }

    /// Power returns: the controller recovers and faults clear.
    pub(crate) fn begin_boot(&self) {
        let mut region = self.region.borrow_mut();
        region.fault = None;
        region.failed = false;
    }

    pub(crate) fn tapped(self, tap: Option<crate::compare::Tap>) -> Self {
        Self { tap, ..self }
    }

    fn fold_device_result(
        &self,
        op: StorageOp,
        sector: u64,
        result: Result<(), Error>,
    ) -> Result<(), Error> {
        if let Some(tap) = &self.tap {
            let folded = result.map_err(StorageError::from_bulk);
            tap.storage(Medium::Bulk, op, sector, SECTOR_SIZE as u64, folded);
        }
        result
    }

    fn attempt(&self, op: StorageOp, sector: u64) -> Result<(), Error> {
        let answer = self.answer(op, sector).map_err(|fault| fault.error.bulk());
        self.fold_device_result(op, sector, answer)
    }

    /// A digest of every sector written, to compare two media.
    pub fn digest(&self) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let region = self.region.borrow();
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
        self.region.borrow().writes
    }

    pub fn reads(&self) -> u64 {
        self.region.borrow().reads
    }

    /// Damage a sector so it no longer verifies.
    pub fn corrupt(&self, sector: u64) {
        self.region
            .borrow_mut()
            .written
            .insert(sector, Box::new([0x5a; SECTOR_SIZE]));
    }

    pub fn sector(&self, sector: u64) -> [u8; SECTOR_SIZE] {
        let region = self.region.borrow();
        region
            .written
            .get(&sector)
            .map_or_else(|| stale(sector), |bytes| **bytes)
    }

    fn answer(&self, op: StorageOp, sector: u64) -> Result<(), StorageFault> {
        let mut region = self.region.borrow_mut();
        let attempt = region.attempts;
        if self.actor == Actor::Device {
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
            Some(_) if self.actor == Actor::Host => None,
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
            if self.actor == Actor::Device {
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
        let mut region = self.region.borrow_mut();
        let mut state = region.entropy;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        region.entropy = state;
        state
    }
    fn owned_info(&mut self) -> Result<OwnedInfo, Error> {
        self.attempt(StorageOp::Info, 0)?;
        Ok(OwnedInfo {
            start_sector: 1_955_841,
            sectors: self.region.borrow().sectors,
            sector_size: SECTOR_SIZE as u16,
        })
    }

    fn owned_read(&mut self, sector: u64, output: &mut [u8; SECTOR_SIZE]) -> Result<(), Error> {
        self.attempt(StorageOp::Read, sector)?;
        *output = self.sector(sector);
        self.region.borrow_mut().reads += 1;
        Ok(())
    }

    fn owned_write(&mut self, sector: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), Error> {
        let written = self.write(sector, input);
        self.fold_device_result(StorageOp::Write, sector, written)
    }
}

impl SimBulk {
    fn write(&mut self, sector: u64, input: &[u8; SECTOR_SIZE]) -> Result<(), Error> {
        if let Err(fault) = self.answer(StorageOp::Write, sector) {
            self.tear(sector, input, fault.landed as usize);
            return Err(fault.error.bulk());
        }
        let mut region = self.region.borrow_mut();
        if let Some(fault) = region.fault.as_mut() {
            if fault.after == 0 {
                let partial = fault.partial.min(SECTOR_SIZE);
                region.fault = None;
                region.failed = true;
                if self.actor == Actor::Device {
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

    fn tear(&self, sector: u64, input: &[u8; SECTOR_SIZE], partial: usize) {
        if partial == 0 {
            return;
        }
        let partial = partial.min(SECTOR_SIZE);
        let mut torn = self.sector(sector);
        torn[..partial].copy_from_slice(&input[..partial]);
        self.region
            .borrow_mut()
            .written
            .insert(sector, Box::new(torn));
    }
}
