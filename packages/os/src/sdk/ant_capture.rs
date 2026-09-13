//! Append-only raw ANT capture in the owned ride reservation. Never erases.
//!
//! Each 256-byte slot is independently exportable, little-endian throughout:
//! 0..4 `ANT1`, 4 version=1, 5 kind (1 packets, 2 stopped, 3 full, 4 legacy link,
//! 5 typed link, 6 position), 6 count
//! (0..8), 7 reserved=0, 8..12 capture ID (first slot index), 12..16 sequence,
//! 16..24 batch preparation uptime in ms. Eight 28-byte packet records begin at
//! 24: type u8, transmission type u8, device number u16, received uptime u64,
//! generation u32, loss count u32, raw page [u8;8]. Unused records remain FF.
//! Kind 4 holds one link record at24: state u8 (0 idle,1 connecting,2 connected,
//! 3 disconnecting,4 disconnected,5 timed_out,6 transport_lost), bytes25..28 zero,
//! 28..36 observation uptime u64,36..40 generation u32,40..44 capture dropped
//! packets u32,44..48 dropped link observations u32; remaining bytes FF.
//! Kind 5 uses the same link layout, with device type at24, state at25 and
//! bytes26..28 zero. Kind 4 remains readable in existing captures.
//! Terminal kinds2/3 store these two capture drop counters at24..28/28..32.
//! Kind 6 holds flags at24 (bit0 source observation present, bit1 fresh fix present),
//! bytes25..28 zero, sample uptime u64 at28, source observation uptime u64 at36,
//! latitude/longitude i32 e7 at44/48, dropped positions u32 at52. Absent values
//! are zero. Terminal records add `GPS1` at32 and dropped positions u32 at36;
//! older terminal records have FF here. GPS records do not count as ANT packets.
//! Counters after the last link/terminal record may be lost on power failure.
//! Header count is1 for link records; they do not contribute to packet counts.
//! 248..252 CRC32/ISO-HDLC over bytes 0..248; 252..256 is the separately written
//! ride_log COMT commit word. Only committed CRC-valid slots are records. Power
//! loss may leave occupied invalid slots; the next capture scans past all of them.
//!
//! The caller must exclude ride recording/reclaim/export mutations throughout
//! scanning and capture. Existing RIDE and unknown occupied slots are preserved.

use super::{
    ride_log::{self, Sector, Slot},
    storage::RideStorage,
};
use crate::ant::{LinkState, Packet};

pub const PACKETS_PER_SLOT: usize = 8;
// Preflight allowance for roughly 20 packets/s over ten minutes, with room for
// link records, partial batches and 600 one-second GPS observations. Actual duration depends on incoming traffic
// and flush cadence; callers must report the remaining append-only tail.
pub const REQUIRED_SLOTS: usize = 2400;
const BUFFERED_PACKETS: usize = 16;
const FLUSH_MS: u64 = 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Idle,
    Scanning,
    Ready,
    Recording,
    Stopping,
    Stopped,
    Full,
    Error,
}

impl Status {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Idle => "idle",
            Self::Scanning => "scanning",
            Self::Ready => "ready",
            Self::Recording => "recording",
            Self::Stopping => "stopping",
            Self::Stopped => "stopped",
            Self::Full => "full",
            Self::Error => "error",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Error {
    Read,
    Occupied,
    Write,
    Commit,
    Verify,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub status: Status,
    pub scanned: usize,
    pub first_slot: Option<usize>,
    pub next_slot: usize,
    pub committed: u32,
    pub packets: u32,
    pub dropped: u32,
    pub dropped_links: u32,
    pub saved_positions: u32,
    pub dropped_positions: u32,
    pub error: Option<Error>,
    /// True only after a packet batch has been committed and read back.
    pub recording: bool,
    pub last_commit_ms: Option<u64>,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Step {
    Idle,
    Check,
    Write,
    Commit,
    Verify,
}

#[derive(Clone, Copy)]
struct LinkRecord {
    device_type: u8,
    state: LinkState,
    generation: u32,
    now: u64,
}

/// Coordinates are a fresh valid fix supplied by the caller, or both None.
/// `now` is the capture sample uptime. `observed_ms` preserves the last source
/// fix observation uptime even for a stale/no-fix sample.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PositionRecord {
    pub now: u64,
    pub observed_ms: Option<u64>,
    pub latitude_e7: Option<i32>,
    pub longitude_e7: Option<i32>,
}

pub struct Capturer {
    status: Status,
    scanned: usize,
    first_slot: Option<usize>,
    next_slot: usize,
    committed: u32,
    packets: u32,
    dropped: u32,
    dropped_links: u32,
    saved_positions: u32,
    dropped_positions: u32,
    position: Option<PositionRecord>,
    next_source: usize,
    links: [Option<LinkRecord>; 16],
    link_count: usize,
    error: Option<Error>,
    last_commit_ms: Option<u64>,
    buffered: [Option<Packet>; BUFFERED_PACKETS],
    count: usize,
    pending: Slot,
    step: Step,
    stopping: bool,
}

impl Default for Capturer {
    fn default() -> Self {
        Self::new()
    }
}

impl Capturer {
    pub const fn new() -> Self {
        Self {
            status: Status::Idle,
            scanned: 0,
            first_slot: None,
            next_slot: 0,
            committed: 0,
            packets: 0,
            dropped: 0,
            dropped_links: 0,
            saved_positions: 0,
            dropped_positions: 0,
            position: None,
            next_source: 0,
            links: [None; 16],
            link_count: 0,
            error: None,
            last_commit_ms: None,
            buffered: [None; BUFFERED_PACKETS],
            count: 0,
            pending: Slot([0xff; ride_log::SLOT_SIZE]),
            step: Step::Idle,
            stopping: false,
        }
    }

