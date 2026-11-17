use super::*;
use crate::testing::Media as Card;
use crate::trace::determined::CAPACITY;
use crate::trace::media::{Access, Content};
use crate::trace::{Cursor, Events};
use crate::volume::{self, Extent};
use core::cell::RefCell;
use device_api::bulk::{ReadWrite, ReadWriteRecorder, SECTOR_SIZE};
use device_api::capability::Kind;
use device_api::input::CAPABILITY_INPUT_SOURCE;
use device_api::observation::Availability;
use device_api::server::{AccessPoint, AccessPointRecorder, AccessPointState, Reach};
use device_api::server::{Method, Server, ServerRecorder};
use device_api::storage::{Geometry, OwnedFlash, OwnedFlashRecorder};
use std::boxed::Box;
use std::vec;
use std::vec::Vec;

#[derive(Default)]
struct Blocks(Vec<u8>);

impl Sink for Blocks {
    fn block(&mut self, bytes: &[u8]) -> bool {
        self.0.extend_from_slice(bytes);
        true
    }
}

type TestLog = Log<Vec<u8>, Blocks, Box<Memory>>;

fn log() -> RefCell<TestLog> {
    let recorder = Recorder::new(vec![0u8; 4096], Blocks::default());
    let mut log = Log::new(Some(recorder), Box::new(Memory::new()));
    log.begin_pass(1_000, false);
    RefCell::new(log)
}

