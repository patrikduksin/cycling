//! Bounded clear-all scheduler and capacity estimates for the owned ride journal.

use crate::ride_log::{SAMPLES_PER_BATCH, SECTORS, SLOTS, Sector, Status};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Progress {
    More,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error<E> {
    Media(E),
    Verify,
}

pub trait Media {
    type Error;
    fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error>;
    fn read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error>;
}

#[derive(Default)]
pub struct Clear {
    sector: usize,
}

impl Clear {
    pub fn service<M: Media>(
        &mut self,
        media: &mut M,
        scratch: &mut Sector,
    ) -> Result<Progress, Error<M::Error>> {
        if self.sector == SECTORS {
            return Ok(Progress::Complete);
        }
        media.erase_sector(self.sector).map_err(Error::Media)?;
        media
            .read_sector(self.sector, scratch)
            .map_err(Error::Media)?;
        if !scratch.0.iter().all(|byte| *byte == 0xff) {
            return Err(Error::Verify);
        }
        self.sector += 1;
        Ok(if self.sector == SECTORS {
            Progress::Complete
        } else {
            Progress::More
        })
    }
}

pub const fn free_slots(used: usize) -> usize {
    SLOTS.saturating_sub(if used > SLOTS { SLOTS } else { used })
}

/// Optimistic 1 Hz sample time, reserving START and terminal event slots.
pub const fn estimated_seconds(used: usize) -> u32 {
    free_slots(used)
        .saturating_sub(2)
        .saturating_mul(SAMPLES_PER_BATCH) as u32
}

pub const fn request_allowed(
    status: Status,
    open_ride: bool,
    busy: bool,
    expected_upper: usize,
    actual_upper: usize,
) -> bool {
    !open_ride
        && !busy
        && expected_upper == actual_upper
        && matches!(
            status,
            Status::NeedsInit | Status::Ready | Status::Saved | Status::Recovered | Status::Full
        )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{vec, vec::Vec};

    struct Fake {
        sectors: Vec<[u8; crate::ride_log::SECTOR_SIZE]>,
        fail_at: Option<usize>,
        calls: Vec<usize>,
        corrupt_readback: Option<usize>,
    }

    impl Fake {
        fn full() -> Self {
            Self {
                sectors: vec![[0; crate::ride_log::SECTOR_SIZE]; SECTORS],
                fail_at: None,
                calls: vec![],
                corrupt_readback: None,
            }
        }
    }

    impl Media for Fake {
        type Error = ();
        fn erase_sector(&mut self, sector: usize) -> Result<(), Self::Error> {
            self.calls.push(sector);
            if self.fail_at == Some(sector) {
                return Err(());
            }
            self.sectors[sector].fill(0xff);
            Ok(())
        }
        fn read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), Self::Error> {
            output.0.copy_from_slice(&self.sectors[sector]);
            if self.corrupt_readback == Some(sector) {
                output.0[0] = 0;
            }
            Ok(())
        }
    }

    #[test]
    fn clears_each_sector_once_within_bounds() {
        let mut media = Fake::full();
        let mut clear = Clear::default();
        let mut scratch = Sector::default();
        while clear.service(&mut media, &mut scratch).unwrap() == Progress::More {}
        assert_eq!(media.calls, (0..SECTORS).collect::<Vec<_>>());
        assert!(media.sectors.iter().flatten().all(|byte| *byte == 0xff));
    }

    #[test]
    fn interruption_and_failure_are_retryable_from_the_start() {
        let mut media = Fake::full();
        let mut clear = Clear::default();
        let mut scratch = Sector::default();
        for _ in 0..7 {
            assert_eq!(clear.service(&mut media, &mut scratch), Ok(Progress::More));
        }
        let _interrupted = clear;
        media.fail_at = Some(9);
        let mut retry = Clear::default();
        for _ in 0..9 {
            retry.service(&mut media, &mut scratch).unwrap();
        }
        assert_eq!(
            retry.service(&mut media, &mut scratch),
            Err(Error::Media(()))
        );
        media.fail_at = None;
        let mut final_retry = Clear::default();
        while final_retry.service(&mut media, &mut scratch).unwrap() == Progress::More {}
        assert!(media.sectors.iter().flatten().all(|byte| *byte == 0xff));
    }

    #[test]
    fn readback_mismatch_fails_without_advancing() {
        let mut media = Fake::full();
        media.corrupt_readback = Some(0);
        let mut clear = Clear::default();
        let mut scratch = Sector::default();
        assert_eq!(clear.service(&mut media, &mut scratch), Err(Error::Verify));
        media.corrupt_readback = None;
        assert_eq!(clear.service(&mut media, &mut scratch), Ok(Progress::More));
        assert_eq!(media.calls, [0, 0]);
    }

    #[test]
    fn scanner_recovers_surviving_suffix_after_interrupted_clear() {
        use crate::ride_log::{Entry, Kind, SLOT_SIZE, SLOTS_PER_SECTOR, Scanner, Source, encode};
        let mut media = Fake {
            sectors: vec![[0xff; crate::ride_log::SECTOR_SIZE]; SECTORS],
            fail_at: None,
            calls: vec![],
            corrupt_readback: None,
        };
        for (offset, entry) in [
            Entry::event(Kind::Start, Source::Demo, 7, 0, 0),
            Entry::event(Kind::Finish, Source::Demo, 7, 1, 10),
        ]
        .into_iter()
        .enumerate()
        {
            let mut slot = encode(&entry);
            slot.0[SLOT_SIZE - 4..].copy_from_slice(&crate::ride_log::commit_word().0);
            assert_eq!(crate::ride_log::decode(&slot), Some(entry));
            let at = (SLOTS_PER_SECTOR + offset) * SLOT_SIZE;
            let sector = at / crate::ride_log::SECTOR_SIZE;
            let within = at % crate::ride_log::SECTOR_SIZE;
            media.sectors[sector][within..within + SLOT_SIZE].copy_from_slice(&slot.0);
        }
        let mut clear = Clear::default();
        let mut scratch = Sector::default();
        assert_eq!(clear.service(&mut media, &mut scratch), Ok(Progress::More));
        let mut scanner = Scanner::default();
        while let Some(sector) = scanner.next_sector() {
            scratch.0.copy_from_slice(&media.sectors[sector]);
            scanner.accept(&scratch);
        }
        let catalog = scanner.finish().unwrap();
        assert_eq!(catalog.valid_slots, 2);
        assert_eq!(
            (catalog.completed, catalog.next_slot),
            (1, SLOTS_PER_SECTOR + 2)
        );
        let mut retry = Clear::default();
        while retry.service(&mut media, &mut scratch).unwrap() == Progress::More {}
        assert!(media.sectors.iter().flatten().all(|byte| *byte == 0xff));
    }

    #[test]
    fn capacity_is_bounded_and_reserves_terminal_slots() {
        assert_eq!(free_slots(30), 4066);
        assert_eq!(estimated_seconds(30), 16_256);
        assert_eq!(free_slots(SLOTS + 1), 0);
        assert_eq!(estimated_seconds(SLOTS), 0);
    }

    #[test]
    fn clear_refuses_busy_uncertain_and_changed_journals_but_accepts_full() {
        assert!(request_allowed(Status::Full, false, false, SLOTS, SLOTS));
        assert!(request_allowed(Status::NeedsInit, false, false, 4, 4));
        assert!(!request_allowed(Status::Recording, true, false, 4, 4));
        assert!(!request_allowed(Status::Scanning, false, false, 4, 4));
        assert!(!request_allowed(Status::Error, false, false, 4, 4));
        assert!(!request_allowed(Status::Ready, false, true, 4, 4));
        assert!(!request_allowed(Status::Ready, false, false, 3, 4));
    }
}