    pub fn start(&mut self) -> bool {
        // An uncertain media failure requires a new instance and full rescan.
        if !matches!(self.status, Status::Idle | Status::Stopped | Status::Full) {
            return false;
        }
        *self = Self::new();
        self.status = Status::Scanning;
        true
    }

    pub fn stop(&mut self) {
        match self.status {
            Status::Scanning => self.status = Status::Stopped,
            Status::Ready | Status::Recording => {
                self.stopping = true;
                self.status = Status::Stopping;
            }
            _ => {}
        }
    }

    pub fn packet(&mut self, packet: Packet) {
        if self.status == Status::Scanning {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        if !matches!(self.status, Status::Ready | Status::Recording) {
            return;
        }
        if self.count == BUFFERED_PACKETS {
            self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.buffered[self.count] = Some(packet);
        self.count += 1;
    }

    pub fn observe_link(&mut self, device_type: u8, state: LinkState, generation: u32, now: u64) {
        if !matches!(
            self.status,
            Status::Scanning | Status::Ready | Status::Recording
        ) {
            return;
        }
        if self.link_count == self.links.len() {
            self.dropped_links = self.dropped_links.saturating_add(1);
            return;
        }
        self.links[self.link_count] = Some(LinkRecord {
            device_type,
            state,
            generation,
            now,
        });
        self.link_count += 1;
    }

    /// Keep the latest one-second observation; count every replaced sample.
    pub fn position(&mut self, position: PositionRecord) {
        if self.status == Status::Scanning {
            self.dropped_positions = self.dropped_positions.saturating_add(1);
            return;
        }
        if !matches!(self.status, Status::Ready | Status::Recording) {
            return;
        }
        if self.position.replace(position).is_some() {
            self.dropped_positions = self.dropped_positions.saturating_add(1);
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            status: self.status,
            scanned: self.scanned,
            first_slot: self.first_slot,
            next_slot: self.next_slot,
            committed: self.committed,
            packets: self.packets,
            dropped: self.dropped,
            dropped_links: self.dropped_links,
            saved_positions: self.saved_positions,
            dropped_positions: self.dropped_positions,
            error: self.error,
            recording: self.status == Status::Recording && self.packets != 0,
            last_commit_ms: self.last_commit_ms,
        }
    }

    /// At most one storage operation per call; scanning reads one 4 KiB sector.
    pub fn service(&mut self, store: &mut impl RideStorage, now: u64) {
        if self.status == Status::Scanning {
            let mut sector = Sector::default();
            if store.ride_read_sector(self.scanned, &mut sector).is_err() {
                self.fail(Error::Read);
                return;
            }
            for (within, slot) in sector
                .0
                .as_chunks::<{ ride_log::SLOT_SIZE }>()
                .0
                .iter()
                .enumerate()
            {
                if !ride_log::erased(slot) {
                    self.next_slot = self.scanned * ride_log::SLOTS_PER_SECTOR + within + 1;
                }
            }
            self.scanned += 1;
            if self.scanned == ride_log::SECTORS {
                self.first_slot = Some(self.next_slot);
                self.status = if ride_log::SLOTS - self.next_slot < REQUIRED_SLOTS {
                    Status::Full
                } else {
                    Status::Ready
                };
            }
            return;
        }
        if !matches!(
            self.status,
            Status::Ready | Status::Recording | Status::Stopping
        ) {
            return;
        }
        if self.step == Step::Idle {
            if self.next_slot >= ride_log::SLOTS - 1 {
                self.stopping = true;
                self.status = Status::Stopping;
                self.discard_buffer();
                self.dropped_links = self.dropped_links.saturating_add(self.link_count as u32);
                self.links.fill(None);
                self.link_count = 0;
                self.discard_position();
                self.prepare(3, now);
            } else {
                let packets_ready = self.count != 0
                    && (self.count >= PACKETS_PER_SLOT
                        || self.stopping
                        || self.buffered[0].is_some_and(|packet| {
                            now.saturating_sub(packet.received_ms) >= FLUSH_MS
                        }));
                // Round-robin ready sources so link churn cannot starve packets
                // or GPS, and a sustained packet stream cannot starve GPS.
                let ready = [self.link_count != 0, packets_ready, self.position.is_some()];
                if let Some(source) = (0..3)
                    .map(|offset| (self.next_source + offset) % 3)
                    .find(|&source| ready[source])
                {
                    self.next_source = (source + 1) % 3;
                    self.prepare([5, 1, 6][source], now);
                } else if self.stopping {
                    self.prepare(2, now);
                } else {
                    return;
                }
            }
        }
        match self.step {
            Step::Idle => {}
            Step::Check => {
                let mut actual = Slot::default();
                if store.ride_read_slot(self.next_slot, &mut actual).is_err() {
                    self.fail(Error::Read);
                } else if !ride_log::erased(&actual.0) {
                    self.fail(Error::Occupied);
                } else {
                    self.step = Step::Write;
                }
            }
            Step::Write => {
                if store
                    .ride_write_slot(self.next_slot, &self.pending)
                    .is_err()
                {
                    self.fail(Error::Write);
                } else {
                    self.step = Step::Commit;
                }
            }
            Step::Commit => {
                if store
                    .ride_commit_slot(self.next_slot, &ride_log::commit_word())
                    .is_err()
                {
                    self.fail(Error::Commit);
                } else {
                    self.step = Step::Verify;
                }
            }
            Step::Verify => {
                let mut actual = Slot::default();
                if store.ride_read_slot(self.next_slot, &mut actual).is_err() {
                    self.fail(Error::Read);
                    return;
                }
                if actual.0[..252] != self.pending.0[..252]
                    || actual.0[252..] != ride_log::commit_word().0
                {
                    self.fail(Error::Verify);
                    return;
                }
                self.next_slot += 1;
                self.committed = self.committed.saturating_add(1);
                if self.pending.0[5] == 1 {
                    self.packets = self.packets.saturating_add(u32::from(self.pending.0[6]));
                }
                if self.pending.0[5] == 6 {
                    self.saved_positions = self.saved_positions.saturating_add(1);
                }
                self.last_commit_ms = Some(now);
                self.step = Step::Idle;
                self.status = match self.pending.0[5] {
                    2 => Status::Stopped,
                    3 => Status::Full,
                    _ if self.stopping => Status::Stopping,
                    _ if self.packets != 0 => Status::Recording,
                    _ => Status::Ready,
                };
            }
        }
    }

    fn prepare(&mut self, kind: u8, now: u64) {
        self.pending.0.fill(0xff);
        let bytes = &mut self.pending.0;
        bytes[..4].copy_from_slice(b"ANT1");
        bytes[4] = 1;
        bytes[5] = kind;
        bytes[6] = match kind {
            1 => self.count.min(PACKETS_PER_SLOT) as u8,
            5 | 6 => 1,
            _ => 0,
        };
        bytes[7] = 0;
        bytes[8..12]
            .copy_from_slice(&(self.first_slot.unwrap_or(self.next_slot) as u32).to_le_bytes());
        bytes[12..16].copy_from_slice(&self.committed.to_le_bytes());
        bytes[16..24].copy_from_slice(&now.to_le_bytes());
        if kind == 5 {
            let link = self.links[0].unwrap();
            self.links.rotate_left(1);
            self.link_count -= 1;
            self.links[self.link_count] = None;
            bytes[24] = link.device_type;
            bytes[25] = match link.state {
                LinkState::Idle => 0,
                LinkState::Connecting => 1,
                LinkState::Connected => 2,
                LinkState::Disconnecting => 3,
                LinkState::Disconnected => 4,
                LinkState::TimedOut => 5,
                LinkState::TransportLost => 6,
            };
            bytes[26..28].fill(0);
            bytes[28..36].copy_from_slice(&link.now.to_le_bytes());
            bytes[36..40].copy_from_slice(&link.generation.to_le_bytes());
            bytes[40..44].copy_from_slice(&self.dropped.to_le_bytes());
            bytes[44..48].copy_from_slice(&self.dropped_links.to_le_bytes());
        } else if kind == 6 {
            let position = self.position.take().unwrap();
            let coordinates = position.latitude_e7.zip(position.longitude_e7);
            bytes[24] =
                u8::from(position.observed_ms.is_some()) | (u8::from(coordinates.is_some()) << 1);
            bytes[25..28].fill(0);
            bytes[28..36].copy_from_slice(&position.now.to_le_bytes());
            bytes[36..44].copy_from_slice(&position.observed_ms.unwrap_or(0).to_le_bytes());
            let (latitude, longitude) = coordinates.unwrap_or((0, 0));
            bytes[44..48].copy_from_slice(&latitude.to_le_bytes());
            bytes[48..52].copy_from_slice(&longitude.to_le_bytes());
            bytes[52..56].copy_from_slice(&self.dropped_positions.to_le_bytes());
        } else if matches!(kind, 2 | 3) {
            bytes[24..28].copy_from_slice(&self.dropped.to_le_bytes());
            bytes[28..32].copy_from_slice(&self.dropped_links.to_le_bytes());
            bytes[32..36].copy_from_slice(b"GPS1");
            bytes[36..40].copy_from_slice(&self.dropped_positions.to_le_bytes());
        }
        for (index, packet) in self.buffered[..if kind == 1 {
            self.count.min(PACKETS_PER_SLOT)
        } else {
            0
        }]
            .iter()
            .enumerate()
        {
            let packet = packet.unwrap();
            let record = &mut bytes[24 + index * 28..][..28];
            record[0] = packet.identity.device_type;
            record[1] = packet.identity.transmission_type;
            record[2..4].copy_from_slice(&packet.identity.device_number.to_le_bytes());
            record[4..12].copy_from_slice(&packet.received_ms.to_le_bytes());
            record[12..16].copy_from_slice(&packet.generation.to_le_bytes());
            record[16..20].copy_from_slice(&packet.loss_count.to_le_bytes());
            record[20..28].copy_from_slice(&packet.data);
        }
        let crc = ride_log::transport_checksum(&bytes[..248]);
        bytes[248..252].copy_from_slice(&crc.to_le_bytes());
        if kind == 1 {
            let written = usize::from(bytes[6]);
            self.buffered.rotate_left(written);
            self.count -= written;
            self.buffered[self.count..].fill(None);
        }
        self.step = Step::Check;
    }

    fn discard_buffer(&mut self) {
        self.dropped = self.dropped.saturating_add(self.count as u32);
        self.buffered.fill(None);
        self.count = 0;
    }

    fn discard_position(&mut self) {
        if self.position.take().is_some() {
            self.dropped_positions = self.dropped_positions.saturating_add(1);
        }
    }

    fn fail(&mut self, error: Error) {
        if self.step != Step::Idle {
            match self.pending.0[5] {
                1 => self.dropped = self.dropped.saturating_add(u32::from(self.pending.0[6])),
                5 => self.dropped_links = self.dropped_links.saturating_add(1),
                6 => self.dropped_positions = self.dropped_positions.saturating_add(1),
                _ => {}
            }
        }
        self.discard_buffer();
        self.discard_position();
        self.dropped_links = self.dropped_links.saturating_add(self.link_count as u32);
        self.links.fill(None);
        self.link_count = 0;
        self.step = Step::Idle;
        self.error = Some(error);
        self.status = Status::Error;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ant::Identity;
    use std::vec;
    use std::vec::Vec;

    struct Memory {
        bytes: Vec<u8>,
        writes: usize,
        calls: usize,
        fail_body: bool,
        fail_commit: bool,
        corrupt_readback: bool,
    }
    impl Memory {
        fn new() -> Self {
            Self {
                bytes: vec![0xff; ride_log::REGION_SIZE],
                writes: 0,
                calls: 0,
                fail_body: false,
                fail_commit: false,
                corrupt_readback: false,
            }
        }
    }
    impl RideStorage for Memory {
        type Error = ();
        fn ride_read_sector(&mut self, sector: usize, output: &mut Sector) -> Result<(), ()> {
            self.calls += 1;
            output.0.copy_from_slice(
                &self.bytes[sector * ride_log::SECTOR_SIZE..][..ride_log::SECTOR_SIZE],
            );
            Ok(())
        }
        fn ride_read_slot(&mut self, slot: usize, output: &mut Slot) -> Result<(), ()> {
            self.calls += 1;
            output
                .0
                .copy_from_slice(&self.bytes[slot * ride_log::SLOT_SIZE..][..ride_log::SLOT_SIZE]);
            if self.corrupt_readback && self.writes > 0 {
                output.0[42] ^= 1;
            }
            Ok(())
        }
        fn ride_erase_sector(&mut self, _: usize) -> Result<(), ()> {
            panic!("capture must never erase");
        }
        fn ride_write_slot(&mut self, slot: usize, data: &Slot) -> Result<(), ()> {
            self.calls += 1;
            self.writes += 1;
            let target = &mut self.bytes[slot * ride_log::SLOT_SIZE..][..ride_log::SLOT_SIZE];
            assert!(ride_log::erased(target));
            if self.fail_body {
                target[..32].copy_from_slice(&data.0[..32]);
                return Err(());
            }
            target.copy_from_slice(&data.0);
            Ok(())
        }
        fn ride_commit_slot(&mut self, slot: usize, commit: &ride_log::Commit) -> Result<(), ()> {
            self.calls += 1;
            if self.fail_commit {
                return Err(());
            }
            self.bytes[slot * ride_log::SLOT_SIZE + 252..][..4].copy_from_slice(&commit.0);
            Ok(())
        }
    }
    fn packet(number: u8) -> Packet {
        Packet {
            identity: Identity {
                device_type: 40,
                device_number: 0x1234,
                transmission_type: 5,
            },
            data: [number; 8],
            received_ms: 100,
            generation: 7,
            loss_count: 2,
        }
    }
    fn scan(capture: &mut Capturer, media: &mut Memory) {
        assert!(capture.start());
        for _ in 0..ride_log::SECTORS {
            let before = media.calls;
            capture.service(media, 0);
            assert_eq!(media.calls, before + 1);
        }
    }
    fn flush(capture: &mut Capturer, media: &mut Memory) {
        for _ in 0..4 {
            let before = media.calls;
            capture.service(media, 2_000);
            assert!(media.calls <= before + 1);
        }
    }

    #[test]
    fn preserves_all_occupied_slots_and_verifies_before_recording() {
        let mut media = Memory::new();
        media.bytes[..4].copy_from_slice(b"RIDE");
        media.bytes[40 * ride_log::SLOT_SIZE + 251] = 0;
        let prefix = media.bytes[..41 * ride_log::SLOT_SIZE].to_vec();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        assert_eq!(capture.snapshot().first_slot, Some(41));
        assert!(!capture.snapshot().recording);
        for n in 0..8 {
            capture.packet(packet(n));
        }
        for _ in 0..3 {
            capture.service(&mut media, 2_000);
            assert!(!capture.snapshot().recording);
        }
        capture.service(&mut media, 2_000);
        assert!(capture.snapshot().recording);
        assert_eq!(capture.snapshot().packets, 8);
        assert_eq!(&media.bytes[..prefix.len()], &prefix);
        let bytes = &media.bytes[41 * ride_log::SLOT_SIZE..][..256];
        assert_eq!(&bytes[..8], b"ANT1\x01\x01\x08\x00");
        assert_eq!(&bytes[24..28], &[40, 5, 0x34, 0x12]);
        assert_eq!(&bytes[44..52], &[0; 8]);
        assert_eq!(
            u32::from_le_bytes(bytes[248..252].try_into().unwrap()),
            ride_log::transport_checksum(&bytes[..248])
        );
        assert_eq!(bytes[252..], ride_log::commit_word().0);
        capture.packet(packet(9));
        capture.stop();
        flush(&mut capture, &mut media);
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().status, Status::Stopped);
        assert_eq!(capture.snapshot().packets, 9);
        assert_eq!(capture.snapshot().committed, 3);
        assert_eq!(media.bytes[43 * ride_log::SLOT_SIZE + 5], 2);
    }

    #[test]
    fn interrupted_body_and_commit_fail_closed_and_rescan_past_torn_slot() {
        for fail_body in [true, false] {
            let mut media = Memory::new();
            let mut capture = Capturer::new();
            scan(&mut capture, &mut media);
            media.fail_body = fail_body;
            media.fail_commit = !fail_body;
            capture.packet(packet(1));
            flush(&mut capture, &mut media);
            assert_eq!(capture.snapshot().status, Status::Error);
            assert!(!capture.start());
            assert_eq!(capture.snapshot().committed, 0);
            assert!(ride_log::erased(&media.bytes[252..256]));
            let calls = media.calls;
            capture.service(&mut media, 3_000);
            assert_eq!(media.calls, calls);
            let mut recovered = Capturer::new();
            scan(&mut recovered, &mut media);
            assert_eq!(recovered.snapshot().first_slot, Some(1));
        }
    }

    #[test]
    fn insufficient_tail_and_new_occupancy_never_write() {
        let mut media = Memory::new();
        media.bytes[(ride_log::SLOTS - REQUIRED_SLOTS) * 256] = 0;
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        assert_eq!(capture.snapshot().status, Status::Full);
        capture.packet(packet(1));
        flush(&mut capture, &mut media);
        assert_eq!(media.writes, 0);
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        media.bytes[0] = 0;
        capture.packet(packet(1));
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().error, Some(Error::Occupied));
        assert_eq!(media.writes, 0);
    }

    #[test]
    fn readback_mismatch_never_claims_durable_data() {
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        media.corrupt_readback = true;
        capture.packet(packet(1));
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().error, Some(Error::Verify));
        assert_eq!(capture.snapshot().packets, 0);
        assert!(!capture.snapshot().recording);
        assert_eq!(capture.snapshot().dropped, 1);
    }

