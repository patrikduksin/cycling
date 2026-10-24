//! Fixed-slot, append-only ride records with commit-last recovery.
//!
//! Persisted version 1 is shared with the host exporter and existing device data.
//! Keep the 256-byte slot, 32-byte header, four 48-byte samples, CRC and separate
//! aligned commit word compatible. `encode` leaves the commit erased; media must
//! program the body before `commit_word`. Demo provenance remains decodable even
//! though ordinary terminal recording creates only live rides.
//!
//! Sample bytes 46..48 hold the smoothed barometric altitude as FIT's
//! `altitude` field, (metres + 500) * 5 as u16, with 0xFFFF for none. There is
//! no flag bit: decoders reject unknown flags, so a flag would make new rides
//! unreadable after a rollback. Older samples left the bytes erased (0xFFFF),
//! and older decoders never read them, so both directions stay compatible.
//!
//! Sample byte 37, the last spare sample byte, marks the first sample of each
//! lap with why the lap began ([`LapStart`]); 0xFF is none. Values 6 to 254
//! are reserved for later marks and read as none here. Firmware that predates a
//! mark reads it as none too, so a ride keeps all its data and only loses that
//! lap boundary: the laps on either side read as one.
//! There is no flag bit and no record kind: decoders reject unknown flags and
//! kinds, and an unknown kind would read as a gap after a rollback. Older
//! samples left the byte erased, and older decoders never read it (only the
//! CRC covers it), so both directions stay compatible. A ride that marks no
//! sample predates lap marks.
//!
//! Occupied invalid slots consume capacity and mark gaps; they are never free
//! space. Scanning recovers an open ride at its last committed active duration,
//! excluding downtime. The final slot is reserved for a terminal record.

use crate::ride::load::{Effort, PowerLoad};
use firmware_services::checksum::crc32;
use firmware_services::le::{get_u16, get_u32, get_u64, put_u16, put_u32, put_u64};

pub const REGION_SIZE: usize = 1024 * 1024;
pub const SECTOR_SIZE: usize = 4096;
pub const SLOT_SIZE: usize = 256;
pub const SLOTS_PER_SECTOR: usize = SECTOR_SIZE / SLOT_SIZE;
pub const SECTORS: usize = REGION_SIZE / SECTOR_SIZE;
pub const SLOTS: usize = REGION_SIZE / SLOT_SIZE;
pub const SAMPLES_PER_BATCH: usize = 4;
pub const HISTORY_CAPACITY: usize = 4;
/// The longest gap between two samples that still counts as ridden time,
/// as the odometer caps its steps.
pub const MAX_STEP_MS: u64 = 3000;

const MAGIC: [u8; 4] = *b"RIDE";
pub const VERSION: u8 = 1;
const HEADER_SIZE: usize = 32;
const SAMPLE_SIZE: usize = 48;
const COMMIT_OFFSET: usize = 252;
const COMMITTED: u32 = 0x434f_4d54;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Status {
    #[default]
    Scanning,
    NeedsInit,
    Formatting,
    Clearing,
    Ready,
    Recording,
    Paused,
    Saved,
    Recovered,
    Full,
    Error,
    /// The device has no such medium.
    Unsupported,
    /// The medium should exist but cannot be reached.
    Unavailable,
    /// Rides are there but recovery metadata cannot be trusted; kept untouched.
    Corrupt,
}

impl Status {
    /// The journal can start or continue a ride, room permitting.
    pub const fn usable(self) -> bool {
        matches!(
            self,
            Self::Ready | Self::Recording | Self::Paused | Self::Saved | Self::Recovered
        )
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Unsupported => "unsupported",
            Self::Unavailable => "unavailable",
            Self::Corrupt => "corrupt",
            Self::Scanning => "scanning",
            Self::NeedsInit => "needs_init",
            Self::Formatting => "formatting",
            Self::Clearing => "clearing",
            Self::Ready => "ready",
            Self::Recording => "recording",
            Self::Paused => "paused",
            Self::Saved => "saved",
            Self::Recovered => "recovered",
            Self::Full => "full",
            Self::Error => "error",
        }
    }

    pub const fn short(self) -> &'static [u8] {
        match self {
            Self::Scanning => b"SCAN",
            Self::NeedsInit => b"INIT",
            Self::Formatting => b"FORMAT",
            Self::Clearing => b"CLEAR",
            Self::Ready => b"READY",
            Self::Recording => b"REC",
            Self::Paused => b"PAUSE",
            Self::Saved => b"SAVED",
            Self::Recovered => b"RECOV",
            Self::Full => b"FULL",
            Self::Error => b"ERROR",
            Self::Unsupported => b"NONE",
            Self::Unavailable => b"NOMEDIA",
            Self::Corrupt => b"CORRUPT",
        }
    }
}

#[repr(C, align(4))]
pub struct Sector(pub [u8; SECTOR_SIZE]);

impl Default for Sector {
    fn default() -> Self {
        Self([0; SECTOR_SIZE])
    }
}

#[repr(C, align(4))]
#[derive(Clone)]
pub struct Slot(pub [u8; SLOT_SIZE]);

impl Default for Slot {
    fn default() -> Self {
        Self([0; SLOT_SIZE])
    }
}

#[repr(C, align(4))]
pub struct Commit(pub [u8; 4]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Kind {
    Start = 1,
    Samples = 2,
    Pause = 3,
    Resume = 4,
    Finish = 5,
    Recovered = 6,
    Full = 7,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum Source {
    Demo = 1,
    Live = 2,
}

impl Source {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Demo => "demo",
            Self::Live => "live",
        }
    }
}

/// Why a lap began, kept on its first sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum LapStart {
    /// The ride's first sample: this ride keeps its laps. Not a boundary.
    Ride = 0,
    /// The rider's lap closed the lap before.
    Manual = 1,
    /// The automatic distance lap closed it.
    Distance = 2,
    /// A workout step that lasts a time, or until it is marked, ended.
    Step = 3,
    /// A workout step that lasts a distance ended.
    DistanceStep = 4,
    /// The rider passed the ride's start point.
    Position = 5,
}

