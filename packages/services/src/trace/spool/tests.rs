use super::*;
use crate::store::{DATA, Next, PAYLOAD};
use crate::testing::Media;
use crate::trace::frozen::Record;
use crate::trace::{Event, Header, decode, encode_header};
use device_api::bulk::{self, OwnedInfo, SECTOR_SIZE};
use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

/// One C606 MMC operation in physical sector operations: every owned
/// operation reads the MBR and the ownership marker again, and a write is
/// read back. The worst append (64 reads, 39 writes) is then 309 reads and
/// 39 programs, which take 125 ms or more; this charges each physical
/// operation its share of that.
const PHYSICAL_US: u64 = 125_000 / (309 + 39);

/// How long the card stalls one write in [`STALL_EVERY_MS`]: the worst
/// append, a chunk link that must rewrite every metadata sector.
const STALL_MS: u64 = 150;
const STALL_EVERY_MS: u64 = 30_000;

/// A store on media that takes time: each pass lasts its 10 ms plus the
/// time its operations took, during which the recorder records nothing.
/// Now and then a write stalls for [`STALL_MS`].
struct Stalling {
    media: Media,
    physical: u64,
    records: u64,
    stalled_ms: u64,
    stall_due: bool,
}

impl Stalling {
    fn take_ms(&mut self) -> u64 {
        let ms = self.physical * PHYSICAL_US / 1_000 + core::mem::take(&mut self.stalled_ms);
        self.physical = 0;
        ms
    }
}

impl ReadWrite for Stalling {
    fn owned_info(&mut self) -> Result<OwnedInfo, bulk::Error> {
        self.media.owned_info()
    }
    fn owned_read(&mut self, sector: u64, output: &mut Sector) -> Result<(), bulk::Error> {
        self.physical += 3;
        self.media.owned_read(sector, output)
    }
    fn owned_write(&mut self, sector: u64, input: &Sector) -> Result<(), bulk::Error> {
        self.physical += 4;
        if sector >= DATA {
            self.records += 1;
        }
        if core::mem::take(&mut self.stall_due) {
            self.stalled_ms += STALL_MS;
        }
        self.media.owned_write(sector, input)
    }
    fn entropy(&mut self) -> u64 {
        self.media.entropy()
    }
}

const HEADER: Header<'static> = Header {
    version: crate::trace::VERSION,
    oldest_reader: crate::trace::OLDEST_READER,
    build: [0; 20],
    boot: 7,
    profile: 0,
    config: &[1; 300],
    media: &[2; 900],
};

fn header() -> Vec<u8> {
    let mut bytes = vec![0u8; 4096];
    let len = encode_header(&HEADER, &mut bytes).unwrap();
    bytes.truncate(len);
    bytes
}

fn name() -> Name {
    Name::new("traces/000001-00000007.trace").unwrap()
}

/// A session recording through a spool of `ring` bytes into a store that
/// stalls, beside a recorder that keeps every block, to compare with.
struct Session {
    disk: Stalling,
    recorder: Spooled<Vec<u8>, Vec<u8>>,
    drain: Drain<Vec<u8>>,
    kept: Recorder<Vec<u8>, Kept>,
    sector: Sector,
    now: u64,
    passes: u64,
    most_records_a_pass: u64,
    longest_pass_ms: u64,
}

struct Kept(Vec<u8>);

impl Sink for Kept {
    fn block(&mut self, bytes: &[u8]) -> bool {
        self.0.extend_from_slice(bytes);
        true
    }
}

impl Session {
    fn new(ring: usize) -> Self {
        let mut disk = Stalling {
            media: Media::new(DATA + 16 * u64::from(store::CHUNK)),
            physical: 0,
            records: 0,
            stalled_ms: 0,
            stall_due: false,
        };
        let mut sector = [0u8; SECTOR_SIZE];
        let store = store::format(&mut disk, 11, &mut sector).unwrap();
        let traces = Dir::new("traces/").unwrap();
        let drain = Drain::create(&mut disk, store, name(), traces, header(), 0, &mut sector);
        disk.take_ms();
        Self {
            disk,
            recorder: Recorder::new(vec![0u8; 4096], Spool::new(vec![0u8; ring])),
            drain: drain.unwrap(),
            kept: Recorder::new(vec![0u8; 4096], Kept(header())),
            sector,
            now: 0,
            passes: 0,
            most_records_a_pass: 0,
            longest_pass_ms: 0,
        }
    }

