//! A capture on disk: `<name>.trace` holds the trace as the device writes
//! it, and beside it `<name>.origin` holds the media the session started
//! on: saved by the simulator, or rebuilt for a device trace from its
//! header and the media `cycling trace pull` copied. A trace without one
//! replays on blank media.
use crate::bulk::{Image, Presence, SimBulk};
use crate::replay::{Capture, Origin};
use crate::storage::{FileFlash, FlashImage};
use device_api::bulk::SECTOR_SIZE;
use device_api::storage::Region;
use std::io;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 12] = b"VANA-ORIGIN2";
/// ORIGIN1 holds neither intent bytes nor marginal words.
const MAGIC_ONE: &[u8; 12] = b"VANA-ORIGIN1";
const PAGE: usize = 4096;

/// Where a loaded capture's media came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Media {
    /// The `.origin` the simulator saved beside the trace.
    Saved,
    /// The `.origin` rebuilt for a device trace, with what it could not
    /// rebuild, one line each, or that nothing is missing.
    Rebuilt(String),
    /// None was saved: erased flash and no bulk medium.
    Blank,
}

impl Media {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Saved => "saved",
            Self::Rebuilt(_) => "rebuilt",
            Self::Blank => "blank",
        }
    }
}

pub fn origin_path(trace: &Path) -> PathBuf {
    trace.with_extension("origin")
}

/// Write `capture` as `trace` and its origin beside it.
pub fn save(capture: &Capture, trace: &Path) -> io::Result<()> {
    if let Some(parent) = trace.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(trace, &capture.trace)?;
    std::fs::write(origin_path(trace), encode(&capture.origin, None)?)
}

/// Write `origin`, rebuilt for the device trace at `trace`, beside it with
/// `report`, what it could not rebuild.
pub fn save_rebuilt(origin: &Origin, report: &str, trace: &Path) -> io::Result<()> {
    std::fs::write(origin_path(trace), encode(origin, Some(report))?)
}

/// Read the trace at `trace` and the media it started on.
pub fn load(trace: &Path) -> io::Result<(Capture, Media)> {
    let bytes = std::fs::read(trace)?;
    let origin = origin_path(trace);
    if origin.exists() {
        let saved = std::fs::read(&origin)?;
        if saved.starts_with(MAGIC_ONE) {
            return Err(io::Error::other(format!(
                "{} is an ORIGIN1 origin, from before the intent region; `cycling trace upgrade {}` converts it",
                origin.display(),
                trace.display()
            )));
        }
        let (origin, report) = decode(&saved, MAGIC)
            .ok_or_else(|| io::Error::other(format!("{} is malformed", origin.display())))?;
        return Ok((
            Capture {
                origin,
                trace: bytes,
            },
            report.map_or(Media::Saved, Media::Rebuilt),
        ));
    }
    let boot = firmware_services::trace::decode(&bytes)
        .map(|(header, _)| header.boot)
        .unwrap_or(1)
        .max(1);
    let origin = Origin {
        flash: FileFlash::from_image(boot - 1, FlashImage::erased()),
        bulk: SimBulk::default(),
        boot,
    };
    Ok((
        Capture {
            origin,
            trace: bytes,
        },
        Media::Blank,
    ))
}

/// Rewrite the ORIGIN1 origin beside `trace` as ORIGIN2, with a blank
/// intent region and no marginal words. False when
/// it is ORIGIN2 already.
pub fn upgrade(trace: &Path) -> io::Result<bool> {
    let path = origin_path(trace);
    let bytes = std::fs::read(&path)?;
    if bytes.starts_with(MAGIC) {
        return Ok(false);
    }
    let (origin, report) = decode(&bytes, MAGIC_ONE)
        .ok_or_else(|| io::Error::other(format!("{} is malformed", path.display())))?;
    std::fs::write(&path, encode(&origin, report.as_deref())?)?;
    Ok(true)
}

const REGIONS: [Region; 4] = [
    Region::Configuration,
    Region::Data,
    Region::Intent,
    Region::Crash,
];

