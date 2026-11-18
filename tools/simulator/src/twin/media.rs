//! One medium as the twin knows it: the bytes the trace carried, and which
//! of them it carried.
//!
//! A byte is known once the trace gives it: an inline read, a write or an
//! erase. A NOR program ANDs onto known bytes and leaves an unknown byte
//! unknown. A failed mutation may have changed any byte it aimed at, so it
//! forgets them all, and a host write forgets the whole medium.
use std::collections::BTreeMap;

pub const PAGE: usize = 512;

#[derive(Clone)]
struct Page {
    bytes: [u8; PAGE],
    known: [u64; PAGE / 64],
}

impl Page {
    const UNKNOWN: Self = Self {
        bytes: [0; PAGE],
        known: [0; PAGE / 64],
    };

    fn get(&self, at: usize) -> Option<u8> {
        (self.known[at / 64] >> (at % 64) & 1 == 1).then_some(self.bytes[at])
    }

    fn set(&mut self, at: usize, byte: u8) {
        self.bytes[at] = byte;
        self.known[at / 64] |= 1 << (at % 64);
    }

    fn unset(&mut self, at: usize) {
        self.known[at / 64] &= !(1 << (at % 64));
    }
}

/// A mutation the device made, as the trace records it.
#[derive(Clone, Copy, Debug)]
pub enum Change<'a> {
    Write(&'a [u8]),
    /// A NOR program: each byte ANDs onto what is there.
    Program(&'a [u8]),
    Erase {
        len: u64,
    },
}

impl Change<'_> {
    fn len(&self) -> u64 {
        match self {
            Self::Write(bytes) | Self::Program(bytes) => bytes.len() as u64,
            Self::Erase { len } => *len,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    Done,
    Failed,
}

/// The first byte of a read whose value the twin does not know.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Unknown {
    pub at: u64,
}

#[derive(Clone, Default)]
pub struct TwinMedium {
    pages: BTreeMap<u64, Page>,
}

impl TwinMedium {
    pub fn new() -> Self {
        Self::default()
    }

    /// A read the trace carried: `bytes` were at `at`.
    pub fn install(&mut self, at: u64, bytes: &[u8]) {
        for (offset, byte) in bytes.iter().enumerate() {
            self.page(at + offset as u64, |page, i| page.set(i, *byte));
        }
    }

    pub fn read(&self, at: u64, out: &mut [u8]) -> Result<(), Unknown> {
        for (offset, slot) in out.iter_mut().enumerate() {
            let address = at + offset as u64;
            let page = self.pages.get(&(address / PAGE as u64));
            *slot = page
                .and_then(|page| page.get((address % PAGE as u64) as usize))
                .ok_or(Unknown { at: address })?;
        }
        Ok(())
    }

    pub fn mutate(&mut self, at: u64, change: Change<'_>, outcome: Outcome) {
        if outcome == Outcome::Failed {
            for address in at..at + change.len() {
                self.page(address, |page, i| page.unset(i));
            }
            return;
        }
        match change {
            Change::Write(bytes) => self.install(at, bytes),
            Change::Program(bytes) => {
                for (offset, byte) in bytes.iter().enumerate() {
                    self.page(at + offset as u64, |page, i| {
                        if let Some(old) = page.get(i) {
                            page.set(i, old & byte);
                        }
                    });
                }
            }
            Change::Erase { len } => {
                for address in at..at + len {
                    self.page(address, |page, i| page.set(i, 0xFF));
                }
            }
        }
    }

    /// Every page that holds a known byte, by its number, with each byte
    /// it does not know as `unknown`.
    pub fn pages(&self, unknown: u8) -> impl Iterator<Item = (u64, [u8; PAGE])> + '_ {
        self.pages.iter().map(move |(&number, page)| {
            let mut bytes = [unknown; PAGE];
            for (at, byte) in bytes.iter_mut().enumerate() {
                *byte = page.get(at).unwrap_or(unknown);
            }
            (number, bytes)
        })
    }

    /// A host wrote the medium, where the trace does not say.
    pub fn forget(&mut self) {
        self.pages.clear();
    }

    /// The first byte of `bytes` at `at` that this medium knows otherwise.
    pub fn differs(&self, at: u64, bytes: &[u8]) -> Option<u64> {
        (bytes.iter().enumerate()).find_map(|(offset, byte)| {
            let address = at + offset as u64;
            let page = self.pages.get(&(address / PAGE as u64))?;
            let held = page.get((address % PAGE as u64) as usize)?;
            (held != *byte).then_some(address)
        })
    }

    /// The bytes `from` knows in `range`.
    pub fn copy_from(&mut self, from: &TwinMedium, range: core::ops::Range<u64>, fill: Fill) {
        for address in range {
            let (page, at) = (address / PAGE as u64, (address % PAGE as u64) as usize);
            let Some(byte) = from.pages.get(&page).and_then(|page| page.get(at)) else {
                continue;
            };
            self.page(address, |page, i| {
                if fill == Fill::Overwrite || page.get(i).is_none() {
                    page.set(i, byte);
                }
            });
        }
    }

    fn page(&mut self, address: u64, change: impl FnOnce(&mut Page, usize)) {
        let page = self
            .pages
            .entry(address / PAGE as u64)
            .or_insert(Page::UNKNOWN);
        change(page, (address % PAGE as u64) as usize);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Fill {
    OnlyUnknown,
    Overwrite,
}

#[cfg(test)]
mod tests;