    #[test]
    fn buffers_three_sensor_bursts_while_an_eight_packet_batch_is_pending() {
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        for n in 0..8 {
            capture.packet(packet(n));
        }
        capture.service(&mut media, 200);
        for n in 8..24 {
            let mut sample = packet(n);
            sample.identity.device_type = [40, 120, 11][usize::from(n) % 3];
            sample.identity.device_number = u16::from(n);
            capture.packet(sample);
        }
        capture.stop();
        for _ in 0..20 {
            capture.service(&mut media, 2_000);
        }
        assert_eq!(capture.snapshot().status, Status::Stopped);
        assert_eq!(capture.snapshot().packets, 24);
        assert_eq!(capture.snapshot().dropped, 0);
        for n in 0..24usize {
            let start = (n / 8) * 256 + 24 + (n % 8) * 28;
            assert_eq!(media.bytes[start + 20], n as u8);
            if n >= 8 {
                assert_eq!(media.bytes[start], [40, 120, 11][n % 3]);
                assert_eq!(
                    u16::from_le_bytes(media.bytes[start + 2..start + 4].try_into().unwrap()),
                    n as u16
                );
            }
        }
    }

    fn position(now: u64) -> PositionRecord {
        PositionRecord {
            now,
            observed_ms: Some(now.saturating_sub(10)),
            latitude_e7: Some(-334567890),
            longitude_e7: Some(-706543210),
        }
    }

