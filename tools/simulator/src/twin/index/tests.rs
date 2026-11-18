use super::*;
use crate::bulk::SimBulk;
use crate::replay::Blocks;
use device_api::bulk::{ReadWrite, ReadWriteRecorder, SECTOR_SIZE};
use device_api::journal::PART;
use device_api::observation::{Availability, Error};
use device_api::storage::{Geometry, OwnedFlash, OwnedFlashRecorder, Region};
use firmware_services::trace::log::Log;
use firmware_services::trace::media::Memory;
use firmware_services::trace::recorder::{Record, Recorder};
use firmware_services::trace::wire::{Wire, Writer};
use firmware_services::volume;
use std::cell::RefCell;

const SIZE: usize = 64 * 1024;

struct Nor {
    bytes: Vec<u8>,
    fail: bool,
}

impl Nor {
    fn new() -> Self {
        let mut rng = firmware_services::testing::Rng::new(7);
        Self {
            bytes: (0..SIZE).map(|_| rng.byte()).collect(),
            fail: false,
        }
    }

    fn torn(&mut self) -> bool {
        core::mem::take(&mut self.fail)
    }
}

impl OwnedFlash for Nor {
    type Error = Error;

    fn availability(&self, _: Region) -> Availability {
        Availability::Ready
    }

    fn geometry(&self, _: Region) -> Geometry {
        Geometry {
            capacity: SIZE,
            program_size: 4,
            erase_size: 4096,
            durable: true,
        }
    }

    fn read(&mut self, _: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        out.copy_from_slice(&self.bytes[offset..offset + out.len()]);
        Ok(())
    }

    fn program(&mut self, _: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        let landed = if self.torn() {
            bytes.len() / 2
        } else {
            bytes.len()
        };
        for (held, byte) in self.bytes[offset..].iter_mut().zip(&bytes[..landed]) {
            *held &= byte;
        }
        match landed == bytes.len() {
            true => Ok(()),
            false => Err(Error::Failed),
        }
    }

    fn erase(&mut self, _: Region, offset: usize, length: usize) -> Result<(), Error> {
        let landed = if self.torn() { length / 2 } else { length };
        self.bytes[offset..offset + landed].fill(0xFF);
        match landed == length {
            true => Ok(()),
            false => Err(Error::Failed),
        }
    }
}

type Journal = RefCell<Log<Vec<u8>, Blocks, Box<Memory>>>;

struct Device {
    log: Journal,
    nor: Nor,
    card: SimBulk,
    flash_seen: RefCell<device_api::storage::OwnedFlashObserved>,
    card_seen: RefCell<device_api::bulk::ReadWriteObserved>,
    now: u64,
}

fn card() -> SimBulk {
    let card = SimBulk::default();
    card.set_presence(crate::bulk::Presence::Ready);
    card.set_sectors(4 * volume::TERRAIN_SECTORS);
    volume::create(&mut card.host(), &mut [0; SECTOR_SIZE]).unwrap();
    card
}

fn volumes() -> Volumes {
    Volumes::read(&mut card().host(), &mut [0; SECTOR_SIZE])
}

fn store() -> u32 {
    volumes().store.unwrap().start as u32
}

impl Device {
    fn new() -> Self {
        let mut file = vec![0u8; 4096];
        let header = trace::Header {
            build: [0; 20],
            device: "c606",
            boot: 1,
            terrain: firmware_services::trace::origin::Found::Absent,
        };
        let len = trace::encode_header(&header, &mut file).unwrap();
        file.truncate(len);
        let recorder = Recorder::new(vec![0u8; 4096], Blocks(file));
        let mut log = Log::new(Some(recorder), Box::new(Memory::new()));
        log.volumes(volumes());
        log.begin_pass(1_000, false);
        Self {
            log: RefCell::new(log),
            nor: Nor::new(),
            card: card(),
            flash_seen: RefCell::default(),
            card_seen: RefCell::default(),
            now: 1_000,
        }
    }

    fn pass(&mut self) {
        self.now += 10;
        self.log.borrow_mut().begin_pass(self.now, false);
    }

