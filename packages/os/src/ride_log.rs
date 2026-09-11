//! Fixed-slot, append-only ride records with commit-last recovery.

pub const REGION_SIZE: usize = 1024 * 1024;
pub const SECTOR_SIZE: usize = 4096;
pub const SLOT_SIZE: usize = 256;
pub const SLOTS_PER_SECTOR: usize = SECTOR_SIZE / SLOT_SIZE;
pub const SECTORS: usize = REGION_SIZE / SECTOR_SIZE;
pub const SLOTS: usize = REGION_SIZE / SLOT_SIZE;
pub const SAMPLES_PER_BATCH: usize = 4;

const MAGIC: [u8; 4] = *b"RIDE";
const VERSION: u8 = 1;
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
    Ready,
    Recording,
    Paused,
    Saved,
    Recovered,
    Full,
    Error,
}

impl Status {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Scanning => "scanning",
            Self::NeedsInit => "needs_init",
            Self::Formatting => "formatting",
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
            Self::Ready => b"READY",
            Self::Recording => b"REC",
            Self::Paused => b"PAUSE",
            Self::Saved => b"SAVED",
            Self::Recovered => b"RECOV",
            Self::Full => b"FULL",
            Self::Error => b"ERROR",
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

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Sample {
    pub active_ms: u64,
    pub utc_ms: Option<u64>,
    pub location_e7: Option<(i32, i32)>,
    pub demo_speed_mm_s: Option<u32>,
    pub heart_bpm: Option<u16>,
    pub cadence_tenths: Option<u16>,
    pub battery_percent: Option<u8>,
}

impl Sample {
    pub fn for_source(mut self, source: Source) -> Self {
        if source == Source::Live {
            self.demo_speed_mm_s = None;
        }
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
    let crc = checksum(&slot.0[..COMMIT_OFFSET]);
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
    if checksum(&checked.0[..COMMIT_OFFSET]) != stored_checksum {
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
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Catalog {
    pub next_slot: usize,
    pub next_ride_id: u32,
    pub completed: u16,
    pub invalid_slots: u16,
    pub valid_slots: u16,
    pub recovery: Option<Recovery>,
}

#[derive(Default)]
pub struct Scanner {
    sector: usize,
    next_slot: usize,
    completed: u16,
    invalid_slots: u16,
    valid_slots: u16,
    open: Option<Recovery>,
    max_ride_id: u32,
}

impl Scanner {
    pub fn next_sector(&self) -> Option<usize> {
        (self.sector < SECTORS).then_some(self.sector)
    }

    pub fn accept(&mut self, sector: &Sector) {
        let base = self.sector * SLOTS_PER_SECTOR;
        for index in 0..SLOTS_PER_SECTOR {
            let bytes = &sector.0[index * SLOT_SIZE..][..SLOT_SIZE];
            if erased(bytes) {
                continue;
            }
            self.next_slot = base + index + 1;
            let mut slot = Slot::default();
            slot.0.copy_from_slice(bytes);
            let Some(entry) = decode(&slot) else {
                self.invalid_slots = self.invalid_slots.saturating_add(1);
                if let Some(open) = self.open.as_mut() {
                    open.gap = true;
                }
                continue;
            };
            self.apply(entry);
        }
        self.sector += 1;
    }

    pub fn finish(&self) -> Option<Catalog> {
        (self.sector == SECTORS).then_some(Catalog {
            next_slot: self.next_slot,
            next_ride_id: self.max_ride_id.wrapping_add(1).max(1),
            completed: self.completed,
            invalid_slots: self.invalid_slots,
            valid_slots: self.valid_slots,
            recovery: self.open,
        })
    }

    fn apply(&mut self, entry: Entry) {
        self.valid_slots = self.valid_slots.saturating_add(1);
        self.max_ride_id = self.max_ride_id.max(entry.ride_id);
        match entry.kind {
            Kind::Start => {
                self.open = Some(Recovery {
                    ride_id: entry.ride_id,
                    source: entry.source,
                    next_sequence: entry.sequence.wrapping_add(1),
                    active_ms: entry.active_ms,
                    gap: false,
                });
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
            }
            Kind::Finish | Kind::Recovered | Kind::Full => {
                let Some(open) = self.open else { return };
                if entry.ride_id == open.ride_id
                    && entry.source == open.source
                    && entry.sequence == open.next_sequence
                {
                    self.completed = self.completed.saturating_add(1);
                    self.open = None;
                } else if let Some(open) = self.open.as_mut() {
                    open.gap = true;
                }
            }
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
    put_u32(bytes, 16, flags);
}

fn decode_sample(bytes: &[u8]) -> Option<Sample> {
    let flags = get_u32(bytes, 16);
    if flags & !(0x8000_003f) != 0 || flags & 4 != 0 && flags & 0x8000_0000 == 0 {
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
    })
}

fn checksum(bytes: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

fn get_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn get_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn get_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}
fn put_u16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}
fn put_u32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
fn put_u64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
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
