use super::*;
use crate::ride::control::Action;
use crate::ride::log::{SAMPLES_PER_BATCH, Sample, Status};
use crate::ride::recorder::Recorder;
use device_api::bulk::OwnedInfo;
use std::collections::BTreeMap;
use std::vec::Vec;

/// Sparse owned MMC region. Unwritten sectors hold stale bytes from an older
/// filesystem; a fault tears one write and then the power is off.
struct Media {
    sectors: u64,
    written: BTreeMap<u64, [u8; 512]>,
    writes: u32,
    fail_after: Option<(u32, usize)>,
    off: bool,
    /// The fault fails one write and the medium keeps working afterwards.
    transient: bool,
    missing: Option<MediaError>,
    /// Hardware entropy: a fresh value on every call, across reboots.
    entropy: u64,
}

impl Media {
    fn new(sectors: u64) -> Self {
        Self {
            sectors,
            written: BTreeMap::new(),
            writes: 0,
            fail_after: None,
            off: false,
            transient: false,
            missing: None,
            entropy: 0,
        }
    }
    fn get(&self, index: u64) -> [u8; 512] {
        self.written.get(&index).copied().unwrap_or_else(|| {
            let mut stale = [0u8; 512];
            for (offset, byte) in stale.iter_mut().enumerate() {
                *byte = (index as usize * 31 + offset * 7) as u8;
            }
            stale
        })
    }
}

impl ReadWrite for Media {
    fn entropy(&mut self) -> u64 {
        self.entropy = self.entropy.wrapping_add(0x9e37_79b9_7f4a_7c15);
        self.entropy
    }
    fn owned_info(&mut self) -> Result<OwnedInfo, MediaError> {
        if let Some(error) = self.missing {
            return Err(error);
        }
        if self.off {
            return Err(MediaError::Failed);
        }
        Ok(OwnedInfo {
            start_sector: 1_955_841,
            sectors: self.sectors,
            sector_size: 512,
        })
    }
    fn owned_read(&mut self, index: u64, output: &mut [u8; 512]) -> Result<(), MediaError> {
        self.owned_info()?;
        if index >= self.sectors {
            return Err(MediaError::Range);
        }
        *output = self.get(index);
        Ok(())
    }
    fn owned_write(&mut self, index: u64, input: &[u8; 512]) -> Result<(), MediaError> {
        self.owned_info()?;
        if index >= self.sectors {
            return Err(MediaError::Range);
        }
        if let Some((after, tear)) = self.fail_after {
            if after == 0 {
                let mut torn = self.get(index);
                torn[..tear].copy_from_slice(&input[..tear]);
                self.written.insert(index, torn);
                self.off = !self.transient;
                self.fail_after = None;
                return Err(MediaError::Failed);
            }
            self.fail_after = Some((after - 1, tear));
        }
        self.writes += 1;
        self.written.insert(index, *input);
        Ok(())
    }
}

/// A C606 with bulk media: the recorder, its retained state, and the region.
struct Device {
    recorder: Recorder,
    state: BulkState,
    media: Media,
    now: u64,
}

impl Device {
    fn new(sectors: u64) -> Self {
        Self {
            recorder: Recorder::default(),
            state: BulkState::default(),
            media: Media::new(sectors),
            now: 0,
        }
    }

    fn service(&mut self) {
        let now = self.now;
        let sample = Sample {
            heart_bpm: Some(120 + (now / 1000 % 20) as u16),
            power_watts: Some(200),
            ..Sample::default()
        };
        self.recorder.service(
            &mut Bulk::new(&mut self.state, &mut self.media),
            now,
            sample,
            &|| now,
        );
    }

    /// Power returns: faults clear, written sectors stay, the scan restarts.
    fn reboot(&mut self) {
        self.recorder = Recorder::default();
        self.state = BulkState::default();
        self.media.fail_after = None;
        self.media.off = false;
        self.now = 0;
        while self.recorder.status() == Status::Scanning {
            self.service();
        }
    }

    fn run_until(&mut self, until: u64, step: u64) {
        while self.now < until {
            self.now += step;
            self.service();
        }
    }

