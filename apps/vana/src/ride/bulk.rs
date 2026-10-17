//! Rides on the owned bulk MMC region, through the sector log mechanism.
//!
//! Each log record holds one committed version-1 slot, so the internal
//! journal's scanner and exporter decode bulk rides unchanged; a slot index is
//! the record index. Checkpoints hold this module's catalog schema: counters,
//! the open ride and the latest summaries. VANA checkpoints after every
//! terminal record and every `CHECKPOINT_RECORDS` records, so boot replays at
//! most that many records however much the region holds. The log never wraps
//! or reclaims: full media closes the ride, as on the internal journal.
//!
//! The log lives in the RIDES volume of the owned region's volume table
//! (`firmware_services::volume`). The region starts unformatted. Only explicit
//! initialize writes: the volume table when there is none (a blank region, or
//! the log from before volumes, which then reports `layout=legacy`), then a
//! superblock; clear (after a verified export) starts a new epoch whose
//! superblock makes older records unreachable without erasing them.

use crate::ride::journal::{Journal, Progress, Scan};
use crate::ride::log::{
    Catalog, Entry, HISTORY_CAPACITY, Recovery, SLOT_SIZE, Scanner, Slot, Source, Summary,
};
use device_api::bulk::{Error as MediaError, ReadWrite};
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};
use firmware_services::sector_log::{self, Error, Log, PAYLOAD, Recovery as LogRecovery, Step};
use firmware_services::volume::{self, Extent, Layout, RIDES, Volume};

/// Owner and schema of the log: VANA rides, slot and catalog version 1.
pub const TAG: &[u8; 16] = b"VANA-RIDES-V1\0\0\0";
/// Records between checkpoints, which bounds the records boot replays.
pub const CHECKPOINT_RECORDS: u32 = 32;

const SCHEMA: u8 = 1;
/// Counters and the open ride's position precede its summary.
const OPEN: usize = 40;
/// Completed-ride summaries follow the open ride's.
const HISTORY: usize = OPEN + Summary::ENCODED;
const ENCODED: usize = HISTORY + HISTORY_CAPACITY * Summary::ENCODED;
const _: () = assert!(ENCODED <= PAYLOAD);

/// Encode the recovery state for a checkpoint.
pub fn encode_catalog(catalog: &Catalog) -> [u8; ENCODED] {
    let mut bytes = [0u8; ENCODED];
    bytes[0] = SCHEMA;
    bytes[1] = u8::from(catalog.recovery.is_some())
        | u8::from(catalog.torn_first) << 1
        | u8::from(catalog.recovery.is_some_and(|r| r.gap)) << 2;
    bytes[2] = catalog.summary_count;
    put_u64(&mut bytes, 4, catalog.next_slot as u64);
    put_u32(&mut bytes, 12, catalog.next_ride_id);
    put_u16(&mut bytes, 16, catalog.completed);
    put_u16(&mut bytes, 18, catalog.invalid_slots);
    put_u16(&mut bytes, 20, catalog.valid_slots);
    if let Some(open) = catalog.recovery {
        bytes[3] = open.source as u8;
        put_u32(&mut bytes, 24, open.ride_id);
        put_u32(&mut bytes, 28, open.next_sequence);
        put_u64(&mut bytes, 32, open.active_ms);
        open.summary.encode(&mut bytes[OPEN..]);
    }
    for (index, summary) in catalog.summaries.iter().enumerate() {
        if let Some(summary) = summary {
            summary.encode(&mut bytes[HISTORY + index * Summary::ENCODED..]);
        }
    }
    bytes
}

pub fn decode_catalog(bytes: &[u8]) -> Option<Catalog> {
    if bytes.len() != ENCODED || bytes[0] != SCHEMA || bytes[1] & !7 != 0 {
        return None;
    }
    let count = usize::from(bytes[2]);
    if count > HISTORY_CAPACITY || bytes[22..24] != [0, 0] {
        return None;
    }
    let recovery = if bytes[1] & 1 != 0 {
        let source = match bytes[3] {
            1 => Source::Demo,
            2 => Source::Live,
            _ => return None,
        };
        Some(Recovery {
            ride_id: get_u32(bytes, 24),
            source,
            next_sequence: get_u32(bytes, 28),
            active_ms: get_u64(bytes, 32),
            gap: bytes[1] & 4 != 0,
            summary: Summary::decode(&bytes[OPEN..HISTORY])?,
        })
    } else {
        if bytes[3] != 0 || bytes[24..HISTORY].iter().any(|b| *b != 0) {
            return None;
        }
        None
    };
    let mut summaries = [None; HISTORY_CAPACITY];
    for (index, summary) in summaries.iter_mut().enumerate() {
        let encoded = &bytes[HISTORY + index * Summary::ENCODED..][..Summary::ENCODED];
        if index < count {
            *summary = Some(Summary::decode(encoded)?);
        } else if encoded.iter().any(|b| *b != 0) {
            return None;
        }
    }
    Some(Catalog {
        next_slot: usize::try_from(get_u64(bytes, 4)).ok()?,
        next_ride_id: get_u32(bytes, 12),
        completed: get_u16(bytes, 16),
        invalid_slots: get_u16(bytes, 18),
        valid_slots: get_u16(bytes, 20),
        recovery,
        summaries,
        summary_count: count as u8,
        torn_first: bytes[1] & 2 != 0,
    })
}

