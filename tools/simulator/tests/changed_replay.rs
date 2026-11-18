use device_api::ant::{AntOperation, Identity};
use firmware_services::trace::media::{Access, Content};
use firmware_services::trace::wire::{Wire, Writer};
use firmware_services::trace::{Event, Medium, StorageOp, decode};
use shell_simulator::ant::device_type;
use shell_simulator::bulk::Presence;
use shell_simulator::replay::{Kind, Mode, Options};
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, minutes};
use shell_simulator::trace::surgery::encode;
use shell_simulator::trace::world::{self, World};

const CHANGED: Options = Options {
    mode: Mode::Changed,
    model: false,
};

fn ridden() -> Vec<u8> {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.ant_sensor(device_type::HEART_RATE, 42);
    sim.ant_sensor(device_type::POWER, 1234);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.init_bulk();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.stop_ride();
    sim.capture()
}

fn altered(trace: Vec<u8>, change: impl FnOnce(&mut Vec<Event<'static>>)) -> Vec<u8> {
    let trace: &'static [u8] = Box::leak(trace.into_boxed_slice());
    let (header, events) = decode(trace).unwrap();
    let mut events: Vec<Event<'static>> = events.collect();
    change(&mut events);
    encode(&header, events)
}

fn read_of(event: &Event<'_>) -> Option<Access<'static>> {
    let Event::Call(call) = event else {
        return None;
    };
    let call: firmware_services::trace::Call<'static> = firmware_services::trace::Call {
        body: Vec::leak(call.body.to_vec()),
        ..*call
    };
    match world::read(&call)? {
        World::Media(StorageOp::Read, access) => Some(access),
        _ => None,
    }
}

fn mutated(events: &[Event<'_>], medium: Medium) -> Vec<(u32, u32)> {
    (events.iter())
        .filter_map(|event| match event {
            Event::Call(call) => match world::read(call) {
                Some(World::Media(StorageOp::Write | StorageOp::Erase, access))
                    if access.span.medium == medium =>
                {
                    Some((access.span.start, access.span.count))
                }
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn unwritten_card_read(events: &[Event<'_>]) -> usize {
    let written = mutated(events, Medium::Bulk);
    let overlaps = |a: (u32, u32), b: (u32, u32)| a.0 < b.0 + b.1 && b.0 < a.0 + a.1;
    (events.iter())
        .position(|event| {
            read_of(event).is_some_and(|access| {
                let span = (access.span.start, access.span.count);
                access.span.medium == Medium::Bulk
                    && matches!(access.content, Content::Inline(_))
                    && access.span.start > 1
                    && !written.iter().any(|w| overlaps(*w, span))
            })
        })
        .expect("a card read of sectors no record writes")
}

fn connect(events: &[Event<'_>]) -> usize {
    (events.iter())
        .position(|event| match event {
            Event::Call(call) => matches!(
                world::read(call),
                Some(World::AntRequest {
                    operation: AntOperation::Connect(_),
                    result: Ok(_),
                })
            ),
            _ => false,
        })
        .expect("a connection the radio accepted")
}

#[test]
fn a_build_that_sends_another_connect_stops_where_it_leaves_the_recorded_world() {
    let capture = altered(ridden(), |events| {
        let at = connect(events);
        let Event::Call(mut call) = events[at] else {
            unreachable!()
        };
        let Some(World::AntRequest {
            operation: AntOperation::Connect(peer),
            result,
        }) = world::read(&call)
        else {
            unreachable!()
        };
        let other = AntOperation::Connect(Identity {
            device_number: peer.device_number ^ 0x5a5a,
            ..peer
        });
        let mut bytes = [0u8; 64];
        let mut w = Writer::new(&mut bytes, call.now_ms);
        other.put(&mut w);
        result.put(&mut w);
        let len = w.written().unwrap();
        call.body = Vec::leak(bytes[..len].to_vec());
        events[at] = Event::Call(call);
    });
    let replayed = Sim::replay_as(&capture, CHANGED);
    let report = replayed.device.report().unwrap();
    let stopped = report.stopped.as_ref().expect("the replay stopped");
    assert_eq!(stopped.kind, Kind::Unrecorded, "{report}");
    assert!(
        stopped.detail.starts_with("left the recorded world at ")
            && stopped.detail.contains("Ant::request"),
        "{}",
        stopped.detail
    );
    assert!(report.verdict().is_err());
}

#[test]
fn a_build_that_caches_a_reread_or_reads_a_pass_earlier_replays_without_a_stop() {
    let capture = altered(ridden(), |events| {
        let digest = (events.iter())
            .position(|event| {
                read_of(event).is_some_and(|access| matches!(access.content, Content::Digest(_)))
            })
            .expect("a reread the trace holds a digest of");
        let reread = events[digest];
        events.insert(digest + 1, reread);
        let early = unwritten_card_read(events);
        let read = events.remove(early);
        let next_pass = (early..events.len())
            .find(|&at| matches!(events[at], Event::Pass { .. }))
            .expect("a later pass");
        events.insert(next_pass + 1, read);
    });
    let replayed = Sim::replay_as(&capture, CHANGED);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.stopped, None, "{report}");
    let verdict = report.verdict().expect("no stop");
    assert!(
        verdict.starts_with("stayed in the recorded world; first difference at"),
        "the reread it skipped and the read it moved are what it recorded otherwise: {verdict}"
    );
}

#[test]
fn a_build_that_reads_one_more_unit_than_recorded_stops_as_unknown_media() {
    let mut sector = 0;
    let capture = altered(ridden(), |events| {
        let read = read_of(&events[unwritten_card_read(events)]).unwrap();
        sector = read.span.start;
        events.retain(|event| {
            read_of(event).is_none_or(|access| {
                access.span != read.span || !matches!(access.content, Content::Inline(_))
            })
        });
    });
    let replayed = Sim::replay_as(&capture, CHANGED);
    let report = replayed.device.report().unwrap();
    let stopped = report.stopped.as_ref().expect("the replay stopped");
    assert!(
        stopped
            .detail
            .ends_with(&format!(": unknown media Bulk {sector}+1")),
        "{}",
        stopped.detail
    );
}