fn recorded<T>(log: &RefCell<TestLog>, read: impl Fn(Event<'_>) -> Option<T>) -> Vec<T> {
    let mut log = log.borrow_mut();
    log.flush_run();
    let recorder = log.recorder.as_mut().unwrap();
    recorder.flush();
    let bytes = core::mem::take(&mut recorder.sink_mut().0);
    let cursor = Cursor {
        block: 0,
        at: 0,
        now: 0,
        end: None,
        checked: None,
    };
    let mut events = Events::resume(&bytes, cursor);
    let all = events.by_ref().filter_map(read).collect();
    assert_eq!(events.end(), Some(crate::trace::End::Complete));
    all
}

fn accesses(log: &RefCell<TestLog>) -> Vec<std::string::String> {
    recorded(log, |event| {
        let Event::Call(call) = event else {
            return None;
        };
        let access = Access::of(&call).expect("a media record");
        let what = match access.content {
            _ if access.outside => " outside",
            Content::Inline(_) => " bytes",
            Content::Digest(_) => " crc",
            Content::None => "",
        };
        let failed = if access.fault.is_some() { "!" } else { "" };
        Some(std::format!(
            "{}.{} {:?} {}+{}{what}{failed}",
            call.method.capability,
            call.method.operation,
            access.span.medium,
            access.span.start,
            access.span.count,
        ))
    })
}

fn sector(fill: u8) -> [u8; SECTOR_SIZE] {
    [fill; SECTOR_SIZE]
}

#[test]
fn a_first_read_carries_its_bytes_and_a_later_one_their_digest() {
    let log = log();
    let mut media = Card::new(64);
    let mut written = sector(0xff);
    written[..3].copy_from_slice(b"abc");
    media.set(5, written);
    let mut card = ReadWriteRecorder::new(media, &log, Box::default());
    let mut out = sector(0);
    card.owned_read(5, &mut out).unwrap();
    let inline = recorded(&log, |event| {
        let Event::Call(call) = event else {
            return None;
        };
        let access = Access::of(&call).unwrap();
        let mut bytes = sector(0);
        assert_eq!(access.expand(&mut bytes), Some(SECTOR_SIZE));
        Some((call.body.len(), bytes))
    });
    assert_eq!(inline.len(), 1);
    assert_eq!(inline[0].1, written);
    assert!(
        inline[0].0 < 16,
        "the blank tail is elided: {} bytes",
        inline[0].0
    );

    card.owned_read(5, &mut out).unwrap();
    let digest = recorded(&log, |event| {
        let Event::Call(call) = event else {
            return None;
        };
        match Access::of(&call).unwrap().content {
            Content::Digest(crc) => Some(crc),
            _ => None,
        }
    });
    assert_eq!(digest, [crate::checksum::crc32(&written)]);
}

#[test]
fn accesses_that_follow_one_another_share_a_record() {
    let log = log();
    let mut card = ReadWriteRecorder::new(Card::new(64), &log, Box::default());
    let mut out = sector(0);
    for index in [3, 4, 5, 9] {
        card.owned_write(index, &sector(index as u8)).unwrap();
    }
    for index in [3, 4, 5, 4] {
        card.owned_read(index, &mut out).unwrap();
    }
    assert_eq!(
        accesses(&log),
        [
            "4.2 Bulk 3+3 crc",
            "4.2 Bulk 9+1 crc",
            "4.1 Bulk 3+3 crc",
            "4.1 Bulk 4+1 crc"
        ],
        "a card write determines its sectors, so their reads are digests"
    );
    card.owned_read(6, &mut out).unwrap();
    card.owned_read(7, &mut out).unwrap();
    assert_eq!(accesses(&log), ["4.1 Bulk 6+1 bytes", "4.1 Bulk 7+1 bytes"]);

    let crcs = {
        card.owned_write(1, &sector(1)).unwrap();
        card.owned_write(2, &sector(2)).unwrap();
        recorded(&log, |event| {
            let Event::Call(call) = event else {
                return None;
            };
            match Access::of(&call).unwrap().content {
                Content::Digest(crc) => Some(crc),
                _ => None,
            }
        })
    };
    let both = crate::checksum::Crc32::new()
        .update(&sector(1))
        .update(&sector(2));
    assert_eq!(crcs, [both.finish()]);
}

#[test]
fn a_run_ends_at_any_other_record_and_at_the_pass() {
    let log = log();
    let mut card = ReadWriteRecorder::new(Card::new(64), &log, Box::default());
    card.owned_write(1, &sector(1)).unwrap();
    card.entropy();
    card.owned_write(2, &sector(2)).unwrap();
    log.borrow_mut().begin_pass(1_010, false);
    card.owned_write(3, &sector(3)).unwrap();
    let order = recorded(&log, |event| match event {
        Event::Call(call) => Some(call.method.operation),
        Event::Pass { .. } => Some(99),
        _ => None,
    });
    assert_eq!(order, [99, 2, 3, 2, 99, 2]);
}

#[test]
fn a_failed_write_forgets_what_it_attempted_and_a_failed_read_determines_nothing() {
    let log = log();
    let mut card = ReadWriteRecorder::new(Card::new(64), &log, Box::default());
    let mut out = sector(0);
    card.owned_write(8, &sector(8)).unwrap();
    card.unrecorded_mut().cut = Some((0, crate::testing::Tear::Prefix(100)));
    assert!(card.owned_write(8, &sector(9)).is_err());
    assert!(card.owned_read(8, &mut out).is_err());
    card.unrecorded_mut().reboot();
    card.owned_read(8, &mut out).unwrap();
    card.owned_read(8, &mut out).unwrap();
    assert_eq!(
        accesses(&log),
        [
            "4.2 Bulk 8+1 crc",
            "4.2 Bulk 8+1!",
            "4.1 Bulk 8+1 bytes!",
            "4.1 Bulk 8+1 bytes",
            "4.1 Bulk 8+1 crc",
        ]
    );
}

#[test]
fn terrain_is_always_a_digest_and_the_store_always_its_bytes() {
    let log = log();
    let extent = |name, start, sectors| {
        Some(Extent {
            name,
            start,
            sectors,
        })
    };
    log.borrow_mut().volumes(Volumes {
        terrain: extent(volume::TERRAIN, 16, 16),
        store: extent(volume::STORE, 32, 16),
    });
    let mut card = ReadWriteRecorder::new(Card::new(64), &log, Box::default());
    let mut out = sector(0);
    for index in [16, 17, 16, 40] {
        card.owned_read(index, &mut out).unwrap();
    }
    card.owned_write(41, &sector(1)).unwrap();
    for index in [41, 40] {
        card.owned_read(index, &mut out).unwrap();
    }
    assert_eq!(
        accesses(&log),
        [
            "4.1 Bulk 16+2 crc",
            "4.1 Bulk 16+1 crc",
            "4.1 Bulk 40+1 bytes",
            "4.2 Bulk 41+1 crc",
            "4.1 Bulk 41+1 bytes",
            "4.1 Bulk 40+1 bytes",
        ]
    );
}

#[test]
fn a_host_write_forgets_the_medium() {
    let log = log();
    let mut card = ReadWriteRecorder::new(Card::new(64), &log, Box::default());
    let mut out = sector(0);
    card.owned_read(2, &mut out).unwrap();
    log.borrow_mut().forget(Medium::Bulk);
    card.owned_read(2, &mut out).unwrap();
    let kinds = recorded(&log, |event| match event {
        Event::Call(call) => Some(match Access::of(&call).unwrap().content {
            Content::Inline(_) => "bytes",
            _ => "other",
        }),
        Event::Forget(Medium::Bulk) => Some("forget"),
        _ => None,
    });
    assert_eq!(kinds, ["bytes", "forget", "bytes"]);
}

/// Internal flash: a program clears bits, an erase sets a whole range.
struct Nor {
    data: Vec<u8>,
    fail: bool,
}

impl OwnedFlash for Nor {
    type Error = Error;
    fn availability(&self, _: Region) -> Availability {
        Availability::Ready
    }
    fn geometry(&self, _: Region) -> Geometry {
        Geometry {
            capacity: self.data.len(),
            program_size: 1,
            erase_size: 4096,
        }
    }
    fn read(&mut self, _: Region, offset: usize, output: &mut [u8]) -> Result<(), Error> {
        if self.fail {
            return Err(Error::Failed);
        }
        output.copy_from_slice(&self.data[offset..offset + output.len()]);
        Ok(())
    }
    fn program(&mut self, _: Region, offset: usize, bytes: &[u8]) -> Result<(), Error> {
        if self.fail {
            return Err(Error::Failed);
        }
        for (slot, byte) in self.data[offset..].iter_mut().zip(bytes) {
            *slot &= byte;
        }
        Ok(())
    }
    fn erase(&mut self, _: Region, offset: usize, length: usize) -> Result<(), Error> {
        if self.fail {
            return Err(Error::Failed);
        }
        self.data[offset..offset + length].fill(0xff);
        Ok(())
    }
}

#[test]
fn a_program_keeps_determined_bytes_determined_and_undetermines_any_other() {
    let log = log();
    let nor = Nor {
        data: (0..8192).map(|i| i as u8).collect(),
        fail: false,
    };
    let mut flash = OwnedFlashRecorder::new(nor, &log, Box::default());
    flash.sample();
    recorded(&log, |_| None::<()>);
    let mut out = [0u8; 16];

    flash.erase(Region::Data, 0, 4096).unwrap();
    flash.program(Region::Data, 0, &[1; 16]).unwrap();
    flash.read(Region::Data, 0, &mut out).unwrap();
    assert_eq!(
        accesses(&log),
        [
            "17.4 Data 0+4096",
            "17.3 Data 0+16 crc",
            "17.2 Data 0+16 crc"
        ],
        "erased bytes are known, so a program onto them is too"
    );

    flash.read(Region::Data, 4096, &mut out).unwrap();
    flash.program(Region::Data, 4100, &[0; 16]).unwrap();
    flash.read(Region::Data, 4096, &mut out).unwrap();
    assert_eq!(
        accesses(&log),
        [
            "17.2 Data 4096+16 bytes",
            "17.3 Data 4100+16 crc",
            "17.2 Data 4096+16 bytes",
        ],
        "a program that reaches past what the trace holds undetermines its range"
    );

    flash.unrecorded_mut().fail = true;
    assert!(flash.erase(Region::Data, 0, 4096).is_err());
    flash.unrecorded_mut().fail = false;
    flash.read(Region::Data, 0, &mut out).unwrap();
    assert_eq!(
        accesses(&log),
        ["17.4 Data 0+4096!", "17.2 Data 0+16 bytes"]
    );
}

#[test]
fn a_long_flash_read_splits_and_the_settings_region_is_outside() {
    let log = log();
    let nor = Nor {
        data: (0..8192).map(|i| (i / 3) as u8).collect(),
        fail: false,
    };
    let mut flash = OwnedFlashRecorder::new(nor, &log, Box::default());
    flash.sample();
    recorded(&log, |_| None::<()>);
    let mut out = vec![0u8; 2500];
    flash.read(Region::Crash, 100, &mut out).unwrap();
    flash.read(Region::Configuration, 0, &mut out[..8]).unwrap();
    assert_eq!(
        accesses(&log),
        [
            "17.2 Crash 100+1024 bytes",
            "17.2 Crash 1124+1024 bytes",
            "17.2 Crash 2148+452 bytes",
            "17.2 Configuration 0+8 outside",
        ]
    );
    flash.read(Region::Crash, 1000, &mut out[..1000]).unwrap();
    assert_eq!(
        accesses(&log),
        ["17.2 Crash 1000+1000 crc"],
        "the parts determine one range"
    );
}

#[test]
fn the_determined_sets_cost_is_recorded_when_it_changes() {
    let log = log();
    let mut card = ReadWriteRecorder::new(Card::new(4 * CAPACITY as u64), &log, Box::default());
    let mut out = sector(0);
    for index in 0..=CAPACITY as u64 {
        card.owned_read(index * 2, &mut out).unwrap();
    }
    log.borrow_mut().cost();
    log.borrow_mut().cost();
    let costs = recorded(&log, |event| match event {
        Event::Cost(cost) => Some(cost),
        _ => None,
    });
    assert_eq!(
        costs,
        [Cost {
            evictions: 1,
            high_water: CAPACITY as u16
        }]
    );
}

struct Phone {
    request: Option<Request>,
}

impl Server for Phone {
    fn listen(&mut self, _: bool) {}
    fn request(&self) -> Option<Request> {
        self.request.clone()
    }
    fn read(&mut self, _: u32, _: &mut [u8]) -> usize {
        0
    }
    fn room(&self) -> usize {
        100
    }
    fn write(&mut self, _: u32, bytes: &[u8]) -> usize {
        bytes.len()
    }
    fn finish(&mut self, _: u32) {}
}

impl AccessPoint for Phone {
    fn start(&mut self, _: AccessPointConfig) -> Result<(), Error> {
        Ok(())
    }
    fn stop(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn state(&self) -> AccessPointState {
        AccessPointState::Off
    }
    fn reach(&self) -> Reach {
        Reach::default()
    }
    fn entropy(&mut self) -> u32 {
        0xfeed_f00d
    }
}

fn phone() -> Phone {
    Phone {
        request: Some(Request {
            id: 3,
            method: Method::Get,
            path: "/rides".try_into().unwrap(),
            query: "t=0123456789abcdef".try_into().unwrap(),
            session: "0123456789abcdef".try_into().unwrap(),
            gzip: false,
            body: 0,
        }),
    }
}

#[test]
fn a_redacting_log_replaces_the_secrets_and_keeps_what_vana_gets() {
    for redacting in [false, true] {
        let log = log();
        if redacting {
            log.borrow_mut().redact();
            log.borrow_mut().session_token(Some(b"0123456789abcdef"));
        }
        let server = ServerRecorder::new(phone(), &log, Box::default());
        let mut access_point = AccessPointRecorder::new(phone(), &log, Box::default());
        assert_eq!(
            server.request().unwrap().session.as_str(),
            "0123456789abcdef"
        );
        assert_eq!(access_point.entropy(), 0xfeed_f00d);
        let secrets = recorded(&log, |event| {
            let Event::Call(call) = event else {
                return None;
            };
            match (call.method.capability, call.method.operation) {
                (15, 1) => {
                    let request = call.decode::<Option<ServerRequest<'_>>>().unwrap().unwrap();
                    Some(std::format!("{} {}", request.session, request.query))
                }
                (14, 4) => Some(std::format!("{:x}", call.decode::<u64>().unwrap())),
                _ => None,
            }
        });
        let expected = match redacting {
            true => ["**************** t=****************", "0"],
            false => ["0123456789abcdef t=0123456789abcdef", "feedf00d"],
        };
        assert_eq!(secrets, expected);
    }
}

#[test]
fn a_log_that_is_off_still_counts_passes_and_records_nothing() {
    let log = RefCell::new(Log::<Vec<u8>, Blocks, _>::new(
        None,
        Box::new(Memory::new()),
    ));
    log.borrow_mut().begin_pass(10, false);
    let mut card = ReadWriteRecorder::new(Card::new(8), &log, Box::default());
    let mut out = sector(0);
    card.owned_read(1, &mut out).unwrap();
    card.entropy();
    log.borrow_mut().begin_pass(20, true);
    assert_eq!(
        log.borrow().pass(),
        Pass {
            number: 2,
            now_ms: 20
        }
    );
}

#[test]
fn the_sealing_record_is_the_input_sources_edge() {
    let capability = &CAPABILITY_INPUT_SOURCE;
    let method = capability.methods[usize::from(EDGE.operation)];
    assert_eq!((capability.id, method.name), (EDGE.capability, "take_edge"));
    assert!(matches!(method.kind, Kind::Take { .. }));
}
