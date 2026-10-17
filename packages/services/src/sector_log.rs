//! Epoch-bound, append-only log of sector records on owned block storage.
//!
//! Relative owned sectors 0 and 1 hold two superblock copies, `RING_START..`
//! holds a ring of checkpoints, and records start at `DATA_START`. Every
//! sector carries magic, kind, the store id of the epoch that wrote it, its
//! index and a CRC32 over the rest of the sector. One sector is one verified
//! write, so the trailing CRC is its commit: a torn write never validates.
//! Bytes left by earlier epochs or other software never validate either (store
//! id, index and CRC), so formatting writes `RING + 2` sectors and erases
//! nothing.
//! That relies on store ids never repeating: callers seed `format` with
//! hardware entropy, and it also avoids every id visible in the metadata.
//!
//! An epoch keeps the capacity it was formatted with. If the owned region
//! later grows at its end (an explicit repartition), records keep their
//! addresses and the epoch stays valid; the next format uses the larger
//! region. A region that shrank is foreign: it may have lost records.
//!
//! Mechanism only: payload schemas and checkpoint cadence belong to the
//! caller, who promises to append at most `replay` records after any
//! checkpoint before writing the next. Every operation is bounded. Recovery
//! takes one sector read per step: both superblocks, the ring, the newest
//! checkpoint, then the records after it up to its reach (below).
//!
//! Recovery reports a head only when it can prove where the log ends:
//! - every checkpoint the ring should still hold, the newest valid one and the
//!   `RING - 2` before it, is valid. The slot after the newest may hold a
//!   checkpoint torn by power loss; any later checkpoint overwrites a slot that
//!   is checked, so damage to it is caught. Format fills the ring, so this
//!   holds from an epoch's first checkpoint on;
//! - every record from the newest checkpoint to the head is valid and was
//!   written after that checkpoint (records carry the checkpoint sequence they
//!   follow, so one written after a newer, now damaged, checkpoint is caught);
//! - no valid record of the epoch lies within `2 * replay` sectors of the
//!   newest checkpoint past the head: the furthest a writer keeping its
//!   promise can have appended after the one damaged checkpoint the ring
//!   cannot check.
//!
//! Otherwise it reports `Step::Uncertain`: the extent it saw, for export and
//! diagnosis, never a shorter healthy log to append to. What no rule can
//! tell apart is a damaged tail of writes from a power cut before them: then
//! the head is where that cut would have left it.
//! Without a superblock, recovery also reads the ring and the first record:
//! records there mean lost metadata (corrupt), not an unformatted region.
//! Recovery never writes. Appends never wrap and never overwrite a valid record
//! of the current epoch. A failed write leaves the head unchanged and its
//! outcome uncertain: stop and recover before writing again.

use crate::checksum::crc32;
use crate::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use device_api::bulk::{ReadWrite, SECTOR_SIZE};

pub const HEADER: usize = 32;
/// Payload bytes available in every record and checkpoint sector.
pub const PAYLOAD: usize = SECTOR_SIZE - HEADER - 4;
pub const RING_START: u64 = 2;
pub const RING: u64 = 8;
pub const DATA_START: u64 = 16;

const MAGIC: [u8; 4] = *b"SLOG";
const VERSION: u8 = 1;
const SUPER: u8 = 1;
const CHECKPOINT: u8 = 2;
const RECORD: u8 = 3;
const CRC: usize = SECTOR_SIZE - 4;
/// Tag, data start and ring size.
const SUPER_BODY: usize = 28;

pub type Sector = [u8; SECTOR_SIZE];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Media(device_api::bulk::Error),
    /// No valid superblock: this region was never formatted as a log.
    Uninitialized,
    /// A valid superblock of another owner, version or region geometry.
    Foreign,
    /// The epoch exists but has no usable checkpoint, or its superblocks disagree.
    Corrupt,
    Full,
    /// The append target already holds a valid record of this epoch.
    Occupied,
    Payload,
    /// The last checkpoint write failed; write it again before appending.
    Unsettled,
}