    fn flash(
        &mut self,
    ) -> OwnedFlashRecorder<&mut Nor, &Journal, &RefCell<device_api::storage::OwnedFlashObserved>>
    {
        OwnedFlashRecorder::new(&mut self.nor, &self.log, &self.flash_seen)
    }

    fn read(&mut self, at: usize, len: usize) -> Vec<u8> {
        let mut out = vec![0; len];
        self.flash().read(Region::Data, at, &mut out).unwrap();
        out
    }

    fn program(&mut self, at: usize, bytes: &[u8]) -> Result<(), Error> {
        self.flash().program(Region::Data, at, bytes)
    }

    fn erase(&mut self, at: usize, len: usize) -> Result<(), Error> {
        self.flash().erase(Region::Data, at, len)
    }

    fn sector(&mut self, at: u32) -> [u8; SECTOR_SIZE] {
        let mut out = [0; SECTOR_SIZE];
        let mut card = ReadWriteRecorder::new(self.card.host(), &self.log, &self.card_seen);
        card.owned_read(u64::from(at), &mut out).unwrap();
        out
    }

    fn write_sector(&mut self, at: u32, bytes: &[u8; SECTOR_SIZE]) {
        let mut card = ReadWriteRecorder::new(self.card.host(), &self.log, &self.card_seen);
        card.owned_write(u64::from(at), bytes).unwrap();
    }

    fn trace(self) -> Vec<u8> {
        let mut log = self.log.into_inner();
        let recorder = log.recorder.as_mut().unwrap();
        let mut bytes = recorder.sink().0.clone();
        bytes.extend_from_slice(recorder.open_block());
        bytes
    }
}

fn method(name: &str) -> MethodId {
    super::super::script::named("OwnedFlash", name).unwrap()
}

fn card_method(name: &str) -> MethodId {
    super::super::script::named("ReadWrite", name).unwrap()
}

fn data(start: usize, count: usize) -> Span {
    Span {
        medium: Medium::Data,
        start: start as u32,
        count: count as u32,
    }
}

fn bulk(start: u32) -> Span {
    Span {
        medium: Medium::Bulk,
        start,
        count: 1,
    }
}

struct Twin(Model);

impl Twin {
    fn of(trace: &[u8]) -> Self {
        Self(Model::new(Index::of(trace, volumes()).unwrap(), None, true))
    }

    fn read(&mut self, at: usize, len: usize) -> Result<Result<Vec<u8>, StorageError>, Leave> {
        let mut out = vec![0; len];
        let read = self.0.read(method("read"), data(at, len), &mut out)?;
        Ok(read.map(|()| out))
    }

    fn program(&mut self, at: usize, bytes: &[u8]) -> Result<(), StorageError> {
        let span = data(at, bytes.len());
        self.0
            .mutate(method("program"), span, Change::Program(bytes))
    }

    fn erase(&mut self, at: usize, len: usize) -> Result<(), StorageError> {
        let change = Change::Erase { len: len as u64 };
        self.0.mutate(method("erase"), data(at, len), change)
    }

    fn sector(&mut self, at: u32) -> Result<Result<Vec<u8>, StorageError>, Leave> {
        let mut out = vec![0; SECTOR_SIZE];
        let read = self.0.read(card_method("owned_read"), bulk(at), &mut out)?;
        Ok(read.map(|()| out))
    }

    fn write_sector(&mut self, at: u32, bytes: &[u8]) -> Result<(), StorageError> {
        self.0
            .mutate(card_method("owned_write"), bulk(at), Change::Write(bytes))
    }
}

fn unknown(at: usize, len: usize, why: Why) -> Result<Result<Vec<u8>, StorageError>, Leave> {
    Err(Leave::Unknown {
        span: data(at, len),
        why,
    })
}

#[test]
fn a_read_cached_or_made_earlier_than_the_devices_is_served_from_the_pristine_image() {
    let mut device = Device::new();
    let early = device.read(0, 64);
    device.pass();
    let late = device.read(1_000, 64);
    let mut twin = Twin::of(&device.trace());
    assert_eq!(twin.read(1_000, 64), Ok(Ok(late)), "read a pass early");
    assert_eq!(twin.read(0, 64), Ok(Ok(early.clone())));
    assert_eq!(
        twin.read(0, 32),
        Ok(Ok(early[..32].to_vec())),
        "a narrower reread"
    );
}