    fn act(&mut self, action: Action) -> bool {
        if !self.recorder.request(action, Source::Live, self.now, 7) {
            return false;
        }
        for _ in 0..4 {
            self.service();
            if let Some(result) = self.recorder.take_result() {
                return result.ok;
            }
        }
        false
    }

    fn initialized(sectors: u64) -> Self {
        let mut device = Self::new(sectors);
        device.reboot();
        assert_eq!(device.recorder.status(), Status::NeedsInit);
        assert!(device.recorder.initialize(1));
        device.service();
        assert!(device.recorder.take_result().unwrap().ok);
        assert_eq!(device.recorder.status(), Status::Ready);
        device
    }

    /// The catalog a scan of every committed slot from zero would rebuild.
    fn full_scan(&mut self) -> Catalog {
        let mut scanner = Scanner::default();
        let mut slot = Slot::default();
        for index in 0..self.recorder.next_slot() {
            let valid = Bulk::new(&mut self.state, &mut self.media)
                .read_slot(index, &mut slot)
                .unwrap();
            if valid {
                scanner.accept_slot(index, &slot.0);
            } else {
                scanner.skip(index, index + 1);
            }
        }
        let mut catalog = scanner.catalog();
        catalog.next_slot = self.recorder.next_slot();
        catalog
    }
}

const CAPACITY: u64 = 1_000_000;

#[test]
fn catalogs_round_trip_through_checkpoints() {
    let mut summary = Summary::started(9, Source::Live);
    summary.add_sample(Sample {
        active_ms: 4_000,
        utc_ms: Some(1_789_000_000_000),
        location_e7: Some((1, 2)),
        heart_bpm: Some(150),
        cadence_tenths: Some(900),
        ..Sample::default()
    });
    let mut finished = summary;
    finished.finish(crate::ride::log::Kind::Recovered, 9_000, true);
    let mut catalog = Catalog {
        next_slot: 5_000_000,
        next_ride_id: 10,
        completed: 8,
        invalid_slots: 3,
        valid_slots: 4_999,
        recovery: Some(Recovery {
            ride_id: 9,
            source: Source::Live,
            next_sequence: 77,
            active_ms: 4_000,
            gap: true,
            summary,
        }),
        ..Catalog::default()
    };
    for _ in 0..5 {
        catalog.push_summary(finished);
    }
    assert_eq!(decode_catalog(&encode_catalog(&catalog)), Some(catalog));
    let empty = Catalog::default();
    assert_eq!(decode_catalog(&encode_catalog(&empty)), Some(empty));
    let mut damaged = encode_catalog(&catalog);
    damaged[0] = 2;
    assert_eq!(decode_catalog(&damaged), None);
    assert_eq!(decode_catalog(&damaged[..10]), None);
}

#[test]
fn day_long_ride_averages_survive_checkpoints_without_saturating() {
    // 24 h at 1 Hz: past the 65,535 samples a 16-bit count could hold.
    let mut summary = Summary::started(1, Source::Live);
    for second in 0..24 * 3600u64 {
        summary.add_sample(Sample {
            active_ms: second * 1000,
            location_e7: Some((1, 2)),
            heart_bpm: Some(150),
            cadence_tenths: Some(900),
            ..Sample::default()
        });
    }
    let catalog = Catalog {
        next_slot: 21_601,
        next_ride_id: 2,
        recovery: Some(Recovery {
            ride_id: 1,
            source: Source::Live,
            next_sequence: 21_601,
            active_ms: 24 * 3600 * 1000,
            gap: false,
            summary,
        }),
        ..Catalog::default()
    };
    let mut restored = decode_catalog(&encode_catalog(&catalog))
        .unwrap()
        .recovery
        .unwrap()
        .summary;
    assert_eq!(restored, summary);
    restored.finish(crate::ride::log::Kind::Recovered, 24 * 3600 * 1000, false);
    assert_eq!(restored.heart_average, Some(150));
    assert_eq!(restored.cadence_average, Some(900));
    assert_eq!(restored.gps_samples, 24 * 3600);
}

