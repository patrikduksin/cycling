//! The media a trace recorded, indexed once when a replay opens it, and the
//! twin's model of each medium, which answers every read the twin makes.
//!
//! A recorded `Forget` ends an era of its medium; era 0 starts at power-on.
//! Each era holds its recorded mutations in order, and every byte a read
//! carried, filed under its version: the last recorded mutation that
//! covered it before the read. Bytes no mutation covered first are the
//! era's pristine image, and the model starts each era from it.
//!
//! The model applies the twin's own mutations, never the recording's. Each
//! is matched to the first recorded mutation of the era not matched yet
//! with its identity: method, span, and the CRC of its content, an erase
//! or a fault. A matched program or fault installs the bytes the device
//! read after it, over the units where the twin is in step: its mutations
//! there so far are exactly the recorded ones up to this one, in order. A
//! failed write installs them only when the twin attempted the bytes the
//! device did; otherwise its span is the twin's own and unknown. So
//! a build that writes something else reads back what it wrote, and one
//! that skips a write keeps the old bytes, never the device's.
//!
//! Reads are not matched, so a cached, earlier or later read replays. The
//! exceptions are keyed inputs, found by method, start and occurrence at
//! that start in the era, and then by count: a recorded read fault, and a
//! read of STORE, whose bytes change unseen by the trace and so belong to
//! no version.
use super::media::{Change, Fill, Outcome, TwinMedium};
use super::script::declared;
use crate::trace::card::Pack;
use device_api::capability::{Kind, Media};
use device_api::journal::MethodId;
use firmware_services::checksum::crc32_more;
use firmware_services::trace::determined::Span;
use firmware_services::trace::media::{Access, Content, Volumes};
use firmware_services::trace::{self, Event, Medium, StorageError};
use firmware_services::volume::Extent;
use std::collections::BTreeMap;
use std::fmt;
use std::ops::Range;

#[derive(Clone, Debug)]
struct Units<T>(BTreeMap<u64, (u64, T)>);

impl<T> Default for Units<T> {
    fn default() -> Self {
        Self(BTreeMap::new())
    }
}

impl<T: Copy> Units<T> {
    fn set(&mut self, range: Range<u64>, value: T) {
        let overlapping: Vec<(u64, (u64, T))> = self
            .0
            .range(..range.end)
            .rev()
            .take_while(|(_, (end, _))| *end > range.start)
            .map(|(start, held)| (*start, *held))
            .collect();
        for (start, (end, held)) in overlapping {
            self.0.remove(&start);
            if start < range.start {
                self.0.insert(start, (range.start, held));
            }
            if end > range.end {
                self.0.insert(range.end, (end, held));
            }
        }
        self.0.insert(range.start, (range.end, value));
    }

    fn segments(&self, range: Range<u64>) -> Vec<(Range<u64>, Option<T>)> {
        let mut segments = Vec::new();
        let mut at = range.start;
        let first = self
            .0
            .range(..=range.start)
            .next_back()
            .filter(|(_, (end, _))| *end > range.start)
            .map(|(start, _)| *start)
            .unwrap_or(range.start);
        for (&start, &(end, value)) in self.0.range(first..range.end) {
            if start > at {
                segments.push((at..start, None));
            }
            let from = start.max(range.start);
            let to = end.min(range.end);
            segments.push((from..to, Some(value)));
            at = to;
        }
        if at < range.end {
            segments.push((at..range.end, None));
        }
        segments
    }

    fn get(&self, unit: u64) -> Option<T> {
        let (_, (end, value)) = self.0.range(..=unit).next_back()?;
        (unit < *end).then_some(*value)
    }

