//! File- or memory-backed reservations with exclusive ownership and injectable
//! torn writes. Memory media outlives a simulated power loss like flash does.
use device_api::observation::{Availability, Error};
use device_api::storage::{OwnedFlash, Region};
use firmware_services::trace::{Medium, StorageError, StorageOp};
use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    rc::Rc,
};
/// The simulated ride reservation, the C606's 1 MiB.
pub const DATA_BYTES: usize = 1024 * 1024;
const JOURNAL_BYTES: usize = 8192;
/// A storage operation that failed, and how many of its bytes reached the
/// medium first. `attempt` is its place among the medium's operations since
/// the pass began; `at` is a byte offset into a flash region, or a sector of
/// the bulk medium.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StorageFault {
    pub medium: Medium,
    pub attempt: u32,
    pub op: StorageOp,
    pub at: u64,
    pub landed: u32,
    pub error: StorageError,
}

/// Tear the mutation after `after` succeed, leaving `partial` bytes of it.
#[derive(Default)]
pub struct Fault {
    pub after: Option<u32>,
    pub partial: usize,
    /// The torn mutation is where power was lost: from it on, no access to
    /// internal flash or the card reaches either until the next boot.
    pub power_cut: bool,
    pub unreadable: Option<Region>,
    /// Each erase this boot that completed, by region and offset.
    pub erased: Vec<(Region, usize)>,
    attempts: u32,
    lost: PowerLost,
}

/// Whether power was lost this boot, shared by every medium of a device.
pub type PowerLost = Rc<std::cell::Cell<bool>>;
impl Fault {
    pub fn begin_pass(&mut self) {
        self.attempts = 0;
    }

    fn fails(
        &mut self,
        region: Region,
        op: StorageOp,
        at: usize,
        len: usize,
    ) -> Option<StorageFault> {
        let attempt = self.attempts;
        self.attempts += 1;
        if self.lost.get() {
            let fault = StorageFault {
                medium: region.into(),
                attempt,
                op,
                at: at as u64,
                landed: 0,
                error: StorageError::Unavailable,
            };
            return Some(fault);
        }
        if op == StorageOp::Read {
            return (self.unreadable == Some(region)).then_some(StorageFault {
                medium: region.into(),
                attempt,
                op,
                at: at as u64,
                landed: 0,
                error: StorageError::Failed,
            });
        }
        let injected = self.after == Some(0);
        if let Some(n) = self.after.as_mut() {
            *n = n.saturating_sub(1);
        }
        if !injected {
            return None;
        }
        self.after = None;
        self.lost.set(self.power_cut);
        let fault = StorageFault {
            medium: region.into(),
            attempt,
            op,
            at: at as u64,
            landed: self.partial.min(len) as u32,
            error: StorageError::Failed,
        };
        Some(fault)
    }
}
#[derive(Clone)]
enum Backing {
    Files { root: PathBuf, _lock: Rc<fs::File> },
    Memory(Rc<RefCell<MemoryMedia>>),
}
struct MemoryMedia {
    boot: u32,
    image: FlashImage,
}

#[derive(Clone)]
pub struct FlashImage {
    pub configuration: Vec<u8>,
    pub data: Vec<u8>,
    pub intent: Vec<u8>,
    /// Bytes of the intent's and the ride journal's word programs cut short,
    /// with the value a whole program leaves. A boot reads each such word
    /// whole or as the cut left it, until a program completes over it or an
    /// erase clears it. The configuration region keeps a cut word as it was
    /// left.
    pub marginal: Vec<(Region, usize, u8)>,
}

impl FlashImage {
    pub fn erased() -> Self {
        Self {
            configuration: vec![0xff; JOURNAL_BYTES],
            data: vec![0xff; DATA_BYTES],
            intent: vec![0xff; JOURNAL_BYTES],
            marginal: Vec::new(),
        }
    }

    fn region(&self, r: Region) -> &[u8] {
        match r {
            Region::Configuration => &self.configuration,
            Region::Data => &self.data,
            Region::Intent => &self.intent,
            Region::Crash => &[],
        }
    }

