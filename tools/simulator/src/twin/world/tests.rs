use super::*;
use crate::bulk::SimBulk;
use crate::replay::Blocks;
use device_api::bulk::{Error as CardError, OwnedInfo, ReadWrite, ReadWriteRecorder, SECTOR_SIZE};
use firmware_services::trace::log::Log;
use firmware_services::trace::media::{Memory, Volumes};
use firmware_services::trace::recorder::Recorder;
use firmware_services::volume::{self, Extent};
use std::cell::RefCell;

const PLAIN: u64 = 3;
const TERRAIN: u64 = 40;
const STORE: u64 = 90;
const BROKEN: u64 = 5;

struct Card;

fn sector(number: u64) -> [u8; SECTOR_SIZE] {
    [number as u8; SECTOR_SIZE]
}

impl ReadWrite for Card {
    fn owned_info(&mut self) -> Result<OwnedInfo, CardError> {
        Err(CardError::Unsupported)
    }
    fn owned_read(&mut self, at: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), CardError> {
        if at == BROKEN {
            return Err(CardError::Failed);
        }
        *out = sector(at);
        Ok(())
    }
    fn owned_write(&mut self, _: u64, _: &[u8; SECTOR_SIZE]) -> Result<(), CardError> {
        Ok(())
    }
    fn entropy(&mut self) -> u64 {
        7
    }
}

fn recorded(sectors: &[u64], after: &[Event<'_>]) -> Vec<u8> {
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
    let extent = |name, start| {
        Some(Extent {
            name,
            start,
            sectors: 10,
        })
    };
    log.volumes(Volumes {
        terrain: extent(volume::TERRAIN, TERRAIN),
        store: extent(volume::STORE, STORE),
    });
    log.begin_pass(1_000, false);
    let log = RefCell::new(log);
    let observed = RefCell::default();
    let mut card = ReadWriteRecorder::new(Card, &log, &observed);
    for &at in sectors {
        let _ = card.owned_read(at, &mut [0; SECTOR_SIZE]);
    }
    let mut log = log.into_inner();
    log.begin_pass(1_010, false);
    for event in after {
        log.record(event);
    }
    let recorder = log.recorder.as_mut().unwrap();
    let mut bytes = recorder.sink().0.clone();
    bytes.extend_from_slice(recorder.open_block());
    bytes
}

fn read_method() -> MethodId {
    script::named("ReadWrite", "owned_read").unwrap()
}

fn read(world: &mut Recorded, at: u32) -> Result<Result<[u8; SECTOR_SIZE], StorageError>, String> {
    let mut out = [0u8; SECTOR_SIZE];
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        world.media_read(read_method(), Medium::Bulk, at, &mut out)
    }));
    match outcome {
        Ok(outcome) => Ok(outcome.map(|()| out)),
        Err(panic) if panic.is::<Left>() => Err(world.parted().unwrap().to_string()),
        Err(panic) => std::panic::resume_unwind(panic),
    }
}

fn replaying(trace: Vec<u8>, pack: Option<Pack>) -> Recorded {
    let mut world = Recorded::open(trace, pack).unwrap();
    assert!(matches!(world.advance(), Next::Pass));
    world
}

#[test]
fn a_first_read_gives_the_bytes_the_trace_carries_and_a_later_one_the_same() {
    let trace = recorded(&[PLAIN, PLAIN], &[]);
    let mut world = replaying(trace.clone(), None);
    assert_eq!(read(&mut world, PLAIN as u32), Ok(Ok(sector(PLAIN))));
    assert_eq!(read(&mut world, PLAIN as u32), Ok(Ok(sector(PLAIN))));
    let report = world.report(&trace);
    let faithful = report.faithful.unwrap();
    assert_eq!(faithful.exclusions, [], "{faithful}");
}

#[test]
fn a_read_the_trace_holds_no_bytes_of_leaves_the_recorded_world() {
    let mut world = replaying(recorded(&[PLAIN], &[]), None);
    assert_eq!(
        read(&mut world, 7),
        Err("left the recorded world: unknown media Bulk 7+1".into())
    );
}

#[test]
fn a_terrain_read_is_served_from_the_pack_and_listed() {
    let trace = recorded(&[TERRAIN], &[]);
    let pack = SimBulk::default();
    pack.set_sectors(200);
    let held = pack.sector(TERRAIN);
    let mut world = replaying(trace.clone(), Some(Pack::Card(pack)));
    assert_eq!(read(&mut world, TERRAIN as u32), Ok(Ok(held)));
    let faithful = world.report(&trace).faithful.unwrap();
    assert_eq!(
        faithful.exclusions,
        [Exclusion::TerrainByDigest(1)],
        "{faithful}"
    );

    let mut packless = replaying(trace, None);
    assert_eq!(
        read(&mut packless, TERRAIN as u32),
        Err(format!(
            "left the recorded world: unknown media Bulk {TERRAIN}+1"
        ))
    );
}

#[test]
fn a_store_read_is_served_from_the_bytes_every_read_of_it_carries() {
    let trace = recorded(&[STORE, STORE], &[]);
    let mut world = replaying(trace.clone(), None);
    let held = [STORE as u8; SECTOR_SIZE];
    assert_eq!(read(&mut world, STORE as u32), Ok(Ok(held)));
    assert_eq!(read(&mut world, STORE as u32), Ok(Ok(held)));
    let faithful = world.report(&trace).faithful.unwrap();
    assert_eq!(faithful.exclusions, [], "{faithful}");
}

#[test]
fn a_read_that_failed_fails_again_and_determines_nothing() {
    let mut world = replaying(recorded(&[BROKEN], &[]), None);
    assert_eq!(
        read(&mut world, BROKEN as u32),
        Ok(Err(StorageError::Failed))
    );
    assert_eq!(
        read(&mut world, BROKEN as u32),
        Err(format!(
            "left the recorded world: unknown media Bulk {BROKEN}+1"
        ))
    );
}

#[test]
fn a_replay_ends_where_the_recording_lost_world_records() {
    let lost = Event::Lost {
        blocks: 2,
        bytes: 900,
    };
    let trace = recorded(&[PLAIN], &[lost]);
    let mut world = Recorded::open(trace.clone(), None).unwrap();
    assert!(
        matches!(world.advance(), Next::End),
        "the pass before the loss may be cut"
    );
    let report = world.report(&trace);
    assert_eq!(report.count(Kind::Lost), 1, "{report}");
    let faithful = report.faithful.unwrap();
    assert_eq!(
        faithful.exclusions,
        [Exclusion::Lost { pass_ms: 1_000 }],
        "{faithful}"
    );
}