#[test]
fn a_read_one_unit_wider_than_recorded_stops_as_unknown_media() {
    let mut device = Device::new();
    device.read(0, 64);
    let mut twin = Twin::of(&device.trace());
    assert_eq!(twin.read(0, 65), unknown(0, 65, Why::Unread));
    assert_eq!(
        Leave::Unknown {
            span: data(0, 65),
            why: Why::Unread
        }
        .to_string(),
        "unknown media Data 0+65"
    );
}

#[test]
fn a_build_that_skips_a_recorded_program_never_reads_the_devices_bytes() {
    let mut device = Device::new();
    device.program(100, &[0x0F; 16]).unwrap();
    let after = device.read(100, 16);
    let trace = device.trace();

    let mut skips = Twin::of(&trace);
    assert_eq!(skips.read(100, 16), unknown(100, 16, Why::Unread));

    let mut programs = Twin::of(&trace);
    programs.program(100, &[0x0F; 16]).unwrap();
    assert_eq!(
        programs.read(100, 16),
        Ok(Ok(after)),
        "the device's bytes, in step"
    );
}

#[test]
fn a_build_that_programs_other_bytes_reads_back_its_own() {
    let mut device = Device::new();
    let before = device.read(100, 16);
    device.program(100, &[0x0F; 16]).unwrap();
    device.read(100, 16);
    let mut twin = Twin::of(&device.trace());
    twin.program(100, &[0xF0; 16]).unwrap();
    let own: Vec<u8> = before.iter().map(|byte| byte & 0xF0).collect();
    assert_eq!(twin.read(100, 16), Ok(Ok(own)));
}

#[test]
fn an_unmatched_program_onto_unread_bytes_says_the_bytes_are_the_twins_own() {
    let mut device = Device::new();
    device.program(100, &[0x0F; 16]).unwrap();
    device.read(100, 16);
    let mut twin = Twin::of(&device.trace());
    twin.program(100, &[0xF0; 16]).unwrap();
    let stop = twin.read(100, 16);
    assert_eq!(stop, unknown(100, 16, Why::Own(Off::Unmatched)));
    assert_eq!(
        stop.unwrap_err().to_string(),
        "unknown media Data 100+16: the twin's own write there matched no recorded one"
    );
}

#[test]
fn programs_made_in_another_order_stop_and_never_read_a_partial_version() {
    let (a, b) = ([0b1110_1111u8; 8], [0b1111_0111u8; 8]);
    let mut device = Device::new();
    device.program(200, &a).unwrap();
    device.program(200, &b).unwrap();
    let after = device.read(200, 8);
    let trace = device.trace();

    let mut reordered = Twin::of(&trace);
    reordered.program(200, &b).unwrap();
    reordered.program(200, &a).unwrap();
    assert_eq!(
        reordered.read(200, 8),
        unknown(200, 8, Why::Own(Off::OutOfStep))
    );

    let mut in_order = Twin::of(&trace);
    in_order.program(200, &a).unwrap();
    in_order.program(200, &b).unwrap();
    assert_eq!(in_order.read(200, 8), Ok(Ok(after)));
}

#[test]
fn repeated_overlapping_programs_replay_and_one_skipped_breaks_step_where_it_overlapped() {
    let mut device = Device::new();
    device.program(300, &[0x7F; 16]).unwrap();
    device.pass();
    device.program(308, &[0xFE; 16]).unwrap();
    let after = device.read(300, 24);
    let trace = device.trace();

    let mut both = Twin::of(&trace);
    both.program(300, &[0x7F; 16]).unwrap();
    both.program(308, &[0xFE; 16]).unwrap();
    assert_eq!(both.read(300, 24), Ok(Ok(after.clone())));

    let mut second = Twin::of(&trace);
    second.program(308, &[0xFE; 16]).unwrap();
    assert_eq!(second.read(300, 8), unknown(300, 8, Why::Unread));
    assert_eq!(
        second.read(308, 8),
        unknown(308, 8, Why::Own(Off::OutOfStep))
    );
    assert_eq!(second.read(316, 8), Ok(Ok(after[16..].to_vec())));
}