    fn region_mut(&mut self, r: Region) -> &mut [u8] {
        match r {
            Region::Configuration => &mut self.configuration,
            Region::Data => &mut self.data,
            Region::Intent => &mut self.intent,
            Region::Crash => &mut [],
        }
    }
}
pub struct FileFlash {
    backing: Backing,
    pub fault: Rc<RefCell<Fault>>,
    tap: Option<crate::compare::Tap>,
}
impl FileFlash {
    fn on(backing: Backing) -> Self {
        Self {
            backing,
            fault: Rc::default(),
            tap: None,
        }
    }
    /// Whether power was lost this boot, to share with the card.
    pub(crate) fn power_lost(&self) -> PowerLost {
        self.fault.borrow().lost.clone()
    }
    pub(crate) fn tapped(self, tap: Option<crate::compare::Tap>) -> Self {
        Self { tap, ..self }
    }
    pub fn untapped(&self) -> Self {
        Self {
            backing: self.backing.clone(),
            fault: self.fault.clone(),
            tap: None,
        }
    }
    pub(crate) fn device_handle(&self) -> Self {
        self.untapped().tapped(self.tap.clone())
    }
    fn fold_device_result(
        &self,
        r: Region,
        op: StorageOp,
        offset: usize,
        len: usize,
        result: Result<(), Error>,
    ) -> Result<(), Error> {
        if let Some(tap) = &self.tap {
            let folded = result.map_err(StorageError::from_flash);
            tap.storage(r.into(), op, offset as u64, len as u64, folded);
        }
        result
    }
    /// Reset injected faults and advance the persisted session generation.
    pub(crate) fn begin_boot(&self) -> io::Result<u32> {
        let lost = self.power_lost();
        lost.set(false);
        *self.fault.borrow_mut() = Fault {
            lost,
            ..Fault::default()
        };
        let root = match &self.backing {
            Backing::Files { root, .. } => root,
            Backing::Memory(media) => {
                let mut media = media.borrow_mut();
                media.boot = media
                    .boot
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("boot counter exhausted"))?;
                return Ok(media.boot);
            }
        };
        let boot_path = root.join("boot");
        let previous = match fs::read_to_string(&boot_path) {
            Ok(s) => s.trim().parse::<u32>().map_err(|_| {
                io::Error::new(io::ErrorKind::InvalidData, "invalid virtual boot counter")
            })?,
            Err(e) if e.kind() == io::ErrorKind::NotFound => 0,
            Err(e) => return Err(e),
        };
        let boot = previous
            .checked_add(1)
            .ok_or_else(|| io::Error::other("boot counter exhausted"))?;
        fs::write(boot_path, boot.to_string())?;
        Ok(boot)
    }

    pub fn open(root: &Path) -> io::Result<Self> {
        fs::create_dir_all(root)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(root.join("session.lock"))?;
        lock.try_lock()
            .map_err(|e| io::Error::other(std::format!("virtual media already owned: {e}")))?;
        for r in [Region::Configuration, Region::Data, Region::Intent] {
            let size = Self::capacity(r);
            let path = Self::path(root, r);
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(mut f) => {
                    f.write_all(&vec![0xff; size])?;
                    f.sync_all()?;
                }
                Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                    if fs::metadata(&path)?.len() != size as u64 {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "virtual media size differs; preserving file",
                        ));
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(Self::on(Backing::Files {
            root: root.to_path_buf(),
            _lock: Rc::new(lock),
        }))
    }
    /// Erased media that lives as long as any clone, for fast scenarios.
    pub fn memory() -> Self {
        Self::from_image(0, FlashImage::erased())
    }
    /// Memory media's boot counter and bytes, to save them; `None` for
    /// file-backed media.
    pub fn image(&self) -> Option<(u32, FlashImage)> {
        let Backing::Memory(media) = &self.backing else {
            return None;
        };
        let media = media.borrow();
        Some((media.boot, media.image.clone()))
    }
    /// Put `image` back on memory media in place, keeping its boot counter.
    pub fn restore(&self, image: &FlashImage) {
        if let Backing::Memory(media) = &self.backing {
            media.borrow_mut().image = image.clone();
        }
    }
    pub fn from_image(boot: u32, image: FlashImage) -> Self {
        Self::on(Backing::Memory(Rc::new(RefCell::new(MemoryMedia {
            boot,
            image,
        }))))
    }
    /// A copy of a region's bytes, for inspection after a scenario.
    pub fn contents(&mut self, r: Region) -> Vec<u8> {
        let mut bytes = vec![0; self.geometry(r).capacity];
        let _ = self.raw_read(r, 0, &mut bytes);
        bytes
    }
    fn path(root: &Path, r: Region) -> PathBuf {
        root.join(match r {
            Region::Configuration => "configuration.bin",
            Region::Intent => "intent.bin",
            Region::Data | Region::Crash => "data.bin",
        })
    }
    fn capacity(r: Region) -> usize {
        match r {
            Region::Configuration | Region::Intent => JOURNAL_BYTES,
            Region::Data => DATA_BYTES,
            Region::Crash => 0,
        }
    }
    fn check(
        &self,
        r: Region,
        offset: usize,
        length: usize,
        alignment: usize,
    ) -> Result<(), Error> {
        device_api::storage::checked_range::<()>(
            self.geometry(r).capacity,
            offset,
            length,
            alignment,
        )
        .map_err(|_| Error::Invalid)
    }
    fn mutation(
        &mut self,
        r: Region,
        offset: usize,
        bytes: &[u8],
        erase: bool,
    ) -> Result<(), Error> {
        self.check(r, offset, bytes.len(), if erase { 4096 } else { 4 })?;
        let op = if erase {
            StorageOp::Erase
        } else {
            StorageOp::Write
        };
        let fault = {
            let mut faults = self.fault.borrow_mut();
            let fault = faults.fails(r, op, offset, bytes.len());
            if erase && fault.is_none() {
                faults.erased.push((r, offset));
            }
            fault
        };
        let count = fault.map_or(bytes.len(), |fault| fault.landed as usize);
        let failed = fault.map(|fault| fault.error.flash());
        let torn = erase && fault.is_some();
        let touched = if torn { bytes.len() } else { count };
        let landed = |old: u8, new: u8, at: usize| match (erase, torn) {
            (true, true) => old | erase_tear(offset, count, bytes.len(), at),
            (true, false) => new,
            (false, _) => old & new,
        };
        let root = match &self.backing {
            Backing::Files { root, .. } => root,
            Backing::Memory(media) => {
                let mut media = media.borrow_mut();
                let image = &mut media.image;
                let region = image.region_mut(r);
                let mut cut = Vec::new();
                for (index, dst) in region[offset..offset + bytes.len()].iter_mut().enumerate() {
                    let whole = *dst & bytes[index];
                    if index < touched {
                        *dst = landed(*dst, bytes[index], offset + index);
                    }
                    if matches!(r, Region::Intent | Region::Data)
                        && !erase
                        && bytes.len() == 4
                        && *dst != whole
                    {
                        cut.push((r, offset + index, whole));
                    }
                }
                image.marginal.retain(|&(region, at, _)| {
                    region != r || !(offset..offset + bytes.len()).contains(&at)
                });
                image.marginal.extend(cut);
                return failed.map_or(Ok(()), Err);
            }
        };
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(Self::path(root, r))
            .map_err(|_| Error::Failed)?;
        let mut result = vec![0; touched];
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| Error::Failed)?;
        file.read_exact(&mut result).map_err(|_| Error::Failed)?;
        for (index, dst) in result.iter_mut().enumerate() {
            *dst = landed(*dst, bytes[index], offset + index);
        }
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| Error::Failed)?;
        file.write_all(&result)
            .and_then(|_| file.sync_data())
            .map_err(|_| Error::Failed)?;
        failed.map_or(Ok(()), Err)
    }
}
/// Whether boot `boot` reads the marginal word holding byte `at` whole: the
/// same answer for every read of a boot, so a replay reads what its
/// recording read.
fn reads_whole(boot: u32, at: usize) -> bool {
    let mut hash = u64::from(boot) << 32 ^ (at / 4) as u64;
    hash = (hash ^ hash >> 30).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ hash >> 27).wrapping_mul(0x94d0_49bb_1331_11eb);
    (hash ^ hash >> 31) & 1 == 1
}

