//! Staging between a trace's recorder and its file in the store.
//!
//! The recorder seals event blocks into a [`Spool`], a ring in memory the
//! caller provides (PSRAM on the C606), and never waits for the medium. Once
//! a pass, [`Drain::pass`] appends whole record payloads ([`store::PAYLOAD`]
//! bytes) of the header and the spooled blocks to the trace file: one while
//! a ride records, so such a pass writes at most one record sector, plus
//! the metadata that links the next chunk once every [`store::CHUNK`]
//! records, and up to [`IDLE_RECORDS`] otherwise.
//! Every [`SYNC_MS`] it hands the recorder's open block to the spool and,
//! once the spool holds less than a record, syncs the file: a power cut
//! loses at most that long of the trace, plus what the spool still holds.
//!
//! A block the spool has no room for is lost; the recorder names it in the
//! next block the spool takes, as [`super::Event::Lost`], so a full ring
//! leaves a gap a reader sees.
use super::frozen::{Record, Recorder, Sink};
use super::origin::{self, Generations};
use crate::store::{self, Appender, Dir, Error, Name, Sector, Store};
use crate::volume::{self, Extent, Layout, Volume};
use device_api::bulk::ReadWrite;

/// The directory traces live in; the oldest rolls over when the store is
/// full.
pub const TRACES: &str = "traces/";

/// The name of the next power-on session's trace: one past the highest
/// sequence number in [`TRACES`], then the boot id, as in
/// `traces/000042-1a2b3c4d.trace`.
pub fn next_name(
    store: &Store,
    media: &mut impl ReadWrite,
    boot: u32,
    sector: &mut Sector,
) -> Result<Name, Error> {
    use core::fmt::Write;
    let mut highest = 0u32;
    store.list(media, &Dir::new(TRACES)?, sector, |entry| {
        let sequence = entry.name.as_str().strip_prefix(TRACES);
        if let Some(sequence) = sequence.and_then(|rest| rest.get(..6)?.parse::<u32>().ok()) {
            highest = highest.max(sequence);
        }
    })?;
    let mut text = heapless::String::<{ store::NAME }>::new();
    write!(text, "{TRACES}{:06}-{boot:08x}.trace", highest + 1).map_err(|_| Error::Name)?;
    Name::new(&text)
}

/// How often the drain makes the trace durable.
pub const SYNC_MS: u64 = 5_000;

/// The spool the C606 keeps in PSRAM. Riding, at the 2.9 MB an hour the
/// simulator measures, a pass adds about 8 bytes and the spool holds little
/// more than the block a flush hands it, even across a 150 ms store stall.
/// What fills it is a phone upload, which never overlaps a ride: VANA reads
/// up to 4 KiB a pass, about nine records, and the drain writes up to
/// [`IDLE_RECORDS`], so the spool holds about one block whatever the
/// upload's size (`tests::UPLOAD_BYTES`, the phone server's largest body).
pub const RING_BYTES: usize = 256 * 1024;

/// The records a pass writes at most while no ride records: more than the
/// nine a phone upload adds, so the drain catches up after a store stall.
pub const IDLE_RECORDS: u32 = 16;

/// Sealed blocks waiting for the store, oldest first, in a ring of bytes.
pub struct Spool<R> {
    ring: R,
    start: usize,
    len: usize,
    peak: usize,
    refused: u32,
}

impl<R: AsRef<[u8]> + AsMut<[u8]>> Spool<R> {
    pub fn new(ring: R) -> Self {
        Self {
            ring,
            start: 0,
            len: 0,
            peak: 0,
            refused: 0,
        }
    }

    /// Bytes waiting for the store.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn capacity(&self) -> usize {
        self.ring.as_ref().len()
    }

    /// The most bytes it has held at once.
    pub fn peak(&self) -> usize {
        self.peak
    }

    /// Blocks it had no room for.
    pub fn refused(&self) -> u32 {
        self.refused
    }

    /// The oldest bytes, at most `max`, that lie together in the ring.
    fn front(&self, max: usize) -> &[u8] {
        let together = self.len.min(self.capacity() - self.start).min(max);
        &self.ring.as_ref()[self.start..self.start + together]
    }

    fn consume(&mut self, n: usize) {
        self.len -= n;
        self.start = if self.len == 0 {
            0
        } else {
            (self.start + n) % self.capacity()
        };
    }
}