    fn iter(&self) -> impl Iterator<Item = (Range<u64>, T)> + '_ {
        (self.0.iter()).map(|(&start, &(end, value))| (start..end, value))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Effect {
    Content(u32),
    Erase,
    /// A failed write carries the CRC of what it attempted.
    Fault {
        fault: StorageError,
        attempted: Option<u32>,
    },
}

#[derive(Clone, Debug)]
struct Mutation {
    start: u32,
    count: u32,
    effect: Effect,
    before: Vec<(Range<u64>, Option<u32>)>,
}

impl Mutation {
    fn end(&self) -> u32 {
        self.start + self.count
    }
}

#[derive(Clone, Debug)]
enum Read {
    Plain,
    Fault(StorageError, Vec<u8>),
    Volatile(Vec<u8>),
}

/// Where a read starts: its occurrences there are counted.
type Key = (MethodId, u32);

#[derive(Default)]
struct Era {
    pristine: TwinMedium,
    versions: BTreeMap<u32, TwinMedium>,
    mutations: Vec<Mutation>,
    by_start: BTreeMap<(MethodId, u32), Vec<u32>>,
    /// Each read at its start, in order, with its count.
    reads: BTreeMap<Key, Vec<(u32, Read)>>,
    digests: BTreeMap<(Option<u32>, Key, u32), Vec<u32>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Class {
    Inline,
    Volatile,
    Terrain,
    Outside,
}

fn class(volumes: &Volumes, medium: Medium, start: u32) -> Class {
    let within = |extent: Option<Extent>| {
        extent.is_some_and(|extent| {
            (extent.start..extent.start + extent.sectors).contains(&u64::from(start))
        })
    };
    match medium {
        Medium::Configuration => Class::Outside,
        Medium::Bulk if within(volumes.store) => Class::Volatile,
        Medium::Bulk if within(volumes.terrain) => Class::Terrain,
        _ => Class::Inline,
    }
}

fn verb(method: MethodId) -> Option<Media> {
    match declared(method)?.1.kind {
        Kind::Media(verb) => Some(verb),
        _ => None,
    }
}

fn units(start: u32, count: u32) -> Range<u64> {
    u64::from(start)..u64::from(start) + u64::from(count)
}

struct Part {
    method: MethodId,
    medium: Medium,
    start: u32,
    count: u32,
    fault: Option<StorageError>,
    bytes: Vec<u8>,
}

#[derive(Default)]
struct Indexer {
    eras: BTreeMap<u8, Vec<Era>>,
    last: BTreeMap<u8, Units<u32>>,
    part: Option<Part>,
    volumes: Volumes,
    conflict: Option<String>,
    orphan: Option<Span>,
}

impl Indexer {
    fn era(&mut self, medium: Medium) -> &mut Era {
        let eras = self.eras.entry(medium as u8).or_default();
        if eras.is_empty() {
            eras.push(Era::default());
        }
        eras.last_mut().expect("an era")
    }

    fn forget(&mut self, medium: Medium) {
        self.flush();
        self.era(medium);
        self.eras
            .get_mut(&(medium as u8))
            .expect("an era")
            .push(Era::default());
        self.last.remove(&(medium as u8));
    }

    fn flush(&mut self) {
        let Some(part) = self.part.take() else {
            return;
        };
        let read = match part.fault {
            Some(fault) => Read::Fault(fault, part.bytes),
            None if class(&self.volumes, part.medium, part.start) == Class::Volatile => {
                Read::Volatile(part.bytes)
            }
            None => Read::Plain,
        };
        self.era(part.medium)
            .reads
            .entry((part.method, part.start))
            .or_default()
            .push((part.count, read));
    }

    fn call(&mut self, call: &trace::Call<'_>) {
        let Some(verb) = verb(call.method) else {
            return self.flush();
        };
        let Some(access) = Access::of(call).filter(|access| !access.outside) else {
            return self.flush();
        };
        let span = access.span;
        match verb {
            Media::Read => self.read(call.method, access),
            Media::Write | Media::Erase => {
                self.flush();
                let effect = match (access.fault, access.content) {
                    (Some(fault), content) => Effect::Fault {
                        fault,
                        attempted: match content {
                            Content::Digest(crc) => Some(crc),
                            _ => None,
                        },
                    },
                    (None, Content::Digest(crc)) => Effect::Content(crc),
                    (None, _) if verb == Media::Erase => Effect::Erase,
                    (None, _) => return,
                };
                let range = units(span.start, span.count);
                let last = self.last.entry(span.medium as u8).or_default();
                let before = last.segments(range.clone());
                let era = self.era(span.medium);
                let ordinal = era.mutations.len() as u32;
                (era.by_start.entry((call.method, span.start)))
                    .or_default()
                    .push(ordinal);
                era.mutations.push(Mutation {
                    start: span.start,
                    count: span.count,
                    effect,
                    before,
                });
                self.last
                    .entry(span.medium as u8)
                    .or_default()
                    .set(range, ordinal);
            }
        }
    }

