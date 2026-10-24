//! Ride index: one short line per saved ride of a journal, so ride history
//! lists every ride, not only the few the journal's catalog keeps.
//!
//! The index is built by reading the journal's slots in order through the
//! export reader, a bounded number per call, with the same `Scanner` that
//! builds the catalog, so a ride's line matches the summary the journal
//! keeps for it. It continues from where it stopped, so a newly saved ride
//! is picked up by the next calls, and starts over when the journal shrinks
//! (cleared or formatted). Entries live in storage the composition lends,
//! `ENTRY` bytes each; when it is full the oldest entry makes room.
//!
//! Each line also holds the ride's normalized power and power time (four
//! bytes, in 8 s units that saturate at 145 h), so its training stress can be worked out at the current FTP.
//!
//! Nothing is persisted: the index is rebuilt after every boot.
use super::load::{Effort, PowerLoad};
use super::log::{Scanner, Slot, Summary};
use firmware_services::le::{get_u16, get_u32, put_u16, put_u32};

/// Bytes per indexed ride.
pub const ENTRY: usize = 24;
/// Rides per journal the composition is expected to lend room for.
pub const RIDES: usize = 1000;

const UNKNOWN: u32 = u32::MAX;
const RECOVERED: u8 = 1;
const FULL: u8 = 2;
const GAP: u8 = 4;

/// What a slot read gave.
pub enum Read {
    /// The slot's bytes are in the caller's slot.
    Slot,
    /// The medium's container around the slot is invalid: a gap.
    Invalid,
    /// The medium failed; try again later.
    Failed,
}

/// A ride's line in the index.
fn encode(summary: &Summary, effort: Option<Effort>, bytes: &mut [u8]) {
    put_u32(bytes, 0, summary.ride_id);
    put_u32(
        bytes,
        4,
        (summary.active_ms / 1000).min(u64::from(u32::MAX)) as u32,
    );
    let seconds = |value: Option<u64>, scale: u64| {
        value.map_or(UNKNOWN, |v| (v / scale).min(u64::from(UNKNOWN - 1)) as u32)
    };
    put_u32(bytes, 8, seconds(summary.first_utc_ms, 1000));
    put_u32(bytes, 12, seconds(summary.distance_mm, 1000));
    put_u16(bytes, 16, summary.heart_average.unwrap_or(0));
    let flag = |set: bool, bit: u8| if set { bit } else { 0 };
    bytes[18] =
        flag(summary.recovered, RECOVERED) | flag(summary.full, FULL) | flag(summary.gap, GAP);
    bytes[19] = 0;
    let effort = effort.unwrap_or(Effort {
        np_watts: 0,
        power_units: 0,
    });
    put_u16(bytes, 20, effort.np_watts);
    put_u16(bytes, 22, effort.power_units);
}

fn decode(bytes: &[u8]) -> Summary {
    let known = |value: u32, scale: u64| (value != UNKNOWN).then(|| u64::from(value) * scale);
    let mut summary = Summary::default();
    summary.ride_id = get_u32(bytes, 0);
    summary.active_ms = u64::from(get_u32(bytes, 4)) * 1000;
    summary.first_utc_ms = known(get_u32(bytes, 8), 1000);
    summary.distance_mm = known(get_u32(bytes, 12), 1000);
    let heart = get_u16(bytes, 16);
    summary.heart_average = (heart != 0).then_some(heart);
    summary.recovered = bytes[18] & RECOVERED != 0;
    summary.full = bytes[18] & FULL != 0;
    summary.gap = bytes[18] & GAP != 0;
    summary
}

/// The power a ride's line holds; none for zero watts.
fn decode_effort(bytes: &[u8]) -> Option<Effort> {
    let np_watts = get_u16(bytes, 20);
    (np_watts != 0).then(|| Effort {
        np_watts,
        power_units: get_u16(bytes, 22),
    })
}

/// The index of one journal.
pub struct Journal {
    entries: &'static mut [u8],
    /// Position of the oldest entry, and how many there are.
    head: usize,
    len: usize,
    /// The next slot to read.
    next: usize,
    scanner: Scanner,
    /// The power of the ride being read.
    load: PowerLoad,
}

impl Journal {
    /// An empty index in `entries` (a multiple of `ENTRY` bytes).
    pub fn new(entries: &'static mut [u8]) -> Self {
        Self {
            entries,
            head: 0,
            len: 0,
            next: 0,
            scanner: Scanner::default(),
            load: PowerLoad::default(),
        }
    }

    fn capacity(&self) -> usize {
        self.entries.len() / ENTRY
    }

    /// Indexed rides.
    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Slots read so far.
    pub fn scanned(&self) -> usize {
        self.next
    }

    /// The `index`-th indexed ride, oldest first.
    pub fn get(&self, index: usize) -> Option<Summary> {
        (index < self.len).then(|| {
            let at = (self.head + index) % self.capacity();
            decode(&self.entries[at * ENTRY..][..ENTRY])
        })
    }

    /// The normalized power and power time of the `index`-th indexed ride, if
    /// it had power.
    pub fn effort(&self, index: usize) -> Option<Effort> {
        (index < self.len).then(|| {
            let at = (self.head + index) % self.capacity();
            decode_effort(&self.entries[at * ENTRY..][..ENTRY])
        })?
    }

    /// The newest indexed ride's id.
    pub fn last_ride_id(&self) -> Option<u32> {
        self.len
            .checked_sub(1)
            .and_then(|i| self.get(i))
            .map(|s| s.ride_id)
    }

    /// Forget everything, for a journal that was cleared or formatted.
    pub fn reset(&mut self) {
        self.head = 0;
        self.len = 0;
        self.next = 0;
        self.scanner = Scanner::default();
        self.load = PowerLoad::default();
    }

    fn push(&mut self, summary: &Summary, effort: Option<Effort>) {
        let capacity = self.capacity();
        if capacity == 0 {
            return;
        }
        let at = if self.len < capacity {
            self.len += 1;
            (self.head + self.len - 1) % capacity
        } else {
            let at = self.head;
            self.head = (self.head + 1) % capacity;
            at
        };
        encode(summary, effort, &mut self.entries[at * ENTRY..][..ENTRY]);
    }

    /// Read up to `budget` more slots below `upper` (the journal's next
    /// slot) with `read`. A journal now shorter than what was read was
    /// cleared: the index starts over. Returns whether any ride was added.
    pub fn step(
        &mut self,
        upper: usize,
        budget: usize,
        slot: &mut Slot,
        mut read: impl FnMut(usize, &mut Slot) -> Read,
    ) -> bool {
        if upper < self.next {
            self.reset();
        }
        let mut added = false;
        for _ in 0..budget {
            if self.next >= upper {
                break;
            }
            let index = self.next;
            let completed = match read(index, slot) {
                Read::Slot => self
                    .scanner
                    .accept_completing(index, &slot.0, &mut self.load),
                Read::Invalid => {
                    self.scanner.skip(index, index + 1);
                    None
                }
                Read::Failed => break,
            };
            self.next += 1;
            if let Some((summary, effort)) = completed {
                self.push(&summary, effort);
                added = true;
            }
        }
        added
    }
}

/// Both journals' indexes, lent by the composition.
pub struct RideIndex {
    pub internal: Journal,
    pub bulk: Journal,
}

impl RideIndex {
    pub fn new(internal: &'static mut [u8], bulk: &'static mut [u8]) -> Self {
        Self {
            internal: Journal::new(internal),
            bulk: Journal::new(bulk),
        }
    }
}

#[cfg(test)]
mod tests;
