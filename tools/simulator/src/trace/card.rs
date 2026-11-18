//! What a strict replay of a device's trace needs beside the trace: a card
//! for the replay's own trace file, and the TERRAIN pack's data.
//!
//! A device's trace comes without an `.origin`. It holds the card's size
//! and its volume table, so the replay's card has the device's volumes and
//! an empty STORE. The pack's data is too large for a trace, which holds
//! digests of it: the replay reads the data from the pack file the header
//! names by its SHA-256, in the local pack store.
use crate::bulk::{Presence, SimBulk};
use crate::trace::world::{self, World};
use crate::twin::script::{Hex, declared};
use device_api::bulk::{self, ReadWrite, SECTOR_SIZE};
use firmware_services::trace::origin::{Found, Terrain};
use firmware_services::trace::{self, Event, Medium, StorageOp};
use firmware_services::volume::{self, Layout, Volume};
use firmware_services::{pack, store};
use sha2::Digest;
use std::fs::File;
use std::io::{self, Read};
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// The environment variable that names the pack store.
pub const STORE: &str = "CYCLING_TERRAIN_PACKS";

/// The directory of pack files a replay reads TERRAIN from: the one
/// [`STORE`] names, or where `cycling terrain` builds packs.
pub fn pack_store() -> PathBuf {
    std::env::var_os(STORE).map_or_else(|| PathBuf::from(".local/terrain"), PathBuf::from)
}

/// The card `trace` recorded, as far as a replay's own trace file needs it:
/// its size, its volume table and an empty STORE. No card when the trace's
/// session had none.
pub fn of(trace: &[u8]) -> SimBulk {
    let card = SimBulk::default();
    let Ok((_, events)) = trace::decode(trace) else {
        return card;
    };
    let mut host = card.host();
    let mut table = [false; 2];
    for event in events {
        let Event::Call(call) = event else {
            continue;
        };
        let named =
            declared(call.method).map(|(capability, method)| (capability.name, method.name));
        if named == Some(("ReadWrite", "owned_info")) {
            let Some(Ok(info)) = call.decode::<Result<bulk::OwnedInfo, bulk::Error>>() else {
                return card;
            };
            card.set_presence(Presence::Ready);
            card.set_sectors(info.sectors);
        }
        let Some(World::Media(StorageOp::Read, access)) = world::read(&call) else {
            continue;
        };
        let copy = access.span.start as usize;
        if access.span.medium != Medium::Bulk || copy >= table.len() || table[copy] {
            continue;
        }
        let mut sector = [0u8; SECTOR_SIZE];
        if access.expand(&mut sector) == Some(SECTOR_SIZE) {
            table[copy] = host.owned_write(copy as u64, &sector).is_ok();
        }
        if table == [true; 2] {
            break;
        }
    }
    let mut sector = [0u8; SECTOR_SIZE];
    if let Ok(Layout::Table(table)) = volume::read(&mut host, &mut sector)
        && let Some(extent) = table.find(&volume::STORE)
    {
        let _ = store::format(&mut Volume::new(&mut host, extent), 0, &mut sector);
    }
    card
}

/// Where a replay reads the TERRAIN pack's data.
pub enum Pack {
    /// On the card the session started on.
    Card(SimBulk),
    /// In a pack file, whose data starts at sector `first` of the
    /// recorded card.
    File { file: File, first: u64, length: u64 },
}

impl Pack {
    /// The pack's data in `sector` of the recorded card; `None` outside it.
    pub fn sector(&self, sector: u64) -> Option<[u8; SECTOR_SIZE]> {
        match self {
            Self::Card(card) => Some(card.sector(sector)),
            Self::File {
                file,
                first,
                length,
            } => {
                let at = sector.checked_sub(*first)? * SECTOR_SIZE as u64;
                let held = length.checked_sub(at).filter(|held| *held > 0)?;
                let mut bytes = [0u8; SECTOR_SIZE];
                let len = held.min(SECTOR_SIZE as u64) as usize;
                file.read_exact_at(&mut bytes[..len], at).ok()?;
                Some(bytes)
            }
        }
    }
}

fn digest(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut sha = sha2::Sha256::new();
    let mut chunk = vec![0u8; 1 << 20];
    loop {
        let read = file.read(&mut chunk)?;
        if read == 0 {
            return Ok(sha.finalize().into());
        }
        sha.update(&chunk[..read]);
    }
}

