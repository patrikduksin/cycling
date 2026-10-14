//! File- or memory-backed reservations with exclusive ownership and injectable
//! torn writes. Memory media outlives a simulated power loss like flash does.
use device_api::observation::{Availability, Error};
use device_api::storage::{OwnedFlash, Region};
use std::{
    cell::RefCell,
    fs::{self, OpenOptions},
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    rc::Rc,
};
const DATA_BYTES: usize = 1024 * 1024;
#[derive(Default)]
pub struct Fault {
    pub after: Option<u32>,
    pub partial: usize,
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
    /// A copy of a region's bytes, for inspection after a scenario.
    pub fn contents(&mut self, r: Region) -> Vec<u8> {
        let mut bytes = vec![0; self.geometry(r).capacity];
        let _ = self.read(r, 0, &mut bytes);
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
        let mut fault = self.fault.borrow_mut();
        let failed = fault.after == Some(0);
        if let Some(n) = fault.after.as_mut() {
            *n = n.saturating_sub(1);
        }
        let count = if failed {
            fault.partial.min(bytes.len())
        } else {
            bytes.len()
        };
        if failed {
            fault.after = None;
        }
        drop(fault);
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
                return if failed { Err(Error::Failed) } else { Ok(()) };
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
        if failed { Err(Error::Failed) } else { Ok(()) }
    }
}
impl OwnedFlash for FileFlash {
    type Error = Error;
    fn availability(&self, _: Region) -> Availability {
        Availability::Ready
    }
    fn geometry(&self, r: Region) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: if r == Region::Configuration {
                8192
            } else {
                DATA_BYTES
            },
            program_size: 4,
            erase_size: 4096,
        }
    }
    fn read(&mut self, r: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        self.check(r, offset, out.len(), 1)?;
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
    fn program(&mut self, r: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        self.mutation(r, offset, bytes, false)
    }
    fn erase(&mut self, r: Region, offset: usize, len: usize) -> Result<(), Error> {
        self.check(r, offset, len, 4096)?;
        self.mutation(r, offset, &vec![0xff; len], true)
    }
}