#[test]
fn unformatted_missing_or_unsupported_media_is_never_written() {
    let mut device = Device::new(CAPACITY);
    device.reboot();
    assert_eq!(device.recorder.status(), Status::NeedsInit);
    assert!(!device.recorder.request(Action::Start, Source::Live, 0, 1));
    assert_eq!(device.recorder.remaining_secs(), 0);
    // No epoch, no capacity: not the internal journal's slot count.
    assert_eq!(device.recorder.slots(), 0);
    for (error, status) in [
        (MediaError::Unavailable, Status::Unavailable),
        (MediaError::Unsupported, Status::Unsupported),
    ] {
        device.media.missing = Some(error);
        device.reboot();
        assert_eq!(device.recorder.status(), status);
        assert!(!device.recorder.initialize(1));
    }
    assert_eq!(device.media.writes, 0);
}

#[test]
fn ten_hour_ride_is_kept_and_boot_reads_stay_bounded() {
    let mut device = Device::initialized(CAPACITY);
    let hours = 10;
    assert!(device.act(Action::Start));
    device.run_until(hours * 3_600_000, 50);
    assert!(device.act(Action::Finish));
    // The checkpoint after FINISH.
    device.service();
    let samples = device.recorder.written_samples();
    assert!(samples >= hours as u32 * 3_600 - 1, "{samples} samples");
    assert_eq!(device.recorder.dropped_samples(), 0);
    let records = device.recorder.next_slot();
    // START, one record per four samples, FINISH.
    assert_eq!(records, 2 + (samples as usize).div_ceil(SAMPLES_PER_BATCH));

    device.reboot();
    assert_eq!(device.recorder.status(), Status::Ready);
    assert_eq!(device.recorder.completed(), 1);
    assert_eq!(device.recorder.next_slot(), records);
    let summary = device.recorder.summaries()[0].unwrap();
    assert_eq!(summary.active_ms, hours * 3_600_000);
    assert!(summary.heart_average.is_some());
    // A checkpoint follows the finish, so boot reads only metadata and the
    // replay window after it, not the 9,000 records.
    let reads = device.state.boot_reads();
    assert!(reads <= 2 + 8 + 1 + 2 * 32, "{reads} boot reads");
    let scanned = device.full_scan();
    assert_eq!(*device.recorder.catalog(), scanned);
    // About 1,111 hours of 1 Hz samples in this million-sector region.
    assert!(device.recorder.remaining_secs() > 1_100 * 3_600);
}

#[test]
fn power_loss_mid_ride_recovers_the_committed_prefix_within_one_checkpoint() {
    let mut device = Device::initialized(CAPACITY);
    assert!(device.act(Action::Start));
    // 50 records: one checkpoint at 32, then 18 more before the power fails.
    device.run_until(4_000 * 50, 10);
    let committed = device.recorder.written_samples();
    device.reboot();
    assert_eq!(device.recorder.status(), Status::Recovered);
    let reads = device.state.boot_reads();
    assert!(
        reads <= 2 + 8 + 1 + 2 * CHECKPOINT_RECORDS,
        "{reads} boot reads"
    );
    let scanned = device.full_scan();
    assert_eq!(*device.recorder.catalog(), scanned);
    assert_eq!(
        device.recorder.catalog().recovery.unwrap().active_ms,
        u64::from(committed - 1) * 1000
    );
    // The recovered ride closes, and a new ride starts after it.
    device.service();
    device.service();
    assert_eq!(device.recorder.completed(), 1);
    assert!(device.act(Action::Start));
    device.reboot();
    assert_eq!(device.recorder.status(), Status::Recovered);
    assert_eq!(device.recorder.summaries()[0].unwrap().ride_id, 1);
    assert!(device.recorder.summaries()[0].unwrap().recovered);
}

