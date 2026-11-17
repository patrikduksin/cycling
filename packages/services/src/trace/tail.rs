//! The end of a trace file as the card holds it, read back by the boot
//! after the session that wrote it.
//!
//! Every boot reads the previous session's trace from its last synced
//! record to its end, for the durable horizon: the last pass whose records
//! survived. A record shorter than [`PAYLOAD`] is one a sync wrote, and the
//! drain syncs only after it has appended every block the spool held, so a
//! block starts right after it. [`tail`] reads the file's last chunk, and
//! steps a chunk back while it finds no such record with a whole block
//! after it.
//!
//! After a panic or a watchdog reset, memory still holds the spool's ring
//! and the recorder's open block ([`Remains`]). Before the next trace
//! opens, [`salvage`] appends to the previous one what they hold after its
//! end:
//! - It finds the ring's block the file holds only the start of: the block
//!   after the file's last whole block, where the ring still holds that
//!   block, or else the block at the ring's start, where the spool starts
//!   again each time it empties. The bytes the file holds of it must match.
//! - From there it keeps each block whose frame and CRC are valid, that
//!   decodes, and that opens at the last pass of the block before it, as a
//!   recorder opens each block (or later, saying blocks were lost). It
//!   stops at the first block that does not, so a torn block ends the
//!   salvage and nothing after it is kept, the open block included.
//! - Then the open block, when it holds events, decodes and follows the
//!   last block kept in the same way.
//!
//! Where the ring holds no block that continues the file, the salvage
//! keeps only the open block, and only when the file ends on a block
//! boundary: appended after a block the file holds only part of, it would
//! read as the rest of that block.
use super::{Clocks, FRAME, clocks};
use crate::checksum::crc32;
use crate::store::{Dir, Error, Name, Next, PAYLOAD, Sector, Store};
use device_api::bulk::ReadWrite;

/// The largest block a recorder seals, frame included: the C606's and the
/// simulator's.
pub const BLOCK: usize = 4096;

/// The work buffer [`tail`] and [`salvage`] read through: a whole block,
/// the start of the next, a record read into it and that record, and for
/// the salvage a block the ring's end splits.
pub const WINDOW: usize = 3 * BLOCK + 2 * PAYLOAD;

/// What a panic or a watchdog reset left in memory of the session before.
#[derive(Clone, Copy, Debug)]
pub struct Remains<'a> {
    /// The spool's ring.
    pub ring: &'a [u8],
    /// The recorder's block buffer. Its length prefix is current; its CRC
    /// is not.
    pub open: &'a [u8],
}

/// Where a trace file's bytes end: `partial` bytes into a block, after
/// `before`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Edge<'w> {
    pub before: Before<'w>,
    pub partial: &'w [u8],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Before<'w> {
    /// The header: the file holds no whole event block.
    Header,
    /// The file's last whole block, framed.
    Block(&'w [u8]),
    /// A block the scan did not read, before a synced record.
    Unread,
}

/// A trace file's end, as [`tail`] reads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Tail<'w> {
    /// `None` when the end is off every block boundary the scan found: a
    /// record was lost or a block is damaged after the last synced record.
    pub edge: Option<Edge<'w>>,
    /// The last pass of the last whole event block the scan read.
    pub last_pass_ms: Option<u64>,
    /// Whole event blocks after the last synced record.
    pub blocks: u32,
}

/// Read `name`'s end through `window`, of at least [`WINDOW`] bytes.
pub fn tail<'w>(
    store: &Store,
    media: &mut impl ReadWrite,
    name: &Name,
    window: &'w mut [u8],
    sector: &mut Sector,
) -> Result<Tail<'w>, Error> {
    assert!(
        window.len() >= WINDOW - BLOCK,
        "a tail reads through WINDOW bytes"
    );
    let (window, out) = window.split_at_mut(window.len() - PAYLOAD);
    let out: &mut [u8; PAYLOAD] = out.try_into().expect("the window ends with a record");
    let mut reader = store.reader(media, name, sector)?;
    let chunk = store.chunk_sectors();
    let mut from = reader.chunks().saturating_sub(1) * chunk;
    let scan = loop {
        reader.seek(store, media, from, sector)?;
        let mut scan = Scan::new(from == 0);
        loop {
            match reader.next(store, media, out, sector)? {
                Next::Record(n) => scan.push(window, &out[..n], n < PAYLOAD),
                Next::Hole { .. } => scan.lose(),
                Next::End => break,
            }
        }
        if from == 0 || !scan.steps_back() {
            break scan;
        }
        from -= chunk;
    };
    Ok(scan.tail(window))
}

#[derive(Clone, Copy)]
enum Held {
    Nothing,
    Header,
    Block,
}

/// A file's records read forward, as offsets into the window: the last
/// whole block (`whole` bytes), then the block being read, up to `held`.
struct Scan {
    /// At a block boundary: from the file's start, or since a synced record.
    aligned: bool,
    /// The next whole block is the header.
    first: bool,
    /// A record was lost or a block was damaged.
    lost: bool,
    held: usize,
    whole: usize,
    before: Held,
    last_pass_ms: Option<u64>,
    blocks: u32,
}

