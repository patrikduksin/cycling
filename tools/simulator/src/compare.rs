//! Comparison mode: three hashes per scenario that a pure refactor leaves
//! unchanged. Every value is folded in as explicit bytes, never as Rust
//! `Debug` text, so renaming a type changes no hash.
//!
//! A `Sim` compares when `SIM_COMPARE=1` is set as it is made, or when it
//! is made with `Sim::comparing`.
//!
//! - The trace stream: every event as the recorder takes it, before the
//!   spool can refuse its block, the BLE calls the trace passes through
//!   unrecorded, and every log line as it is logged, VANA's `info` lines
//!   included, with its pass time. Each session's stream is folded in with
//!   its ordinal when the session ends.
//! - The storage stream: every operation on either medium through the
//!   device's handles, with its result. That includes a scenario's setup
//!   through those handles. Handles without the tap (`SimBulk::host`,
//!   `untapped`, `FileFlash::contents`) and copies are left out, and so are the
//!   trace spool's own card writes, which go through host handles.
//! - The media: what both media hold once the scenario ends. FILES and STORE
//!   count by file name and bytes, so the order files were written in does
//!   not matter, and the trace spool is left out. A volume that does not
//!   read whole counts as its raw sectors.
use crate::bulk::{Presence, SimBulk};
use crate::logs::Line;
use crate::storage::FileFlash;
use device_api::storage::Region;
use firmware_services::trace::frozen::{Passed, Tap as RecorderTap};
use firmware_services::trace::{Event, Medium, StorageError, StorageOp, spool, tapped};
use firmware_services::{files, store, volume};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::{cell::RefCell, ops::Range, rc::Rc};

pub const ENV: &str = "SIM_COMPARE";
/// Written beside a run's scenario directories by a simulator that compares.
pub const MARKER: &str = "comparison-mode";

pub fn requested() -> bool {
    std::env::var_os(ENV).is_some_and(|value| value == "1")
}

/// The build a trace header and `INFO` name. Comparison mode names forty
/// zeros, so both sides of `cycling sim-compare` name one build.
pub fn build_hash() -> &'static str {
    if requested() {
        "0000000000000000000000000000000000000000"
    } else {
        env!("CYCLING_BUILD_HASH")
    }
}

/// One scenario's hashes, as its `hashes.json` holds them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hashes {
    pub media: String,
    pub trace: String,
    pub storage: String,
}

impl Hashes {
    pub fn json(&self) -> String {
        format!(
            "{{\"media\": \"{}\", \"trace\": \"{}\", \"storage\": \"{}\"}}\n",
            self.media, self.trace, self.storage
        )
    }
}

/// A simulated device's running digests. Every session the device boots
/// folds into the same one.
#[derive(Clone, Default)]
pub struct Tap(Rc<RefCell<Streams>>);

#[derive(Default)]
struct Streams {
    session: Sha256,
    sessions: Sha256,
    ended: u32,
    storage: Sha256,
}