/// Start, ride, pause, resume, finish: every phase of a ride.
fn workload(device: &mut Device) -> bool {
    device.act(Action::Start)
        && {
            device.run_until(150_000, 10);
            device.recorder.status() == Status::Recording
        }
        && device.act(Action::Pause)
        && {
            device.now = 160_000;
            device.act(Action::Resume)
        }
        && {
            device.run_until(300_000, 10);
            device.recorder.status() == Status::Recording
        }
        && device.act(Action::Finish)
        && {
            // The checkpoint after FINISH.
            device.service();
            device.recorder.status() == Status::Saved
        }
}

#[test]
fn power_loss_at_every_write_of_a_ride_keeps_what_was_acknowledged() {
    let mut clean = Device::initialized(10_000);
    let before = clean.media.writes;
    assert!(workload(&mut clean));
    let writes = clean.media.writes - before;
    assert!(writes > 80, "{writes} writes");
    for fail in 0..writes {
        for tear in [0, 200, 512] {
            let mut device = Device::initialized(10_000);
            device.media.fail_after = Some((fail, tear));
            assert!(!workload(&mut device), "write {fail} must fail");
            let acknowledged = device.recorder.written_samples();
            device.reboot();
            let status = device.recorder.status();
            assert!(
                matches!(status, Status::Recovered | Status::Saved | Status::Ready),
                "write {fail} tear {tear}: {status:?}"
            );
            // Checkpoint plus replay equals a scan of everything committed.
            let scanned = device.full_scan();
            assert_eq!(
                *device.recorder.catalog(),
                scanned,
                "write {fail} tear {tear}"
            );
            let recovered: u32 = (0..device.recorder.next_slot())
                .filter_map(|index| {
                    let mut slot = Slot::default();
                    Bulk::new(&mut device.state, &mut device.media)
                        .read_slot(index, &mut slot)
                        .unwrap()
                        .then(|| crate::ride::log::decode(&slot))
                        .flatten()
                })
                .map(|entry| u32::from(entry.count))
                .sum();
            assert!(
                (acknowledged..=acknowledged + 4).contains(&recovered),
                "write {fail} tear {tear}: {recovered} of {acknowledged} samples"
            );
            // Recording continues normally after the reboot.
            device.service();
            device.service();
            assert!(device.act(Action::Start), "write {fail} tear {tear}");
        }
    }
}

#[test]
fn an_action_queued_before_a_failed_checkpoint_fails_and_nothing_more_is_written() {
    for action in [Action::Pause, Action::Resume] {
        let mut device = Device::initialized(CAPACITY);
        assert!(device.act(Action::Start));
        if action == Action::Pause {
            // Ride until an append makes a checkpoint due.
            while device.state.since_checkpoint < CHECKPOINT_RECORDS {
                device.now += 250;
                device.service();
            }
        } else {
            device.run_until(device.now + 5_000, 250);
            // The PAUSE record is the one that makes a checkpoint due.
            device.state.since_checkpoint = CHECKPOINT_RECORDS - 1;
            assert!(device.act(Action::Pause));
            assert_eq!(device.recorder.status(), Status::Paused);
        }
        // The action is queued; the checkpoint written first fails once.
        assert!(device.recorder.request(action, Source::Live, device.now, 9));
        device.media.transient = true;
        device.media.fail_after = Some((0, 0));
        device.service();
        assert_eq!(device.recorder.status(), Status::Error, "{action:?}");
        let result = device
            .recorder
            .take_result()
            .expect("the queued action completes");
        assert_eq!((result.token, result.ok), (9, false), "{action:?}");
        // The medium works again, yet nothing is written until recovery.
        let writes = device.media.writes;
        for _ in 0..20 {
            device.now += 250;
            device.service();
        }
        assert_eq!(device.media.writes, writes, "{action:?}");
        assert_eq!(device.recorder.status(), Status::Error, "{action:?}");
        assert!(device.recorder.take_result().is_none());
    }
}

