use super::*;
use crate::trace::Header;
use device_api::ant::{Identity, LinkState, STALE_MS};
use device_api::input::{Button, Input};
use std::format;
use std::string::String;
use std::vec;
use std::vec::Vec;

#[derive(Default)]
struct Log(Vec<String>);

impl Record for Log {
    fn record(&mut self, event: &Event<'_>) {
        self.0.push(format!("{event:?}"));
    }
}

fn kinds(log: &RefCell<Log>) -> Vec<String> {
    log.borrow_mut()
        .0
        .drain(..)
        .map(|text| text.split(['(', ' ']).next().unwrap().into())
        .collect()
}

const POWER: Identity = Identity {
    device_type: 11,
    device_number: 1234,
    transmission_type: 1,
};

struct Radio {
    heard: u64,
    link: LinkState,
    packets: Vec<ant::Packet>,
    pages: std::rc::Rc<core::cell::Cell<u32>>,
}

impl Radio {
    fn new() -> Self {
        Self {
            heard: 0,
            link: LinkState::Connected,
            packets: Vec::new(),
            pages: std::rc::Rc::default(),
        }
    }
}

impl Ant for Radio {
    fn scanning(&self) -> bool {
        false
    }
    fn discoveries(&self) -> [Option<ant::Discovery>; 8] {
        [None; 8]
    }
    fn request(&mut self, _: AntOperation, _: u64) -> Result<ant::Admission, ant::Error> {
        self.link = LinkState::Disconnecting;
        Ok(ant::Admission::Accepted)
    }
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn channels(&self, now: u64) -> [Option<ant::Snapshot>; ant::CHANNEL_CAPACITY] {
        let age = now.saturating_sub(self.heard);
        let mut channels = [None; ant::CHANNEL_CAPACITY];
        channels[0] = Some(ant::Snapshot {
            scanning: false,
            link: self.link,
            selected: Some(POWER),
            generation: 1,
            packets: self.pages.get(),
            dropped_packets: 0,
            discovery_overflows: 0,
            transport_losses: 0,
            tx_failures: 0,
            age_ms: Some(age),
            stale: age >= STALE_MS,
        });
        channels
    }
    fn take_packet(&mut self) -> Option<ant::Packet> {
        self.packets.pop()
    }
}

#[test]
fn a_pass_reads_one_value_until_its_own_request() {
    let log = RefCell::new(Log::default());
    let mut frozen = FrozenAnt::default();
    let mut radio = Radio::new();
    let pages = radio.pages.clone();
    let mut ant = frozen.bind(&mut radio, 100, &log);
    pages.set(5);
    let channel = ant.channel(11, 100).unwrap();
    assert_eq!(channel.packets, 0, "a page heard mid-pass waits");
    assert_eq!(
        ant.request(AntOperation::StopScan, 100),
        Ok(ant::Admission::Accepted)
    );
    let channel = ant.channel(11, 100).unwrap();
    assert_eq!(channel.link, LinkState::Disconnecting, "the request shows");
    assert_eq!(channel.packets, 5, "and what came before it");
}

#[test]
fn an_unchanged_radio_is_recorded_once() {
    let log = RefCell::new(Log::default());
    let mut frozen = FrozenAnt::default();
    let mut radio = Radio::new();
    radio.heard = 95;
    frozen.bind(&mut radio, 100, &log);
    let first = kinds(&log);
    assert_eq!(first[..2], ["AntRadio", "AntDiscoveries"]);
    assert_eq!(
        first[2..],
        ["AntLink"; ant::CHANNEL_CAPACITY],
        "empty ones too"
    );
    for now in (110..STALE_MS).step_by(10) {
        frozen.bind(&mut radio, now, &log);
    }
    assert!(kinds(&log).is_empty(), "age alone is not a change");
    radio.heard = 2_000;
    frozen.bind(&mut radio, 2_010, &log);
    assert_eq!(kinds(&log), ["AntLink"], "a new page is");
}