#[test]
fn a_program_delayed_past_others_still_matches_across_the_era() {
    let mut device = Device::new();
    device.program(400, &[0x3C; 8]).unwrap();
    device.pass();
    device.program(500, &[0xC3; 8]).unwrap();
    let (first, second) = (device.read(400, 8), device.read(500, 8));
    let mut twin = Twin::of(&device.trace());
    twin.program(500, &[0xC3; 8]).unwrap();
    twin.program(400, &[0x3C; 8]).unwrap();
    assert_eq!(twin.read(400, 8), Ok(Ok(first)));
    assert_eq!(twin.read(500, 8), Ok(Ok(second)));
}

#[test]
fn a_failed_erase_fails_again_and_the_bytes_after_it_are_the_devices() {
    let mut device = Device::new();
    device.nor.fail = true;
    assert_eq!(device.erase(4096, 4096), Err(Error::Failed));
    let after = device.read(4096, 4096);
    let trace = device.trace();

    let mut twin = Twin::of(&trace);
    assert_eq!(twin.erase(4096, 4096), Err(StorageError::Failed));
    assert_eq!(twin.read(4096, 4096), Ok(Ok(after)));

    let mut skips = Twin::of(&trace);
    assert_eq!(skips.read(4096, 16), unknown(4096, 16, Why::Unread));
}

#[test]
fn card_writes_the_recorder_joined_match_one_by_one() {
    let mut device = Device::new();
    for at in 10..13 {
        device.write_sector(at, &[at as u8; SECTOR_SIZE]);
    }
    device.pass();
    let mut ring = ReadWriteRecorder::new(FailingCard, &device.log, &device.card_seen);
    assert!(ring.owned_write(11, &[0xAA; SECTOR_SIZE]).is_err());
    let after = device.sector(11);
    let mut twin = Twin::of(&device.trace());
    for at in 10..13 {
        twin.write_sector(at, &[at as u8; SECTOR_SIZE]).unwrap();
    }
    assert_eq!(
        twin.write_sector(11, &[0xAA; SECTOR_SIZE]),
        Err(StorageError::Failed)
    );
    assert_eq!(twin.sector(11), Ok(Ok(after.to_vec())));
}

struct FailingCard;

impl ReadWrite for FailingCard {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        Err(device_api::bulk::Error::Unsupported)
    }
    fn owned_read(
        &mut self,
        _: u64,
        _: &mut [u8; SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        Err(device_api::bulk::Error::Failed)
    }
    fn owned_write(
        &mut self,
        _: u64,
        _: &[u8; SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        Err(device_api::bulk::Error::Failed)
    }
    fn entropy(&mut self) -> u64 {
        0
    }
}

#[test]
fn two_store_reads_that_differ_are_each_the_input_of_their_occurrence() {
    let store = store();
    let mut device = Device::new();
    let first = device.sector(store);
    device
        .card
        .host()
        .owned_write(u64::from(store), &[0x42; SECTOR_SIZE])
        .unwrap();
    device.pass();
    let second = device.sector(store);
    assert_ne!(first, second);
    let trace = device.trace();
    let index = Index::of(&trace, volumes()).unwrap();
    assert_eq!(index.conflict, None, "STORE belongs to no version");
    let mut twin = Twin::of(&trace);
    assert_eq!(twin.sector(store), Ok(Ok(first.to_vec())));
    assert_eq!(twin.sector(store), Ok(Ok(second.to_vec())));
    assert_eq!(
        twin.sector(store),
        Err(Leave::Unknown {
            span: bulk(store),
            why: Why::Store
        })
    );
}

#[test]
fn two_reads_of_one_version_that_differ_are_reported_as_a_recording_bug() {
    let mut device = Device::new();
    device.read(0, 64);
    device.nor.bytes[40] ^= 0xFF;
    device.pass();
    device.read(32, 64);
    let index = Index::of(&device.trace(), volumes()).unwrap();
    assert_eq!(
        index.conflict.as_deref(),
        Some(
            "two reads of Data byte 40 in era 0, before any recorded mutation, gave different bytes"
        )
    );
}

#[test]
fn a_recorded_read_fault_is_the_input_of_its_occurrence() {
    let mut device = Device::new();
    let good = device.sector(5_000);
    let mut broken = ReadWriteRecorder::new(FailingCard, &device.log, &device.card_seen);
    assert!(broken.owned_read(5_000, &mut [0; SECTOR_SIZE]).is_err());
    let mut twin = Twin::of(&device.trace());
    assert_eq!(twin.sector(5_000), Ok(Ok(good.to_vec())));
    assert_eq!(twin.sector(5_000), Ok(Err(StorageError::Failed)));
    assert_eq!(
        twin.sector(5_000),
        Ok(Ok(good.to_vec())),
        "a fault determines nothing"
    );
}

#[test]
fn a_settings_region_access_is_outside_the_recorded_world() {
    let mut twin = Twin::of(&Device::new().trace());
    let span = Span {
        medium: Medium::Configuration,
        start: 0,
        count: 4,
    };
    assert_eq!(
        twin.0.read(method("read"), span, &mut [0; 4]),
        Err(Leave::Outside { span })
    );
    assert_eq!(twin.0.outside, 1);
}

#[test]
fn units_hold_the_last_value_set_over_each() {
    let mut units = Units::default();
    units.set(10..20, 1);
    units.set(15..25, 2);
    units.set(12..13, 3);
    assert_eq!(
        units.segments(5..30),
        [
            (5..10, None),
            (10..12, Some(1)),
            (12..13, Some(3)),
            (13..15, Some(1)),
            (15..25, Some(2)),
            (25..30, None),
        ]
    );
    assert_eq!(units.get(14), Some(1));
    assert_eq!(units.get(25), None);
}

struct Flaky<'a> {
    nor: &'a mut Nor,
    fail_read: bool,
}