    fn read(&mut self, method: MethodId, access: Access<'_>) {
        let span = access.span;
        let unit = span.medium.unit();
        if let Content::Digest(crc) = access.content {
            self.flush();
            let version = self.version(span);
            let era = self.era(span.medium);
            let each = access.each.unwrap_or(span.count);
            for start in (span.start..span.start + span.count).step_by(each as usize) {
                let reads = era.reads.entry((method, start)).or_default();
                reads.push((each, Read::Plain));
            }
            if let Some(version) = version {
                (era.digests
                    .entry((version, (method, span.start), span.count)))
                .or_default()
                .push(crc);
            }
            return;
        }
        let mut bytes = vec![0u8; span.count as usize * unit];
        let Some(len) = access.expand(&mut bytes) else {
            return self.flush();
        };
        bytes.truncate(len);
        let continues = access.continues;
        if !continues {
            self.flush();
        } else if !self.part.as_ref().is_some_and(|part| {
            part.method == method
                && part.medium == span.medium
                && part.fault == access.fault
                && part.start + part.count == span.start
        }) {
            self.orphan.get_or_insert(span);
            return;
        }
        let class = class(&self.volumes, span.medium, span.start);
        if access.fault.is_none() && class != Class::Volatile {
            self.file(span, &bytes);
        }
        match &mut self.part {
            Some(part) if continues => {
                part.count += span.count;
                part.bytes.extend_from_slice(&bytes);
            }
            _ => {
                self.part = Some(Part {
                    method,
                    medium: span.medium,
                    start: span.start,
                    count: span.count,
                    fault: access.fault,
                    bytes,
                });
            }
        }
    }

    fn version(&self, span: Span) -> Option<Option<u32>> {
        let segments = match self.last.get(&(span.medium as u8)) {
            Some(last) => last.segments(units(span.start, span.count)),
            None => return Some(None),
        };
        let first = segments.first()?.1;
        segments
            .iter()
            .all(|(_, version)| *version == first)
            .then_some(first)
    }

    fn file(&mut self, span: Span, bytes: &[u8]) {
        let unit = span.medium.unit() as u64;
        let segments = match self.last.get(&(span.medium as u8)) {
            Some(last) => last.segments(units(span.start, span.count)),
            None => vec![(units(span.start, span.count), None)],
        };
        let mut conflict = None;
        let era_number = self
            .eras
            .get(&(span.medium as u8))
            .map_or(0, |eras| eras.len().max(1) - 1);
        let era = self.era(span.medium);
        for (range, version) in segments {
            let held = match version {
                None => &mut era.pristine,
                Some(k) => era.versions.entry(k).or_default(),
            };
            let from = (range.start - u64::from(span.start)) * unit;
            let to = ((range.end - u64::from(span.start)) * unit).min(bytes.len() as u64);
            if from >= to {
                continue;
            }
            let at = range.start * unit;
            if let Some(differs) = held.differs(at, &bytes[from as usize..to as usize])
                && conflict.is_none()
            {
                let after = match version {
                    Some(k) => format!("after its recorded mutation {k}"),
                    None => "before any recorded mutation".into(),
                };
                conflict = Some(format!(
                    "two reads of {:?} byte {differs} in era {era_number}, {after}, gave different bytes",
                    span.medium
                ));
            }
            held.install(at, &bytes[from as usize..to as usize]);
        }
        if self.conflict.is_none() {
            self.conflict = conflict;
        }
    }
}

/// The recorded media of a trace.
pub struct Index {
    eras: BTreeMap<u8, Vec<Era>>,
    volumes: Volumes,
    /// Two reads gave different bytes for one byte of one version: a
    /// writer the trace does not see, so a recording bug.
    pub conflict: Option<String>,
}

/// Why a trace's media do not index.
#[derive(Debug)]
pub enum Error {
    Trace(trace::Error),
    /// A record says it goes on a read no record before it began.
    Orphan(Span),
}

impl From<trace::Error> for Error {
    fn from(error: trace::Error) -> Self {
        Self::Trace(error)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Trace(error) => error.fmt(f),
            Self::Orphan(span) => write!(
                f,
                "a malformed trace: the read of {} goes on no read before it",
                place(span)
            ),
        }
    }
}

impl Index {
    pub fn of(trace: &[u8], volumes: Volumes) -> Result<Self, Error> {
        let (_, events) = trace::decode(trace)?;
        let mut indexer = Indexer {
            volumes,
            ..Indexer::default()
        };
        for event in events {
            match event {
                Event::Call(call) => indexer.call(&call),
                Event::Forget(medium) => indexer.forget(medium),
                _ => indexer.flush(),
            }
        }
        indexer.flush();
        if let Some(span) = indexer.orphan {
            return Err(Error::Orphan(span));
        }
        Ok(Self {
            eras: indexer.eras,
            volumes: indexer.volumes,
            conflict: indexer.conflict,
        })
    }

