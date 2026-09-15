//! File-backed reservations with exclusive ownership and injectable torn writes.
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
pub struct FileFlash {
    root: PathBuf,
    _lock: Rc<fs::File>,
    pub fault: Rc<RefCell<Fault>>,
}
impl FileFlash {
    /// Reset injected faults and advance the persisted session generation.
    pub(crate) fn begin_boot(&self) -> io::Result<u32> {
        *self.fault.borrow_mut() = Fault::default();
        let boot_path = self.root.join("boot");
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
            root: root.to_path_buf(),
            _lock: Rc::new(lock),
            fault: Rc::default(),
        })
    }
    fn path(&self, r: Region) -> PathBuf {
        self.root.join(if r == Region::Configuration {
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
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(self.path(r))
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
        let mut f = fs::File::open(self.path(r)).map_err(|_| Error::Failed)?;
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