    fn record(&mut self, event: &Event<'_>) {
        self.recorder.record(event);
        self.kept.record(event);
    }

    /// One main-loop pass that records `events` after its clock.
    fn pass(&mut self, events: &[Event<'_>]) {
        let stall = self.now / STALL_EVERY_MS != (self.now + 10) / STALL_EVERY_MS;
        self.disk.stall_due |= stall;
        self.record(&Event::Pass { now_ms: self.now });
        for event in events {
            self.record(event);
        }
        let records = self.disk.records;
        self.drain
            .pass(
                &mut self.recorder,
                &mut self.disk,
                self.now,
                &mut self.sector,
            )
            .unwrap();
        let written = self.disk.records - records;
        self.most_records_a_pass = self.most_records_a_pass.max(written);
        let took = 10 + self.disk.take_ms();
        self.longest_pass_ms = self.longest_pass_ms.max(took);
        self.now += took;
        self.passes += 1;
    }

    /// The trace a session records at the rate the simulator measures, 2.9
    /// MB an hour: a pass every 10 ms and a 70-byte record every 100 ms.
    fn ride(&mut self, ms: u64) {
        let end = self.now + ms;
        let sample = str::from_utf8(&[b's'; 66]).unwrap();
        while self.now < end {
            if self.passes.is_multiple_of(10) {
                self.pass(&[Event::Command(sample)]);
            } else {
                self.pass(&[]);
            }
        }
    }

    /// A phone upload of `bytes`, read as fast as VANA reads one: eight
    /// 512-byte pieces a pass.
    fn upload(&mut self, bytes: usize) {
        let piece = [b'u'; 512];
        let mut left = bytes;
        while left > 0 {
            let mut reads = Vec::new();
            for _ in 0..8 {
                let n = left.min(512);
                if n == 0 {
                    break;
                }
                reads.push(Event::ServerRead {
                    id: 1,
                    part: &piece[..n],
                    more: false,
                });
                left -= n;
            }
            self.pass(&reads);
        }
    }

    fn file(&mut self) -> Vec<u8> {
        let store = store::mount(&mut self.disk, &mut self.sector).unwrap();
        let mut reader = store
            .reader(&mut self.disk, &name(), &mut self.sector)
            .unwrap();
        let mut bytes = Vec::new();
        let mut out = [0u8; PAYLOAD];
        loop {
            match reader
                .next(&store, &mut self.disk, &mut out, &mut self.sector)
                .unwrap()
            {
                Next::Record(n) => bytes.extend_from_slice(&out[..n]),
                Next::Hole { from, to } => panic!("records {from}..{to} lost"),
                Next::End => return bytes,
            }
        }
    }

    fn kept(&mut self) -> Vec<String> {
        let mut bytes = self.kept.sink().0.clone();
        bytes.extend_from_slice(self.kept.open_block());
        events(&bytes)
    }
}

fn events(bytes: &[u8]) -> Vec<String> {
    let (header, events) = decode(bytes).unwrap();
    assert_eq!(header, HEADER);
    events.map(|event| format!("{event:?}")).collect()
}

#[test]
fn at_the_measured_rate_a_stalling_store_keeps_every_event_one_record_a_pass() {
    let mut session = Session::new(RING_BYTES);
    session.ride(8 * 60_000);
    let riding = session.recorder.sink().peak();
    assert!(
        session.longest_pass_ms >= STALL_MS,
        "the card stalled a pass: {} ms",
        session.longest_pass_ms
    );
    assert!(
        riding <= 4096 + 512,
        "riding, the spool holds about a flushed block: {riding}"
    );
    session.upload(UPLOAD_BYTES);
    session.ride(60_000);
    let now = session.now;
    session
        .drain
        .sync(
            &mut session.recorder,
            &mut session.disk,
            now,
            &mut session.sector,
        )
        .unwrap();
    let file = session.file();
    assert!(
        file.len() > 2 * PAYLOAD * store::CHUNK as usize,
        "the file links chunks, the drain's longest stalls: {} bytes",
        file.len()
    );
    assert_eq!(events(&file), session.kept());
    assert_eq!(session.recorder.sink().refused(), 0);
    assert_eq!(session.most_records_a_pass, 1);
    let spool = session.recorder.sink();
    std::println!(
        "riding peak {riding} B, longest pass {} ms; a {} KiB upload peaks at {} of {} B",
        session.longest_pass_ms,
        UPLOAD_BYTES / 1024,
        spool.peak(),
        spool.capacity()
    );
    assert!(spool.peak() < spool.capacity());
}

/// The largest phone upload the spool takes whole: VANA reads 4 KiB a pass
/// and the drain writes one record, so the spool fills by about 88% of an
/// upload.
const UPLOAD_BYTES: usize = 280 * 1024;

#[test]
fn a_burst_larger_than_the_spool_is_lost_and_named_where_it_was_lost() {
    let mut session = Session::new(16 * 1024);
    session.ride(10_000);
    session.upload(100 * 1024);
    session.ride(20_000);
    let now = session.now;
    session
        .drain
        .sync(
            &mut session.recorder,
            &mut session.disk,
            now,
            &mut session.sector,
        )
        .unwrap();
    let file = session.file();
    let (_, mut decoded) = decode(&file).unwrap();
    let read: Vec<String> = decoded.by_ref().map(|event| format!("{event:?}")).collect();
    assert_eq!(decoded.end(), Some(crate::trace::End::Complete));
    let lost: Vec<&String> = read
        .iter()
        .filter(|event| event.starts_with("Lost"))
        .collect();
    assert!(!lost.is_empty(), "the overflow is in the trace");
    assert!(session.recorder.sink().refused() > 0);
    let kept = session.kept();
    let mut rest = kept.iter();
    for event in read.iter().filter(|event| !event.starts_with("Lost")) {
        assert!(
            rest.any(|kept| kept == event),
            "{event} is out of order or invented"
        );
    }
    assert_eq!(read.last(), kept.last(), "recording goes on after the loss");
}

#[test]
fn a_power_cut_loses_at_most_the_last_sync_interval() {
    for cut in [17_000, 61_230, 118_990] {
        let mut session = Session::new(RING_BYTES);
        session.ride(cut);
        let cut = session.now;
        let file = session.file();
        let last = events(&file)
            .iter()
            .rev()
            .find_map(|event| {
                event
                    .strip_prefix("Pass { now_ms: ")?
                    .trim_end_matches(" }")
                    .parse::<u64>()
                    .ok()
            })
            .unwrap();
        assert!(
            last + SYNC_MS + 10 >= cut,
            "the file ends at {last} ms of {cut} ms"
        );
    }
}

#[test]
fn a_drain_goes_on_after_another_writer_changed_the_store_and_stops_when_its_file_went() {
    let mut session = Session::new(RING_BYTES);
    session.ride(30_000);
    let now = session.now;
    session
        .drain
        .sync(
            &mut session.recorder,
            &mut session.disk,
            now,
            &mut session.sector,
        )
        .unwrap();
    session
        .drain
        .resume(&mut session.disk, &mut session.sector)
        .unwrap();
    let mut host = store::mount(&mut session.disk, &mut session.sector).unwrap();
    let other = Name::new("traces/000000-00000001.trace").unwrap();
    host.create(&mut session.disk, &other, None, &mut session.sector)
        .unwrap();
    let before = session.drain.generation();
    session
        .drain
        .resume(&mut session.disk, &mut session.sector)
        .unwrap();
    assert_ne!(session.drain.generation(), before, "the drain opened again");
    session.ride(30_000);
    let now = session.now;
    session
        .drain
        .sync(
            &mut session.recorder,
            &mut session.disk,
            now,
            &mut session.sector,
        )
        .unwrap();
    let file = session.file();
    assert_eq!(events(&file), session.kept());

    let mut host = store::mount(&mut session.disk, &mut session.sector).unwrap();
    host.delete(&mut session.disk, &name(), &mut session.sector)
        .unwrap();
    assert_eq!(
        session.drain.resume(&mut session.disk, &mut session.sector),
        Err(Error::NotFound)
    );
}

#[test]
fn a_spool_keeps_blocks_in_order_across_its_end() {
    let mut spool = Spool::new([0u8; 10]);
    assert!(spool.block(b"abcdef"));
    assert!(!spool.block(b"ghijk"), "no room");
    spool.consume(4);
    assert!(spool.block(b"ghijk"));
    assert_eq!(spool.refused(), 1);
    assert_eq!(spool.len(), 7);
    assert_eq!(spool.front(10), b"efghij");
    spool.consume(6);
    assert_eq!(spool.front(10), b"k");
    spool.consume(1);
    assert!(spool.is_empty());
    assert_eq!(spool.peak(), 7);
}

#[test]
fn each_session_names_its_trace_one_past_the_highest_in_traces() {
    let mut media = Media::new(DATA + 4 * u64::from(store::CHUNK));
    let mut sector = [0u8; SECTOR_SIZE];
    let mut store = store::format(&mut media, 3, &mut sector).unwrap();
    let first = next_name(&store, &mut media, 0xa, &mut sector).unwrap();
    assert_eq!(first.as_str(), "traces/000001-0000000a.trace");
    for name in [
        "traces/000041-00000001.trace",
        "traces/000007-00000002.trace",
        "notes",
    ] {
        store
            .create(&mut media, &Name::new(name).unwrap(), None, &mut sector)
            .unwrap();
    }
    let next = next_name(&store, &mut media, 0x1a2b3c4d, &mut sector).unwrap();
    assert_eq!(next.as_str(), "traces/000042-1a2b3c4d.trace");
}

mod file {
    use super::*;
    use crate::volume::{self, Table, Volume};