impl LapStart {
    const fn from_byte(byte: u8) -> Option<Self> {
        match byte {
            0 => Some(Self::Ride),
            1 => Some(Self::Manual),
            2 => Some(Self::Distance),
            3 => Some(Self::Step),
            4 => Some(Self::DistanceStep),
            5 => Some(Self::Position),
            _ => None,
        }
    }

    pub const fn name(self) -> &'static str {
        match self {
            Self::Ride => "ride",
            Self::Manual => "manual",
            Self::Distance => "distance",
            Self::Step => "step",
            Self::DistanceStep => "distance_step",
            Self::Position => "position",
        }
    }
}

const LAP_BYTE: usize = 37;
const NO_LAP: u8 = 0xff;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sample {
    pub active_ms: u64,
    pub utc_ms: Option<u64>,
    pub location_e7: Option<(i32, i32)>,
    pub demo_speed_mm_s: Option<u32>,
    pub heart_bpm: Option<u16>,
    pub cadence_tenths: Option<u16>,
    pub battery_percent: Option<u8>,
    pub speed_mm_s: Option<u32>,
    pub power_watts: Option<u16>,
    pub gradient_tenths: Option<i16>,
    pub altitude: Option<Altitude>,
    /// This sample is the first of a lap; see [`LapStart`].
    pub lap: Option<LapStart>,
}