    #[test]
    fn positions_preserve_fix_gaps_and_count_replaced_and_failed_samples() {
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        capture.position(position(100));
        capture.position(position(200));
        assert_eq!(capture.snapshot().dropped_positions, 1);
        for _ in 0..3 {
            capture.service(&mut media, 200);
            assert_eq!(capture.snapshot().saved_positions, 0);
        }
        capture.service(&mut media, 200);
        assert_eq!(capture.snapshot().saved_positions, 1);
        assert_eq!(capture.snapshot().packets, 0);
        assert!(!capture.snapshot().recording);
        assert_eq!(&media.bytes[5..7], &[6, 1]);
        assert_eq!(media.bytes[24], 3);
        assert_eq!(
            u64::from_le_bytes(media.bytes[28..36].try_into().unwrap()),
            200
        );
        assert_eq!(
            i32::from_le_bytes(media.bytes[44..48].try_into().unwrap()),
            -334567890
        );
        let mut gap = position(300);
        gap.latitude_e7 = None;
        gap.longitude_e7 = None;
        capture.position(gap);
        flush(&mut capture, &mut media);
        assert_eq!(media.bytes[256 + 24], 1);
        assert_eq!(&media.bytes[256 + 44..256 + 52], &[0; 8]);
        capture.position(position(400));
        media.fail_commit = true;
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().status, Status::Error);
        assert_eq!(capture.snapshot().saved_positions, 2);
        assert_eq!(capture.snapshot().dropped_positions, 2);
        assert!(ride_log::erased(&media.bytes[2 * 256 + 252..3 * 256]));
    }