impl<R: AsRef<[u8]> + AsMut<[u8]>> Sink for Spool<R> {
    fn block(&mut self, bytes: &[u8]) -> bool {
        let capacity = self.capacity();
        if capacity - self.len < bytes.len() {
            self.refused = self.refused.saturating_add(1);
            return false;
        }
        let at = (self.start + self.len) % capacity.max(1);
        let first = (capacity - at).min(bytes.len());
        let ring = self.ring.as_mut();
        ring[at..at + first].copy_from_slice(&bytes[..first]);
        ring[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        self.len += bytes.len();
        self.peak = self.peak.max(self.len);
        true
    }
}

/// A recorder whose sealed blocks wait in a spool.
pub type Spooled<B, R> = Recorder<B, Spool<R>>;

/// A sink whose blocks wait in a spool for a [`Drain`], such as the spool
/// itself or one that also keeps a copy.
pub trait Staging: Sink {
    type Ring: AsRef<[u8]> + AsMut<[u8]>;
    fn spool(&mut self) -> &mut Spool<Self::Ring>;
}

impl<R: AsRef<[u8]> + AsMut<[u8]>> Staging for Spool<R> {
    type Ring = R;
    fn spool(&mut self) -> &mut Self {
        self
    }
}

/// The trace file a spool drains into: its header, then the blocks.
pub struct Drain<H> {
    store: Store,
    appender: Appender,
    name: Name,
    rollover: Dir,
    header: H,
    header_written: usize,
    flushed_ms: u64,
    synced_ms: u64,
    written: u64,
}

impl<H: AsRef<[u8]>> Drain<H> {
    /// Create `name` empty, deleting the oldest file in `rollover` when the
    /// store is full. `header` goes ahead of the blocks.
    pub fn create(
        media: &mut impl ReadWrite,
        mut store: Store,
        name: Name,
        rollover: Dir,
        header: H,
        now: u64,
        sector: &mut Sector,
    ) -> Result<Self, Error> {
        let appender = store.create(media, &name, Some(rollover), sector)?;
        Ok(Self {
            store,
            appender,
            name,
            rollover,
            header,
            header_written: 0,
            flushed_ms: now,
            synced_ms: now,
            written: 0,
        })
    }

    pub fn name(&self) -> &Name {
        &self.name
    }

    /// The generation of the store as this drain last committed or mounted
    /// it.
    pub fn generation(&self) -> u64 {
        self.store.generation()
    }

    /// Bytes appended to the file, header included.
    pub fn written(&self) -> u64 {
        self.written
    }

    /// This pass's share: at most `records` whole records of the header and
    /// the spool, and, in a pass that wrote none, every [`SYNC_MS`] the
    /// recorder's open block and a sync.
    pub fn pass<B: AsRef<[u8]> + AsMut<[u8]>, S: Staging>(
        &mut self,
        recorder: &mut Recorder<B, S>,
        media: &mut impl ReadWrite,
        records: u32,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        if now.saturating_sub(self.flushed_ms) >= SYNC_MS {
            recorder.flush();
            self.flushed_ms = now;
        }
        let spool = recorder.sink_mut().spool();
        let mut written = 0;
        while written < records {
            let room = self.appender.room();
            if self.header_left() + spool.len() < room {
                break;
            }
            self.append(spool, room, media, sector)?;
            written += 1;
        }
        if written > 0 {
            return Ok(());
        }
        let pending = self.header_left() + spool.len();
        if now.saturating_sub(self.synced_ms) >= SYNC_MS {
            self.append(spool, pending, media, sector)?;
            self.appender.sync(&mut self.store, media, sector)?;
            self.synced_ms = now;
        }
        Ok(())
    }