    fn era(&self, medium: Medium, era: usize) -> Option<&Era> {
        self.eras.get(&(medium as u8))?.get(era)
    }
}

/// Why the twin's own mutation left a unit's bytes its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Off {
    /// It matched no recorded mutation.
    Unmatched,
    /// It matched one, but not in the order the recording made them.
    OutOfStep,
    /// It failed as a recorded one did, which attempted other bytes.
    Torn,
    /// The recording mutated the unit before a host session, and the twin
    /// did not.
    Skipped,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Step {
    After(u32),
    Off(Off),
}

#[derive(Clone, Copy, Debug)]
struct Run {
    medium: Medium,
    method: MethodId,
    target: u32,
    start: u32,
    next: u32,
    crc: u32,
}

#[derive(Default)]
struct Live {
    era: usize,
    bytes: TwinMedium,
    steps: Units<Step>,
    matched: Vec<bool>,
    reads: BTreeMap<Key, u32>,
}

impl Live {
    fn matched(&mut self, k: u32) {
        if self.matched.len() <= k as usize {
            self.matched.resize(k as usize + 1, false);
        }
        self.matched[k as usize] = true;
    }
}

/// Where the twin left the recorded world, as far as media go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Leave {
    Unknown {
        span: Span,
        why: Why,
    },
    Outside {
        span: Span,
    },
    /// A read gave other bytes than the recording's digest of the same
    /// span and version.
    Digest {
        span: Span,
    },
}

/// Why bytes the twin read are unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Why {
    Unread,
    Own(Off),
    /// A STORE read the recording does not hold.
    Store,
}

fn place(span: &Span) -> String {
    format!("{:?} {}+{}", span.medium, span.start, span.count)
}

impl fmt::Display for Leave {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unknown { span, why } => {
                write!(f, "unknown media {}", place(span))?;
                match why {
                    Why::Unread => Ok(()),
                    Why::Own(Off::Unmatched) => {
                        write!(f, ": the twin's own write there matched no recorded one")
                    }
                    Why::Own(Off::OutOfStep) => write!(
                        f,
                        ": the twin's own writes there came in another order than the recording's"
                    ),
                    Why::Own(Off::Torn) => write!(
                        f,
                        ": the twin's own write there failed, and the recording's failed write there attempted other bytes"
                    ),
                    Why::Own(Off::Skipped) => write!(
                        f,
                        ": the recording wrote there before a host session, and the twin did not"
                    ),
                    Why::Store => write!(f, ": a read of STORE the recording does not hold"),
                }
            }
            Self::Outside { span } => write!(f, "outside {}", place(span)),
            Self::Digest { span } => write!(
                f,
                "{} reads other bytes than the recording's digest of it",
                place(span)
            ),
        }
    }
}

/// The twin's media: each medium as the twin's own mutations left it.
pub struct Model {
    index: Index,
    live: BTreeMap<u8, Live>,
    run: Option<Run>,
    pack: Option<Pack>,
    check_digests: bool,
    /// Reads of TERRAIN served from the pack.
    pub terrain_reads: u32,
    /// Accesses outside the recorded world: the settings region.
    pub outside: u32,
}

