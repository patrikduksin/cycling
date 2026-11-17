use super::*;
use crate::store::{self, DATA};
use crate::testing::Media;
use crate::trace::recorder::{Record, Recorder, Sink};
use crate::trace::spool::{Budget, Drain, Spool};
use crate::trace::{End, Event, Header, decode, encode_header};
use device_api::bulk::SECTOR_SIZE;
use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

const HEADER: Header<'static> = Header {
    build: [0; 20],
    device: "c606",
    boot: 7,
    terrain: crate::trace::origin::Found::Absent,
};

fn header() -> Vec<u8> {
    let mut bytes = vec![0u8; 256];
    let len = encode_header(&HEADER, &mut bytes).unwrap();
    bytes.truncate(len);
    bytes
}

fn name() -> Name {
    Name::new("traces/000001-00000007.trace").unwrap()
}

struct Kept(Vec<u8>);

impl Sink for Kept {
    fn block(&mut self, bytes: &[u8]) -> bool {
        self.0.extend_from_slice(bytes);
        true
    }
}

/// A session recording through a spool into its trace file, beside a
/// recorder that keeps every block, to compare with.
struct Session {
    media: Media,
    recorder: Recorder<Vec<u8>, Spool<Vec<u8>>>,
    drain: Drain,
    kept: Recorder<Vec<u8>, Kept>,
    sector: Sector,
    now: u64,
    passes: u64,
    closed: bool,
}

impl Session {
    fn new() -> Self {
        let mut media = Media::new(DATA + 8 * u64::from(store::CHUNK));
        let mut sector = [0u8; SECTOR_SIZE];
        let store = store::format(&mut media, 11, &mut sector).unwrap();
        let traces = Dir::new("traces/").unwrap();
        let drain =
            Drain::create(&mut media, store, name(), traces, &header(), 0, &mut sector).unwrap();
        Self {
            media,
            recorder: Recorder::new(vec![0u8; BLOCK], Spool::new(vec![0u8; 64 * 1024])),
            drain,
            kept: Recorder::new(vec![0u8; BLOCK], Kept(header())),
            sector,
            now: 0,
            passes: 0,
            closed: false,
        }
    }

    fn record(&mut self, event: &Event<'_>) {
        self.recorder.record(event);
        self.kept.record(event);
    }

    /// Passes every 10 ms for `ms`, with a 70-byte record every tenth, as
    /// the simulator measures a ride.
    fn ride(&mut self, ms: u64) {
        let end = self.now + ms;
        let sample = str::from_utf8(&[b's'; 66]).unwrap();
        while self.now < end {
            let events = if self.passes.is_multiple_of(10) {
                &[Event::Command(sample)][..]
            } else {
                &[]
            };
            self.pass(events, Budget::Records(1));
        }
    }

    fn upload(&mut self, bytes: usize) {
        let piece = [b'u'; 512];
        let read = Event::Call(crate::trace::Call {
            method: device_api::journal::MethodId {
                capability: 3,
                operation: 4,
            },
            now_ms: 0,
            body: &piece,
        });
        for _ in 0..bytes.div_ceil(8 * 512) {
            self.pass(
                &[read; 8],
                Budget::Records(crate::trace::spool::IDLE_RECORDS),
            );
        }
    }