/// The bits an erase of `len` bytes at `offset`, cut by power after
/// `landed` of them, set in the byte at `at`. A NOR sector erase cut short
/// leaves cells across the whole sector partly erased, not a clean prefix:
/// each bit is erased with odds `landed / len`, in a pattern fixed by the
/// erase's place and progress, so a replay tears it the same way.
pub fn erase_tear(offset: usize, landed: usize, len: usize, at: usize) -> u8 {
    let threshold = landed.min(len) * 256 / len.max(1);
    let mut hash = (offset as u64) << 32 ^ (landed as u64) << 16 ^ at as u64;
    hash = (hash ^ hash >> 30).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    hash = (hash ^ hash >> 27).wrapping_mul(0x94d0_49bb_1331_11eb);
    hash ^= hash >> 31;
    (0..8)
        .filter(|bit| ((hash >> (8 * bit)) & 0xff) < threshold as u64)
        .fold(0, |mask, bit| mask | 1 << bit)
}

impl OwnedFlash for FileFlash {
    type Error = Error;
    fn availability(&self, _: Region) -> Availability {
        Availability::Ready
    }
    fn geometry(&self, r: Region) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: Self::capacity(r),
            program_size: 4,
            erase_size: 4096,
            durable: true,
        }
    }
    fn read(&mut self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        let result = self.guest_read(r, offset, out);
        self.fold_device_result(r, StorageOp::Read, offset, out.len(), result)
    }
    fn program(&mut self, r: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        let result = self.mutation(r, offset, bytes, false);
        self.fold_device_result(r, StorageOp::Write, offset, bytes.len(), result)
    }
    fn erase(&mut self, r: Region, offset: usize, len: usize) -> Result<(), Error> {
        let result = self
            .check(r, offset, len, 4096)
            .and_then(|()| self.mutation(r, offset, &vec![0xff; len], true));
        self.fold_device_result(r, StorageOp::Erase, offset, len, result)
    }
}
impl FileFlash {
    fn guest_read(&self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        self.check(r, offset, out.len(), 1)?;
        let fault = self
            .fault
            .borrow_mut()
            .fails(r, StorageOp::Read, offset, out.len());
        if let Some(fault) = fault {
            return Err(fault.error.flash());
        }
        self.raw_read(r, offset, out)
    }