impl OwnedFlash for Flaky<'_> {
    type Error = Error;

    fn availability(&self, region: Region) -> Availability {
        self.nor.availability(region)
    }

    fn geometry(&self, region: Region) -> Geometry {
        self.nor.geometry(region)
    }

    fn read(&mut self, region: Region, offset: usize, out: &mut [u8]) -> Result<(), Error> {
        self.nor.read(region, offset, out)?;
        match core::mem::take(&mut self.fail_read) {
            true => Err(Error::Failed),
            false => Ok(()),
        }
    }

    fn program(&mut self, region: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        self.nor.program(region, offset, bytes)
    }

    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), Error> {
        self.nor.erase(region, offset, length)
    }
}

impl Device {
    fn flaky_read(&mut self, at: usize, len: usize, fail: bool) -> Result<Vec<u8>, Error> {
        let mut out = vec![0; len];
        let mut flaky = Flaky {
            nor: &mut self.nor,
            fail_read: fail,
        };
        let mut flash = OwnedFlashRecorder::new(&mut flaky, &self.log, &self.flash_seen);
        flash.read(Region::Data, at, &mut out).map(|()| out)
    }
}

#[test]
fn a_read_fault_longer_than_two_parts_is_the_input_of_its_occurrence() {
    for len in [2048, 3072, 4096] {
        let mut device = Device::new();
        assert!(device.flaky_read(8192, len, true).is_err());
        let mut twin = Twin::of(&device.trace());
        assert_eq!(
            twin.read(8192, len).map(|read| read.map(|_| ())),
            Ok(Err(StorageError::Failed)),
            "a {len}-byte read"
        );
    }
}

#[test]
fn reads_the_recorder_joined_into_one_digest_shift_no_later_read_fault() {
    let mut device = Device::new();
    device.flaky_read(0, 64, false).unwrap();
    device.flaky_read(64, 64, false).unwrap();
    device.pass();
    device.flaky_read(0, 64, false).unwrap();
    device.flaky_read(64, 64, false).unwrap();
    assert!(device.flaky_read(0, 64, true).is_err());
    let mut twin = Twin::of(&device.trace());
    let mut read = |at| twin.read(at, 64).map(|read| read.is_ok());
    let reads = [read(0), read(64), read(0), read(64), read(0)];
    assert_eq!(reads, [Ok(true), Ok(true), Ok(true), Ok(true), Ok(false)]);
}