#[test]
fn a_replay_presents_what_the_radio_reported_every_pass() {
    let log = RefCell::new(Log::default());
    let mut frozen = FrozenAnt::default();
    let mut radio = Radio::new();
    let mut replayed: Latest<Option<ant::Snapshot>> = Latest::default();
    let mut recorded = 0;
    for now in (0..10_000).step_by(10) {
        if now == 5_000 {
            radio.heard = 5_000;
        }
        let actual = frozen.bind(&mut radio, now, &log).channels(now)[0];
        let events = kinds(&log);
        if events.contains(&"AntLink".into()) {
            replayed.set(actual, now);
            recorded += 1;
        }
        assert_eq!(replayed.at(now), Some(actual), "at {now} ms");
    }
    assert_eq!(recorded, 4, "start, stale, a page, stale again");
}

#[test]
fn takes_and_requests_are_recorded_in_call_order() {
    let log = RefCell::new(Log::default());
    let mut frozen = FrozenAnt::default();
    let mut radio = Radio::new();
    frozen.bind(&mut radio, 0, &log);
    kinds(&log);
    radio.packets.push(ant::Packet {
        identity: POWER,
        data: [0x10; 8],
        received_ms: 5,
        generation: 1,
        loss_count: 0,
    });
    let mut ant = frozen.bind(&mut radio, 10, &log);
    assert!(ant.take_packet().is_some());
    assert!(ant.take_packet().is_none());
    let _ = ant.request(AntOperation::StopScan, 10);
    let _ = ant.send_status(ant::OperationId(1));
    assert_eq!(
        kinds(&log),
        ["AntPacket", "AntRequest", "AntLink", "AntSend"],
        "what the request changed follows its answer"
    );
}

struct Queue(Vec<Edge>);

impl InputSource for Queue {
    fn controls(&self) -> Controls {
        Controls {
            touch: Availability::Ready,
            buttons: &[],
        }
    }
    fn take_edge(&mut self) -> Option<Edge> {
        (!self.0.is_empty()).then(|| self.0.remove(0))
    }
}

#[test]
fn edges_past_one_tick_wait_for_the_next_pass() {
    let press = |sequence| Edge {
        sequence,
        received_ms: 0,
        input: Input::Button {
            button: Button::TopLeft,
            code: 1,
        },
    };
    let mut edges = RecordedEdges::new(Queue((1..=20).map(press).collect()));
    let mut taken = 0;
    while edges.take_edge().is_some() {
        taken += 1;
    }
    assert_eq!(taken, 16);
    let mut log = Log::default();
    edges.record(&mut log);
    assert_eq!(log.0.len(), 16);
    let rest: Vec<_> = core::iter::from_fn(|| edges.take_edge()).collect();
    assert_eq!(rest.first().map(|edge| edge.sequence), Some(17));
}

/// Blocks behind a header, as a trace file holds them; `room` bytes in all,
/// or no limit.
struct File {
    bytes: Vec<u8>,
    room: Option<usize>,
}

impl File {
    fn new(room: Option<usize>) -> Self {
        let header = Header {
            version: crate::trace::VERSION,
            oldest_reader: crate::trace::OLDEST_READER,
            build: [0; 20],
            boot: 1,
            profile: 0,
            config: &[],
            media: &[],
        };
        let mut bytes = vec![0u8; 128];
        let len = crate::trace::encode_header(&header, &mut bytes).unwrap();
        bytes.truncate(len);
        Self { bytes, room }
    }
}

impl Sink for File {
    fn block(&mut self, bytes: &[u8]) -> bool {
        if self
            .room
            .is_some_and(|room| self.bytes.len() + bytes.len() > room)
        {
            return false;
        }
        self.bytes.extend_from_slice(bytes);
        true
    }
}

fn events(recorder: &mut Recorder<[u8; 128], File>) -> Vec<Event<'static>> {
    let mut bytes = recorder.sink().bytes.clone();
    bytes.extend_from_slice(recorder.open_block());
    let bytes = Vec::leak(bytes);
    let (_, events) = crate::trace::decode(bytes).unwrap();
    events.collect()
}

#[test]
fn a_full_block_moves_on_to_the_next() {
    let mut recorder = Recorder::new([0u8; 128], File::new(None));
    for now in 1..=500 {
        recorder.record(&Event::Pass { now_ms: now * 10 });
    }
    assert!(recorder.sink().bytes.len() > 4 * 64, "several blocks");
    let passes = events(&mut recorder);
    assert_eq!(passes.len(), 500);
    assert_eq!(passes.last(), Some(&Event::Pass { now_ms: 5_000 }));
}