    /// The region's bytes, as a host reading the medium sees them: no fault
    /// meets it and the device's operations do not count it.
    fn raw_read(&self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        let root = match &self.backing {
            Backing::Files { root, .. } => root,
            Backing::Memory(media) => {
                let media = media.borrow();
                let region = media.image.region(r);
                out.copy_from_slice(&region[offset..offset + out.len()]);
                for &(region, at, whole) in &media.image.marginal {
                    if let Some(byte) = at.checked_sub(offset).and_then(|i| out.get_mut(i))
                        && region == r
                        && reads_whole(media.boot, at)
                    {
                        *byte = whole;
                    }
                }
                return Ok(());
            }
        };
        let mut f = fs::File::open(Self::path(root, r)).map_err(|_| Error::Failed)?;
        f.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| f.read_exact(out))
            .map_err(|_| Error::Failed)
    }
}

/// Each memory-backed region and its bytes.
type Ram = std::rc::Rc<std::cell::RefCell<Vec<(Region, Vec<u8>)>>>;

/// A model's memory-backed regions over the flash that backs the rest. It is
/// built at every boot, so those regions start erased, as a device fills its
/// memory with the erased byte, and keep nothing across power-off. Its
/// handles share the one memory.
pub struct Volatile<F> {
    flash: F,
    memory: Ram,
}