#[test]
fn a_failed_program_of_other_bytes_never_reads_the_devices_torn_bytes() {
    let mut device = Device::new();
    device.nor.fail = true;
    assert!(device.program(100, &[0x0F; 16]).is_err());
    let after = device.read(100, 16);
    let trace = device.trace();

    let mut other = Twin::of(&trace);
    assert_eq!(other.program(100, &[0xF0; 16]), Err(StorageError::Failed));
    let stop = other.read(100, 16);
    assert_eq!(stop, unknown(100, 16, Why::Own(Off::Torn)));
    assert_eq!(
        stop.unwrap_err().to_string(),
        "unknown media Data 100+16: the twin's own write there failed, and the recording's failed write there attempted other bytes"
    );

    let mut same = Twin::of(&trace);
    assert_eq!(same.program(100, &[0x0F; 16]), Err(StorageError::Failed));
    assert_eq!(
        same.read(100, 16),
        Ok(Ok(after)),
        "the same attempt, in step"
    );
}

#[test]
fn a_failed_card_write_of_other_bytes_never_reads_the_devices_bytes() {
    let mut device = Device::new();
    let mut ring = ReadWriteRecorder::new(FailingCard, &device.log, &device.card_seen);
    assert!(ring.owned_write(11, &[0xAA; SECTOR_SIZE]).is_err());
    device.sector(11);
    let mut twin = Twin::of(&device.trace());
    assert_eq!(
        twin.write_sector(11, &[0xBB; SECTOR_SIZE]),
        Err(StorageError::Failed)
    );
    assert_eq!(
        twin.sector(11),
        Err(Leave::Unknown {
            span: bulk(11),
            why: Why::Own(Off::Torn)
        })
    );
}

#[test]
fn a_host_session_never_restarts_a_card_write_the_twin_skipped_or_made_otherwise() {
    let mut device = Device::new();
    device.write_sector(5_000, &[0xAB; SECTOR_SIZE]);
    device.pass();
    device.log.borrow_mut().forget(Medium::Bulk);
    device.pass();
    let after = device.sector(5_000);
    let untouched = device.sector(6_000);
    let trace = device.trace();
    let stop = |off| {
        Err(Leave::Unknown {
            span: bulk(5_000),
            why: Why::Own(off),
        })
    };

    let mut skips = Twin::of(&trace);
    skips.0.forget(Medium::Bulk);
    assert_eq!(skips.sector(5_000), stop(Off::Skipped));
    assert_eq!(
        skips.sector(6_000),
        Ok(Ok(untouched.to_vec())),
        "a unit neither touched"
    );

    let mut other = Twin::of(&trace);
    other.write_sector(5_000, &[0xCD; SECTOR_SIZE]).unwrap();
    other.0.forget(Medium::Bulk);
    assert_eq!(other.sector(5_000), stop(Off::Unmatched));

    let mut same = Twin::of(&trace);
    same.write_sector(5_000, &[0xAB; SECTOR_SIZE]).unwrap();
    same.0.forget(Medium::Bulk);
    assert_eq!(same.sector(5_000), Ok(Ok(after.to_vec())), "in step");
}

fn terrain_pack(first: u32, sectors: usize) -> Pack {
    let path = std::env::temp_dir().join(format!("index-pack-{}", std::process::id()));
    std::fs::write(&path, vec![0x77; sectors * SECTOR_SIZE]).unwrap();
    let file = std::fs::File::open(&path).unwrap();
    std::fs::remove_file(&path).unwrap();
    Pack {
        file,
        first: u64::from(first),
        length: (sectors * SECTOR_SIZE) as u64,
    }
}

#[test]
fn a_terrain_sector_a_failed_write_touched_is_never_refilled_from_the_pack() {
    let terrain = volumes().terrain.unwrap().start as u32;
    let device = Device::new();
    let mut ring = ReadWriteRecorder::new(FailingCard, &device.log, &device.card_seen);
    assert!(
        ring.owned_write(terrain.into(), &[0xAA; SECTOR_SIZE])
            .is_err()
    );
    let trace = device.trace();
    let twin = |attempt: u8| {
        let pack = terrain_pack(terrain, 2);
        let mut twin = Twin(Model::new(
            Index::of(&trace, volumes()).unwrap(),
            Some(pack),
            false,
        ));
        let failed = twin.write_sector(terrain, &[attempt; SECTOR_SIZE]);
        assert_eq!(failed, Err(StorageError::Failed));
        twin
    };
    let unknown = |why| {
        Err(Leave::Unknown {
            span: bulk(terrain),
            why,
        })
    };

    let mut same = twin(0xAA);
    assert_eq!(same.sector(terrain), unknown(Why::Unread));
    assert_eq!(
        same.sector(terrain + 1),
        Ok(Ok(vec![0x77; SECTOR_SIZE])),
        "an untouched sector"
    );
    let mut other = twin(0xBB);
    assert_eq!(other.sector(terrain), unknown(Why::Own(Off::Torn)));
}