/// What the owned region's volume table said, once read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Unread,
    /// A table with a RIDES volume.
    Volumes,
    Blank,
    /// The whole-region log from before volumes; initialize replaces it.
    Legacy,
    /// The table is damaged over a ride log; kept untouched for diagnosis.
    Lost,
    /// A table this firmware cannot use, or one without RIDES. Never written.
    Foreign,
}

impl Region {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unread => "unknown",
            Self::Volumes => "volumes",
            Self::Blank => "blank",
            Self::Legacy => "legacy",
            Self::Lost => "lost",
            Self::Foreign => "foreign",
        }
    }
}

/// Recovery progress and the open epoch, retained across service calls.
pub struct BulkState {
    region: Region,
    rides: Option<Extent>,
    recovery: Option<LogRecovery>,
    scanner: Option<Scanner>,
    log: Option<Log>,
    since_checkpoint: u32,
    /// Head of the checkpoint recovery started from.
    replay_from: u64,
    sector: sector_log::Sector,
    boot_reads: u32,
}

impl Default for BulkState {
    fn default() -> Self {
        Self {
            region: Region::Unread,
            rides: None,
            recovery: Some(LogRecovery::new(TAG, u64::from(CHECKPOINT_RECORDS))),
            scanner: None,
            log: None,
            since_checkpoint: 0,
            replay_from: 0,
            sector: [0; 512],
            boot_reads: 0,
        }
    }
}

impl BulkState {
    /// The open epoch, once a scan or initialize found or created it.
    pub fn log(&self) -> Option<&Log> {
        self.log.as_ref()
    }

    /// Sector reads boot recovery took, the volume table's included.
    pub fn boot_reads(&self) -> u32 {
        self.boot_reads
    }

    pub fn region(&self) -> Region {
        self.region
    }
}

pub struct Bulk<'a, B> {
    state: &'a mut BulkState,
    media: &'a mut B,
}

impl<'a, B: ReadWrite> Bulk<'a, B> {
    pub fn new(state: &'a mut BulkState, media: &'a mut B) -> Self {
        Self { state, media }
    }

    fn format(&mut self, seed: u64) -> Progress {
        let initial = encode_catalog(&Catalog {
            next_ride_id: 1,
            ..Catalog::default()
        });
        // The recorder's seed is clock based and can repeat across boots.
        let seed = seed ^ self.media.entropy();
        // Creates the table over a blank or legacy region; refuses a foreign one.
        let rides = volume::create(self.media, &mut self.state.sector)
            .ok()
            .and_then(|table| table.find(&RIDES));
        let Some(rides) = rides else {
            self.state.log = None;
            return Progress::Failed;
        };
        self.state.region = Region::Volumes;
        self.state.rides = Some(rides);
        let mut media = Volume::new(self.media, rides);
        match sector_log::format(&mut media, TAG, seed, &initial, &mut self.state.sector) {
            Ok(log) => {
                self.state.log = Some(log);
                self.state.since_checkpoint = 0;
                Progress::Done
            }
            Err(_) => {
                self.state.log = None;
                Progress::Failed
            }
        }
    }
}

/// Sector reads of the volume table at boot.
const TABLE_READS: u32 = 2;

impl<B: ReadWrite> Bulk<'_, B> {
    /// First scan step: find RIDES in the volume table.
    fn scan_table(&mut self) -> Scan {
        let found = volume::read(self.media, &mut self.state.sector);
        self.state.boot_reads = TABLE_READS;
        let result = match found {
            Ok(Layout::Table(table)) => match table.find(&RIDES) {
                Some(rides) => {
                    self.state.region = Region::Volumes;
                    self.state.rides = Some(rides);
                    return Scan::More;
                }
                None => {
                    self.state.region = Region::Foreign;
                    Scan::Corrupt
                }
            },
            Ok(Layout::Blank) => {
                self.state.region = Region::Blank;
                Scan::Uninitialized
            }
            // Test data from before volumes: report it, write nothing.
            Ok(Layout::Legacy) => {
                self.state.region = Region::Legacy;
                Scan::Uninitialized
            }
            // Rides may be there; only an explicit initialize writes.
            Ok(Layout::Lost) => {
                self.state.region = Region::Lost;
                Scan::Corrupt
            }
            Err(volume::Error::Foreign | volume::Error::Missing | volume::Error::Small) => {
                self.state.region = Region::Foreign;
                Scan::Corrupt
            }
            Err(volume::Error::Media(MediaError::Unsupported)) => Scan::Unsupported,
            Err(volume::Error::Media(MediaError::Unavailable)) => Scan::Unavailable,
            Err(volume::Error::Media(_)) => Scan::Failed,
        };
        self.state.recovery = None;
        result
    }
}