fn find(store: &Path, terrain: &Terrain) -> io::Result<PathBuf> {
    let missing = || {
        let tag = String::from_utf8_lossy(&terrain.tag);
        io::Error::other(format!(
            "the session read the TERRAIN pack {} (sha256 {}, {} bytes), and no file in {} is that pack; \
             put the pack file there, or name its directory in {STORE}",
            tag.trim_end_matches('\0'),
            Hex(&terrain.digest),
            terrain.length,
            store.display(),
        ))
    };
    let mut files: Vec<PathBuf> = std::fs::read_dir(store)
        .map_err(|_| missing())?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| {
            std::fs::metadata(path).is_ok_and(|file| file.is_file() && file.len() == terrain.length)
        })
        .collect();
    files.sort();
    for path in files {
        if digest(&path)? == terrain.digest {
            return Ok(path);
        }
    }
    Err(missing())
}

/// The TERRAIN pack `trace`'s header names: on the `origin` card when it
/// holds that pack, else a file in `store`. `None` when the session found
/// no ready pack.
pub fn pack(trace: &[u8], origin: &SimBulk, store: &Path) -> io::Result<Option<Pack>> {
    let header = trace::decode(trace)
        .map(|(header, _)| header)
        .map_err(|error| io::Error::other(format!("trace: {error}")))?;
    let Found::Read(terrain) = header.terrain else {
        return Ok(None);
    };
    if !terrain.ready {
        return Ok(None);
    }
    let mut sector = [0u8; SECTOR_SIZE];
    let held = |card: &SimBulk| {
        let mut host = card.host();
        let mut sector = [0u8; SECTOR_SIZE];
        let Ok(Layout::Table(table)) = volume::read(&mut host, &mut sector) else {
            return None;
        };
        let extent = table.find(&volume::TERRAIN)?;
        let header = pack::status(&mut Volume::new(&mut host, extent), &mut sector).ok()??;
        Some(header.spec.digest)
    };
    if held(origin) == Some(terrain.digest) {
        return Ok(Some(Pack::Card(origin.untapped())));
    }
    let recorded = of(trace);
    let first = match volume::read(&mut recorded.host(), &mut sector) {
        Ok(Layout::Table(table)) => table.find(&volume::TERRAIN).map(|extent| extent.start),
        _ => None,
    };
    let Some(first) = first else {
        return Ok(None);
    };
    let path = find(store, &terrain)?;
    Ok(Some(Pack::File {
        file: File::open(path)?,
        first: first + pack::DATA,
        length: terrain.length,
    }))
}

/// The TERRAIN pack `trace`'s header names as a file a bundle can carry: the
/// pack file in `store`, else the pack's data on the `origin` card, written
/// to `spill`. `None` when the session found no ready pack.
pub fn pack_file(
    trace: &[u8],
    origin: &SimBulk,
    store: &Path,
    spill: &Path,
) -> io::Result<Option<PathBuf>> {
    let header = trace::decode(trace)
        .map(|(header, _)| header)
        .map_err(|error| io::Error::other(format!("trace: {error}")))?;
    let Found::Read(terrain) = header.terrain else {
        return Ok(None);
    };
    if !terrain.ready {
        return Ok(None);
    }
    let missing = match find(store, &terrain) {
        Ok(path) => return Ok(Some(path)),
        Err(missing) => missing,
    };
    let mut host = origin.host();
    let mut sector = [0u8; SECTOR_SIZE];
    let Ok(Layout::Table(table)) = volume::read(&mut host, &mut sector) else {
        return Err(missing);
    };
    let Some(extent) = table.find(&volume::TERRAIN) else {
        return Err(missing);
    };
    let held = pack::status(&mut Volume::new(&mut host, extent), &mut sector);
    if !matches!(held, Ok(Some(header)) if header.spec.digest == terrain.digest) {
        return Err(missing);
    }
    let first = extent.start + pack::DATA;
    let mut bytes = Vec::with_capacity(terrain.length as usize);
    for at in (0..terrain.length).step_by(SECTOR_SIZE) {
        let data = origin.sector(first + at / SECTOR_SIZE as u64);
        let len = (terrain.length - at).min(SECTOR_SIZE as u64) as usize;
        bytes.extend_from_slice(&data[..len]);
    }
    std::fs::write(spill, bytes)?;
    Ok(Some(spill.to_path_buf()))
}