    const REGION: u64 = 5_777_407;

    fn table(media: &mut Media, sector: &mut Sector) -> Table {
        match volume::read(media, sector).unwrap() {
            volume::Layout::Table(table) => table,
            _ => unreachable!(),
        }
    }

    fn card(store: bool) -> (Media, Sector) {
        let mut media = Media::new(REGION);
        let mut sector = [0u8; SECTOR_SIZE];
        let table = volume::create(&mut media, &mut sector).unwrap();
        let files = table.find(&volume::FILES).unwrap();
        crate::files::format(&mut Volume::new(&mut media, files), 1, &mut sector).unwrap();
        if store {
            let extent = table.find(&volume::STORE).unwrap();
            store::format(&mut Volume::new(&mut media, extent), 2, &mut sector).unwrap();
        }
        (media, sector)
    }

    fn recorder() -> Spooled<Vec<u8>, Vec<u8>> {
        Recorder::new(vec![0u8; 4096], Spool::new(vec![0u8; 64 * 1024]))
    }

    fn opened(file: &File<Vec<u8>>) -> String {
        match file {
            File::Open(_, _, drain) => String::from(drain.name().as_str()),
            _ => panic!("no open file"),
        }
    }

    fn read(media: &mut Media, sector: &mut Sector, name: &str) -> Vec<u8> {
        let extent = table(media, sector).find(&volume::STORE).unwrap();
        let mut store = Volume::new(media, extent);
        let mounted = store::mount(&mut store, sector).unwrap();
        let name = Name::new(name).unwrap();
        let mut reader = mounted.reader(&mut store, &name, sector).unwrap();
        let (mut bytes, mut out) = (Vec::new(), [0u8; PAYLOAD]);
        while let Next::Record(n) = reader.next(&mounted, &mut store, &mut out, sector).unwrap() {
            bytes.extend_from_slice(&out[..n]);
        }
        bytes
    }