    #[test]
    fn ready_sources_all_progress_and_stop_flushes_position_drops() {
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        for n in 0..16 {
            capture.observe_link(40, LinkState::Connected, 1, 100);
            capture.packet(packet(n));
        }
        capture.position(position(100));
        for _ in 0..3 {
            flush(&mut capture, &mut media);
        }
        assert_eq!(
            [media.bytes[5], media.bytes[256 + 5], media.bytes[512 + 5]],
            [5, 1, 6]
        );
        assert_eq!(capture.snapshot().saved_positions, 1);
        assert_eq!(capture.snapshot().packets, 8);
        capture.position(position(200));
        capture.position(position(300));
        capture.stop();
        for _ in 0..100 {
            capture.service(&mut media, 2000);
        }
        assert_eq!(capture.snapshot().status, Status::Stopped);
        assert_eq!(capture.snapshot().saved_positions, 2);
        assert_eq!(capture.snapshot().packets, 16);
        let terminal = (capture.snapshot().next_slot - 1) * 256;
        assert_eq!(&media.bytes[terminal + 32..terminal + 36], b"GPS1");
        assert_eq!(
            u32::from_le_bytes(
                media.bytes[terminal + 36..terminal + 40]
                    .try_into()
                    .unwrap()
            ),
            1
        );
    }