impl Scan {
    fn new(from_start: bool) -> Self {
        Self {
            aligned: from_start,
            first: from_start,
            lost: false,
            held: 0,
            whole: 0,
            before: Held::Nothing,
            last_pass_ms: None,
            blocks: 0,
        }
    }

    /// The next record's payload; `synced` when a sync wrote it.
    fn push(&mut self, window: &mut [u8], bytes: &[u8], synced: bool) {
        if self.aligned {
            match window.get_mut(self.held..self.held + bytes.len()) {
                Some(room) => {
                    room.copy_from_slice(bytes);
                    self.held += bytes.len();
                    while self.read_block(window) {}
                }
                None => self.lose(),
            }
        }
        if synced {
            if !self.aligned || self.held != self.whole {
                self.align();
            }
            self.blocks = 0;
        }
    }

    fn read_block(&mut self, window: &mut [u8]) -> bool {
        let start = self.whole;
        if self.held < start + 2 {
            return false;
        }
        let size = usize::from(u16::from_le_bytes([window[start], window[start + 1]])) + FRAME;
        if size > BLOCK {
            self.lose();
            return false;
        }
        if self.held < start + size {
            return false;
        }
        let (payload, crc) = window[start + 2..start + size].split_at(size - FRAME);
        if crc32(payload).to_le_bytes() != crc {
            self.lose();
            return false;
        }
        if core::mem::take(&mut self.first) {
            self.before = Held::Header;
        } else {
            let Some(clocks) = clocks(payload) else {
                self.lose();
                return false;
            };
            self.last_pass_ms = Some(clocks.end);
            self.blocks += 1;
            self.before = Held::Block;
        }
        window.copy_within(start..self.held, 0);
        self.held -= start;
        self.whole = size;
        true
    }

    fn lose(&mut self) {
        self.lost = true;
        self.aligned = false;
        self.first = false;
        (self.held, self.whole, self.before) = (0, 0, Held::Nothing);
    }

    /// A synced record ended here: a block starts after it.
    fn align(&mut self) {
        self.aligned = true;
        (self.held, self.whole, self.before) = (0, 0, Held::Nothing);
    }

    fn steps_back(&self) -> bool {
        !self.lost && (!self.aligned || matches!(self.before, Held::Nothing))
    }

    fn tail(self, window: &[u8]) -> Tail<'_> {
        let before = match self.before {
            Held::Nothing => Before::Unread,
            Held::Header => Before::Header,
            Held::Block => Before::Block(&window[..self.whole]),
        };
        Tail {
            edge: self.aligned.then(|| Edge {
                before,
                partial: &window[self.whole..self.held],
            }),
            last_pass_ms: self.last_pass_ms,
            blocks: self.blocks,
        }
    }
}

/// Append to `name` what `remains` holds after its end (see the module),
/// and sync; the blocks appended. `window` is as [`tail`] takes it.
pub fn salvage(
    store: &mut Store,
    media: &mut impl ReadWrite,
    name: &Name,
    rollover: Option<Dir>,
    remains: Remains<'_>,
    window: &mut [u8],
    sector: &mut Sector,
) -> Result<u32, Error> {
    let (window, scratch) = window.split_at_mut(window.len() - BLOCK);
    let found = Found::of(remains, &tail(store, media, name, window, sector)?, scratch);
    if found.blocks == 0 {
        return Ok(0);
    }
    let mut appender = store.open_append(media, name, rollover, sector)?;
    let (first, second) = circular(remains.ring, found.from, found.len);
    for part in [first, second] {
        appender.append(store, media, part, sector)?;
    }
    if let Some(open) = found.open {
        let framed = &remains.open[..2 + open.len];
        appender.append(store, media, framed, sector)?;
        appender.append(store, media, &open.crc.to_le_bytes(), sector)?;
    }
    appender.sync(store, media, sector)?;
    Ok(found.blocks)
}

/// What of [`Remains`] follows a file's end: `len` bytes of the ring from
/// `from`, then the open block.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Found {
    from: usize,
    len: usize,
    open: Option<Open>,
    blocks: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Open {
    len: usize,
    crc: u32,
}

/// What the next block must open at to follow the last one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Follows {
    /// The last block's last pass: a recorder opens each block at it. A
    /// block that opens later follows only when it opens by saying blocks
    /// were lost before it.
    End(u64),
    /// No earlier than this: the last block was not read.
    NotBefore(u64),
}

impl Follows {
    fn by(self, clocks: &Clocks) -> bool {
        match self {
            Self::End(end) => clocks.opening == end || (clocks.opening > end && clocks.after_loss),
            Self::NotBefore(floor) => clocks.opening >= floor,
        }
    }
}

/// Ring blocks that follow each other from a place in the ring: `bytes` of
/// them, the last at `last`, and what a block after them must open at.
#[derive(Clone, Copy)]
struct Chain {
    bytes: usize,
    blocks: u32,
    last: Option<(usize, usize)>,
    follows: Follows,
}