impl Model {
    pub fn new(index: Index, pack: Option<Pack>, check_digests: bool) -> Self {
        Self {
            index,
            live: BTreeMap::new(),
            run: None,
            pack,
            check_digests,
            terrain_reads: 0,
            outside: 0,
        }
    }

    pub fn conflict(&self) -> Option<&str> {
        self.index.conflict.as_deref()
    }

    fn live(&mut self, medium: Medium) -> &mut Live {
        let index = &self.index;
        self.live.entry(medium as u8).or_insert_with(|| Live {
            bytes: (index.era(medium, 0)).map_or_else(TwinMedium::new, |era| era.pristine.clone()),
            ..Live::default()
        })
    }

    /// A host wrote `medium`: its next era starts, from that era's pristine
    /// image, except where the twin's history parted from the recording's.
    /// Those units start the era unknown and Off.
    pub fn forget(&mut self, medium: Medium) {
        self.end_run();
        let apart = self.apart(medium);
        let era = self.live[&(medium as u8)].era + 1;
        let mut bytes =
            (self.index.era(medium, era)).map_or_else(TwinMedium::new, |era| era.pristine.clone());
        let mut steps = Units::default();
        let unit = medium.unit() as u64;
        for (range, off) in apart.iter() {
            let len = (range.end - range.start) * unit;
            bytes.mutate(range.start * unit, Change::Erase { len }, Outcome::Failed);
            steps.set(range, Step::Off(off));
        }
        self.live.insert(
            medium as u8,
            Live {
                era,
                bytes,
                steps,
                ..Live::default()
            },
        );
    }

    /// The units of `medium` where the twin's history parts from the
    /// recording's at the end of this era: its own mutation is Off there,
    /// or a recorded mutation it never matched covers them.
    fn apart(&mut self, medium: Medium) -> Units<Off> {
        self.live(medium);
        let live = &self.live[&(medium as u8)];
        let mut apart = Units::default();
        let mutations = self.index.era(medium, live.era).map(|era| &era.mutations);
        for (k, mutation) in mutations.into_iter().flatten().enumerate() {
            if !live.matched.get(k).copied().unwrap_or(false) {
                apart.set(units(mutation.start, mutation.count), Off::Skipped);
            }
        }
        for (range, step) in live.steps.iter() {
            if let Step::Off(off) = step {
                apart.set(range, off);
            }
        }
        apart
    }

    /// The pass ended, and with it any mutation the twin was making.
    pub fn end_pass(&mut self) {
        self.end_run();
    }

    fn class(&self, medium: Medium, start: u32) -> Class {
        class(&self.index.volumes, medium, start)
    }

    pub fn read(
        &mut self,
        method: MethodId,
        span: Span,
        out: &mut [u8],
    ) -> Result<Result<(), StorageError>, Leave> {
        self.end_run();
        let class = self.class(span.medium, span.start);
        if class == Class::Outside {
            self.outside += 1;
            return Err(Leave::Outside { span });
        }
        let key = (method, span.start);
        let live = self.live(span.medium);
        let occurrence = live.reads.entry(key).or_default();
        let nth = *occurrence as usize;
        *occurrence += 1;
        let era = live.era;
        let keyed = (self.index.era(span.medium, era))
            .and_then(|era| era.reads.get(&key)?.get(nth))
            .filter(|(count, _)| *count == span.count)
            .map(|(_, read)| read);
        let given = |bytes: &[u8], out: &mut [u8]| {
            let len = bytes.len().min(out.len());
            out[..len].copy_from_slice(&bytes[..len]);
        };
        match keyed {
            Some(Read::Fault(fault, bytes)) => {
                given(bytes, out);
                return Ok(Err(*fault));
            }
            Some(Read::Volatile(bytes)) if class == Class::Volatile => {
                given(bytes, out);
                return Ok(Ok(()));
            }
            _ if class == Class::Volatile => {
                return Err(Leave::Unknown {
                    span,
                    why: Why::Store,
                });
            }
            _ => {}
        }
        let unit = span.medium.unit() as u64;
        let at = u64::from(span.start) * unit;
        self.live(span.medium);
        let live = &self.live[&(span.medium as u8)];
        if let Err(first) = live.bytes.read(at, out) {
            let unknown = |at: u64| Leave::Unknown {
                span,
                why: match live.steps.get(at / unit) {
                    Some(Step::Off(off)) => Why::Own(off),
                    _ => Why::Unread,
                },
            };
            let Some(pack) = self.pack.as_ref().filter(|_| class == Class::Terrain) else {
                return Err(unknown(first.at));
            };
            for (index, sector) in out.chunks_mut(unit as usize).enumerate() {
                let sector_at = at + index as u64 * unit;
                if live.bytes.read(sector_at, sector).is_ok() {
                    continue;
                }
                let bytes = (live.steps.get(sector_at / unit).is_none())
                    .then(|| pack.sector(sector_at / unit))
                    .flatten()
                    .ok_or_else(|| unknown(sector_at))?;
                sector.copy_from_slice(&bytes[..sector.len()]);
            }
            self.terrain_reads += 1;
        }
        if self.check_digests && !self.digest_holds(method, span, out) {
            return Err(Leave::Digest { span });
        }
        Ok(Ok(()))
    }

