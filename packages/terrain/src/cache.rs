//! Recently used tiles in a caller-owned arena (PSRAM on the device).
//!
//! The arena is cut into `PAGE_BYTES` pages. A tile takes as many
//! consecutive pages as it needs, up to `MAX_TILE_BYTES` (the builder keeps
//! every tile within that): most rural tiles fit one page, a dense city tile
//! a few. A tile loads in `CHUNK` reads, one per `load` call, and is
//! checked (CRC and structure) once, when its last chunk arrives. Tiles
//! without roads, too large or corrupt are remembered as missing, so they
//! cost one directory read. A new tile replaces the run of pages
//! whose most recent use is oldest.
use crate::pack::{ENTRY_BYTES, Error, Header, Source, TileKey};
use crate::tile::Tile;
use core::cell::Cell;

/// The arena unit; the cache holds up to `MAX_PAGES` of them.
pub const PAGE_BYTES: usize = 8 * 1024;
/// The largest tile the cache holds (the builder's limit too).
pub const MAX_TILE_BYTES: usize = 48 * 1024;
/// Pages the largest tile takes.
pub const MAX_SPAN: usize = MAX_TILE_BYTES / PAGE_BYTES;
/// Bytes read per `load` call: four sectors, a few milliseconds of MMC time.
pub const CHUNK: usize = 2048;
const MAX_PAGES: usize = 96;
const MISSING: usize = 64;

/// A page: the first of a tile's pages holds its key and length, the rest
/// point back at it.
#[derive(Clone, Copy, Debug, Default)]
struct Slot {
    key: Option<TileKey>,
    len: usize,
    ready: bool,
    /// For a continuation page, the tile's first page.
    part_of: Option<usize>,
}

fn span(len: usize) -> usize {
    len.div_ceil(PAGE_BYTES).max(1)
}

#[derive(Clone, Copy, Debug)]
struct Loading {
    key: TileKey,
    slot: usize,
    offset: u32,
    len: usize,
    done: usize,
}

/// What the cache holds for a tile.
#[derive(Clone, Copy, Debug)]
pub enum Lookup<'t> {
    Ready(Tile<'t>),
    /// No roads there (or an unusable tile): treat as off the map.
    Missing,
    /// Not loaded yet.
    Absent,
}

pub struct Cache<'a> {
    arena: &'a mut [u8],
    slots: [Slot; MAX_PAGES],
    used: [Cell<u32>; MAX_PAGES],
    count: usize,
    missing: [Option<TileKey>; MISSING],
    next_missing: usize,
    clock: Cell<u32>,
    loading: Option<Loading>,
    /// Tiles read since start, for diagnostics.
    pub loads: u32,
}

impl<'a> Cache<'a> {
    /// A cache over `arena`, in `arena.len() / PAGE_BYTES` pages up to
    /// `MAX_PAGES`. Fewer than `MAX_SPAN` pages hold only smaller tiles.
    pub fn new(arena: &'a mut [u8]) -> Self {
        let count = (arena.len() / PAGE_BYTES).min(MAX_PAGES);
        Self {
            arena,
            slots: [Slot::default(); MAX_PAGES],
            used: core::array::from_fn(|_| Cell::new(0)),
            count,
            missing: [None; MISSING],
            next_missing: 0,
            clock: Cell::new(0),
            loading: None,
            loads: 0,
        }
    }

    /// Pages in the arena.
    pub fn capacity(&self) -> usize {
        self.count
    }

    /// Pages a ready tile takes (one for anything else).
    pub fn pages(&self, key: TileKey) -> usize {
        self.slots[..self.count]
            .iter()
            .find(|slot| slot.ready && slot.key == Some(key))
            .map_or(1, |slot| span(slot.len))
    }

