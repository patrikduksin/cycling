//! A journal holds rides one after another, so slot ride ids never fall.
use crate::ride::log::Entry;

const PROBES: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The first slot of the ride lies in `lo..hi`.
    Find {
        lo: usize,
        hi: usize,
    },
    /// Reading the ride from slot `next`.
    Read {
        next: usize,
    },
    Done,
}

/// A walk over a journal from a ride's first slot: it finds that slot by
/// bisection, then reads on in order, a budget of slots at a time. It hands
/// on every readable entry of the ride and of any ride after it, until told
/// to stop; an entry is never handed on twice.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Walk {
    ride_id: u32,
    phase: Phase,
}

impl Walk {
    /// Walk from ride `ride_id` in a journal whose slots below `upper` are
    /// written.
    pub fn new(ride_id: u32, upper: usize) -> Self {
        Self {
            ride_id,
            phase: Phase::Find { lo: 0, hi: upper },
        }
    }

    pub fn stop(&mut self) {
        self.phase = Phase::Done;
    }

    pub fn stopped(&self) -> bool {
        self.phase == Phase::Done
    }

    /// Every slot below `upper` is read.
    pub fn caught_up(&self, upper: usize) -> bool {
        matches!(self.phase, Phase::Read { next } if next >= upper)
    }

    /// Read up to `budget` slots below `upper` through `read`, which returns
    /// the decoded entry of a slot, or `None` for one that cannot be read or
    /// decoded. `visit` takes each entry of the ride or a later one, and
    /// returns false to stop the walk.
    pub fn step(
        &mut self,
        upper: usize,
        budget: usize,
        mut read: impl FnMut(usize) -> Option<Entry>,
        mut visit: impl FnMut(&Entry) -> bool,
    ) {
        let mut left = budget;
        while left > 0 {
            match self.phase {
                Phase::Done => break,
                Phase::Find { lo, hi } if lo >= hi => {
                    self.phase = Phase::Read { next: lo };
                }
                Phase::Find { lo, hi } => {
                    let mid = lo + (hi - lo) / 2;
                    // An unreadable slot: the next readable one decides.
                    let mut probe = mid;
                    let mut entry = None;
                    while probe < hi && probe < mid + PROBES && left > 0 {
                        left -= 1;
                        entry = read(probe);
                        if entry.is_some() {
                            break;
                        }
                        probe += 1;
                    }
                    if left == 0 && entry.is_none() && probe < hi && probe < mid + PROBES {
                        // Out of budget mid-probe: try this bisection again.
                        break;
                    }
                    self.phase = match entry {
                        Some(entry) if entry.ride_id < self.ride_id => {
                            Phase::Find { lo: probe + 1, hi }
                        }
                        _ => Phase::Find { lo, hi: mid },
                    };
                }
                Phase::Read { next } if next >= upper => break,
                Phase::Read { next } => {
                    left -= 1;
                    self.phase = Phase::Read { next: next + 1 };
                    // Before the ride's first readable slot nothing is lost.
                    let Some(entry) = read(next).filter(|e| e.ride_id >= self.ride_id) else {
                        continue;
                    };
                    if !visit(&entry) {
                        self.phase = Phase::Done;
                    }
                }
            }
        }
    }
}
