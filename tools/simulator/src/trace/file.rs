//! A capture on disk: `<name>.trace` holds the trace as the device writes
//! it, and beside it `<name>.origin` holds the media the session started
//! on: saved by the simulator, or rebuilt for a device trace from its
//! header and the media `cycling trace pull` copied. A trace without one
//! replays on blank media.
use crate::bulk::{Image, Presence, SimBulk};
use crate::replay::{Capture, Origin};
use crate::storage::FileFlash;
use device_api::bulk::SECTOR_SIZE;
use std::io;
use std::path::{Path, PathBuf};

const MAGIC: &[u8; 12] = b"VANA-ORIGIN1";
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
        let (origin, report) = decode(&std::fs::read(&origin)?)
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
    let (_, configuration, data) = FileFlash::memory().image().expect("memory media");
    let origin = Origin {
        flash: FileFlash::from_image(boot - 1, configuration, data),
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

fn encode(origin: &Origin, report: Option<&str>) -> io::Result<Vec<u8>> {
    let (_, configuration, data) = origin.flash.image().ok_or_else(|| {
        io::Error::other("the capture's media is file-backed and cannot be saved")
    })?;
    let mut out = MAGIC.to_vec();
    out.extend_from_slice(&origin.boot.to_le_bytes());
    for region in [&configuration, &data] {
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

fn decode(bytes: &[u8]) -> Option<(Origin, Option<String>)> {
    let mut r = Bytes(bytes.strip_prefix(MAGIC)?);
    let boot = r.u32()?;
    let mut regions = Vec::new();
    for _ in 0..2 {
        let mut region = vec![0xff; r.u32()? as usize];
        for _ in 0..r.u32()? {
            let at = r.u32()? as usize * PAGE;
            let len = r.u32()? as usize;
            region.get_mut(at..at + len)?.copy_from_slice(r.take(len)?);
        }
        regions.push(region);
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
    let data = regions.pop()?;
    let configuration = regions.pop()?;
    let origin = Origin {
        flash: FileFlash::from_image(boot.checked_sub(1)?, configuration, data),
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