impl<F> Volatile<F> {
    pub fn boot(flash: F, model: &crate::devices::Model) -> Self {
        let memory = model
            .memory
            .iter()
            .map(|&(region, size)| (region, vec![0xff; size]))
            .collect();
        Self {
            flash,
            memory: std::rc::Rc::new(std::cell::RefCell::new(memory)),
        }
    }

    /// Another handle on the same memory, over `flash`.
    pub fn handle<G>(&self, flash: G) -> Volatile<G> {
        Volatile {
            flash,
            memory: self.memory.clone(),
        }
    }

    fn memory(&self, r: Region) -> Option<std::cell::RefMut<'_, Vec<u8>>> {
        std::cell::RefMut::filter_map(self.memory.borrow_mut(), |memory| {
            memory
                .iter_mut()
                .find(|(region, _)| *region == r)
                .map(|(_, bytes)| bytes)
        })
        .ok()
    }
}

impl<F: OwnedFlash<Error = Error>> OwnedFlash for Volatile<F> {
    type Error = Error;
    fn availability(&self, r: Region) -> Availability {
        self.flash.availability(r)
    }
    fn geometry(&self, r: Region) -> device_api::storage::Geometry {
        match self.memory(r) {
            Some(bytes) => device_api::storage::Geometry {
                capacity: bytes.len(),
                durable: false,
                ..self.flash.geometry(r)
            },
            None => self.flash.geometry(r),
        }
    }
    fn read(&mut self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        let Some(bytes) = self.memory(r) else {
            return self.flash.read(r, offset, out);
        };
        device_api::storage::checked_range::<()>(bytes.len(), offset, out.len(), 1)
            .map_err(|_| Error::Invalid)?;
        out.copy_from_slice(&bytes[offset..offset + out.len()]);
        Ok(())
    }
    fn program(&mut self, r: Region, offset: usize, input: &[u8]) -> Result<(), Error> {
        let Some(mut bytes) = self.memory(r) else {
            return self.flash.program(r, offset, input);
        };
        device_api::storage::checked_range::<()>(bytes.len(), offset, input.len(), 4)
            .map_err(|_| Error::Invalid)?;
        for (stored, new) in bytes[offset..].iter_mut().zip(input) {
            *stored &= new;
        }
        Ok(())
    }
    fn erase(&mut self, r: Region, offset: usize, len: usize) -> Result<(), Error> {
        let Some(mut bytes) = self.memory(r) else {
            return self.flash.erase(r, offset, len);
        };
        device_api::storage::checked_range::<()>(bytes.len(), offset, len, 4096)
            .map_err(|_| Error::Invalid)?;
        bytes[offset..offset + len].fill(0xff);
        Ok(())
    }
}

#[cfg(test)]
mod volatile_tests {
    use super::*;

    #[test]
    fn a_memory_backed_region_boots_erased_and_says_it_is_not_durable() {
        let model = &crate::models::VOLATILE;
        let flash = FileFlash::memory;
        let mut volatile = Volatile::boot(flash(), model);
        let data = volatile.geometry(Region::Data);
        assert_eq!((data.capacity, data.durable), (0x40000, false));
        assert!(volatile.geometry(Region::Intent).durable);
        volatile.program(Region::Data, 0, &[0; 256]).unwrap();
        let mut read = [0xaa; 256];
        volatile.read(Region::Data, 0, &mut read).unwrap();
        assert_eq!(read, [0; 256]);
        let mut rebooted = Volatile::boot(flash(), model);
        rebooted.read(Region::Data, 0, &mut read).unwrap();
        assert_eq!(read, [0xff; 256], "a power-off loses the memory");
        assert_eq!(
            rebooted.program(Region::Data, 0x40000, &[0; 4]),
            Err(Error::Invalid)
        );
    }
}