    /// The twin's own bytes of `medium` from its start, erased where the
    /// model knows none.
    pub fn contents(&mut self, medium: Medium, out: &mut [u8]) {
        let live = self.live(medium);
        for (address, slot) in out.iter_mut().enumerate() {
            let mut byte = [0xFF];
            let _ = live.bytes.read(address as u64, &mut byte);
            *slot = byte[0];
        }
    }

    /// `--model`: the bytes of `span` the model does not know, from the
    /// nearest recorded version of each byte, and erased where no version
    /// holds it. They are the model's from here on. A STORE read takes the
    /// latest the era recorded of its span.
    pub fn assume(&mut self, method: MethodId, span: Span, out: &mut [u8]) {
        let unit = span.medium.unit() as u64;
        let at = u64::from(span.start) * unit;
        self.live(span.medium);
        let live = self
            .live
            .get_mut(&(span.medium as u8))
            .expect("a live medium");
        let era = self.index.era(span.medium, live.era);
        if class(&self.index.volumes, span.medium, span.start) == Class::Volatile {
            let latest =
                (era.and_then(|era| era.reads.get(&(method, span.start)))).and_then(|reads| {
                    reads.iter().rev().find_map(|read| match read {
                        (count, Read::Volatile(bytes)) if *count == span.count => Some(bytes),
                        _ => None,
                    })
                });
            out.fill(0xFF);
            if let Some(bytes) = latest {
                let len = bytes.len().min(out.len());
                out[..len].copy_from_slice(&bytes[..len]);
            }
            return;
        }
        let frontier = live.matched.iter().rposition(|matched| *matched);
        let one = |address: u64| -> Option<u8> {
            let era = era?;
            let byte = |held: &TwinMedium| {
                let mut byte = [0];
                held.read(address, &mut byte).ok().map(|()| byte[0])
            };
            let k = frontier.map_or(0, |k| k as u32 + 1);
            (era.versions.range(..k).rev())
                .find_map(|(_, held)| byte(held))
                .or_else(|| byte(&era.pristine))
                .or_else(|| era.versions.range(k..).find_map(|(_, held)| byte(held)))
        };
        for (offset, slot) in out.iter_mut().enumerate() {
            let address = at + offset as u64;
            let mut known = [0];
            *slot = match live.bytes.read(address, &mut known) {
                Ok(()) => known[0],
                Err(_) => one(address).unwrap_or(0xFF),
            };
        }
        live.bytes.install(at, out);
    }

    fn digest_holds(&mut self, method: MethodId, span: Span, bytes: &[u8]) -> bool {
        let live = self.live(span.medium);
        let segments = live.steps.segments(units(span.start, span.count));
        let version = match segments.as_slice() {
            [(_, None)] => None,
            [(_, Some(Step::After(k)))] => Some(*k),
            _ => return true,
        };
        let era = live.era;
        let Some(crcs) = self.index.era(span.medium, era).and_then(|era| {
            era.digests
                .get(&(version, (method, span.start), span.count))
        }) else {
            return true;
        };
        crcs.contains(&crc32_more(0, bytes))
    }