#[test]
fn boot_reads_stay_bounded_when_power_is_cut_before_every_checkpoint() {
    let mut device = Device::initialized(CAPACITY);
    for ride in 0..40 {
        assert!(device.act(Action::Start));
        device.run_until(device.now + 2_000, 250);
        assert!(device.act(Action::Finish));
        // Power is cut before the checkpoint the finish made due.
        device.reboot();
        assert_eq!(device.recorder.status(), Status::Ready, "ride {ride}");
        let reads = device.state.boot_reads();
        assert!(
            reads <= 2 + 8 + 1 + 2 * 32,
            "ride {ride}: {reads} boot reads"
        );
    }
    let catalog = device.full_scan();
    assert_eq!(device.recorder.catalog(), &catalog);
}

#[test]
fn a_reformat_after_lost_superblocks_never_revives_an_older_store() {
    use crate::ride::journal::Journal;
    use crate::ride::log::{Entry, Kind};
    let mut media = Media::new(sector_log::DATA_START + 1_000);
    let entry = Entry::event(Kind::Start, Source::Live, 1, 0, 0);
    // Store A with 200 records, from a clock-based seed.
    let mut state = BulkState::default();
    let mut bulk = Bulk::new(&mut state, &mut media);
    assert_eq!(bulk.initialize(0, 7), Progress::Done);
    for index in 0..200 {
        assert!(bulk.append(index, &entry, false).is_some());
    }
    let a = state.log().unwrap().store_id();
    // Cleared to store B, which overwrites the first records and the ring.
    let mut bulk = Bulk::new(&mut state, &mut media);
    assert_eq!(bulk.clear(0, 99), Progress::Done);
    for index in 0..3 {
        assert!(bulk.append(index, &entry, false).is_some());
    }
    for _ in 0..sector_log::RING {
        assert!(bulk.checkpoint(&Catalog::default()));
    }
    // Both superblocks are lost; the clock seed repeats after a reboot.
    media.written.insert(0, [0; 512]);
    media.written.insert(1, [0; 512]);
    let mut state = BulkState::default();
    let mut bulk = Bulk::new(&mut state, &mut media);
    assert_eq!(bulk.initialize(0, 7), Progress::Done);
    assert_ne!(
        state.log().unwrap().store_id(),
        a,
        "store A would validate again"
    );
    // A boot of the new store finds none of A's 200 records.
    let mut device = Device {
        recorder: Recorder::default(),
        state: BulkState::default(),
        media,
        now: 0,
    };
    device.reboot();
    assert_eq!(
        (device.recorder.status(), device.recorder.next_slot()),
        (Status::Ready, 0)
    );
}

#[test]
fn an_overdue_checkpoint_is_written_before_the_next_append_after_boot() {
    let mut device = Device::initialized(CAPACITY);
    for boot in 0..100 {
        // A START, then power is cut; the next boot appends RECOVERED, whose
        // checkpoint is cut too. Neither boot may replay more each time.
        assert!(device.act(Action::Start), "boot {boot}");
        device.reboot();
        assert_eq!(device.recorder.status(), Status::Recovered);
        // The overdue checkpoint, if any, then the RECOVERED record.
        let next = device.recorder.next_slot();
        while device.recorder.next_slot() == next {
            device.service();
        }
        device.reboot();
        let reads = device.state.boot_reads();
        assert!(
            reads <= 2 + 8 + 1 + 2 * 32,
            "boot {boot}: {reads} boot reads"
        );
    }
    let catalog = device.full_scan();
    assert_eq!(device.recorder.catalog(), &catalog);
}

#[test]
fn full_media_closes_the_ride_and_keeps_it() {
    // 16 metadata sectors and 12 record sectors: START, 10 batches, FULL.
    let mut device = Device::initialized(16 + 12);
    assert_eq!(device.recorder.remaining_secs(), 40);
    assert!(device.act(Action::Start));
    device.run_until(120_000, 10);
    assert_eq!(device.recorder.status(), Status::Full);
    assert_eq!(device.recorder.written_samples(), 40);
    assert_eq!(device.recorder.remaining_secs(), 0);
    device.service();
    device.reboot();
    assert_eq!(device.recorder.status(), Status::Full);
    assert_eq!(device.recorder.completed(), 1);
    assert!(device.recorder.summaries()[0].unwrap().full);
}