    #[test]
    fn queue_overflow_and_full_close_are_bounded() {
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        for n in 0..17 {
            capture.packet(packet(n));
        }
        assert_eq!(capture.snapshot().dropped, 1);
        // Exercise the final two slots without generating thousands of batches.
        capture.next_slot = ride_log::SLOTS - 2;
        flush(&mut capture, &mut media);
        capture.packet(packet(10));
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().status, Status::Full);
        assert_eq!(capture.snapshot().packets, 8);
        assert_eq!(capture.snapshot().dropped, 10);
        assert_eq!(media.bytes[(ride_log::SLOTS - 1) * 256 + 5], 3);
        assert_eq!(ride_log::transport_checksum(b"123456789"), 0xcbf4_3926);
    }
    #[test]
    fn saves_link_transitions_separately_without_claiming_packets() {
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        scan(&mut capture, &mut media);
        capture.observe_link(40, LinkState::Connected, 7, 123);
        capture.observe_link(120, LinkState::TransportLost, 8, 456);
        capture.packet(packet(1));
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().packets, 0);
        assert!(!capture.snapshot().recording);
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().packets, 1);
        flush(&mut capture, &mut media);
        assert_eq!(&media.bytes[5..7], &[5, 1]);
        assert_eq!(&media.bytes[24..28], &[40, 2, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(media.bytes[28..36].try_into().unwrap()),
            123
        );
        assert_eq!(&media.bytes[512 + 24..512 + 28], &[120, 6, 0, 0]);
        assert_eq!(
            u32::from_le_bytes(media.bytes[512 + 36..512 + 40].try_into().unwrap()),
            8
        );
        flush(&mut capture, &mut media);
        assert_eq!(capture.snapshot().packets, 1);
        assert!(capture.snapshot().recording);
    }
    #[test]
    fn idle_never_accesses_media_or_counts_packets_and_scan_keeps_link_events() {
        let mut media = Memory::new();
        let mut capture = Capturer::new();
        capture.packet(packet(1));
        capture.service(&mut media, 100);
        assert_eq!(media.calls, 0);
        assert_eq!(capture.snapshot().dropped, 0);
        assert!(capture.start());
        capture.observe_link(40, LinkState::Connected, 1, 100);
        capture.packet(packet(1));
        for _ in 0..ride_log::SECTORS {
            capture.service(&mut media, 100);
        }
        flush(&mut capture, &mut media);
        assert_eq!(&media.bytes[24..28], &[40, 2, 0, 0]);
        assert_eq!(
            u32::from_le_bytes(media.bytes[40..44].try_into().unwrap()),
            1
        );
        assert_eq!(capture.snapshot().packets, 0);
    }
}