    /// Append everything the recorder holds and sync: before another writer
    /// takes the medium, and before power goes. Writes as many records as
    /// that takes.
    pub fn sync<B: AsRef<[u8]> + AsMut<[u8]>, S: Staging>(
        &mut self,
        recorder: &mut Recorder<B, S>,
        media: &mut impl ReadWrite,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        recorder.flush();
        let spool = recorder.sink_mut().spool();
        let pending = self.header_left() + spool.len();
        self.append(spool, pending, media, sector)?;
        self.appender.sync(&mut self.store, media, sector)?;
        (self.flushed_ms, self.synced_ms) = (now, now);
        Ok(())
    }

    /// Go on after another writer, such as the host's transfer session, may
    /// have changed the store. When its generation changed, open the file
    /// again, which continues in a new chunk; `NotFound` when the file is
    /// gone. An error leaves the drain closed.
    pub fn resume(&mut self, media: &mut impl ReadWrite, sector: &mut Sector) -> Result<(), Error> {
        let mut store = store::mount(media, sector)?;
        if store.generation() == self.store.generation() {
            return Ok(());
        }
        let appender = store.open_append(media, &self.name, Some(self.rollover), sector)?;
        (self.store, self.appender) = (store, appender);
        Ok(())
    }

    fn header_left(&self) -> usize {
        self.header.as_ref().len() - self.header_written
    }

    fn append<R: AsRef<[u8]> + AsMut<[u8]>>(
        &mut self,
        spool: &mut Spool<R>,
        mut n: usize,
        media: &mut impl ReadWrite,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        while n > 0 {
            let left = self.header_left();
            let taken = if left > 0 {
                let at = self.header_written;
                let part = &self.header.as_ref()[at..at + left.min(n)];
                self.appender.append(&mut self.store, media, part, sector)?;
                self.header_written += part.len();
                part.len()
            } else {
                let part = spool.front(n);
                self.appender.append(&mut self.store, media, part, sector)?;
                let taken = part.len();
                spool.consume(taken);
                taken
            };
            n -= taken;
            self.written += taken as u64;
        }
        Ok(())
    }
}

/// A session's trace file in the card's STORE volume, from power-on: what
/// both the C606 and the simulator do with it. One per session, held where
/// composition keeps its state (PSRAM on the C606), so the open file's
/// record buffer stays inline: there is no allocator to box it.
#[allow(clippy::large_enum_variant)]
pub enum File<H> {
    /// From power-on until the card admits work: nothing read from it yet.
    Waiting,
    /// No file this session, and why.
    Off(&'static str),
    /// Blocks wait in the spool; the file opens once its header is known.
    Staging(Extent, Generations),
    Open(Extent, Generations, Drain<H>),
    /// The file ended mid-session, and why.
    Ended(&'static str),
}

impl<H: AsRef<[u8]>> File<H> {
    /// At the first pass the card admits work: staging when the owned
    /// region `bulk` has a STORE volume that mounts.
    pub fn stage(bulk: &mut impl ReadWrite, sector: &mut Sector) -> Self {
        let Ok(Layout::Table(table)) = volume::read(bulk, sector) else {
            return Self::Off("no volume table");
        };
        let Some(extent) = table.find(&volume::STORE) else {
            return Self::Off("no STORE volume");
        };
        if store::mount(&mut Volume::new(&mut *bulk, extent), sector).is_err() {
            return Self::Off("STORE does not mount");
        }
        Self::Staging(extent, origin::generations(bulk, sector))
    }

    /// The file is open: a STORE format would erase it.
    pub fn is_open(&self) -> bool {
        matches!(self, Self::Open(..))
    }

    /// This pass's share of the spool, at most `records` whole records,
    /// while the card admits work (`admitted`). The file opens at the first
    /// such pass `header` gives the header and the boot id, named by
    /// [`next_name`]. An error ends the file and says why.
    #[allow(clippy::too_many_arguments)]
    pub fn pass<B: AsRef<[u8]> + AsMut<[u8]>, S: Staging>(
        &mut self,
        recorder: &mut Recorder<B, S>,
        bulk: &mut impl ReadWrite,
        admitted: bool,
        records: u32,
        header: impl FnOnce() -> Option<(H, u32)>,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), &'static str> {
        if !admitted {
            return Ok(());
        }
        if let Self::Staging(extent, generations) = *self {
            let Some((header, boot)) = header() else {
                return Ok(());
            };
            let mut volume = Volume::new(&mut *bulk, extent);
            let opened = store::mount(&mut volume, sector).and_then(|mounted| {
                let name = next_name(&mounted, &mut volume, boot, sector)?;
                let traces = Dir::new(TRACES)?;
                Drain::create(&mut volume, mounted, name, traces, header, now, sector)
            });
            match opened {
                Ok(drain) => *self = Self::Open(extent, generations, drain),
                Err(_) => return self.end("the trace file does not open"),
            }
        }
        let Self::Open(extent, _, drain) = self else {
            return Ok(());
        };
        let mut volume = Volume::new(&mut *bulk, *extent);
        if drain
            .pass(recorder, &mut volume, records, now, sector)
            .is_err()
        {
            return self.end("the store failed");
        }
        Ok(())
    }

    /// Append everything the spool holds and sync.
    pub fn sync<B: AsRef<[u8]> + AsMut<[u8]>, S: Staging>(
        &mut self,
        recorder: &mut Recorder<B, S>,
        bulk: &mut impl ReadWrite,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), &'static str> {
        let Self::Open(extent, _, drain) = self else {
            return Ok(());
        };
        let mut volume = Volume::new(&mut *bulk, *extent);
        if drain.sync(recorder, &mut volume, now, sector).is_err() {
            return self.end("the store failed");
        }
        Ok(())
    }