#[test]
fn a_record_larger_than_a_block_leaves_a_marker() {
    let mut recorder = Recorder::new([0u8; 128], File::new(None));
    let text = core::str::from_utf8(&[b'x'; 200]).unwrap();
    recorder.record(&Event::Pass { now_ms: 10 });
    recorder.record(&Event::Command(text));
    recorder.record(&Event::Pass { now_ms: 20 });
    let command = Event::Command("").tag();
    assert_eq!(
        events(&mut recorder),
        [
            Event::Pass { now_ms: 10 },
            Event::TooLarge(command),
            Event::Pass { now_ms: 20 },
        ]
    );
}

#[test]
fn a_flush_hands_over_what_the_open_block_holds_and_nothing_when_empty() {
    let mut recorder = Recorder::new([0u8; 128], File::new(None));
    let header = recorder.sink().bytes.len();
    recorder.flush();
    assert_eq!(recorder.sink().bytes.len(), header, "an empty block stays");
    recorder.record(&Event::Pass { now_ms: 10 });
    recorder.flush();
    let sealed = recorder.sink().bytes.clone();
    let (_, events) = crate::trace::decode(&sealed).unwrap();
    assert_eq!(events.collect::<Vec<_>>(), [Event::Pass { now_ms: 10 }]);
    recorder.record(&Event::Pass { now_ms: 20 });
    assert_eq!(
        self::events(&mut recorder),
        [Event::Pass { now_ms: 10 }, Event::Pass { now_ms: 20 }]
    );
}

#[test]
fn blocks_the_sink_refuses_are_counted_in_the_next_block_it_takes() {
    let mut recorder = Recorder::new([0u8; 128], File::new(Some(400)));
    let text = core::str::from_utf8(&[b'x'; 100]).unwrap();
    for now in 1..=12 {
        recorder.record(&Event::Pass { now_ms: now * 10 });
        recorder.record(&Event::Command(text));
    }
    let taken = recorder.sink().bytes.len();
    assert!(taken <= 400);
    recorder.sink_mut().room = None;
    recorder.flush();
    let events = events(&mut recorder);
    let lost: Vec<_> = events
        .iter()
        .filter(|event| matches!(event, Event::Lost { .. }))
        .collect();
    assert_eq!(lost.len(), 1, "{events:?}");
    let Event::Lost { blocks, bytes } = *lost[0] else {
        unreachable!()
    };
    let commands = events
        .iter()
        .filter(|event| matches!(event, Event::Command(_)))
        .count();
    assert!(blocks >= 2, "{blocks}");
    assert_eq!(commands + blocks as usize, 12, "a block holds one command");
    assert!(bytes as usize >= 100 * blocks as usize);
    assert_eq!(events.last(), Some(&Event::Command(text)));
}

#[test]
fn a_synced_clock_is_recorded_once_and_reads_on_as_the_clock_owner_computes() {
    use crate::network_time::{Anchor, STALE_AFTER_MS, Snapshot};
    let anchor = Anchor {
        unix_ms: 1_793_577_599_400,
        uptime_ms: 2_000,
    };
    let mut latest = Latest::default();
    let mut recorded = 0;
    let passes = (2_000..10_000)
        .step_by(10)
        .chain((STALE_AFTER_MS - 1_000..STALE_AFTER_MS + 3_000).step_by(10));
    for now in passes {
        let reading = anchor.snapshot(now, -180, false);
        if latest.changed(reading, now) {
            recorded += 1;
        }
        assert_eq!(latest.at(now), Some(reading), "at {now} ms");
    }
    assert_eq!(recorded, 1);
    let now = STALE_AFTER_MS + 3_000;
    let corrected = Anchor {
        unix_ms: anchor.unix_ms + 1_500,
        uptime_ms: now,
    };
    assert!(latest.changed(corrected.snapshot(now, -180, false), now));
    assert!(latest.changed(Snapshot::unsynced(false), now + 10));
}
