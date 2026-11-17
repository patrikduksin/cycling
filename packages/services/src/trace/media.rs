//! How a media access is recorded.
//!
//! A media record's body is `flags medium start count`, then what its flags
//! say. `start` and `count` are in the medium's units: sectors on the card,
//! bytes on internal flash.
//!
//! - A read of bytes the trace does not determine yet carries them
//!   ([`INLINE`]), with runs of `0x00` and `0xFF` elided, and determines
//!   them. A read longer than [`PART`] bytes splits into records.
//! - A read of determined bytes carries their CRC32 ([`DIGEST`]). Reads
//!   that follow one another on the medium share one record.
//! - A write carries the CRC32 of what it wrote, and consecutive writes
//!   share one record. A card write determines its sectors. A flash program
//!   only clears bits, so it keeps a determined range determined and
//!   undetermines any other.
//! - An erase carries only its range, which it determines.
//! - A failed write or erase carries its error ([`FAULT`]) and undetermines
//!   the whole range it attempted. A failed read carries the buffer as it
//!   left it and its error, and determines nothing.
//! - The TERRAIN pack's data is too large to carry: its reads are always a
//!   CRC32, and the header names the pack. The pack's own headers are
//!   carried like any other sector.
//! - STORE holds the trace itself, whose writes the trace does not see: a
//!   read of it always carries its bytes and determines nothing.
//! - The settings region is recorded as settings: an access to it carries
//!   nothing ([`OUTSIDE`]).
use super::determined::{Determined, Span};
use super::wire::{Reader, Wire, Writer};
use super::{Call, Medium, StorageError};
use crate::volume::{self, Extent};
use device_api::bulk::ReadWrite;
use device_api::journal::MethodId;
use device_api::storage::Region;

/// The body carries the bytes, elided.
pub const INLINE: u8 = 1;
/// The body carries a CRC32 of the bytes.
pub const DIGEST: u8 = 2;
/// The range is outside what the trace records.
pub const OUTSIDE: u8 = 4;
/// The access failed; its error closes the body.
pub const FAULT: u8 = 8;

pub use device_api::journal::PART;

/// The shortest run of `0x00` or `0xFF` that is elided.
const RUN: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Class {
    Inline,
    /// Carried at every read: the bytes change unseen by the trace.
    Volatile,
    DigestOnly,
    Outside,
}

/// Which card sectors are which: set once the volume table reads.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Volumes {
    /// The TERRAIN pack's data, after its headers.
    pub terrain: Option<Extent>,
    pub store: Option<Extent>,
}

impl Volumes {
    pub fn of(table: &volume::Table) -> Self {
        let data = |pack: Extent| Extent {
            start: pack.start + crate::pack::DATA,
            sectors: pack.sectors.saturating_sub(crate::pack::DATA),
            ..pack
        };
        Self {
            terrain: table.find(&volume::TERRAIN).map(data),
            store: table.find(&volume::STORE),
        }
    }

    /// The volumes of the owned region `bulk`: none without a volume table.
    pub fn read(bulk: &mut impl ReadWrite, sector: &mut volume::Sector) -> Self {
        match volume::read(bulk, sector) {
            Ok(volume::Layout::Table(table)) => Self::of(&table),
            _ => Self::default(),
        }
    }

    pub(super) fn class(&self, sector: u64) -> Class {
        let within = |extent: Option<Extent>| {
            extent.is_some_and(|extent| {
                (extent.start..extent.start + extent.sectors).contains(&sector)
            })
        };
        if within(self.store) {
            Class::Volatile
        } else if within(self.terrain) {
            Class::DigestOnly
        } else {
            Class::Inline
        }
    }
}

/// A place on a medium and how its bytes are recorded.
#[derive(Clone, Copy, Debug)]
pub(super) struct Place {
    pub span: Span,
    pub class: Class,
    /// Bytes in one unit of the span.
    pub unit: usize,
    /// Internal flash: a write clears bits rather than replacing bytes.
    pub nor: bool,
}

impl Place {
    pub fn card(volumes: &Volumes, sector: u64, bytes: usize) -> Self {
        let unit = Medium::Bulk.unit();
        let (start, class) = match u32::try_from(sector) {
            Ok(start) => (start, volumes.class(sector)),
            Err(_) => (u32::MAX, Class::Outside),
        };
        Self {
            span: Span {
                medium: Medium::Bulk,
                start,
                count: (bytes / unit) as u32,
            },
            class,
            unit,
            nor: false,
        }
    }

    pub fn flash(region: Region, offset: usize, bytes: usize) -> Self {
        let (start, class) = match (u32::try_from(offset), region) {
            (Ok(start), Region::Data | Region::Crash | Region::Intent) => (start, Class::Inline),
            (Ok(start), Region::Configuration) => (start, Class::Outside),
            (Err(_), _) => (u32::MAX, Class::Outside),
        };
        let medium: Medium = region.into();
        Self {
            span: Span {
                medium,
                start,
                count: u32::try_from(bytes).unwrap_or(u32::MAX),
            },
            class,
            unit: medium.unit(),
            nor: true,
        }
    }
}

/// Accesses that follow one another on a medium, waiting to share a record.
#[derive(Clone, Copy, Debug)]
pub(super) struct Run {
    pub method: MethodId,
    pub span: Span,
    /// The CRC32 of the run so far.
    pub crc: u32,
}

