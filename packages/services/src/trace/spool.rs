//! Staging between a trace's recorder and its file in the store.
//!
//! The recorder seals event blocks into a [`Spool`], a ring in memory the
//! caller provides (PSRAM on the C606), and never waits for the medium. Once
//! a pass, [`Drain::pass`] appends at most one record payload
//! ([`store::PAYLOAD`] bytes) of the header and the spooled blocks to the
//! trace file, so a pass writes at most one record sector, plus the
//! metadata that links the next chunk once every [`store::CHUNK`] records.
//! Every [`SYNC_MS`] it hands the recorder's open block to the spool and,
//! once the spool holds less than a record, syncs the file: a power cut
//! loses at most that long of the trace, plus what the spool still holds.
//!
//! A block the spool has no room for is lost; the recorder names it in the
//! next block the spool takes, as [`super::Event::Lost`], so a full ring
//! leaves a gap a reader sees.
use super::frozen::{Recorder, Sink};
use crate::store::{self, Appender, Dir, Error, Name, Sector, Store};
use device_api::bulk::ReadWrite;

/// How often the drain makes the trace durable.
pub const SYNC_MS: u64 = 5_000;

/// The spool the C606 keeps in PSRAM. Riding, at the 2.9 MB an hour the
/// simulator measures, a pass adds about 8 bytes and the spool holds little
/// more than the block a flush hands it, even across a 150 ms store stall.
/// What fills it is a phone upload: VANA reads up to 4 KiB a pass while the
/// drain writes one record, so the spool fills by about 88% of an upload,
/// whatever the pass rate, and this one takes uploads up to about 280 KiB
/// whole (`tests::UPLOAD_BYTES`).
pub const RING_BYTES: usize = 256 * 1024;

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

    /// This pass's share: at most one record of the header and the spool,
    /// and every [`SYNC_MS`] the recorder's open block and a sync.
    pub fn pass<B: AsRef<[u8]> + AsMut<[u8]>, R: AsRef<[u8]> + AsMut<[u8]>>(
        &mut self,
        recorder: &mut Spooled<B, R>,
        media: &mut impl ReadWrite,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        if now.saturating_sub(self.flushed_ms) >= SYNC_MS {
            recorder.flush();
            self.flushed_ms = now;
        }
        let spool = recorder.sink_mut();
        let room = self.appender.room();
        let pending = self.header_left() + spool.len();
        if pending >= room {
            return self.append(spool, room, media, sector);
        }
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
    pub fn sync<B: AsRef<[u8]> + AsMut<[u8]>, R: AsRef<[u8]> + AsMut<[u8]>>(
        &mut self,
        recorder: &mut Spooled<B, R>,
        media: &mut impl ReadWrite,
        now: u64,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        recorder.flush();
        let spool = recorder.sink_mut();
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

#[cfg(test)]
mod tests;