    fn pass(&mut self, events: &[Event<'_>], budget: Budget) {
        self.record(&Event::Pass { now_ms: self.now });
        for event in events {
            self.record(event);
        }
        if !self.closed {
            self.drain
                .pass(
                    &mut self.recorder,
                    &mut self.media,
                    budget,
                    self.now,
                    &mut self.sector,
                )
                .unwrap();
        }
        self.now += 10;
        self.passes += 1;
    }

    fn salvage(&mut self) -> u32 {
        self.salvage_from(None)
    }

    fn salvage_from(&mut self, ring: Option<&[u8]>) -> u32 {
        let remains = Remains {
            ring: ring.unwrap_or(self.recorder.sink().memory()),
            open: self.recorder.open_memory(),
        };
        let mut store = store::mount(&mut self.media, &mut self.sector).unwrap();
        let mut window = vec![0u8; WINDOW];
        salvage(
            &mut store,
            &mut self.media,
            &name(),
            None,
            remains,
            &mut window,
            &mut self.sector,
        )
        .unwrap()
    }

    fn file(&mut self) -> Vec<u8> {
        let store = store::mount(&mut self.media, &mut self.sector).unwrap();
        let mut reader = store
            .reader(&mut self.media, &name(), &mut self.sector)
            .unwrap();
        let mut bytes = Vec::new();
        let mut out = [0u8; PAYLOAD];
        loop {
            match reader
                .next(&store, &mut self.media, &mut out, &mut self.sector)
                .unwrap()
            {
                Next::Record(n) => bytes.extend_from_slice(&out[..n]),
                Next::Hole { from, to } => panic!("records {from}..{to} lost"),
                Next::End => return bytes,
            }
        }
    }

    fn tail(&mut self) -> (Option<u64>, u32) {
        let store = store::mount(&mut self.media, &mut self.sector).unwrap();
        let mut window = vec![0u8; WINDOW];
        let tail = tail(
            &store,
            &mut self.media,
            &name(),
            &mut window,
            &mut self.sector,
        )
        .unwrap();
        (tail.last_pass_ms, tail.blocks)
    }

    fn kept(&mut self) -> Vec<String> {
        let mut bytes = self.kept.sink().0.clone();
        bytes.extend_from_slice(self.kept.open_block());
        events(&bytes).0
    }
}

fn events(bytes: &[u8]) -> (Vec<String>, Option<End>) {
    let (header, mut events) = decode(bytes).unwrap();
    assert_eq!(header, HEADER);
    let read = events.by_ref().map(|event| format!("{event:?}")).collect();
    (read, events.end())
}

fn last_pass(events: &[String]) -> Option<u64> {
    events.iter().rev().find_map(|event| {
        event
            .strip_prefix("Pass { now_ms: ")?
            .trim_end_matches(" }")
            .parse::<u64>()
            .ok()
    })
}

fn blocks_after_last_sync(file: &[u8], records: &[usize]) -> u32 {
    let synced = records
        .iter()
        .rposition(|&len| len < PAYLOAD)
        .expect("the header record is synced");
    let from: usize = records[..=synced].iter().sum();
    let mut at = from;
    let mut blocks = 0;
    while let Some(len) = file.get(at..at + 2) {
        let end = at + FRAME + usize::from(u16::from_le_bytes([len[0], len[1]]));
        if end > file.len() {
            break;
        }
        blocks += 1;
        at = end;
    }
    blocks
}

fn record_lengths(session: &mut Session) -> Vec<usize> {
    let store = store::mount(&mut session.media, &mut session.sector).unwrap();
    let mut reader = store
        .reader(&mut session.media, &name(), &mut session.sector)
        .unwrap();
    let mut out = [0u8; PAYLOAD];
    let mut lengths = Vec::new();
    while let Next::Record(n) = reader
        .next(&store, &mut session.media, &mut out, &mut session.sector)
        .unwrap()
    {
        lengths.push(n);
    }
    lengths
}

#[test]
fn the_horizon_is_the_last_pass_the_file_holds_after_a_power_cut() {
    for cut in [3_000, 17_000, 61_230, 118_990, 700_000] {
        let mut session = Session::new();
        session.ride(cut);
        let file = session.file();
        let (read, _) = events(&file);
        let records = record_lengths(&mut session);
        assert_eq!(
            session.tail(),
            (last_pass(&read), blocks_after_last_sync(&file, &records)),
            "cut at {cut} ms"
        );
    }
}

#[test]
fn a_trace_of_only_its_header_has_no_horizon() {
    let mut session = Session::new();
    assert_eq!(session.tail(), (None, 0));
}

#[test]
fn after_a_crash_the_salvage_keeps_every_event_the_session_recorded() {
    for cut in [10, 3_000, 5_000, 17_000, 61_230, 118_990, 300_000] {
        let mut session = Session::new();
        session.ride(cut);
        let before = events(&session.file()).0.len();
        let blocks = session.salvage();
        let (read, end) = events(&session.file());
        assert_eq!(end, Some(End::Complete), "cut at {cut} ms");
        assert_eq!(read, session.kept(), "cut at {cut} ms");
        assert!(
            blocks > 0 && read.len() > before,
            "cut at {cut} ms: {blocks} blocks"
        );
    }
}

#[test]
fn a_salvage_mid_upload_continues_after_the_files_last_block_in_the_ring() {
    for upload in [9_000, 40_000, 77_777, 120_000] {
        let mut session = Session::new();
        session.ride(7_000);
        session.upload(upload);
        assert!(!session.recorder.sink().is_empty(), "upload {upload}");
        session.salvage();
        let (read, end) = events(&session.file());
        assert_eq!(end, Some(End::Complete), "upload {upload}");
        assert_eq!(read, session.kept(), "upload {upload}");
    }
}

#[test]
fn a_salvage_keeps_what_a_closed_card_left_in_the_spool() {
    let mut session = Session::new();
    session.ride(12_000);
    session.closed = true;
    session.ride(40_000);
    assert!(session.recorder.sink().len() > 4 * BLOCK);
    session.salvage();
    let (read, end) = events(&session.file());
    assert_eq!(end, Some(End::Complete));
    assert_eq!(read, session.kept());
}

#[test]
fn a_salvage_after_only_the_header_was_written_keeps_the_spool() {
    let mut session = Session::new();
    session.closed = true;
    session.ride(30_000);
    session.salvage();
    let (read, end) = events(&session.file());
    assert_eq!(end, Some(End::Complete));
    assert_eq!(read, session.kept());
}

#[test]
fn a_salvage_after_a_sync_starts_where_the_spool_started_again() {
    let mut session = Session::new();
    session.ride(20_000);
    let now = session.now;
    session
        .drain
        .sync(
            &mut session.recorder,
            &mut session.media,
            now,
            &mut session.sector,
        )
        .unwrap();
    assert!(session.recorder.sink().is_empty());
    session.closed = true;
    session.ride(25_000);
    session.salvage();
    let (read, end) = events(&session.file());
    assert_eq!(end, Some(End::Complete));
    assert_eq!(read, session.kept());
}

#[test]
fn a_torn_block_ends_the_salvage_at_the_block_before_it() {
    let mut session = Session::new();
    session.closed = true;
    session.ride(40_000);
    let mut ring = session.recorder.sink().memory().to_vec();
    let size =
        |ring: &[u8], at: usize| FRAME + usize::from(u16::from_le_bytes([ring[at], ring[at + 1]]));
    let third = size(&ring, 0) + size(&ring, size(&ring, 0));
    let torn = third + size(&ring, third);
    assert!(session.recorder.sink().len() > torn, "the spool holds more");
    ring[torn - 10] ^= 0xff;
    assert_eq!(session.salvage_from(Some(&ring)), 2);
    let mut expected = header();
    expected.extend_from_slice(&ring[..third]);
    let (read, end) = events(&session.file());
    assert_eq!(end, Some(End::Complete));
    assert_eq!(read, events(&expected).0, "the blocks before the torn one");
}

#[test]
fn an_empty_open_block_and_an_empty_ring_salvage_nothing() {
    let mut session = Session::new();
    session.ride(20_000);
    let now = session.now;
    session
        .drain
        .sync(
            &mut session.recorder,
            &mut session.media,
            now,
            &mut session.sector,
        )
        .unwrap();
    let file = session.file();
    assert_eq!(session.salvage(), 0);
    assert_eq!(session.file(), file);
}