    #[test]
    fn a_card_without_a_store_records_nothing_and_says_why() {
        let mut sector = [0u8; SECTOR_SIZE];
        let blank: File<Vec<u8>> = File::stage(&mut Media::new(REGION), &mut sector);
        assert!(matches!(blank, File::Off("no volume table")));
        let (mut media, mut sector) = card(false);
        let unformatted: File<Vec<u8>> = File::stage(&mut media, &mut sector);
        assert!(matches!(unformatted, File::Off("STORE does not mount")));
    }

    #[test]
    fn the_file_opens_with_its_header_once_the_header_is_known() {
        let (mut media, mut sector) = card(true);
        let mut file: File<Vec<u8>> = File::stage(&mut media, &mut sector);
        assert!(matches!(file, File::Staging(..)));
        let mut recorder = recorder();
        recorder.record(&Event::Pass { now_ms: 10 });
        file.pass(&mut recorder, &mut media, true, || None, 10, &mut sector)
            .unwrap();
        assert!(!file.is_open(), "no header yet");
        recorder.record(&Event::Pass { now_ms: 20 });
        let known = || Some((header(), 7));
        file.pass(&mut recorder, &mut media, true, known, 20, &mut sector)
            .unwrap();
        file.sync(&mut recorder, &mut media, 30, &mut sector)
            .unwrap();
        let name = opened(&file);
        assert_eq!(name, "traces/000001-00000007.trace");
        let bytes = read(&mut media, &mut sector, &name);
        assert_eq!(
            events(&bytes),
            ["Pass { now_ms: 10 }", "Pass { now_ms: 20 }"]
        );
    }