impl Chain {
    fn new(follows: Follows) -> Self {
        Self {
            bytes: 0,
            blocks: 0,
            last: None,
            follows,
        }
    }
}

impl Found {
    const NOTHING: Self = Self {
        from: 0,
        len: 0,
        open: None,
        blocks: 0,
    };

    fn of(remains: Remains<'_>, tail: &Tail<'_>, scratch: &mut [u8]) -> Self {
        let Some(edge) = tail.edge else {
            return Self::NOTHING;
        };
        let follows = match edge.before {
            Before::Header => Follows::End(0),
            Before::Block(_) => Follows::End(tail.last_pass_ms.unwrap_or(0)),
            Before::Unread => Follows::NotBefore(tail.last_pass_ms.unwrap_or(0)),
        };
        let ring = remains.ring;
        let skip = edge.partial.len();
        let (from, len, chain) = match place(ring, edge, follows, scratch) {
            Some((at, chain)) => ((at + skip) % ring.len(), chain.bytes - skip, chain),
            None if skip == 0 => (0, 0, Chain::new(follows)),
            None => return Self::NOTHING,
        };
        let open = Open::of(remains, &chain);
        Self {
            from,
            len,
            open,
            blocks: chain.blocks + u32::from(open.is_some()),
        }
    }
}

/// Where the ring continues the file: the block after its last whole block
/// where the ring holds that block, or else the ring's first block.
fn place(
    ring: &[u8],
    edge: Edge<'_>,
    follows: Follows,
    scratch: &mut [u8],
) -> Option<(usize, Chain)> {
    let mut fits = |at: usize| {
        circular_eq(ring, at, edge.partial)
            .then(|| walk(ring, at, follows, scratch))
            .filter(|chain| chain.bytes > edge.partial.len())
            .map(|chain| (at, chain))
    };
    if let Before::Block(last) = edge.before {
        let after = (0..ring.len())
            .filter(|&at| ring[at] == last[0] && circular_eq(ring, at, last))
            .find_map(|at| fits((at + last.len()) % ring.len()));
        if after.is_some() {
            return after;
        }
    }
    fits(0)
}

/// The blocks from `at` whose frame and CRC are valid, that decode, and
/// that each follow the one before, from `follows`. `scratch` holds a
/// block the ring's end splits.
fn walk(ring: &[u8], at: usize, follows: Follows, scratch: &mut [u8]) -> Chain {
    let mut chain = Chain::new(follows);
    let cap = ring.len();
    while chain.bytes + FRAME <= cap {
        let start = (at + chain.bytes) % cap;
        let mut len = [0u8; 2];
        copy_circular(ring, start, &mut len);
        let size = usize::from(u16::from_le_bytes(len)) + FRAME;
        if size > BLOCK || chain.bytes + size > cap {
            break;
        }
        let payload = match circular(ring, start + 2, size - FRAME) {
            (whole, []) => whole,
            _ => {
                copy_circular(ring, start + 2, &mut scratch[..size - FRAME]);
                &scratch[..size - FRAME]
            }
        };
        let mut stored = [0u8; 4];
        copy_circular(ring, start + size - 4, &mut stored);
        let clocks = (crc32(payload).to_le_bytes() == stored)
            .then(|| clocks(payload))
            .flatten()
            .filter(|clocks| chain.follows.by(clocks));
        let Some(clocks) = clocks else {
            break;
        };
        chain.follows = Follows::End(clocks.end);
        chain.last = Some((start, size));
        chain.bytes += size;
        chain.blocks += 1;
    }
    chain
}

impl Open {
    fn of(remains: Remains<'_>, chain: &Chain) -> Option<Self> {
        let open = remains.open;
        let len = usize::from(u16::from_le_bytes(open.get(..2)?.try_into().ok()?));
        let payload = open.get(2..2 + len)?;
        let clocks = clocks(payload)?;
        if clocks.events == 0 || !chain.follows.by(&clocks) {
            return None;
        }
        let again = chain.last.is_some_and(|(at, size)| {
            size == len + FRAME && circular_eq(remains.ring, at + 2, payload)
        });
        (!again).then(|| Self {
            len,
            crc: crc32(payload),
        })
    }
}

/// `len` bytes of the ring from `at`, in the two parts its end splits them
/// into. `len` is at most the ring's length.
fn circular(ring: &[u8], at: usize, len: usize) -> (&[u8], &[u8]) {
    if ring.is_empty() {
        return (&[], &[]);
    }
    let at = at % ring.len();
    let first = (ring.len() - at).min(len);
    (&ring[at..at + first], &ring[..len - first])
}

fn copy_circular(ring: &[u8], at: usize, out: &mut [u8]) {
    let (first, second) = circular(ring, at, out.len());
    out[..first.len()].copy_from_slice(first);
    out[first.len()..].copy_from_slice(second);
}

fn circular_eq(ring: &[u8], at: usize, bytes: &[u8]) -> bool {
    if bytes.len() > ring.len() {
        return false;
    }
    let (first, second) = circular(ring, at, bytes.len());
    bytes[..first.len()] == *first && bytes[first.len()..] == *second
}

#[cfg(test)]
mod tests;