    /// Before a host takes the card in a transfer session, which the trace
    /// does not see: sync, and note FILES and TERRAIN as they stand, VANA's
    /// own writes and recorded commands included.
    pub fn before_host<B: AsRef<[u8]> + AsMut<[u8]>, S: Staging>(
        &mut self,
        recorder: &mut Recorder<B, S>,
        bulk: &mut impl ReadWrite,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), &'static str> {
        self.sync(recorder, bulk, now, sector)?;
        if let Self::Staging(_, generations) | Self::Open(_, generations, _) = self {
            *generations = origin::generations(bulk, sector);
        }
        Ok(())
    }

    /// After a host had the card: in a transfer session (`unseen`), a write
    /// to what VANA reads, FILES or TERRAIN, records
    /// [`super::Event::HostWrote`] and ends the file; so does a moved STORE
    /// after a recorded volume command too. A store that changed otherwise
    /// opens the file again.
    pub fn after_host<B: AsRef<[u8]> + AsMut<[u8]>, S: Staging>(
        &mut self,
        recorder: &mut Recorder<B, S>,
        bulk: &mut impl ReadWrite,
        unseen: bool,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), &'static str> {
        let (Self::Staging(extent, generations) | Self::Open(extent, generations, _)) = *self
        else {
            return Ok(());
        };
        let wrote = unseen && origin::generations(bulk, sector) != generations;
        let table = volume::read(bulk, sector);
        let moved = !matches!(table, Ok(Layout::Table(table)) if table.find(&volume::STORE) == Some(extent));
        if wrote || moved {
            recorder.record(&super::Event::HostWrote);
            self.sync(recorder, bulk, now, sector)?;
            return self.end("the host wrote the card");
        }
        if let Self::Open(_, _, drain) = self
            && drain
                .resume(&mut Volume::new(&mut *bulk, extent), sector)
                .is_err()
        {
            return self.end("the host deleted the trace");
        }
        Ok(())
    }

    fn end(&mut self, why: &'static str) -> Result<(), &'static str> {
        *self = Self::Ended(why);
        Err(why)
    }
}

#[cfg(test)]
mod tests;