impl Run {
    /// Take `at` into the run when it is the same method's and starts where
    /// the run ends.
    pub fn extend(&mut self, method: MethodId, at: Span, bytes: &[u8], crc: Crc) -> bool {
        let follows = self.method == method
            && self.span.medium == at.medium
            && self.span.start.checked_add(self.span.count) == Some(at.start)
            && self.span.count.checked_add(at.count).is_some();
        if follows {
            self.span.count += at.count;
            self.crc = crc(self.crc, bytes);
        }
        follows
    }
}

/// Keep this large recorder state in PSRAM on devices to conserve internal RAM.
pub struct Memory {
    pub(super) determined: Determined,
    pub(super) run: Option<Run>,
    pub(super) crc: Crc,
    pub(super) scratch: [u8; SCRATCH],
}

/// Continues a CRC32: the CRC of some bytes, then more bytes, to the CRC
/// of both. A device gives its ROM's.
pub type Crc = fn(u32, &[u8]) -> u32;

/// Room for the largest record body: an inline part that elides nothing.
pub const SCRATCH: usize = PART + 256;

impl Memory {
    pub const fn new() -> Self {
        Self::with_crc(crate::checksum::crc32_more)
    }

    pub const fn with_crc(crc: Crc) -> Self {
        Self {
            determined: Determined::new(),
            run: None,
            crc,
            scratch: [0; SCRATCH],
        }
    }
}

impl Default for Memory {
    fn default() -> Self {
        Self::new()
    }
}

pub(super) fn put_span(w: &mut Writer, flags: u8, span: Span) {
    w.byte(flags);
    span.medium.put(w);
    span.start.put(w);
    span.count.put(w);
}

/// `bytes` with each run of `0x00` or `0xFF` of [`RUN`] or more as its
/// length alone: a sequence of `varint(length << 2 | kind)`, a literal's
/// bytes after it.
pub(super) fn elide(w: &mut Writer, bytes: &[u8]) {
    const LITERAL: u64 = 0;
    let filler = |at: usize| -> Option<(u64, usize)> {
        let kind = match bytes[at] {
            0xff => 1,
            0x00 => 2,
            _ => return None,
        };
        let len = bytes[at..].iter().take_while(|&&b| b == bytes[at]).count();
        (len >= RUN).then_some((kind, len))
    };
    let (mut at, mut literal) = (0, 0);
    while at < bytes.len() {
        let Some((kind, len)) = filler(at) else {
            at += 1;
            continue;
        };
        if literal < at {
            w.varint(((at - literal) as u64) << 2 | LITERAL);
            w.bytes(&bytes[literal..at]);
        }
        w.varint((len as u64) << 2 | kind);
        at += len;
        literal = at;
    }
    if literal < bytes.len() {
        w.varint(((bytes.len() - literal) as u64) << 2 | LITERAL);
        w.bytes(&bytes[literal..]);
    }
}

/// What a media record carries.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Content<'a> {
    None,
    Digest(u32),
    /// The bytes, elided: see [`Access::expand`].
    Inline(&'a [u8]),
}

/// A media record, decoded.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access<'a> {
    pub span: Span,
    pub content: Content<'a>,
    pub outside: bool,
    pub fault: Option<StorageError>,
}

impl<'a> Access<'a> {
    /// The record `call` holds, when its method is a media method.
    pub fn of(call: &Call<'a>) -> Option<Self> {
        let mut r = Reader::new(call.body, call.now_ms);
        let flags = r.byte();
        let span = Span {
            medium: Wire::take(&mut r),
            start: Wire::take(&mut r),
            count: Wire::take(&mut r),
        };
        let fault = (flags & FAULT != 0).then(|| {
            let end = call.body.len().saturating_sub(1);
            let mut last = Reader::new(&call.body[end..], 0);
            StorageError::take(&mut last)
        });
        let rest = call.body.len() - usize::from(fault.is_some());
        let content = if flags & INLINE != 0 {
            Content::Inline(call.body.get(r.at..rest)?)
        } else if flags & DIGEST != 0 {
            let crc = r.bytes(4);
            Content::Digest(u32::from_le_bytes(crc.try_into().ok()?))
        } else {
            Content::None
        };
        let whole = matches!(content, Content::Inline(_)) || r.at == rest;
        (r.ok() && whole).then_some(Self {
            span,
            content,
            outside: flags & OUTSIDE != 0,
            fault,
        })
    }

    /// The inline bytes into `out`, and how many; `None` when the record
    /// carries none, they do not decode or `out` is too short.
    pub fn expand(&self, out: &mut [u8]) -> Option<usize> {
        let Content::Inline(runs) = self.content else {
            return None;
        };
        let mut r = Reader::new(runs, 0);
        let mut at = 0usize;
        while r.at < runs.len() {
            let head = r.varint();
            let len = usize::try_from(head >> 2).ok()?;
            let slot = out.get_mut(at..at.checked_add(len)?)?;
            match head & 3 {
                0 => slot.copy_from_slice(r.bytes(len).get(..len)?),
                1 => slot.fill(0xff),
                2 => slot.fill(0x00),
                _ => return None,
            }
            at += len;
        }
        r.ok().then_some(at)
    }
}

/// An internal flash error as the trace names it.
pub trait FlashError {
    fn storage_error(&self) -> StorageError;
}

impl FlashError for device_api::observation::Error {
    fn storage_error(&self) -> StorageError {
        StorageError::from_flash(*self)
    }
}

impl<E> FlashError for device_api::storage::AccessError<E> {
    fn storage_error(&self) -> StorageError {
        match self {
            Self::Unavailable => StorageError::Unavailable,
            Self::OutOfBounds | Self::Unaligned => StorageError::Invalid,
            Self::Device(_) => StorageError::Failed,
        }
    }
}

#[cfg(test)]
mod tests;
