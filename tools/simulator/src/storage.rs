//! File- or memory-backed reservations with exclusive ownership and injectable
//! torn writes. Memory media outlives a simulated power loss like flash does.
use device_api::observation::{Availability, Error};
use device_api::storage::{OwnedFlash, Region};
use firmware_services::trace::{StorageError, StorageFault, StorageOp};
use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    rc::Rc,
};
const DATA_BYTES: usize = 1024 * 1024;
/// Tear the mutation after `after` succeed, leaving `partial` bytes of it.
/// Every failure is logged for the trace; a replay fails exactly the
/// operations its trace scripts.
#[derive(Default)]
pub struct Fault {
    pub after: Option<u32>,
    pub partial: usize,
    pub log: Vec<StorageFault>,
    pub script: std::collections::VecDeque<StorageFault>,
    attempts: u32,
}
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
        let scripted = self.script.iter().position(|f| {
            (f.medium, f.attempt, f.op, f.at) == (region.into(), attempt, op, at as u64)
        });
        let fault = match scripted {
            Some(index) => self.script.remove(index)?,
            None if op == StorageOp::Read => return None,
            None => {
                let injected = self.after == Some(0);
                if let Some(n) = self.after.as_mut() {
                    *n = n.saturating_sub(1);
                }
                if !injected {
                    return None;
                }
                self.after = None;
                StorageFault {
                    medium: region.into(),
                    attempt,
                    op,
                    at: at as u64,
                    landed: self.partial.min(len) as u32,
                    error: StorageError::Failed,
                }
            }
        };
        self.log.push(fault);
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
    configuration: Vec<u8>,
    data: Vec<u8>,
}
#[derive(Clone)]
pub struct FileFlash {
    backing: Backing,
    pub fault: Rc<RefCell<Fault>>,
}
impl FileFlash {
    /// Reset injected faults and advance the persisted session generation.
    pub(crate) fn begin_boot(&self) -> io::Result<u32> {
        *self.fault.borrow_mut() = Fault::default();
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
        for (name, size) in [("configuration.bin", 8192), ("data.bin", DATA_BYTES)] {
            let path = root.join(name);
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
        Ok(Self {
            backing: Backing::Files {
                root: root.to_path_buf(),
                _lock: Rc::new(lock),
            },
            fault: Rc::default(),
        })
    }
    /// Erased media that lives as long as any clone, for fast scenarios.
    pub fn memory() -> Self {
        Self {
            backing: Backing::Memory(Rc::new(RefCell::new(MemoryMedia {
                boot: 0,
                configuration: vec![0xff; 8192],
                data: vec![0xff; DATA_BYTES],
            }))),
            fault: Rc::default(),
        }
    }
    /// Separate memory media holding these bytes, whose next boot is
    /// `boot`. `None` for file-backed media.
    pub fn copy_before_boot(&self, boot: u32) -> Option<Self> {
        let Backing::Memory(media) = &self.backing else {
            return None;
        };
        let media = media.borrow();
        Some(Self {
            backing: Backing::Memory(Rc::new(RefCell::new(MemoryMedia {
                boot: boot.checked_sub(1)?,
                configuration: media.configuration.clone(),
                data: media.data.clone(),
            }))),
            fault: Rc::default(),
        })
    }
    /// Memory media's boot counter, configuration and data bytes, to save
    /// them; `None` for file-backed media.
    pub fn image(&self) -> Option<(u32, Vec<u8>, Vec<u8>)> {
        let Backing::Memory(media) = &self.backing else {
            return None;
        };
        let media = media.borrow();
        Some((media.boot, media.configuration.clone(), media.data.clone()))
    }
    /// Memory media holding a saved `image`.
    pub fn from_image(boot: u32, configuration: Vec<u8>, data: Vec<u8>) -> Self {
        Self {
            backing: Backing::Memory(Rc::new(RefCell::new(MemoryMedia {
                boot,
                configuration,
                data,
            }))),
            fault: Rc::default(),
        }
    }
    /// A copy of a region's bytes, for inspection after a scenario.
    pub fn contents(&mut self, r: Region) -> Vec<u8> {
        let mut bytes = vec![0; self.geometry(r).capacity];
        let _ = self.raw_read(r, 0, &mut bytes);
        bytes
    }
    fn path(root: &Path, r: Region) -> PathBuf {
        root.join(if r == Region::Configuration {
            "configuration.bin"
        } else {
            "data.bin"
        })
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
        let fault = self.fault.borrow_mut().fails(r, op, offset, bytes.len());
        let count = fault.map_or(bytes.len(), |fault| fault.landed as usize);
        let failed = fault.map(|fault| fault.error.flash());
        let root = match &self.backing {
            Backing::Files { root, .. } => root,
            Backing::Memory(media) => {
                let mut media = media.borrow_mut();
                let region = if r == Region::Configuration {
                    &mut media.configuration
                } else {
                    &mut media.data
                };
                for (dst, src) in region[offset..offset + count].iter_mut().zip(bytes) {
                    *dst = if erase { *src } else { *dst & *src };
                }
                return failed.map_or(Ok(()), Err);
            }
        };
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(Self::path(root, r))
            .map_err(|_| Error::Failed)?;
        let mut result = vec![0; count];
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| Error::Failed)?;
        file.read_exact(&mut result).map_err(|_| Error::Failed)?;
        for (dst, src) in result.iter_mut().zip(bytes) {
            *dst = if erase { *src } else { *dst & *src };
        }
        file.seek(SeekFrom::Start(offset as u64))
            .map_err(|_| Error::Failed)?;
        file.write_all(&result)
            .and_then(|_| file.sync_data())
            .map_err(|_| Error::Failed)?;
        failed.map_or(Ok(()), Err)
    }
}
impl OwnedFlash for FileFlash {
    type Error = Error;
    fn availability(&self, _: Region) -> Availability {
        Availability::Ready
    }
    fn geometry(&self, r: Region) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: match r {
                Region::Configuration => 8192,
                Region::Data => DATA_BYTES,
                // The simulator never panics into a record.
                Region::Crash => 0,
            },
            program_size: 4,
            erase_size: 4096,
        }
    }
    fn read(&mut self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
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
    fn program(&mut self, r: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        self.mutation(r, offset, bytes, false)
    }
    fn erase(&mut self, r: Region, offset: usize, len: usize) -> Result<(), Error> {
        self.check(r, offset, len, 4096)?;
        self.mutation(r, offset, &vec![0xff; len], true)
    }
}
impl FileFlash {
    /// The region's bytes, as a host reading the medium sees them: no fault
    /// meets it and the device's operations do not count it.
    fn raw_read(&self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        let root = match &self.backing {
            Backing::Files { root, .. } => root,
            Backing::Memory(media) => {
                let media = media.borrow();
                let region = if r == Region::Configuration {
                    &media.configuration
                } else {
                    &media.data
                };
                out.copy_from_slice(&region[offset..offset + out.len()]);
                return Ok(());
            }
        };
        let mut f = fs::File::open(Self::path(root, r)).map_err(|_| Error::Failed)?;
        f.seek(SeekFrom::Start(offset as u64))
            .and_then(|_| f.read_exact(out))
            .map_err(|_| Error::Failed)
    }
}
