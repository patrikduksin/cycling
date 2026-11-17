//! The determined set: the media ranges whose bytes the trace already
//! carries this session, so that a read of one records a digest, not its
//! bytes.
//!
//! It is a bounded cache. A range it has evicted or never held is a miss,
//! and a miss records the bytes again, so the trace stays sound whatever
//! the cache keeps.
use super::Medium;

pub const CAPACITY: usize = 256;

/// A range of one medium, in that medium's units: sectors on the card,
/// bytes on internal flash.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub medium: Medium,
    pub start: u32,
    pub count: u32,
}

impl Span {
    fn end(&self) -> u64 {
        u64::from(self.start) + u64::from(self.count)
    }
}

#[derive(Clone, Copy, Debug)]
struct Entry {
    span: Span,
    used: u32,
}

const _: () = assert!(core::mem::size_of::<Entry>() == 16);

impl Entry {
    fn key(&self) -> (u8, u32) {
        (self.span.medium as u8, self.span.start)
    }
}

/// Disjoint spans; spans that overlap or touch merge on insert. When it is full, the span used longest ago goes.
pub struct Determined {
    entries: heapless::Vec<Entry, CAPACITY>,
    pass: u32,
    evictions: u32,
    high_water: u16,
}

impl Default for Determined {
    fn default() -> Self {
        Self::new()
    }
}

impl Determined {
    pub const fn new() -> Self {
        Self {
            entries: heapless::Vec::new(),
            pass: 0,
            evictions: 0,
            high_water: 0,
        }
    }

    /// A new pass begins: a span used from here on was used later than any
    /// used before.
    pub fn next_pass(&mut self) {
        self.pass = self.pass.wrapping_add(1);
    }

    /// Whether one span holds all of `at`, marking it used. A partial cover
    /// is a miss.
    pub fn covers(&mut self, at: Span) -> bool {
        if at.count == 0 {
            return true;
        }
        let Some(index) = self.containing(at) else {
            return false;
        };
        self.entries[index].used = self.pass;
        true
    }

    /// The trace now carries `at`'s bytes.
    pub fn insert(&mut self, at: Span) {
        if at.count == 0 {
            return;
        }
        let (mut start, mut end) = (u64::from(at.start), at.end());
        let index = self.first_touching(at);
        while let Some(entry) = self.entries.get(index)
            && entry.span.medium == at.medium
            && u64::from(entry.span.start) <= end
        {
            start = start.min(u64::from(entry.span.start));
            end = end.max(entry.span.end());
            self.entries.remove(index);
        }
        self.place_evicting(index, span(at.medium, start, end), self.pass);
    }

    /// The trace no longer determines `at`'s bytes: a failed mutation, or a
    /// NOR program onto bytes it does not determine.
    pub fn remove(&mut self, at: Span) {
        if at.count == 0 {
            return;
        }
        let index = self.first_overlapping(at);
        let mut pieces = [None, None];
        while let Some(entry) = self.entries.get(index).copied()
            && entry.span.medium == at.medium
            && u64::from(entry.span.start) < at.end()
        {
            self.entries.remove(index);
            if entry.span.start < at.start {
                pieces[0] = Some((u64::from(entry.span.start), u64::from(at.start), entry.used));
            }
            if entry.span.end() > at.end() && at.end() <= u64::from(u32::MAX) {
                pieces[1] = Some((at.end(), entry.span.end(), entry.used));
            }
        }
        let mut at_piece = index;
        for (start, end, used) in pieces.into_iter().flatten() {
            at_piece = self.place_evicting(at_piece, span(at.medium, start, end), used) + 1;
        }
    }

    /// A host wrote `medium`: nothing of it is determined.
    pub fn forget(&mut self, medium: Medium) {
        self.entries.retain(|entry| entry.span.medium != medium);
    }

    /// Spans it dropped for room.
    pub fn evictions(&self) -> u32 {
        self.evictions
    }

    /// The most spans it has held at once.
    pub fn high_water(&self) -> u16 {
        self.high_water
    }

    fn containing(&self, at: Span) -> Option<usize> {
        let index = self
            .entries
            .partition_point(|entry| entry.key() <= (at.medium as u8, at.start));
        let entry = self.entries.get(index.checked_sub(1)?)?;
        (entry.span.medium == at.medium && entry.span.end() >= at.end()).then_some(index - 1)
    }

    fn first_touching(&self, at: Span) -> usize {
        self.entries.partition_point(|entry| {
            (entry.span.medium as u8, entry.span.end()) < (at.medium as u8, u64::from(at.start))
        })
    }

    fn first_overlapping(&self, at: Span) -> usize {
        self.entries.partition_point(|entry| {
            (entry.span.medium as u8, entry.span.end()) <= (at.medium as u8, u64::from(at.start))
        })
    }

    fn place_evicting(&mut self, mut index: usize, span: Span, used: u32) -> usize {
        if self.entries.is_full() {
            let pass = self.pass;
            let oldest = (0..self.entries.len())
                .max_by_key(|&at| pass.wrapping_sub(self.entries[at].used))
                .unwrap_or(0);
            self.entries.remove(oldest);
            self.evictions = self.evictions.saturating_add(1);
            if oldest < index {
                index -= 1;
            }
        }
        let _ = self.entries.insert(index, Entry { span, used });
        self.high_water = self.high_water.max(self.entries.len() as u16);
        index
    }
}

fn span(medium: Medium, start: u64, end: u64) -> Span {
    Span {
        medium,
        start: start as u32,
        count: u32::try_from(end - start).unwrap_or(u32::MAX),
    }
}

#[cfg(test)]
mod tests;