    pub fn get(&self, key: TileKey) -> Lookup<'_> {
        if self.missing.contains(&Some(key)) {
            return Lookup::Missing;
        }
        for (index, slot) in self.slots[..self.count].iter().enumerate() {
            if slot.ready && slot.key == Some(key) {
                let now = self.clock.get().wrapping_add(1);
                self.clock.set(now);
                self.used[index].set(now);
                let start = index * PAGE_BYTES;
                return Lookup::Ready(Tile::view(&self.arena[start..start + slot.len]));
            }
        }
        Lookup::Absent
    }

    fn forget(&mut self, key: TileKey) {
        self.missing[self.next_missing] = Some(key);
        self.next_missing = (self.next_missing + 1) % MISSING;
    }

    /// One bounded step towards having `key`: a directory read or one
    /// chunk. Returns true once the tile is ready or known missing.
    pub fn load(
        &mut self,
        source: &mut impl Source,
        header: &Header,
        key: TileKey,
    ) -> Result<bool, Error> {
        if !matches!(self.get(key), Lookup::Absent) {
            return Ok(true);
        }
        if self.count == 0 {
            self.forget(key);
            return Ok(true);
        }
        let loading = match self.loading {
            Some(loading) if loading.key == key => loading,
            _ => {
                if let Some(abandoned) = self.loading.take() {
                    self.evict(abandoned.slot, 1);
                }
                let mut entry = [0u8; ENTRY_BYTES];
                source.read(header.entry(key), &mut entry)?;
                let Some((offset, len)) = header.locate(&entry)? else {
                    self.forget(key);
                    return Ok(true);
                };
                let len = len as usize;
                let pages = span(len);
                let Some(slot) = self.victim(pages).filter(|_| len <= MAX_TILE_BYTES) else {
                    self.forget(key);
                    return Ok(true);
                };
                self.evict(slot, pages);
                self.slots[slot] = Slot {
                    key: Some(key),
                    len,
                    ready: false,
                    part_of: None,
                };
                for page in slot + 1..slot + pages {
                    self.slots[page].part_of = Some(slot);
                }
                let loading = Loading {
                    key,
                    slot,
                    offset,
                    len,
                    done: 0,
                };
                self.loading = Some(loading);
                return Ok(false);
            }
        };
        let start = loading.slot * PAGE_BYTES;
        let take = (loading.len - loading.done).min(CHUNK);
        let into = &mut self.arena[start + loading.done..start + loading.done + take];
        if let Err(error) = source.read(u64::from(loading.offset) + loading.done as u64, into) {
            self.loading = None;
            self.evict(loading.slot, 1);
            return Err(error);
        }
        let done = loading.done + take;
        if done < loading.len {
            self.loading = Some(Loading { done, ..loading });
            return Ok(false);
        }
        self.loading = None;
        let size = header.tile;
        let valid = Tile::parse(&self.arena[start..start + loading.len], key, size).is_ok();
        if valid {
            self.slots[loading.slot].ready = true;
            let now = self.clock.get().wrapping_add(1);
            self.clock.set(now);
            self.used[loading.slot].set(now);
            self.loads = self.loads.wrapping_add(1);
        } else {
            self.evict(loading.slot, 1);
            self.forget(key);
        }
        Ok(true)
    }

    /// The first of the `pages` consecutive pages whose most recently used
    /// tile is the least recently used; empty pages count as oldest.
    fn victim(&self, pages: usize) -> Option<usize> {
        let now = self.clock.get();
        let age = |page: usize| {
            let head = self.slots[page].part_of.unwrap_or(page);
            if self.slots[head].key.is_none() {
                u32::MAX
            } else {
                now.wrapping_sub(self.used[head].get())
            }
        };
        (0..=self.count.checked_sub(pages)?)
            .rev()
            .max_by_key(|&start| (start..start + pages).map(age).min().unwrap_or(u32::MAX))
    }

    /// Empty every tile overlapping the `pages` pages from `first`.
    fn evict(&mut self, first: usize, pages: usize) {
        for page in first..first + pages {
            let head = self.slots[page].part_of.unwrap_or(page);
            let whole = if self.slots[head].key.is_some() {
                span(self.slots[head].len)
            } else {
                1
            };
            for part in head..(head + whole).min(self.count) {
                self.slots[part] = Slot::default();
                self.used[part].set(0);
            }
        }
    }

    /// Drop everything, for a pack that changed.
    pub fn clear(&mut self) {
        self.slots = [Slot::default(); MAX_PAGES];
        self.missing = [None; MISSING];
        self.loading = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;

    /// Place a ready tile of `len` bytes at `first`, used at `when`.
    fn place(cache: &mut Cache<'_>, first: usize, col: u16, len: usize, when: u32) {
        cache.evict(first, span(len));
        cache.slots[first] = Slot {
            key: Some(TileKey { row: 0, col }),
            len,
            ready: true,
            part_of: None,
        };
        for page in first + 1..first + span(len) {
            cache.slots[page].part_of = Some(first);
        }
        cache.used[first].set(when);
    }

    #[test]
    fn a_large_tile_takes_the_oldest_run_and_evicts_whole_tiles() {
        let mut arena = vec![0u8; 6 * PAGE_BYTES];
        let mut cache = Cache::new(&mut arena);
        cache.clock.set(100);
        // Pages: [a a][b][c][d], used at 10, 50, 20, 90; page 5 empty.
        place(&mut cache, 0, 1, PAGE_BYTES + 1, 10);
        place(&mut cache, 2, 2, 100, 50);
        place(&mut cache, 3, 3, 100, 20);
        place(&mut cache, 4, 4, 100, 90);
        assert_eq!(cache.pages(TileKey { row: 0, col: 1 }), 2);
        // One page: the empty one.
        assert_eq!(cache.victim(1), Some(5));
        place(&mut cache, 5, 5, 100, 95);
        // One page now: the oldest, a's first page.
        assert_eq!(cache.victim(1), Some(0));
        // Three pages: [a a b] and [a b c] were last used at 50, later runs later:
        // the first run whose newest use is oldest.
        let run = cache.victim(3).unwrap();
        assert_eq!(run, 0);
        // Evicting from a's second page drops all of a, and b.
        cache.evict(1, 2);
        assert!(
            cache.slots[..3]
                .iter()
                .all(|s| s.key.is_none() && s.part_of.is_none())
        );
        assert_eq!(cache.slots[4].key, Some(TileKey { row: 0, col: 4 }));
        assert_eq!(cache.slots[5].key, Some(TileKey { row: 0, col: 5 }));
        // A tile larger than the arena never fits.
        assert_eq!(cache.victim(7), None);
    }
}