fn encode(origin: &Origin, report: Option<&str>) -> io::Result<Vec<u8>> {
    let (_, flash) = origin.flash.image().ok_or_else(|| {
        io::Error::other("the capture's media is file-backed and cannot be saved")
    })?;
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&origin.boot.to_le_bytes());
    for region in [&flash.configuration, &flash.data, &flash.intent] {
        out.extend_from_slice(&(region.len() as u32).to_le_bytes());
        let pages: Vec<(usize, &[u8])> = region
            .chunks(PAGE)
            .enumerate()
            .filter(|(_, page)| page.iter().any(|&byte| byte != 0xff))
            .collect();
        out.extend_from_slice(&(pages.len() as u32).to_le_bytes());
        for (index, page) in pages {
            out.extend_from_slice(&(index as u32).to_le_bytes());
            out.extend_from_slice(&(page.len() as u32).to_le_bytes());
            out.extend_from_slice(page);
        }
    }
    out.extend_from_slice(&(flash.marginal.len() as u32).to_le_bytes());
    for &(region, at, whole) in &flash.marginal {
        let code = REGIONS.iter().position(|&r| r == region).expect("a region");
        out.push(code as u8);
        out.extend_from_slice(&(at as u32).to_le_bytes());
        out.push(whole);
    }
    let bulk = origin.bulk.image();
    out.push(match bulk.presence {
        Presence::Absent => 0,
        Presence::Missing => 1,
        Presence::Ready => 2,
    });
    out.extend_from_slice(&bulk.sectors.to_le_bytes());
    out.extend_from_slice(&bulk.entropy.to_le_bytes());
    out.extend_from_slice(&(bulk.written.len() as u32).to_le_bytes());
    for (sector, bytes) in &bulk.written {
        out.extend_from_slice(&sector.to_le_bytes());
        out.extend_from_slice(bytes);
    }
    if let Some(report) = report {
        out.extend_from_slice(&(report.len() as u32).to_le_bytes());
        out.extend_from_slice(report.as_bytes());
    }
    Ok(out)
}

fn decode(bytes: &[u8], magic: &[u8; 12]) -> Option<(Origin, Option<String>)> {
    let mut r = Bytes(bytes.strip_prefix(magic)?);
    let boot = r.u32()?;
    let mut flash = FlashImage::erased();
    let mut regions = vec![&mut flash.configuration, &mut flash.data];
    if magic == MAGIC {
        regions.push(&mut flash.intent);
    }
    for region in regions {
        *region = vec![0xff; r.u32()? as usize];
        for _ in 0..r.u32()? {
            let at = r.u32()? as usize * PAGE;
            let len = r.u32()? as usize;
            region.get_mut(at..at + len)?.copy_from_slice(r.take(len)?);
        }
    }
    if magic == MAGIC {
        for _ in 0..r.u32()? {
            let region = *REGIONS.get(usize::from(r.take(1)?[0]))?;
            let at = r.u32()? as usize;
            flash.marginal.push((region, at, r.take(1)?[0]));
        }
    }
    let presence = match r.take(1)?[0] {
        0 => Presence::Absent,
        1 => Presence::Missing,
        2 => Presence::Ready,
        _ => return None,
    };
    let sectors = r.u64()?;
    let entropy = r.u64()?;
    let mut written = Vec::new();
    for _ in 0..r.u32()? {
        let sector = r.u64()?;
        written.push((sector, r.take(SECTOR_SIZE)?.try_into().ok()?));
    }
    let report = if r.0.is_empty() {
        None
    } else {
        let len = r.u32()? as usize;
        Some(String::from_utf8(r.take(len)?.to_vec()).ok()?)
    };
    if !r.0.is_empty() {
        return None;
    }
    let origin = Origin {
        flash: FileFlash::from_image(boot.checked_sub(1)?, flash),
        bulk: SimBulk::from_image(Image {
            presence,
            sectors,
            entropy,
            written,
        }),
        boot,
    };
    Some((origin, report))
}

struct Bytes<'a>(&'a [u8]);

impl<'a> Bytes<'a> {
    fn take(&mut self, len: usize) -> Option<&'a [u8]> {
        let (head, rest) = self.0.split_at_checked(len)?;
        self.0 = rest;
        Some(head)
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }
    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }
}