    #[test]
    fn a_host_write_to_files_ends_the_trace_and_says_so_a_store_change_reopens_it() {
        let (mut media, mut sector) = card(true);
        let mut file: File<Vec<u8>> = File::stage(&mut media, &mut sector);
        let mut recorder = recorder();
        recorder.record(&Event::Pass { now_ms: 10 });
        let known = || Some((header(), 7));
        file.pass(&mut recorder, &mut media, true, known, 10, &mut sector)
            .unwrap();
        let table = table(&mut media, &mut sector);
        let store_extent = table.find(&volume::STORE).unwrap();
        let files_extent = table.find(&volume::FILES).unwrap();
        let mut host_store = Volume::new(&mut media, store_extent);
        let mut host = store::mount(&mut host_store, &mut sector).unwrap();
        let notes = Name::new("notes").unwrap();
        host.create(&mut host_store, &notes, None, &mut sector)
            .unwrap();
        file.after_host(&mut recorder, &mut media, true, 20, &mut sector)
            .unwrap();
        assert!(file.is_open(), "the store changed; the file goes on");
        recorder.record(&Event::Pass { now_ms: 30 });

        let course = crate::files::Name::new("course").unwrap();
        let mut seen = Volume::new(&mut media, files_extent);
        crate::files::write(&mut seen, &course, b"gpx", &mut sector).unwrap();
        file.after_host(&mut recorder, &mut media, false, 35, &mut sector)
            .unwrap();
        assert!(file.is_open(), "a recorded command's write replays");
        file.before_host(&mut recorder, &mut media, 36, &mut sector)
            .unwrap();
        file.after_host(&mut recorder, &mut media, true, 37, &mut sector)
            .unwrap();
        assert!(file.is_open(), "a transfer that wrote nothing VANA reads");

        let mut host_files = Volume::new(&mut media, files_extent);
        crate::files::write(&mut host_files, &course, b"gpx 2", &mut sector).unwrap();
        let name = opened(&file);
        assert_eq!(
            file.after_host(&mut recorder, &mut media, true, 40, &mut sector),
            Err("the host wrote the card")
        );
        assert!(matches!(file, File::Ended(_)));
        let bytes = read(&mut media, &mut sector, &name);
        assert_eq!(
            events(&bytes),
            ["Pass { now_ms: 10 }", "Pass { now_ms: 30 }", "HostWrote"]
        );
    }
}