impl<B: ReadWrite> Journal for Bulk<'_, B> {
    fn slots(&self) -> usize {
        self.state.log.map_or(0, |log| {
            usize::try_from(log.capacity()).unwrap_or(usize::MAX)
        })
    }

    fn scan(&mut self) -> Scan {
        if self.state.recovery.is_none() {
            return Scan::Failed;
        }
        let Some(rides) = self.state.rides else {
            return self.scan_table();
        };
        let Some(recovery) = self.state.recovery.as_mut() else {
            return Scan::Failed;
        };
        let mut media = Volume::new(self.media, rides);
        let step = recovery.step(&mut media, &mut self.state.sector);
        self.state.boot_reads = TABLE_READS + recovery.reads();
        let result = match step {
            Ok(Step::More) => return Scan::More,
            Ok(Step::Checkpoint(payload)) => match decode_catalog(payload) {
                Some(catalog) => {
                    self.state.replay_from = catalog.next_slot as u64;
                    self.state.scanner = Some(Scanner::resume(&catalog));
                    return Scan::More;
                }
                None => Scan::Corrupt,
            },
            Ok(Step::Record { index, payload }) => {
                let (Some(scanner), Ok(index)) =
                    (self.state.scanner.as_mut(), usize::try_from(index))
                else {
                    return Scan::Failed;
                };
                if payload.len() == SLOT_SIZE {
                    scanner.accept_slot(index, payload);
                } else {
                    scanner.skip(index, index + 1);
                }
                return Scan::More;
            }
            Ok(Step::Uncertain(log)) => match self.state.scanner.take() {
                // Read only: the log refuses appends and checkpoints.
                Some(scanner) => {
                    let mut catalog = scanner.catalog();
                    catalog.next_slot = log.head() as usize;
                    self.state.log = Some(log);
                    Scan::Uncertain(catalog)
                }
                None => Scan::Corrupt,
            },
            Ok(Step::Done(log)) => match self.state.scanner.take() {
                Some(scanner) => {
                    let mut catalog = scanner.catalog();
                    catalog.next_slot = log.head() as usize;
                    // Records replayed since that checkpoint count towards
                    // the next one, so cuts before checkpoints cannot make
                    // each boot replay more.
                    self.state.since_checkpoint =
                        log.head()
                            .saturating_sub(self.state.replay_from)
                            .min(u64::from(u32::MAX)) as u32;
                    self.state.log = Some(log);
                    Scan::Done(catalog)
                }
                None => Scan::Corrupt,
            },
            Err(Error::Uninitialized) => Scan::Uninitialized,
            Err(Error::Media(MediaError::Unsupported)) => Scan::Unsupported,
            Err(Error::Media(MediaError::Unavailable)) => Scan::Unavailable,
            Err(Error::Foreign | Error::Corrupt) => Scan::Corrupt,
            Err(_) => Scan::Failed,
        };
        self.state.recovery = None;
        result
    }

    fn initialize(&mut self, _: usize, seed: u64) -> Progress {
        self.format(seed)
    }

    fn clear(&mut self, _: usize, seed: u64) -> Progress {
        self.format(seed)
    }

    fn append(&mut self, index: usize, entry: &Entry, terminal: bool) -> Option<bool> {
        let log = self.state.log.as_mut()?;
        if log.head() != index as u64 {
            return None;
        }
        let mut slot = crate::ride::log::encode(entry);
        slot.0[SLOT_SIZE - 4..].copy_from_slice(&crate::ride::log::commit_word().0);
        let mut media = Volume::new(self.media, self.state.rides?);
        log.append(&mut media, &slot.0, &mut self.state.sector)
            .ok()?;
        self.state.since_checkpoint += 1;
        Some(terminal || self.state.since_checkpoint >= CHECKPOINT_RECORDS)
    }

    fn checkpoint_overdue(&self) -> bool {
        self.state.since_checkpoint >= CHECKPOINT_RECORDS
    }

    fn checkpoint(&mut self, catalog: &Catalog) -> bool {
        let (Some(log), Some(rides)) = (self.state.log.as_mut(), self.state.rides) else {
            return false;
        };
        let mut media = Volume::new(self.media, rides);
        let ok = log
            .checkpoint(&mut media, &encode_catalog(catalog), &mut self.state.sector)
            .is_ok();
        if ok {
            self.state.since_checkpoint = 0;
        }
        ok
    }

    fn read_slot(&mut self, index: usize, output: &mut Slot) -> Option<bool> {
        let log = self.state.log?;
        let mut media = Volume::new(self.media, self.state.rides?);
        match log.read(&mut media, index as u64, &mut self.state.sector) {
            Ok(Some(payload)) if payload.len() == SLOT_SIZE => {
                output.0.copy_from_slice(payload);
                Some(true)
            }
            Ok(_) => {
                output.0.fill(0);
                Some(false)
            }
            Err(_) => None,
        }
    }
}

#[cfg(test)]
mod tests;