impl From<device_api::bulk::Error> for Error {
    fn from(error: device_api::bulk::Error) -> Self {
        Self::Media(error)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Header {
    store: u64,
    index: u64,
    aux: u64,
    length: usize,
}

fn seal(
    sector: &mut Sector,
    kind: u8,
    store: u64,
    index: u64,
    aux: u64,
    payload: &[u8],
) -> Result<(), Error> {
    if payload.len() > PAYLOAD {
        return Err(Error::Payload);
    }
    sector.fill(0);
    sector[..4].copy_from_slice(&MAGIC);
    sector[4] = kind;
    sector[5] = VERSION;
    put_u16(sector, 6, payload.len() as u16);
    put_u64(sector, 8, store);
    put_u64(sector, 16, index);
    put_u64(sector, 24, aux);
    sector[HEADER..HEADER + payload.len()].copy_from_slice(payload);
    let checksum = crc32(&sector[..CRC]);
    put_u32(sector, CRC, checksum);
    Ok(())
}

fn open(sector: &Sector, kind: u8) -> Option<Header> {
    let length = usize::from(get_u16(sector, 6));
    (sector[..4] == MAGIC
        && sector[4] == kind
        && sector[5] == VERSION
        && length <= PAYLOAD
        && get_u32(sector, CRC) == crc32(&sector[..CRC]))
    .then(|| Header {
        store: get_u64(sector, 8),
        index: get_u64(sector, 16),
        aux: get_u64(sector, 24),
        length,
    })
}

fn payload(sector: &Sector, header: Header) -> &[u8] {
    &sector[HEADER..HEADER + header.length]
}

/// A valid record of `store` at `index`, if the sector holds one.
fn record(sector: &Sector, store: u64, index: u64) -> Option<Header> {
    open(sector, RECORD).filter(|h| h.store == store && h.index == index)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Super {
    store: u64,
    generation: u64,
    capacity: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Replica {
    Ours(Super),
    Foreign,
    Invalid,
}

fn capacity(sectors: u64) -> Result<u64, Error> {
    sectors
        .checked_sub(DATA_START)
        .filter(|c| *c > 0)
        .ok_or(Error::Foreign)
}

fn super_copy(sector: &Sector, tag: &[u8; 16], capacity: u64) -> Replica {
    // Magic and kind identify a superblock even when its version is unknown.
    if sector[..4] != MAGIC || sector[4] != SUPER {
        return Replica::Invalid;
    }
    if get_u32(sector, CRC) != crc32(&sector[..CRC]) {
        return Replica::Invalid;
    }
    let Some(header) = open(sector, SUPER) else {
        return Replica::Foreign;
    };
    let body = payload(sector, header);
    if body.len() != SUPER_BODY
        || body[..16] != tag[..]
        || get_u64(body, 16) != DATA_START
        || u64::from(get_u32(body, 24)) != RING
        // A region that grew at its end keeps every record address; a region
        // that shrank or moved would lose records.
        || header.aux == 0
        || header.aux > capacity
        || header.store == 0
    {
        return Replica::Foreign;
    }
    Replica::Ours(Super {
        store: header.store,
        generation: header.index,
        capacity: header.aux,
    })
}

/// The current epoch from both copies: an interrupted format leaves the newer
/// generation in one copy; two different stores of one generation are corrupt.
fn choose(copies: [Replica; 2]) -> Result<Option<Super>, Error> {
    match copies {
        [Replica::Foreign, _] | [_, Replica::Foreign] => Err(Error::Foreign),
        [Replica::Invalid, Replica::Invalid] => Ok(None),
        [Replica::Ours(s), Replica::Invalid] | [Replica::Invalid, Replica::Ours(s)] => Ok(Some(s)),
        [Replica::Ours(a), Replica::Ours(b)] if a == b => Ok(Some(a)),
        [Replica::Ours(a), Replica::Ours(b)] if a.generation != b.generation => {
            Ok(Some(if a.generation > b.generation { a } else { b }))
        }
        [Replica::Ours(_), Replica::Ours(_)] => Err(Error::Corrupt),
    }
}

fn mix(mut value: u64) -> u64 {
    // splitmix64 finalizer: spreads a weak seed over all store-id bits.
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

/// An open log epoch: where the next record goes and the newest checkpoint.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Log {
    store: u64,
    generation: u64,
    capacity: u64,
    head: u64,
    sequence: u64,
    /// Read only: recovery could not prove where this log ends.
    uncertain: bool,
    /// A checkpoint write failed and may or may not have landed; appends wait
    /// for one that succeeds, so no record follows an unknown checkpoint.
    unsettled: bool,
}

impl Log {
    pub fn store_id(&self) -> u64 {
        self.store
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Record sectors in this epoch.
    pub fn capacity(&self) -> u64 {
        self.capacity
    }
    /// Index of the next record; every lower index was written and verified.
    pub fn head(&self) -> u64 {
        self.head
    }
    pub fn checkpoint_sequence(&self) -> u64 {
        self.sequence
    }
    /// Whether recovery proved where this log ends, so it may be appended to.
    pub fn writable(&self) -> bool {
        !self.uncertain
    }

    /// Write `payload` as record `head`: one read of the target, refusing a
    /// valid record of this epoch, then one write the medium verifies.
    pub fn append(
        &mut self,
        media: &mut impl ReadWrite,
        payload: &[u8],
        sector: &mut Sector,
    ) -> Result<u64, Error> {
        if payload.len() > PAYLOAD {
            return Err(Error::Payload);
        }
        if self.uncertain {
            return Err(Error::Corrupt);
        }
        if self.unsettled {
            return Err(Error::Unsettled);
        }
        if self.head >= self.capacity {
            return Err(Error::Full);
        }
        let index = self.head;
        media.owned_read(DATA_START + index, sector)?;
        if record(sector, self.store, index).is_some() {
            return Err(Error::Occupied);
        }
        // The checkpoint it follows, so recovery can tell a record written
        // after a newer checkpoint that has since been damaged.
        seal(sector, RECORD, self.store, index, self.sequence, payload)?;
        media.owned_write(DATA_START + index, sector)?;
        self.head += 1;
        Ok(index)
    }

    /// Persist `payload` as the state at `head`, in the next ring slot. A torn
    /// checkpoint only makes the next recovery start at an older one.
    pub fn checkpoint(
        &mut self,
        media: &mut impl ReadWrite,
        payload: &[u8],
        sector: &mut Sector,
    ) -> Result<(), Error> {
        if self.uncertain {
            return Err(Error::Corrupt);
        }
        let sequence = self.sequence + 1;
        seal(sector, CHECKPOINT, self.store, sequence, self.head, payload)?;
        // Retrying rewrites the same sequence into the same slot.
        self.unsettled = true;
        media.owned_write(RING_START + sequence % RING, sector)?;
        self.sequence = sequence;
        self.unsettled = false;
        Ok(())
    }

    /// Record `index`'s payload, or `None` when that sector is not a valid
    /// record of this epoch (torn, damaged, foreign or never written).
    pub fn read<'a>(
        &self,
        media: &mut impl ReadWrite,
        index: u64,
        sector: &'a mut Sector,
    ) -> Result<Option<&'a [u8]>, Error> {
        if index >= self.capacity {
            return Err(Error::Media(device_api::bulk::Error::Range));
        }
        media.owned_read(DATA_START + index, sector)?;
        Ok(record(sector, self.store, index).map(|header| payload(sector, header)))
    }
}

/// Start a new epoch with `initial` as its first checkpoint. Reads both
/// superblocks and the ring first: never overwrites a foreign superblock. The
/// checkpoint is written before the superblocks, into the slot the previous
/// epoch would use next, so an interruption leaves that epoch (or none)
/// current; its own records stay where they were. The rest of the ring is then
/// filled with older checkpoints of the new epoch, so recovery can check every
/// slot; an interruption there leaves the new, empty epoch corrupt.
/// Writes `RING + 2` sectors.
pub fn format(
    media: &mut impl ReadWrite,
    tag: &[u8; 16],
    seed: u64,
    initial: &[u8],
    sector: &mut Sector,
) -> Result<Log, Error> {
    if initial.len() > PAYLOAD {
        return Err(Error::Payload);
    }
    let capacity = capacity(media.owned_info()?.sectors)?;
    let mut copies = [Replica::Invalid; 2];
    for (index, copy) in copies.iter_mut().enumerate() {
        media.owned_read(index as u64, sector)?;
        *copy = super_copy(sector, tag, capacity);
    }
    let previous = match choose(copies) {
        // Two stores of one generation: supersede both with a newer generation.
        Err(Error::Corrupt) => match copies[0] {
            Replica::Ours(s) => Some(s),
            _ => None,
        },
        other => other?,
    };
    let generation = previous.map_or(1, |p| p.generation + 1);
    // Identities whose sectors are visible here: the superblocks, the ring
    // and the first record. The new store must differ from all of them.
    let mut seen = [0u64; 2 + RING as usize + 1];
    for (copy, seen) in copies.iter().zip(seen.iter_mut()) {
        if let Replica::Ours(s) = copy {
            *seen = s.store;
        }
    }
    // Continue the current epoch's checkpoint numbering, so an interrupted
    // format never overwrites that epoch's newest checkpoint.
    let mut sequence = 0;
    for slot in 0..RING {
        media.owned_read(RING_START + slot, sector)?;
        if let Some(h) = open(sector, CHECKPOINT) {
            seen[2 + slot as usize] = h.store;
            if previous.is_some_and(|p| h.store == p.store) && h.index % RING == slot {
                sequence = sequence.max(h.index);
            }
        }
    }
    media.owned_read(DATA_START, sector)?;
    if let Some(h) = open(sector, RECORD) {
        seen[2 + RING as usize] = h.store;
    }
    let mut store = mix(seed ^ previous.map_or(0, |p| p.store).rotate_left(17) ^ generation);
    while store == 0 || seen.contains(&store) {
        store = mix(store);
    }
    let mut log = Log {
        store,
        generation,
        capacity,
        head: 0,
        // The first checkpoint lands in the slot after the previous newest;
        // the fillers below take sequences between, so all exceed it.
        sequence: sequence + RING,
        uncertain: false,
        unsettled: false,
    };
    log.checkpoint(media, initial, sector)?;
    let mut body = [0u8; SUPER_BODY];
    body[..16].copy_from_slice(tag);
    put_u64(&mut body, 16, DATA_START);
    put_u32(&mut body, 24, RING as u32);
    for copy in [1, 0] {
        seal(sector, SUPER, store, generation, capacity, &body)?;
        media.owned_write(copy, sector)?;
    }
    for filler in log.sequence - (RING - 1)..log.sequence {
        seal(sector, CHECKPOINT, store, filler, 0, initial)?;
        media.owned_write(RING_START + filler % RING, sector)?;
    }
    Ok(log)
}

/// What one recovery step found.
#[derive(Debug, PartialEq, Eq)]
pub enum Step<'a> {
    More,
    /// The newest checkpoint's payload; the records written after it follow.
    Checkpoint(&'a [u8]),
    Record {
        index: u64,
        payload: &'a [u8],
    },
    /// The log ends at the returned head, proven.
    Done(Log),
    /// Where the log ends cannot be proven: a checkpoint is damaged, or a
    /// record after the newest checkpoint is, or one lies past the end. The
    /// log's head is one past the last valid record seen, for reading and
    /// export only; never append to it.
    Uncertain(Log),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Info,
    Super(u8),
    Ring(u8),
    /// No superblock: look for checkpoints (ring slots) or a first record
    /// (slot `RING`) that show this region held a log.
    Headless(u8),
    Checkpoint,
    Forward,
    /// The log ends at `from` unless a valid record follows within reach.
    Probe {
        from: u64,
        at: u64,
    },
    Done,
}

/// Incremental, read-only recovery of the current epoch.
#[derive(Clone, Copy, Debug)]
pub struct Recovery {
    tag: [u8; 16],
    phase: Phase,
    capacity: u64,
    copies: [Replica; 2],
    current: Option<Super>,
    /// Valid checkpoints of the epoch by ring slot: (sequence, head).
    ring: [Option<(u64, u64)>; RING as usize],
    /// Newest checkpoint: (sequence, head).
    best: Option<(u64, u64)>,
    /// Records the caller may append after a checkpoint before the next.
    replay: u64,
    index: u64,
    /// A record that cannot be accounted for was seen; and the last valid one.
    uncertain: bool,
    last_valid: Option<u64>,
    reads: u32,
}

impl Recovery {
    /// Recover the log tagged `tag`, whose writer appends at most `replay`
    /// records after any checkpoint before writing the next.
    pub fn new(tag: &[u8; 16], replay: u64) -> Self {
        Self {
            tag: *tag,
            phase: Phase::Info,
            capacity: 0,
            copies: [Replica::Invalid; 2],
            current: None,
            ring: [None; RING as usize],
            best: None,
            replay,
            index: 0,
            uncertain: false,
            last_valid: None,
            reads: 0,
        }
    }

    /// Sector reads so far, for boot-time bounds.
    pub fn reads(&self) -> u32 {
        self.reads
    }

    fn read(
        &mut self,
        media: &mut impl ReadWrite,
        at: u64,
        sector: &mut Sector,
    ) -> Result<(), Error> {
        self.reads += 1;
        media.owned_read(at, sector)?;
        Ok(())
    }

    /// Advance by at most one sector read. After `Done` or an error, the
    /// recovery stays finished; start a new one to retry.
    pub fn step<'a>(
        &mut self,
        media: &mut impl ReadWrite,
        sector: &'a mut Sector,
    ) -> Result<Step<'a>, Error> {
        let result = self.advance(media, sector);
        if result.is_err() {
            self.phase = Phase::Done;
        }
        result
    }

    fn advance<'a>(
        &mut self,
        media: &mut impl ReadWrite,
        sector: &'a mut Sector,
    ) -> Result<Step<'a>, Error> {
        match self.phase {
            Phase::Info => {
                self.capacity = capacity(media.owned_info()?.sectors)?;
                self.phase = Phase::Super(0);
                Ok(Step::More)
            }
            Phase::Super(copy) => {
                self.read(media, u64::from(copy), sector)?;
                self.copies[usize::from(copy)] = super_copy(sector, &self.tag, self.capacity);
                if copy == 0 {
                    self.phase = Phase::Super(1);
                } else {
                    self.current = choose(self.copies)?;
                    self.phase = if self.current.is_some() {
                        Phase::Ring(0)
                    } else {
                        Phase::Headless(0)
                    };
                }
                Ok(Step::More)
            }
            Phase::Ring(slot) => {
                self.read(media, RING_START + u64::from(slot), sector)?;
                let store = self.current.map_or(0, |s| s.store);
                if let Some(h) = open(sector, CHECKPOINT)
                    && h.store == store
                    && h.index % RING == u64::from(slot)
                    && h.aux <= self.capacity
                {
                    self.ring[usize::from(slot)] = Some((h.index, h.aux));
                    if self.best.is_none_or(|(sequence, _)| h.index > sequence) {
                        self.best = Some((h.index, h.aux));
                    }
                }
                if u64::from(slot) + 1 < RING {
                    self.phase = Phase::Ring(slot + 1);
                    return Ok(Step::More);
                }
                let (newest, _) = self.best.ok_or(Error::Corrupt)?;
                // Every checkpoint the ring should still hold is there, except
                // the slot a torn newer checkpoint may have taken.
                if newest < RING
                    || (newest - (RING - 2)..=newest).any(|sequence| {
                        self.ring[(sequence % RING) as usize].is_none_or(|(s, _)| s != sequence)
                    })
                {
                    return Err(Error::Corrupt);
                }
                self.phase = Phase::Checkpoint;
                Ok(Step::More)
            }
            Phase::Headless(slot) => {
                let at = if u64::from(slot) < RING {
                    RING_START + u64::from(slot)
                } else {
                    DATA_START
                };
                self.read(media, at, sector)?;
                // Superblocks lost while records remain: keep it all for
                // diagnosis rather than offer it as empty, unformatted media.
                // A checkpoint at head 0 alone is a first format interrupted
                // before its superblocks: nothing was recorded.
                let records = open(sector, CHECKPOINT).is_some_and(|h| h.aux > 0)
                    || (at == DATA_START && open(sector, RECORD).is_some());
                if records {
                    return Err(Error::Corrupt);
                }
                if u64::from(slot) < RING {
                    self.phase = Phase::Headless(slot + 1);
                    return Ok(Step::More);
                }
                Err(Error::Uninitialized)
            }
            Phase::Checkpoint => {
                let (sequence, head) = self.best.ok_or(Error::Corrupt)?;
                self.read(media, RING_START + sequence % RING, sector)?;
                let store = self.current.map_or(0, |s| s.store);
                let header = open(sector, CHECKPOINT)
                    .filter(|h| h.store == store && h.index == sequence && h.aux == head)
                    .ok_or(Error::Corrupt)?;
                self.index = head;
                self.phase = Phase::Forward;
                Ok(Step::Checkpoint(payload(sector, header)))
            }
            Phase::Forward => {
                if self.index >= self.capacity {
                    return Ok(self.done(self.index));
                }
                let index = self.index;
                self.read(media, DATA_START + index, sector)?;
                let store = self.current.map_or(0, |s| s.store);
                let newest = self.best.map_or(0, |(sequence, _)| sequence);
                match record(sector, store, index) {
                    Some(header) if header.aux == newest => {
                        self.index += 1;
                        self.last_valid = Some(index);
                        Ok(Step::Record {
                            index,
                            payload: payload(sector, header),
                        })
                    }
                    other => {
                        // Written after another checkpoint than the newest
                        // valid one: that checkpoint is damaged.
                        if other.is_some() {
                            self.uncertain = true;
                            self.last_valid = Some(index);
                        }
                        self.phase = Phase::Probe {
                            from: index,
                            at: index + 1,
                        };
                        Ok(Step::More)
                    }
                }
            }
            Phase::Probe { from, at } => {
                // Records after the newest checkpoint and the next, unchecked
                // one.
                let reach = self
                    .best
                    .map_or(0, |(_, head)| head)
                    .saturating_add(2 * self.replay)
                    .min(self.capacity);
                if at >= reach {
                    return Ok(self.done(from));
                }
                self.read(media, DATA_START + at, sector)?;
                let store = self.current.map_or(0, |s| s.store);
                if record(sector, store, at).is_some() {
                    self.uncertain = true;
                    self.last_valid = Some(at);
                }
                self.phase = Phase::Probe { from, at: at + 1 };
                Ok(Step::More)
            }
            Phase::Done => Err(Error::Corrupt),
        }
    }

    fn done<'a>(&mut self, head: u64) -> Step<'a> {
        self.phase = Phase::Done;
        let current = self.current.unwrap_or(Super {
            store: 0,
            generation: 0,
            capacity: self.capacity,
        });
        let mut log = Log {
            store: current.store,
            generation: current.generation,
            capacity: current.capacity,
            head,
            sequence: self.best.map_or(0, |(sequence, _)| sequence),
            uncertain: self.uncertain,
            unsettled: false,
        };
        if self.uncertain {
            log.head = self.last_valid.map_or(head, |index| (index + 1).max(head));
            return Step::Uncertain(log);
        }
        Step::Done(log)
    }
}

#[cfg(test)]
mod tests;