impl Sample {
    /// The readings a slot of a `source` ride holds. A battery reading above
    /// 100 % is dropped: `decode` refuses it, so the batch would not read
    /// back and recording would stop as a media failure.
    pub fn for_source(mut self, source: Source) -> Self {
        if source == Source::Live {
            self.demo_speed_mm_s = None;
        }
        self.battery_percent = self.battery_percent.filter(|percent| *percent <= 100);
        self
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Entry {
    pub kind: Kind,
    pub source: Source,
    pub ride_id: u32,
    pub sequence: u32,
    pub active_ms: u64,
    pub samples: [Sample; SAMPLES_PER_BATCH],
    pub count: u8,
}

impl Entry {
    pub const fn event(
        kind: Kind,
        source: Source,
        ride_id: u32,
        sequence: u32,
        active_ms: u64,
    ) -> Self {
        Self {
            kind,
            source,
            ride_id,
            sequence,
            active_ms,
            samples: [Sample {
                active_ms: 0,
                utc_ms: None,
                location_e7: None,
                demo_speed_mm_s: None,
                heart_bpm: None,
                cadence_tenths: None,
                battery_percent: None,
                speed_mm_s: None,
                power_watts: None,
                gradient_tenths: None,
                altitude: None,
                lap: None,
            }; SAMPLES_PER_BATCH],
            count: 0,
        }
    }

    pub fn batch(source: Source, ride_id: u32, sequence: u32, samples: &[Sample]) -> Option<Self> {
        if samples.is_empty() || samples.len() > SAMPLES_PER_BATCH {
            return None;
        }
        let mut entry = Self::event(
            Kind::Samples,
            source,
            ride_id,
            sequence,
            samples.last()?.active_ms,
        );
        entry.samples[..samples.len()].copy_from_slice(samples);
        entry.count = samples.len() as u8;
        Some(entry)
    }
}

pub fn encode(entry: &Entry) -> Slot {
    let mut slot = Slot([0xff; SLOT_SIZE]);
    slot.0[..4].copy_from_slice(&MAGIC);
    slot.0[4] = VERSION;
    slot.0[5] = entry.kind as u8;
    slot.0[6] = entry.count;
    slot.0[7] = entry.source as u8;
    put_u32(&mut slot.0, 8, entry.ride_id);
    put_u32(&mut slot.0, 12, entry.sequence);
    put_u64(&mut slot.0, 16, entry.active_ms);
    put_u16(&mut slot.0, 24, u16::from(entry.count) * SAMPLE_SIZE as u16);
    slot.0[26..28].fill(0);
    slot.0[28..32].fill(0);
    for (index, sample) in entry.samples[..usize::from(entry.count)].iter().enumerate() {
        encode_sample(
            sample,
            &mut slot.0[HEADER_SIZE + index * SAMPLE_SIZE..][..SAMPLE_SIZE],
        );
    }
    let crc = crc32(&slot.0[..COMMIT_OFFSET]);
    put_u32(&mut slot.0, 28, crc);
    slot
}

pub fn decode(slot: &Slot) -> Option<Entry> {
    let count = usize::from(slot.0[6]);
    if slot.0[..4] != MAGIC
        || slot.0[4] != VERSION
        || count > SAMPLES_PER_BATCH
        || get_u16(&slot.0, 24) as usize != count * SAMPLE_SIZE
        || slot.0[26..28] != [0, 0]
        || get_u32(&slot.0, COMMIT_OFFSET) != COMMITTED
    {
        return None;
    }
    let stored_checksum = get_u32(&slot.0, 28);
    let mut checked = slot.clone();
    checked.0[28..32].fill(0);
    if crc32(&checked.0[..COMMIT_OFFSET]) != stored_checksum {
        return None;
    }
    let kind = match slot.0[5] {
        1 => Kind::Start,
        2 => Kind::Samples,
        3 => Kind::Pause,
        4 => Kind::Resume,
        5 => Kind::Finish,
        6 => Kind::Recovered,
        7 => Kind::Full,
        _ => return None,
    };
    let source = match slot.0[7] {
        1 => Source::Demo,
        2 => Source::Live,
        _ => return None,
    };
    if (kind == Kind::Samples) != (count > 0) {
        return None;
    }
    let mut samples = [Sample::default(); SAMPLES_PER_BATCH];
    for (index, sample) in samples[..count].iter_mut().enumerate() {
        *sample = decode_sample(&slot.0[HEADER_SIZE + index * SAMPLE_SIZE..][..SAMPLE_SIZE])?;
    }
    let active_ms = get_u64(&slot.0, 16);
    if kind == Kind::Samples && samples[count - 1].active_ms != active_ms {
        return None;
    }
    Some(Entry {
        kind,
        source,
        ride_id: get_u32(&slot.0, 8),
        sequence: get_u32(&slot.0, 12),
        active_ms,
        samples,
        count: count as u8,
    })
}

pub fn commit_word() -> Commit {
    Commit(COMMITTED.to_le_bytes())
}

pub fn erased(slot: &[u8]) -> bool {
    slot.iter().all(|byte| *byte == 0xff)
}

/// Whether an invalid slot can be this journal's own interrupted append: its
/// magic and version bytes are programmed, or partly programmed (NOR flash only
/// clears bits), but never all still erased.
pub fn torn_append(slot: &[u8]) -> bool {
    let expected = [MAGIC[0], MAGIC[1], MAGIC[2], MAGIC[3], VERSION];
    slot.len() == SLOT_SIZE
        && !erased(&slot[..expected.len()])
        && slot
            .iter()
            .zip(expected)
            .all(|(byte, bits)| byte & bits == bits)
}

/// Active duration at `now`, bounded by an already accepted stop request.
pub fn active_at(accumulated_ms: u64, running_since: u64, stop_at: Option<u64>, now: u64) -> u64 {
    accumulated_ms.saturating_add(stop_at.unwrap_or(now).saturating_sub(running_since))
}

pub fn freeze_at(
    accumulated_ms: u64,
    running_since: u64,
    stop_at: Option<u64>,
    now: u64,
) -> (u64, u64, Option<u64>) {
    (
        active_at(accumulated_ms, running_since, stop_at, now),
        now,
        Some(now),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Recovery {
    pub ride_id: u32,
    pub source: Source,
    pub next_sequence: u32,
    pub active_ms: u64,
    pub gap: bool,
    pub summary: Summary,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Summary {
    pub ride_id: u32,
    pub source: Option<Source>,
    pub recovered: bool,
    pub full: bool,
    pub gap: bool,
    pub active_ms: u64,
    pub first_utc_ms: Option<u64>,
    pub distance_mm: Option<u64>,
    pub gps_samples: u32,
    pub heart_average: Option<u16>,
    pub cadence_average: Option<u16>,
    demo_speed_mm_s: Option<u32>,
    // Wide enough for a ride as long as the bulk log (thousands of hours).
    heart_sum: u64,
    heart_count: u32,
    cadence_sum: u64,
    cadence_count: u32,
}

impl Summary {
    pub fn started(ride_id: u32, source: Source) -> Self {
        Self {
            ride_id,
            source: Some(source),
            ..Self::default()
        }
    }

    /// Account one sample; returns the active time it covers.
    pub fn add_sample(&mut self, sample: Sample) -> u64 {
        let elapsed = step_ms(
            (self.active_ms != 0).then_some(self.active_ms),
            sample.active_ms,
        );
        self.first_utc_ms = self.first_utc_ms.or(sample.utc_ms);
        self.gps_samples = self
            .gps_samples
            .saturating_add(u32::from(sample.location_e7.is_some()));
        self.demo_speed_mm_s = self.demo_speed_mm_s.or(sample.demo_speed_mm_s);
        if let Some(value) = sample.heart_bpm {
            self.heart_sum = self.heart_sum.saturating_add(u64::from(value));
            self.heart_count = self.heart_count.saturating_add(1);
        }
        if let Some(value) = sample.cadence_tenths {
            self.cadence_sum = self.cadence_sum.saturating_add(u64::from(value));
            self.cadence_count = self.cadence_count.saturating_add(1);
        }
        if let Some(speed) = sample.speed_mm_s {
            self.distance_mm = Some(
                self.distance_mm
                    .unwrap_or(0)
                    .saturating_add(u64::from(speed) * elapsed / 1000),
            );
        }
        self.active_ms = sample.active_ms;
        elapsed
    }

    /// Persisted size of one summary in a bulk checkpoint.
    pub const ENCODED: usize = 72;

    /// Encode for a bulk checkpoint, including the running sums an open ride
    /// needs to keep averaging after reboot.
    pub fn encode(&self, bytes: &mut [u8]) {
        bytes[..Self::ENCODED].fill(0);
        put_u32(bytes, 0, self.ride_id);
        bytes[4] = self.source.map_or(0, |s| s as u8);
        let options = [
            self.first_utc_ms.is_some(),
            self.distance_mm.is_some(),
            self.heart_average.is_some(),
            self.cadence_average.is_some(),
            self.demo_speed_mm_s.is_some(),
            self.recovered,
            self.full,
            self.gap,
        ];
        bytes[5] = options
            .iter()
            .enumerate()
            .fold(0, |flags, (bit, set)| flags | (u8::from(*set) << bit));
        put_u64(bytes, 8, self.active_ms);
        put_u64(bytes, 16, self.first_utc_ms.unwrap_or(0));
        put_u64(bytes, 24, self.distance_mm.unwrap_or(0));
        put_u16(bytes, 32, self.heart_average.unwrap_or(0));
        put_u16(bytes, 34, self.cadence_average.unwrap_or(0));
        put_u32(bytes, 36, self.demo_speed_mm_s.unwrap_or(0));
        put_u32(bytes, 40, self.gps_samples);
        put_u32(bytes, 44, self.heart_count);
        put_u32(bytes, 48, self.cadence_count);
        put_u64(bytes, 56, self.heart_sum);
        put_u64(bytes, 64, self.cadence_sum);
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let flags = bytes[5];
        let source = match bytes[4] {
            0 => None,
            1 => Some(Source::Demo),
            2 => Some(Source::Live),
            _ => return None,
        };
        if bytes[6..8] != [0, 0] || bytes[52..56] != [0; 4] {
            return None;
        }
        let has = |bit: u8| flags & (1 << bit) != 0;
        Some(Self {
            ride_id: get_u32(bytes, 0),
            source,
            recovered: has(5),
            full: has(6),
            gap: has(7),
            active_ms: get_u64(bytes, 8),
            first_utc_ms: has(0).then(|| get_u64(bytes, 16)),
            distance_mm: has(1).then(|| get_u64(bytes, 24)),
            gps_samples: get_u32(bytes, 40),
            heart_average: has(2).then(|| get_u16(bytes, 32)),
            cadence_average: has(3).then(|| get_u16(bytes, 34)),
            demo_speed_mm_s: has(4).then(|| get_u32(bytes, 36)),
            heart_sum: get_u64(bytes, 56),
            heart_count: get_u32(bytes, 44),
            cadence_sum: get_u64(bytes, 64),
            cadence_count: get_u32(bytes, 48),
        })
    }

    pub fn finish(&mut self, kind: Kind, active_ms: u64, gap: bool) {
        self.active_ms = active_ms;
        self.recovered = kind == Kind::Recovered;
        self.full = kind == Kind::Full;
        self.gap |= gap;
        self.distance_mm = self
            .demo_speed_mm_s
            .map(|speed| active_ms.saturating_mul(u64::from(speed)) / 1_000)
            .or(self.distance_mm);
        let average = |sum: u64, count: u32| {
            (count != 0).then(|| (sum / u64::from(count)).min(u64::from(u16::MAX)) as u16)
        };
        self.heart_average = average(self.heart_sum, self.heart_count);
        self.cadence_average = average(self.cadence_sum, self.cadence_count);
    }
}

/// Active time a sample covers, `last_ms` being the previous sample's (none
/// before the first, which covers at most a second).
pub fn step_ms(last_ms: Option<u64>, active_ms: u64) -> u64 {
    match last_ms {
        Some(last) => active_ms.saturating_sub(last),
        None => active_ms.min(1000),
    }
    .min(MAX_STEP_MS)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Catalog {
    pub next_slot: usize,
    pub next_ride_id: u32,
    pub completed: u16,
    pub invalid_slots: u16,
    pub valid_slots: u16,
    pub recovery: Option<Recovery>,
    pub summaries: [Option<Summary>; HISTORY_CAPACITY],
    pub summary_count: u8,
    /// Slot 0 is occupied but invalid and looks like a torn first append.
    pub torn_first: bool,
}

impl Catalog {
    /// Occupied data the recorder did not write: only explicit initialize may
    /// erase it. A single torn first append (power lost during the first START
    /// of a fresh or cleared journal) is the recorder's own and stays in place
    /// as an occupied invalid slot.
    pub fn unknown_data(&self) -> bool {
        self.valid_slots == 0
            && self.invalid_slots != 0
            && !(self.invalid_slots == 1 && self.next_slot == 1 && self.torn_first)
    }

    pub fn push_summary(&mut self, summary: Summary) {
        if usize::from(self.summary_count) < HISTORY_CAPACITY {
            self.summaries[usize::from(self.summary_count)] = Some(summary);
            self.summary_count += 1;
        } else {
            self.summaries.rotate_left(1);
            self.summaries[HISTORY_CAPACITY - 1] = Some(summary);
        }
    }
}

#[derive(Default)]
pub struct Scanner {
    sector: usize,
    next_slot: usize,
    completed: u16,
    invalid_slots: u16,
    valid_slots: u16,
    open: Option<Recovery>,
    summaries: [Option<Summary>; HISTORY_CAPACITY],
    summary_count: u8,
    /// The latest record's ride; ids count up and wrap past zero, so the
    /// largest on record is not the newest.
    last_ride_id: u32,
    torn_first: bool,
    /// The ride the last accepted slot completed, for the ride index.
    completed_now: Option<Summary>,
}

impl Scanner {
    /// Scan one slot, as `accept_slot`, and return the summary of the ride
    /// it completed, if it completed one, with that ride's power. The caller
    /// lends the `load` that follows the open ride's power; only the ride
    /// index does, so the catalog scanners skip that work and its memory.
    pub fn accept_completing(
        &mut self,
        index: usize,
        bytes: &[u8],
        load: &mut PowerLoad,
    ) -> Option<(Summary, Option<Effort>)> {
        self.completed_now = None;
        self.accept_with(index, bytes, Some(&mut *load));
        self.completed_now
            .take()
            .map(|summary| (summary, load.effort()))
    }

    pub fn next_sector(&self) -> Option<usize> {
        (self.sector < SECTORS).then_some(self.sector)
    }

    pub fn accept(&mut self, sector: &Sector) {
        let base = self.sector * SLOTS_PER_SECTOR;
        for index in 0..SLOTS_PER_SECTOR {
            self.accept_slot(base + index, &sector.0[index * SLOT_SIZE..][..SLOT_SIZE]);
        }
        self.sector += 1;
    }

    /// Continue from a catalog persisted at `catalog.next_slot`, as if every
    /// earlier slot had been scanned.
    pub fn resume(catalog: &Catalog) -> Self {
        Self {
            sector: SECTORS,
            next_slot: catalog.next_slot,
            completed: catalog.completed,
            invalid_slots: catalog.invalid_slots,
            valid_slots: catalog.valid_slots,
            open: catalog.recovery,
            summaries: catalog.summaries,
            summary_count: catalog.summary_count,
            last_ride_id: catalog.next_ride_id.wrapping_sub(1),
            torn_first: catalog.torn_first,
            completed_now: None,
        }
    }

    /// Scan one slot at `index`; erased slots are unused, not gaps.
    pub fn accept_slot(&mut self, index: usize, bytes: &[u8]) {
        self.accept_with(index, bytes, None);
    }

    fn accept_with(&mut self, index: usize, bytes: &[u8], load: Option<&mut PowerLoad>) {
        if erased(bytes) {
            return;
        }
        self.next_slot = index + 1;
        let mut slot = Slot::default();
        slot.0.copy_from_slice(bytes);
        let Some(entry) = decode(&slot) else {
            self.torn_first |= index == 0 && torn_append(bytes);
            self.gap(1);
            return;
        };
        self.apply(entry, load);
    }

    /// Slots `from..to` are occupied but unreadable as records.
    pub fn skip(&mut self, from: usize, to: usize) {
        self.next_slot = self.next_slot.max(to);
        self.gap(to.saturating_sub(from));
    }

    fn gap(&mut self, slots: usize) {
        self.invalid_slots = self
            .invalid_slots
            .saturating_add(slots.min(usize::from(u16::MAX)) as u16);
        if let Some(open) = self.open.as_mut() {
            open.gap = true;
        }
    }

    /// The catalog of everything accepted so far.
    pub fn catalog(&self) -> Catalog {
        Catalog {
            next_slot: self.next_slot,
            next_ride_id: self.last_ride_id.wrapping_add(1).max(1),
            completed: self.completed,
            invalid_slots: self.invalid_slots,
            valid_slots: self.valid_slots,
            recovery: self.open,
            summaries: self.summaries,
            summary_count: self.summary_count,
            torn_first: self.torn_first,
        }
    }

    pub fn finish(&self) -> Option<Catalog> {
        (self.sector == SECTORS).then(|| self.catalog())
    }

    fn apply(&mut self, entry: Entry, mut load: Option<&mut PowerLoad>) {
        self.valid_slots = self.valid_slots.saturating_add(1);
        self.last_ride_id = entry.ride_id;
        match entry.kind {
            Kind::Start => {
                self.open = Some(Recovery {
                    ride_id: entry.ride_id,
                    source: entry.source,
                    next_sequence: entry.sequence.wrapping_add(1),
                    active_ms: entry.active_ms,
                    gap: false,
                    summary: Summary::started(entry.ride_id, entry.source),
                });
                if let Some(load) = load.as_deref_mut() {
                    *load = PowerLoad::default();
                }
            }
            Kind::Samples | Kind::Pause | Kind::Resume => {
                let Some(open) = self.open.as_mut() else {
                    return;
                };
                if entry.ride_id != open.ride_id
                    || entry.source != open.source
                    || entry.sequence != open.next_sequence
                {
                    open.gap = true;
                    return;
                }
                open.next_sequence = open.next_sequence.wrapping_add(1);
                open.active_ms = entry.active_ms;
                if entry.kind == Kind::Samples {
                    for sample in &entry.samples[..usize::from(entry.count)] {
                        let elapsed = open.summary.add_sample(*sample);
                        if let (Some(load), Some(watts), true) =
                            (load.as_deref_mut(), sample.power_watts, elapsed > 0)
                        {
                            load.add(elapsed, watts);
                        }
                    }
                }
            }
            Kind::Finish | Kind::Recovered | Kind::Full => {
                let Some(open) = self.open else { return };
                if entry.ride_id == open.ride_id
                    && entry.source == open.source
                    && entry.sequence == open.next_sequence
                {
                    self.completed = self.completed.saturating_add(1);
                    let mut summary = open.summary;
                    summary.finish(entry.kind, entry.active_ms, open.gap);
                    self.push_summary(summary);
                    self.open = None;
                } else if let Some(open) = self.open.as_mut() {
                    open.gap = true;
                }
            }
        }
    }

    fn push_summary(&mut self, summary: Summary) {
        self.completed_now = Some(summary);
        if usize::from(self.summary_count) < HISTORY_CAPACITY {
            self.summaries[usize::from(self.summary_count)] = Some(summary);
            self.summary_count += 1;
        } else {
            self.summaries.rotate_left(1);
            self.summaries[HISTORY_CAPACITY - 1] = Some(summary);
        }
    }
}

fn encode_sample(sample: &Sample, bytes: &mut [u8]) {
    bytes.fill(0xff);
    put_u64(bytes, 0, sample.active_ms);
    let mut flags = 0u32;
    if let Some(value) = sample.utc_ms {
        flags |= 1;
        put_u64(bytes, 8, value);
    }
    if let Some((latitude, longitude)) = sample.location_e7 {
        flags |= 2;
        put_u32(bytes, 20, latitude as u32);
        put_u32(bytes, 24, longitude as u32);
    }
    if let Some(value) = sample.demo_speed_mm_s {
        flags |= 4 | 0x8000_0000;
        put_u32(bytes, 28, value);
    }
    if let Some(value) = sample.heart_bpm {
        flags |= 8;
        put_u16(bytes, 32, value);
    }
    if let Some(value) = sample.cadence_tenths {
        flags |= 16;
        put_u16(bytes, 34, value);
    }
    if let Some(value) = sample.battery_percent {
        flags |= 32;
        bytes[36] = value;
    }
    if let Some(value) = sample.speed_mm_s {
        flags |= 64;
        put_u32(bytes, 40, value);
    }
    if let Some(value) = sample.power_watts {
        flags |= 128;
        put_u16(bytes, 38, value);
    }
    if let Some(value) = sample.gradient_tenths {
        flags |= 256;
        put_u16(bytes, 44, value as u16);
    }
    if let Some(value) = sample.altitude {
        put_u16(bytes, 46, value.0);
    }
    bytes[LAP_BYTE] = sample.lap.map_or(NO_LAP, |lap| lap as u8);
    put_u32(bytes, 16, flags);
}

fn decode_sample(bytes: &[u8]) -> Option<Sample> {
    let flags = get_u32(bytes, 16);
    if flags & !(0x8000_01ff) != 0 || flags & 4 != 0 && flags & 0x8000_0000 == 0 {
        return None;
    }
    let battery = (flags & 32 != 0).then_some(bytes[36]);
    if battery.is_some_and(|value| value > 100) {
        return None;
    }
    Some(Sample {
        active_ms: get_u64(bytes, 0),
        utc_ms: (flags & 1 != 0).then(|| get_u64(bytes, 8)),
        location_e7: (flags & 2 != 0)
            .then(|| (get_u32(bytes, 20) as i32, get_u32(bytes, 24) as i32)),
        demo_speed_mm_s: (flags & 4 != 0).then(|| get_u32(bytes, 28)),
        heart_bpm: (flags & 8 != 0).then(|| get_u16(bytes, 32)),
        cadence_tenths: (flags & 16 != 0).then(|| get_u16(bytes, 34)),
        battery_percent: battery,
        speed_mm_s: (flags & 64 != 0).then(|| get_u32(bytes, 40)),
        power_watts: (flags & 128 != 0).then(|| get_u16(bytes, 38)),
        gradient_tenths: (flags & 256 != 0).then(|| get_u16(bytes, 44) as i16),
        altitude: Altitude::from_code(get_u16(bytes, 46)),
        lap: LapStart::from_byte(bytes[LAP_BYTE]),
    })
}

/// A barometric altitude as the journal keeps it: FIT's `altitude` code,
/// (metres + 500) * 5, so 0.2 m steps from -500 m to about 12.6 km. Holding
/// the code itself means a sample encodes and decodes exactly, whoever made
/// it; 0xFFFF, which means none, cannot be held.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Altitude(u16);

impl Altitude {
    /// The nearest step to `altitude_dm` decimetres, clamped to the range.
    pub fn from_dm(altitude_dm: i32) -> Self {
        Self(((altitude_dm.saturating_add(1)).div_euclid(2) + 2500).clamp(0, 0xfffe) as u16)
    }

    fn from_code(code: u16) -> Option<Self> {
        (code != 0xffff).then_some(Self(code))
    }

    /// In decimetres: even, as the steps are 0.2 m.
    pub fn dm(self) -> i32 {
        (i32::from(self.0) - 2500) * 2
    }

    pub fn fit(self) -> u16 {
        self.0
    }
}

pub fn transport_checksum(bytes: &[u8]) -> u32 {
    crc32(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(active_ms: u64) -> Sample {
        Sample {
            active_ms,
            utc_ms: Some(1_789_000_000_123),
            location_e7: Some((-333_646_900, -705_155_800)),
            demo_speed_mm_s: Some(5_000),
            battery_percent: Some(87),
            speed_mm_s: Some(12345),
            power_watts: Some(215),
            gradient_tenths: Some(-37),
            ..Sample::default()
        }
    }

    #[test]
    fn events_and_bounded_samples_round_trip() {
        let event = Entry::event(Kind::Pause, Source::Demo, 4, 9, 12_345);
        let mut encoded = encode(&event);
        encoded.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
        assert_eq!(decode(&encoded), Some(event));

        let samples = [sample(1_000), sample(2_000), sample(3_000), sample(4_000)];
        let batch = Entry::batch(Source::Demo, 4, 10, &samples).unwrap();
        let mut encoded = encode(&batch);
        encoded.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
        assert_eq!(decode(&encoded), Some(batch));
        assert!(Entry::batch(Source::Demo, 1, 1, &[]).is_none());
        assert_eq!(sample(1).for_source(Source::Live).demo_speed_mm_s, None);
        assert_eq!(
            sample(1).for_source(Source::Demo).demo_speed_mm_s,
            Some(5_000)
        );
    }

    /// Frozen copy of the decoder that shipped before lap marks (with altitude): never update it.
    fn decode_before_lap_marks(slot: &Slot) -> Option<std::vec::Vec<Sample>> {
        let count = usize::from(slot.0[6]);
        let mut checked = slot.clone();
        checked.0[28..32].fill(0);
        if slot.0[..4] != *b"RIDE"
            || slot.0[4] != 1
            || !(1..=7).contains(&slot.0[5])
            || !(1..=2).contains(&slot.0[7])
            || count > 4
            || get_u16(&slot.0, 24) as usize != count * 48
            || slot.0[26..28] != [0, 0]
            || get_u32(&slot.0, 252) != 0x434f_4d54
            || crc32(&checked.0[..252]) != get_u32(&slot.0, 28)
            || (slot.0[5] == 2) != (count > 0)
        {
            return None;
        }
        let mut samples = std::vec::Vec::new();
        for index in 0..count {
            let bytes = &slot.0[32 + index * 48..][..48];
            let flags = get_u32(bytes, 16);
            if flags & !0x8000_01ff != 0 || flags & 4 != 0 && flags & 0x8000_0000 == 0 {
                return None;
            }
            if flags & 32 != 0 && bytes[36] > 100 {
                return None;
            }
            samples.push(Sample {
                active_ms: get_u64(bytes, 0),
                utc_ms: (flags & 1 != 0).then(|| get_u64(bytes, 8)),
                location_e7: (flags & 2 != 0)
                    .then(|| (get_u32(bytes, 20) as i32, get_u32(bytes, 24) as i32)),
                demo_speed_mm_s: (flags & 4 != 0).then(|| get_u32(bytes, 28)),
                heart_bpm: (flags & 8 != 0).then(|| get_u16(bytes, 32)),
                cadence_tenths: (flags & 16 != 0).then(|| get_u16(bytes, 34)),
                battery_percent: (flags & 32 != 0).then_some(bytes[36]),
                speed_mm_s: (flags & 64 != 0).then(|| get_u32(bytes, 40)),
                power_watts: (flags & 128 != 0).then(|| get_u16(bytes, 38)),
                gradient_tenths: (flags & 256 != 0).then(|| get_u16(bytes, 44) as i16),
                altitude: Altitude::from_code(get_u16(bytes, 46)),
                lap: None,
            });
        }
        if samples
            .last()
            .is_some_and(|s| s.active_ms != get_u64(&slot.0, 16))
        {
            return None;
        }
        Some(samples)
    }

    fn committed(entry: &Entry) -> Slot {
        let mut slot = encode(entry);
        slot.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
        slot
    }

    #[test]
    fn lap_marks_round_trip_and_older_decoders_read_every_sample() {
        let marks = [
            Some(LapStart::Ride),
            Some(LapStart::Manual),
            Some(LapStart::Distance),
            Some(LapStart::Step),
        ];
        let samples = marks.map(|lap| Sample {
            lap,
            ..sample(1_000)
        });
        let entry = Entry::batch(Source::Live, 2, 1, &samples).unwrap();
        let slot = committed(&entry);
        assert_eq!(decode(&slot), Some(entry), "the recorder's own check");

        let old = decode_before_lap_marks(&slot).expect("an older decoder accepts it");
        let unmarked: std::vec::Vec<Sample> =
            samples.iter().map(|s| Sample { lap: None, ..*s }).collect();
        assert_eq!(old, unmarked, "no sample lost or changed");

        let plain = committed(&Entry::batch(Source::Live, 2, 1, &unmarked).unwrap());
        let differ: std::vec::Vec<usize> = (0..SLOT_SIZE)
            .filter(|&i| slot.0[i] != plain.0[i])
            .filter(|i| !(28..32).contains(i))
            .collect();
        assert_eq!(differ, [32 + 37, 80 + 37, 128 + 37, 176 + 37]);

        let ride = [
            committed(&Entry::event(Kind::Start, Source::Live, 2, 0, 0)),
            slot,
            committed(&Entry::event(Kind::Finish, Source::Live, 2, 2, 1_000)),
        ];
        let mut scan = Scanner::default();
        for (index, slot) in ride.iter().enumerate() {
            assert!(decode_before_lap_marks(slot).is_some());
            scan.accept_slot(index, &slot.0);
        }
        let catalog = scan.catalog();
        assert_eq!((catalog.invalid_slots, catalog.completed), (0, 1));
        assert!(!catalog.summaries[0].unwrap().gap);
    }

    #[test]
    fn the_position_mark_round_trips_and_later_values_read_as_none() {
        let marked = Sample {
            lap: Some(LapStart::Position),
            ..sample(1_000)
        };
        let slot = committed(&Entry::batch(Source::Live, 2, 1, &[marked]).unwrap());
        assert_eq!(slot.0[32 + 37], 5);
        assert_eq!(
            decode(&slot).unwrap().samples[0].lap,
            Some(LapStart::Position)
        );
        assert!(
            decode_before_lap_marks(&slot).is_some(),
            "older decoders keep the sample"
        );
        for byte in 6..=254 {
            assert_eq!(LapStart::from_byte(byte), None, "{byte}");
        }
    }

    #[test]
    fn samples_written_before_lap_marks_have_none() {
        let entry = Entry::batch(Source::Live, 1, 1, &[sample(1_000)]).unwrap();
        let mut slot = committed(&entry);
        assert_eq!(slot.0[HEADER_SIZE + LAP_BYTE], 0xff);
        assert_eq!(decode(&slot).unwrap().samples[0].lap, None);
        slot.0[HEADER_SIZE + LAP_BYTE] = 0x42;
        let mut checked = slot.clone();
        checked.0[28..32].fill(0);
        put_u32(&mut slot.0, 28, crc32(&checked.0[..COMMIT_OFFSET]));
        let read = decode(&slot).unwrap();
        assert_eq!(read.samples[0].lap, None);
        assert_eq!(read.samples[0].power_watts, Some(215));
    }

    #[test]
    fn altitude_rides_in_the_spare_sample_bytes_without_a_flag() {
        let mut sample = sample(1_000);
        sample.altitude = Some(Altitude::from_dm(1_234));
        let encoded = encode(&Entry::batch(Source::Live, 1, 1, &[sample]).unwrap());
        assert_eq!(get_u16(&encoded.0, HEADER_SIZE + 46), 3_117);
        let mut committed = encoded.clone();
        committed.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
        let back = decode(&committed).unwrap();
        assert_eq!(back.samples[0].altitude.map(Altitude::dm), Some(1_234));
        assert_eq!(Altitude::from_dm(1_235).dm(), 1_236);
        assert_eq!(Altitude::from_dm(-5_000).dm(), -5_000);
        assert_eq!(Altitude::from_dm(-9_000).dm(), -5_000);
        assert_eq!(Altitude::from_dm(i32::MAX).fit(), 0xfffe);
        assert_eq!(Altitude::from_dm(i32::MIN).fit(), 0);
        for dm in [-9_000, -5_001, 0, 1, 1_235, 126_000, i32::MAX] {
            let mut odd = sample;
            odd.altitude = Some(Altitude::from_dm(dm));
            let mut slot = encode(&Entry::batch(Source::Live, 1, 1, &[odd]).unwrap());
            slot.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
            assert_eq!(decode(&slot).unwrap().samples[0], odd, "{dm}");
        }
        // No flag bit is used, so an older decoder accepts the sample.
        let flags = get_u32(&encoded.0, HEADER_SIZE + 16);
        assert_eq!(flags & !0x8000_01ff, 0);

        // A sample of the old layout, bytes 46..48 erased, has no altitude.
        let mut old = committed.clone();
        old.0[HEADER_SIZE + 46..HEADER_SIZE + 48].fill(0xff);
        assert_eq!(decode_sample(&old.0[HEADER_SIZE..]).unwrap().altitude, None);
    }

    #[test]
    fn torn_corrupt_unknown_and_malformed_records_are_occupied_invalid() {
        let entry = Entry::event(Kind::Start, Source::Demo, 1, 0, 0);
        let valid = encode(&entry);
        assert_eq!(decode(&valid), None);
        for offset in [0, 4, 5, 6, 7, 12, 28, 100, COMMIT_OFFSET] {
            let mut damaged = valid.clone();
            damaged.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
            damaged.0[offset] ^= 1;
            assert_eq!(decode(&damaged), None, "offset {offset}");
            assert!(!erased(&damaged.0));
        }

        let committed = commit_word();
        for prefix in 0..=3 {
            let mut torn = valid.clone();
            torn.0[COMMIT_OFFSET..COMMIT_OFFSET + prefix].copy_from_slice(&committed.0[..prefix]);
            assert_eq!(decode(&torn), None, "commit prefix {prefix}");
        }
        for prefix in [1, 31, 127, COMMIT_OFFSET - 1] {
            let mut torn = Slot([0xff; SLOT_SIZE]);
            torn.0[..prefix].copy_from_slice(&valid.0[..prefix]);
            assert_eq!(decode(&torn), None, "body prefix {prefix}");
        }
    }

    #[test]
    fn scanner_preserves_finished_and_recovers_last_committed_duration() {
        let mut sector = Sector([0xff; SECTOR_SIZE]);
        let entries = [
            Entry::event(Kind::Start, Source::Demo, 1, 0, 0),
            Entry::event(Kind::Finish, Source::Demo, 1, 1, 8_000),
            Entry::event(Kind::Start, Source::Demo, 2, 0, 0),
            Entry::batch(Source::Demo, 2, 1, &[sample(1_000), sample(2_000)]).unwrap(),
        ];
        for (index, entry) in entries.iter().enumerate() {
            let mut slot = encode(entry);
            slot.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
            sector.0[index * SLOT_SIZE..][..SLOT_SIZE].copy_from_slice(&slot.0);
        }
        sector.0[4 * SLOT_SIZE] = 0;
        let mut scan = Scanner::default();
        scan.accept(&sector);
        for _ in 1..SECTORS {
            scan.accept(&Sector([0xff; SECTOR_SIZE]));
        }
        let catalog = scan.finish().unwrap();
        assert_eq!(catalog.completed, 1);
        assert_eq!(catalog.next_slot, 5);
        assert_eq!(catalog.invalid_slots, 1);
        assert_eq!(catalog.recovery.unwrap().active_ms, 2_000);
        assert!(catalog.recovery.unwrap().gap);
    }

    #[test]
    fn scanner_does_not_call_an_interrupted_ride_completed() {
        let mut sector = Sector([0xff; SECTOR_SIZE]);
        for (index, source) in [Source::Live, Source::Demo].into_iter().enumerate() {
            let mut slot = encode(&Entry::event(Kind::Start, source, index as u32 + 1, 0, 0));
            slot.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
            sector.0[index * SLOT_SIZE..][..SLOT_SIZE].copy_from_slice(&slot.0);
        }
        let mut scan = Scanner::default();
        scan.accept(&sector);
        for _ in 1..SECTORS {
            scan.accept(&Sector([0xff; SECTOR_SIZE]));
        }
        let catalog = scan.finish().unwrap();
        assert_eq!(catalog.completed, 0);
        assert_eq!(catalog.recovery.unwrap().source, Source::Demo);
    }

    #[test]
    fn history_is_empty_or_keeps_four_latest_complete_summaries() {
        let mut empty = Scanner::default();
        for _ in 0..SECTORS {
            empty.accept(&Sector([0xff; SECTOR_SIZE]));
        }
        assert_eq!(empty.finish().unwrap().summary_count, 0);

        let mut sector = Sector([0xff; SECTOR_SIZE]);
        let mut index = 0;
        for ride_id in 1..=5 {
            let source = if ride_id == 5 {
                Source::Live
            } else {
                Source::Demo
            };
            let mut value = sample(60_000 + u64::from(ride_id) * 1_000).for_source(source);
            value.utc_ms = Some(1_789_000_000_000 + u64::from(ride_id));
            value.location_e7 = (ride_id == 5).then_some((-333_646_900, -705_155_800));
            value.heart_bpm = (ride_id == 5).then_some(72);
            value.cadence_tenths = (ride_id == 5).then_some(615);
            let entries = [
                Entry::event(Kind::Start, source, ride_id, 0, 0),
                Entry::batch(source, ride_id, 1, &[value]).unwrap(),
                Entry::event(
                    if ride_id == 4 {
                        Kind::Recovered
                    } else {
                        Kind::Finish
                    },
                    source,
                    ride_id,
                    2,
                    value.active_ms,
                ),
            ];
            for entry in entries {
                let mut slot = encode(&entry);
                slot.0[COMMIT_OFFSET..].copy_from_slice(&commit_word().0);
                sector.0[index * SLOT_SIZE..][..SLOT_SIZE].copy_from_slice(&slot.0);
                index += 1;
            }
        }
        sector.0[index * SLOT_SIZE] = 0;
        let mut scan = Scanner::default();
        scan.accept(&sector);
        for _ in 1..SECTORS {
            scan.accept(&Sector([0xff; SECTOR_SIZE]));
        }
        let catalog = scan.finish().unwrap();
        assert_eq!(catalog.completed, 5);
        assert_eq!(catalog.invalid_slots, 1);
        assert_eq!(catalog.summary_count, 4);
        assert_eq!(catalog.summaries[0].unwrap().ride_id, 2);
        assert!(catalog.summaries[2].unwrap().recovered);
        let live = catalog.summaries[3].unwrap();
        assert_eq!(live.source, Some(Source::Live));
        // One sample 65 s in: the first covers at most a second at 12.3 m/s.
        assert_eq!(live.distance_mm, Some(12_345));
        assert_eq!(live.gps_samples, 1);
        assert_eq!(live.heart_average, Some(72));
        assert_eq!(live.cadence_average, Some(615));
        assert_eq!(live.first_utc_ms, Some(1_789_000_000_005));
    }

    #[test]
    fn a_ride_adds_up_its_recorded_speed_wheel_or_gps() {
        let at = |active_ms: u64, speed: Option<u32>| Sample {
            active_ms,
            speed_mm_s: speed,
            ..Sample::default()
        };
        let mut summary = Summary::started(1, Source::Live);
        // No speed yet: no distance, not zero.
        summary.add_sample(at(1_000, None));
        assert_eq!(summary.distance_mm, None);
        // 1 s steps at 10 m/s, a stop, then 1 s at 5 m/s.
        for second in 2..=4 {
            summary.add_sample(at(second * 1_000, Some(10_000)));
        }
        summary.add_sample(at(5_000, Some(0)));
        summary.add_sample(at(6_000, Some(5_000)));
        assert_eq!(summary.distance_mm, Some(35_000));
        // A hole in the log is ridden for the replay's longest step only.
        summary.add_sample(at(106_000, Some(10_000)));
        assert_eq!(summary.distance_mm, Some(65_000));
        summary.finish(Kind::Finish, 106_000, false);
        assert_eq!(summary.distance_mm, Some(65_000));
        // It survives a checkpoint and keeps adding.
        let mut bytes = [0; Summary::ENCODED];
        summary.encode(&mut bytes);
        let mut summary = Summary::decode(&bytes).unwrap();
        summary.add_sample(at(107_000, Some(10_000)));
        assert_eq!(summary.distance_mm, Some(75_000));
    }

    #[test]
    fn occupied_final_slot_is_reported_at_the_exact_capacity_boundary() {
        let mut scan = Scanner::default();
        for sector_index in 0..SECTORS {
            let mut sector = Sector([0xff; SECTOR_SIZE]);
            if sector_index == SECTORS - 1 {
                sector.0[(SLOTS_PER_SECTOR - 1) * SLOT_SIZE] = 0;
            }
            scan.accept(&sector);
        }
        let catalog = scan.finish().unwrap();
        assert_eq!(catalog.next_slot, SLOTS);
        assert_eq!(catalog.invalid_slots, 1);
    }

    #[test]
    fn accepted_pause_boundary_does_not_include_later_service_time() {
        assert_eq!(active_at(4_000, 10_000, Some(10_999), 11_001), 4_999);
        assert_eq!(active_at(4_000, 10_000, None, 11_001), 5_001);
        assert_eq!(active_at(4_000, 12_000, Some(10_999), 11_001), 4_000);
        let frozen = freeze_at(4_000, 10_000, None, 11_001);
        assert_eq!(frozen, (5_001, 11_001, Some(11_001)));
        assert_eq!(active_at(frozen.0, frozen.1, frozen.2, 20_000), 5_001);
    }
}