    pub fn mutate(
        &mut self,
        method: MethodId,
        span: Span,
        change: Change<'_>,
    ) -> Result<(), StorageError> {
        if self.class(span.medium, span.start) == Class::Outside {
            self.outside += 1;
            return Ok(());
        }
        match change {
            Change::Erase { .. } => self.erase(method, span, change),
            Change::Write(bytes) | Change::Program(bytes) => {
                self.content(method, span, change, bytes)
            }
        }
    }

    fn unmatched_at(
        &mut self,
        method: MethodId,
        medium: Medium,
        start: u32,
    ) -> impl Iterator<Item = (u32, &Mutation)> {
        self.live(medium);
        let live = &self.live[&(medium as u8)];
        let era = self.index.era(medium, live.era);
        let at = era.and_then(|era| era.by_start.get(&(method, start)));
        at.into_iter().flatten().filter_map(move |&k| {
            let matched = live.matched.get(k as usize).copied().unwrap_or(false);
            (!matched).then(|| (k, &era.expect("an era").mutations[k as usize]))
        })
    }

    fn target(&mut self, method: MethodId, medium: Medium, start: u32) -> Option<(u32, Mutation)> {
        self.unmatched_at(method, medium, start)
            .next()
            .map(|(k, mutation)| (k, mutation.clone()))
    }

    fn identity(
        &mut self,
        method: MethodId,
        medium: Medium,
        start: u32,
        count: u32,
        effect: Effect,
    ) -> Option<u32> {
        self.unmatched_at(method, medium, start)
            .find(|(_, mutation)| mutation.count == count && mutation.effect == effect)
            .map(|(k, _)| k)
    }

    fn erase(
        &mut self,
        method: MethodId,
        span: Span,
        change: Change<'_>,
    ) -> Result<(), StorageError> {
        self.end_run();
        if let Some((k, mutation)) = self.target(method, span.medium, span.start)
            && let Effect::Fault { fault, .. } = mutation.effect
            && mutation.count == span.count
        {
            self.failed(span.medium, k, &mutation, true);
            return Err(fault);
        }
        let unit = span.medium.unit() as u64;
        let live = self.live(span.medium);
        live.bytes
            .mutate(u64::from(span.start) * unit, change, Outcome::Done);
        match self.identity(method, span.medium, span.start, span.count, Effect::Erase) {
            Some(k) => self.matched(span.medium, k),
            None => self.unmatched(span.medium, units(span.start, span.count)),
        }
        Ok(())
    }

    fn content(
        &mut self,
        method: MethodId,
        span: Span,
        change: Change<'_>,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        let medium = span.medium;
        let unit = medium.unit();
        let (mut start, mut bytes) = (span.start, bytes);
        while !bytes.is_empty() {
            let continues = self.run.is_some_and(|run| {
                run.medium == medium && run.method == method && run.next == start
            });
            if !continues {
                self.end_run();
                if let Some((k, mutation)) = self.target(method, medium, start) {
                    match mutation.effect {
                        Effect::Fault { fault, attempted }
                            if start == span.start && mutation.count == span.count =>
                        {
                            let same = attempted == Some(crc32_more(0, bytes));
                            self.failed(medium, k, &mutation, same);
                            return Err(fault);
                        }
                        Effect::Content(_) => {
                            self.run = Some(Run {
                                medium,
                                method,
                                target: k,
                                start,
                                next: start,
                                crc: 0,
                            })
                        }
                        _ => {}
                    }
                }
            }
            let Some(run) = self.run else {
                let count = (bytes.len() / unit) as u32;
                self.apply(medium, start, change, bytes);
                self.unmatched(medium, units(start, count));
                return Ok(());
            };
            let live = self.live(medium);
            let era = live.era;
            let end = self
                .index
                .era(medium, era)
                .map_or(run.next, |era| era.mutations[run.target as usize].end());
            let count = ((bytes.len() / unit) as u32).min(end - run.next);
            let (piece, rest) = bytes.split_at(count as usize * unit);
            self.apply(medium, start, change, piece);
            let run = self.run.as_mut().expect("a run");
            run.crc = crc32_more(run.crc, piece);
            run.next += count;
            if run.next == end {
                self.finish_run();
            }
            start += count;
            bytes = rest;
        }
        Ok(())
    }