#[test]
fn a_fault_after_adjacent_whole_part_reads_lands_on_its_own_read() {
    let mut device = Device::new();
    device.flaky_read(0, PART, false).unwrap();
    device.flaky_read(PART, PART, false).unwrap();
    device.pass();
    assert!(device.flaky_read(PART, PART, true).is_err());
    let mut twin = Twin::of(&device.trace());
    let mut read = |at| twin.read(at, PART).map(|read| read.is_ok());
    let reads = [read(0), read(PART), read(PART)];
    assert_eq!(reads, [Ok(true), Ok(true), Ok(false)]);
}

#[test]
fn adjacent_whole_part_faults_each_give_their_own_fault() {
    for faults in [2, 3] {
        let mut device = Device::new();
        for at in (0..faults).map(|n| n * PART) {
            assert!(device.flaky_read(at, PART, true).is_err());
        }
        let mut twin = Twin::of(&device.trace());
        for at in (0..faults).map(|n| n * PART) {
            assert_eq!(
                twin.read(at, PART).map(|read| read.is_ok()),
                Ok(false),
                "the fault at {at} of {faults}"
            );
        }
    }
}

#[test]
fn reads_joined_into_one_digest_shift_no_later_fault_at_the_second_start() {
    let mut device = Device::new();
    device.flaky_read(0, 64, false).unwrap();
    device.flaky_read(64, 64, false).unwrap();
    device.pass();
    device.flaky_read(0, 64, false).unwrap();
    device.flaky_read(64, 64, false).unwrap();
    assert!(device.flaky_read(64, 64, true).is_err());
    let mut twin = Twin::of(&device.trace());
    let mut read = |at| twin.read(at, 64).map(|read| read.is_ok());
    let reads = [read(0), read(64), read(0), read(64), read(64)];
    assert_eq!(reads, [Ok(true), Ok(true), Ok(true), Ok(true), Ok(false)]);
}

fn record_part(device: &Device, at: u32) {
    let mut body = [0u8; 128];
    let mut w = Writer::new(&mut body, device.now);
    w.byte(firmware_services::trace::media::INLINE | firmware_services::trace::media::CONTINUES);
    Medium::Data.put(&mut w);
    at.put(&mut w);
    64u32.put(&mut w);
    w.varint(64 << 2);
    w.bytes(&[0x5A; 64]);
    let len = w.written().unwrap();
    let call = trace::Call {
        method: method("read"),
        now_ms: device.now,
        body: &body[..len],
    };
    (device.log.borrow_mut().recorder.as_mut())
        .expect("a recorder")
        .record(&Event::Call(call));
}

#[test]
fn a_read_whose_first_part_was_lost_indexes_up_to_the_loss() {
    let mut device = Device::new();
    let before = device.read(0, 64);
    device.pass();
    (device.log.borrow_mut().recorder.as_mut())
        .expect("a recorder")
        .record(&Event::Lost {
            blocks: 1,
            bytes: 900,
        });
    record_part(&device, 4096);
    let mut twin = Twin::of(&device.trace());
    assert_eq!(
        twin.read(0, 64),
        Ok(Ok(before)),
        "the media before the loss"
    );
}

#[test]
fn a_part_that_goes_on_no_read_is_a_malformed_trace() {
    let device = Device::new();
    record_part(&device, 4096);
    let error = Index::of(&device.trace(), volumes()).err();
    assert!(
        error.is_some_and(|error| error.to_string().contains("Data 4096+64")),
        "a part that goes on no read stops the index"
    );
}