fn fold(digest: &mut Sha256, kind: u8, bytes: &[u8]) {
    digest.update([kind]);
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

impl Tap {
    pub fn end_session(&self) {
        let streams = &mut *self.0.borrow_mut();
        let session = core::mem::take(&mut streams.session).finalize();
        streams.sessions.update(streams.ended.to_le_bytes());
        streams.sessions.update(session);
        streams.ended += 1;
    }

    pub(crate) fn storage(
        &self,
        medium: Medium,
        op: StorageOp,
        at: u64,
        len: u64,
        result: Result<(), StorageError>,
    ) {
        let operation = tapped::storage(medium, op, at, len, result);
        fold(&mut self.0.borrow_mut().storage, b's', &operation);
    }

    pub(crate) fn log(&self, now_ms: u64, line: Line, target: &str, text: &str) {
        let session = &mut self.0.borrow_mut().session;
        fold(session, b'l', &now_ms.to_le_bytes());
        match line {
            Line::Kept(level) => fold(
                session,
                b'w',
                &tapped::event(&Event::Log {
                    level,
                    target,
                    text,
                }),
            ),
            Line::Info => {
                fold(session, b'i', target.as_bytes());
                fold(session, b'i', text.as_bytes());
            }
        }
    }

    /// The scenario's hashes so far, the open session folded in as if it
    /// ended now, with `media` as the media hash.
    pub fn hashes(&self, media: String) -> Hashes {
        let streams = self.0.borrow();
        let mut trace = streams.sessions.clone();
        trace.update(streams.ended.to_le_bytes());
        trace.update(streams.session.clone().finalize());
        Hashes {
            media,
            trace: hex(&trace.finalize()),
            storage: hex(&streams.storage.clone().finalize()),
        }
    }
}

fn folded_where_logged(event: &Event<'_>) -> bool {
    matches!(event, Event::Log { .. })
}

impl RecorderTap for Tap {
    fn event(&mut self, event: &Event<'_>) {
        if folded_where_logged(event) {
            return;
        }
        fold(
            &mut self.0.borrow_mut().session,
            b'e',
            &tapped::event(event),
        );
    }

    fn passed(&mut self, call: Passed) {
        fold(
            &mut self.0.borrow_mut().session,
            b'p',
            &tapped::passed(call),
        );
    }
}

/// What the media hold, as a host would read them: the internal regions
/// whole; on the card every written sector outside a volume that reads
/// whole, and the files of those volumes by name and bytes.
pub fn media(flash: &mut FileFlash, bulk: &SimBulk) -> String {
    let mut digest = Sha256::new();
    for region in [Region::Configuration, Region::Data, Region::Intent] {
        fold(&mut digest, b'f', &flash.contents(region));
    }
    let image = bulk.image();
    let presence = match image.presence {
        Presence::Absent => 0,
        Presence::Missing => 1,
        Presence::Ready => 2,
    };
    fold(&mut digest, b'b', &[presence]);
    let written = image.written.iter().map(|(at, _)| *at).collect();
    let listed = listed(&mut bulk.inspect(), &written);
    for (at, bytes) in &image.written {
        if !listed
            .iter()
            .any(|volume| volume.covered_sectors.iter().any(|run| run.contains(at)))
        {
            fold(&mut digest, b'w', &at.to_le_bytes());
            fold(&mut digest, b'w', bytes);
        }
    }
    for (name, bytes) in listed.iter().flat_map(|volume| &volume.files) {
        fold(&mut digest, b'n', name.as_bytes());
        fold(&mut digest, b'n', bytes);
    }
    hex(&digest.finalize())
}

struct Listed {
    files: Vec<(String, Vec<u8>)>,
    covered_sectors: Vec<Range<u64>>,
}

fn listed(card: &mut SimBulk, written: &BTreeSet<u64>) -> Vec<Listed> {
    let mut sector = [0u8; 512];
    let Ok(volume::Layout::Table(table)) = volume::read(card, &mut sector) else {
        return Vec::new();
    };
    let files = table
        .find(&volume::FILES)
        .and_then(|extent| files_in(card, extent));
    let store = table
        .find(&volume::STORE)
        .and_then(|extent| store_in(card, extent, written));
    files.into_iter().chain(store).collect()
}

fn whole(extent: &volume::Extent) -> Range<u64> {
    extent.start..extent.start + extent.sectors
}

fn files_in(card: &mut SimBulk, extent: volume::Extent) -> Option<Listed> {
    let mut sector = [0u8; 512];
    let mut vol = volume::Volume::new(card, extent);
    let dir = files::mount(&mut vol, &mut sector).ok()?;
    let mut entries = Vec::new();
    dir.list(&mut vol, &mut sector, |entry| entries.push(*entry))
        .ok()?;
    let mut found = Vec::new();
    for entry in entries {
        let mut bytes = vec![0u8; entry.length as usize];
        dir.read(&mut vol, &entry, 0, &mut bytes, &mut sector)
            .ok()?;
        found.push((format!("FILES/{}", entry.name.as_str()), bytes));
    }
    found.sort();
    Some(Listed {
        files: found,
        covered_sectors: vec![whole(&extent)],
    })
}

fn store_in(card: &mut SimBulk, extent: volume::Extent, written: &BTreeSet<u64>) -> Option<Listed> {
    let mut sector = [0u8; 512];
    let mut vol = volume::Volume::new(card, extent);
    let mounted = store::mount(&mut vol, &mut sector).ok()?;
    let traces = store::Dir::new(spool::TRACES).expect("the spool's directory");
    let mut names = Vec::new();
    mounted
        .list(&mut vol, &store::Dir::ROOT, &mut sector, |entry| {
            names.push(entry.name)
        })
        .ok()?;
    let (spool, names): (Vec<_>, Vec<_>) = names.into_iter().partition(|name| name.is_in(&traces));
    let mut found = Vec::new();
    for name in names {
        let read = records(&mounted, &mut vol, &name, &mut sector);
        let sectors = written_record_sectors(&mounted, &mut vol, extent, &name, written);
        match (read, sectors) {
            (Some((bytes, records)), Some(sectors)) if reads_whole(records, sectors) => {
                found.push((format!("STORE/{}", name.as_str()), bytes));
            }
            _ => return raw_except_spool_chunks(&mounted, &mut vol, extent, &spool),
        }
    }
    found.sort();
    Some(Listed {
        files: found,
        covered_sectors: vec![whole(&extent)],
    })
}

fn raw_except_spool_chunks(
    mounted: &store::Store,
    vol: &mut impl device_api::bulk::ReadWrite,
    extent: volume::Extent,
    spool: &[store::Name],
) -> Option<Listed> {
    let mut sector = [0u8; 512];
    let mut covered_sectors = Vec::new();
    for name in spool {
        mounted
            .chunk_starts(vol, name, &mut sector, |at| {
                covered_sectors.push(chunk(extent, mounted, at));
            })
            .ok()?;
    }
    Some(Listed {
        files: Vec::new(),
        covered_sectors,
    })
}

fn chunk(extent: volume::Extent, mounted: &store::Store, at: u64) -> Range<u64> {
    let at = extent.start + at;
    at..at + u64::from(mounted.chunk_sectors())
}

fn records(
    mounted: &store::Store,
    vol: &mut impl device_api::bulk::ReadWrite,
    name: &store::Name,
    sector: &mut [u8; 512],
) -> Option<(Vec<u8>, usize)> {
    let mut reader = mounted.reader(vol, name, sector).ok()?;
    let (mut bytes, mut record, mut count) = (Vec::new(), [0u8; store::PAYLOAD], 0);
    loop {
        match reader.next(mounted, vol, &mut record, sector).ok()? {
            store::Next::Record(n) => {
                bytes.push(b'r');
                bytes.extend_from_slice(&(n as u32).to_le_bytes());
                bytes.extend_from_slice(&record[..n]);
                count += 1;
            }
            store::Next::Hole { .. } => return None,
            store::Next::End => return Some((bytes, count)),
        }
    }
}

fn reads_whole(records: usize, written_record_sectors: usize) -> bool {
    records == written_record_sectors
}

fn written_record_sectors(
    mounted: &store::Store,
    vol: &mut impl device_api::bulk::ReadWrite,
    extent: volume::Extent,
    name: &store::Name,
    written: &BTreeSet<u64>,
) -> Option<usize> {
    let mut sector = [0u8; 512];
    let mut count = 0;
    mounted
        .chunk_starts(vol, name, &mut sector, |at| {
            count += written.range(chunk(extent, mounted, at)).count();
        })
        .ok()?;
    Some(count)
}