    fn apply(&mut self, medium: Medium, start: u32, change: Change<'_>, bytes: &[u8]) {
        let at = u64::from(start) * medium.unit() as u64;
        let piece = match change {
            Change::Write(_) => Change::Write(bytes),
            _ => Change::Program(bytes),
        };
        self.live(medium).bytes.mutate(at, piece, Outcome::Done);
    }

    fn finish_run(&mut self) {
        let Some(run) = self.run.take() else {
            return;
        };
        let count = run.next - run.start;
        let effect = Effect::Content(run.crc);
        match self.identity(run.method, run.medium, run.start, count, effect) {
            Some(k) => self.matched(run.medium, k),
            None => self.unmatched(run.medium, units(run.start, count)),
        }
    }

    fn end_run(&mut self) {
        if let Some(run) = self.run.take() {
            self.unmatched(run.medium, units(run.start, run.next - run.start));
        }
    }

    fn unmatched(&mut self, medium: Medium, range: Range<u64>) {
        if range.is_empty() {
            return;
        }
        self.live(medium)
            .steps
            .set(range, Step::Off(Off::Unmatched));
    }

    fn step(&mut self, medium: Medium, k: u32) -> Vec<Range<u64>> {
        let live = self.live(medium);
        let era = live.era;
        let Some(mutation) = self
            .index
            .era(medium, era)
            .map(|era| &era.mutations[k as usize])
        else {
            return Vec::new();
        };
        let live = self.live.get_mut(&(medium as u8)).expect("a live medium");
        live.matched(k);
        let mut in_step = Vec::new();
        for (range, before) in &mutation.before {
            for (segment, twin) in live.steps.segments(range.clone()) {
                let after = match twin {
                    None => None,
                    Some(Step::After(j)) => Some(j),
                    Some(Step::Off(_)) => continue,
                };
                if after == *before {
                    in_step.push(segment);
                } else {
                    live.steps.set(segment, Step::Off(Off::OutOfStep));
                }
            }
        }
        for segment in &in_step {
            live.steps.set(segment.clone(), Step::After(k));
        }
        in_step
    }

    fn matched(&mut self, medium: Medium, k: u32) {
        let in_step = self.step(medium, k);
        if medium != Medium::Bulk {
            self.install(medium, k, &in_step, Fill::OnlyUnknown);
        }
    }

    /// The twin's mutation failed as the recorded mutation `k` did. The
    /// bytes the device read after it are the twin's only when the twin
    /// attempted the `same` content, in step.
    fn failed(&mut self, medium: Medium, k: u32, mutation: &Mutation, same: bool) {
        let unit = medium.unit() as u64;
        let at = u64::from(mutation.start) * unit;
        let len = u64::from(mutation.count) * unit;
        let live = self.live(medium);
        live.bytes
            .mutate(at, Change::Erase { len }, Outcome::Failed);
        if same {
            let in_step = self.step(medium, k);
            self.install(medium, k, &in_step, Fill::Overwrite);
        } else {
            live.matched(k);
            let range = units(mutation.start, mutation.count);
            live.steps.set(range, Step::Off(Off::Torn));
        }
    }

    fn install(&mut self, medium: Medium, k: u32, ranges: &[Range<u64>], fill: Fill) {
        let unit = medium.unit() as u64;
        let live = self.live.get_mut(&(medium as u8)).expect("a live medium");
        let Some(version) = self
            .index
            .eras
            .get(&(medium as u8))
            .and_then(|eras| eras.get(live.era))
            .and_then(|era| era.versions.get(&k))
        else {
            return;
        };
        for range in ranges {
            live.bytes
                .copy_from(version, range.start * unit..range.end * unit, fill);
        }
    }
}

#[cfg(test)]
mod tests;
