use super::*;
use crate::bulk::SimBulk;
use crate::replay::Blocks;
use device_api::bulk::{Error as CardError, OwnedInfo, ReadWrite, ReadWriteRecorder, SECTOR_SIZE};
use firmware_services::trace::log::Log;
use firmware_services::trace::media::{Memory, Volumes};
use firmware_services::trace::recorder::Recorder;
use firmware_services::volume;
use std::cell::RefCell;

const PLAIN: u64 = 3_000;
const BROKEN: u64 = 5_000;

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

fn terrain() -> u64 {
    volumes().terrain.unwrap().start
}

fn store() -> u64 {
    volumes().store.unwrap().start
}

struct Card(SimBulk);

impl ReadWrite for Card {
    fn owned_info(&mut self) -> Result<OwnedInfo, CardError> {
        self.0.owned_info()
    }
    fn owned_read(&mut self, at: u64, out: &mut [u8; SECTOR_SIZE]) -> Result<(), CardError> {
        if at == BROKEN {
            return Err(CardError::Failed);
        }
        self.0.owned_read(at, out)
    }
    fn owned_write(&mut self, at: u64, bytes: &[u8; SECTOR_SIZE]) -> Result<(), CardError> {
        self.0.owned_write(at, bytes)
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
    let log = Log::new(Some(recorder), Box::new(Memory::new()));
    let log = RefCell::new(log);
    log.borrow_mut().begin_pass(1_000, false);
    let observed = RefCell::default();
    let mut card = ReadWriteRecorder::new(Card(card().host()), &log, &observed);
    let volumes = Volumes::read(&mut card, &mut [0; SECTOR_SIZE]);
    log.borrow_mut().volumes(volumes);
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
    let mut world = Recorded::open(trace, pack, Options::default()).unwrap();
    assert!(matches!(world.advance(), Next::Pass));
    world
}

#[test]
fn a_first_read_gives_the_bytes_the_trace_carries_and_a_later_one_the_same() {
    let trace = recorded(&[PLAIN, PLAIN], &[]);
    let mut world = replaying(trace.clone(), None);
    let held = card().sector(PLAIN);
    assert_eq!(read(&mut world, PLAIN as u32), Ok(Ok(held)));
    assert_eq!(read(&mut world, PLAIN as u32), Ok(Ok(held)));
    let report = world.report(&trace);
    let faithful = report.faithful.unwrap();
    assert_eq!(faithful.exclusions, [], "{faithful}");
}

#[test]
fn a_read_the_trace_holds_no_bytes_of_leaves_the_recorded_world() {
    let mut world = replaying(recorded(&[PLAIN], &[]), None);
    assert_eq!(
        read(&mut world, 7),
        Err("left the recorded world at 00:01.000: unknown media Bulk 7+1".into())
    );
}

#[test]
fn a_terrain_read_is_served_from_the_pack_and_listed() {
    let terrain = terrain() + firmware_services::pack::DATA;
    let trace = recorded(&[terrain], &[]);
    let held = card().sector(terrain);
    let path = std::env::temp_dir().join(format!("pack-{}", std::process::id()));
    std::fs::write(&path, held).unwrap();
    let pack = Pack {
        file: std::fs::File::open(&path).unwrap(),
        first: terrain,
        length: SECTOR_SIZE as u64,
    };
    std::fs::remove_file(&path).unwrap();
    let mut world = replaying(trace.clone(), Some(pack));
    assert_eq!(read(&mut world, terrain as u32), Ok(Ok(held)));
    let faithful = world.report(&trace).faithful.unwrap();
    assert_eq!(
        faithful.exclusions,
        [Exclusion::TerrainByDigest(1)],
        "{faithful}"
    );

    let mut packless = replaying(trace, None);
    assert_eq!(
        read(&mut packless, terrain as u32),
        Err(format!(
            "left the recorded world at 00:01.000: unknown media Bulk {terrain}+1"
        ))
    );
}

#[test]
fn a_store_read_is_served_from_the_bytes_every_read_of_it_carries() {
    let store = store();
    let trace = recorded(&[store, store], &[]);
    let mut world = replaying(trace.clone(), None);
    let held = card().sector(store);
    assert_eq!(read(&mut world, store as u32), Ok(Ok(held)));
    assert_eq!(read(&mut world, store as u32), Ok(Ok(held)));
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
            "left the recorded world at 00:01.000: unknown media Bulk {BROKEN}+1"
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
    let mut world = Recorded::open(trace.clone(), None, Options::default()).unwrap();
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

#[test]
fn a_flash_region_the_trace_sampled_no_geometry_of_replays_as_unreserved() {
    use device_api::storage::{Geometry, OwnedFlash, Region};
    let mut body = [0u8; 64];
    let mut w = Writer::new(&mut body, 1_010);
    Region::Data.put(&mut w);
    Geometry {
        capacity: 0x100000,
        program_size: 4,
        erase_size: 4096,
        durable: true,
    }
    .put(&mut w);
    let len = w.written().unwrap();
    let data = Event::Call(trace::Call {
        method: script::named("OwnedFlash", "geometry").unwrap(),
        now_ms: 1_010,
        body: &body[..len],
    });
    let world = std::rc::Rc::new(RefCell::new(replaying(recorded(&[], &[data]), None)));
    let flash = crate::twin::flash::ScriptedFlash::new(world.clone());
    assert_eq!(flash.geometry(Region::Screens).capacity, 0);
    assert_eq!(world.borrow().parted(), None);
}

#[test]
fn a_flash_geometry_the_trace_never_observed_leaves_the_recorded_world() {
    use device_api::storage::{OwnedFlash, Region};
    let world = std::rc::Rc::new(RefCell::new(replaying(recorded(&[], &[]), None)));
    let flash = crate::twin::flash::ScriptedFlash::new(world.clone());
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        flash.geometry(Region::Data)
    }));
    assert!(outcome.is_err_and(|panic| panic.is::<Left>()));
    assert_eq!(
        world.borrow().parted(),
        Some("left the recorded world at 00:01.000: OwnedFlash::geometry was never observed")
    );
}