#[test]
fn lost_checkpoints_are_kept_until_an_explicit_initialize() {
    let mut device = Device::initialized(CAPACITY);
    assert!(device.act(Action::Start));
    device.run_until(60_000, 10);
    assert!(device.act(Action::Finish));
    for slot in 0..sector_log::RING {
        device
            .media
            .written
            .insert(sector_log::RING_START + slot, [0; 512]);
    }
    device.reboot();
    assert_eq!(device.recorder.status(), Status::Corrupt);
    // No epoch recovered, so no extent to export or clear up to.
    assert!(device.state.log().is_none());
    assert!(!device.recorder.request(Action::Start, Source::Live, 0, 1));
    let writes = device.media.writes;
    device.run_until(10_000, 10);
    assert_eq!(device.media.writes, writes);
    // Records stay on media for diagnosis; a new epoch writes only metadata.
    let records: Vec<_> = (16..30).map(|i| device.media.get(i)).collect();
    assert!(device.recorder.initialize(2));
    device.service();
    assert!(device.recorder.take_result().unwrap().ok);
    assert_eq!(device.media.writes, writes + 2 + sector_log::RING as u32);
    assert_eq!(
        (16..30).map(|i| device.media.get(i)).collect::<Vec<_>>(),
        records
    );
    device.reboot();
    assert_eq!(
        (device.recorder.status(), device.recorder.next_slot()),
        (Status::Ready, 0)
    );
}

#[test]
fn an_unproven_end_is_corrupt_and_exportable_but_never_written_or_cleared() {
    let mut device = Device::initialized(CAPACITY);
    assert!(device.act(Action::Start));
    // 40 records: a checkpoint at 32, then 8 more.
    device.run_until(4_000 * 40, 10);
    assert!(device.act(Action::Finish));
    let upper = device.recorder.next_slot();
    // Power is cut before the finish's checkpoint; then a record after the
    // newest checkpoint is damaged, so the end cannot be proven.
    let damaged = sector_log::DATA_START + upper as u64 - 4;
    device.media.written.insert(damaged, [0xa5; 512]);
    device.reboot();
    assert_eq!(device.recorder.status(), Status::Corrupt);
    let log = *device.state.log().unwrap();
    assert!(!log.writable());
    // The extent seen stays exportable, the damaged slot reading invalid.
    assert_eq!(device.recorder.next_slot(), upper);
    assert!(device.recorder.exportable());
    let mut slot = Slot::default();
    let mut bulk = Bulk::new(&mut device.state, &mut device.media);
    assert_eq!(bulk.read_slot(upper - 4, &mut slot), Some(false));
    assert_eq!(bulk.read_slot(upper - 1, &mut slot), Some(true));
    // Nothing starts, appends or clears; only an explicit initialize may.
    let writes = device.media.writes;
    assert!(!device.recorder.request(Action::Start, Source::Live, 0, 1));
    assert!(!device.recorder.clear(upper, 3));
    device.run_until(10_000, 10);
    assert_eq!(device.media.writes, writes);
    assert!(device.recorder.initialize(4));
    device.service();
    assert!(device.recorder.take_result().unwrap().ok);
    device.reboot();
    assert_eq!(
        (device.recorder.status(), device.recorder.next_slot()),
        (Status::Ready, 0)
    );
}

#[test]
fn clear_starts_an_empty_epoch_only_for_the_exported_bound() {
    let mut device = Device::initialized(CAPACITY);
    assert!(device.act(Action::Start));
    device.run_until(20_000, 10);
    assert!(device.act(Action::Finish));
    device.service();
    let upper = device.recorder.next_slot();
    let old = device.state.log().unwrap().store_id();
    assert!(!device.recorder.clear(upper - 1, 3));
    assert!(device.recorder.clear(upper, 3));
    device.service();
    assert!(device.recorder.take_result().unwrap().ok);
    assert_eq!(device.recorder.next_slot(), 0);
    device.reboot();
    assert_eq!(
        (device.recorder.status(), device.recorder.completed()),
        (Status::Ready, 0)
    );
    assert_ne!(device.state.log().unwrap().store_id(), old);
    assert!(device.act(Action::Start));
}
