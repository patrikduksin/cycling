use super::*;
use crate::trace::timeline::At;

fn packet() -> MethodId {
    crate::twin::script::named("Ant", "take_packet").unwrap()
}

fn call(body: &'static [u8]) -> Event<'static> {
    Event::Call(trace::Call {
        method: packet(),
        now_ms: 0,
        body,
    })
}

fn header() -> Header<'static> {
    Header {
        build: [0; 20],
        device: "c606",
        boot: 1,
        terrain: firmware_services::trace::origin::Found::Absent,
    }
}

fn recorded() -> Vec<u8> {
    let events = [
        Event::Pass { now_ms: 1_000 },
        call(&[1]),
        call(&[2]),
        Event::Pass { now_ms: 2_000 },
        call(&[3]),
    ];
    encode(&header(), events)
}

fn bodies(trace: &[u8]) -> Vec<(u64, Vec<u8>)> {
    let (_, events) = trace::decode(trace).unwrap();
    let mut pass_ms = 0;
    events
        .filter_map(|event| match event {
            Event::Pass { now_ms } => {
                pass_ms = now_ms;
                None
            }
            Event::Call(call) => Some((pass_ms, call.body.to_vec())),
            _ => None,
        })
        .collect()
}

fn session_ms(text: &str) -> Result<u64, String> {
    match At::parse(text)? {
        At::Session(ms) => Ok(ms),
        At::Ride(_) => Err("a session time".into()),
    }
}

#[test]
fn a_set_resolves_to_the_first_pass_at_or_after_its_time_that_holds_the_record() {
    let set = Set::parse("00:01.500:Ant::take_packet=drop", session_ms).unwrap();
    let (altered, resolved) = super::set(&recorded(), &set).unwrap();
    assert_eq!(resolved, 2_000);
    assert_eq!(bodies(&altered), [(1_000, vec![1]), (1_000, vec![2])]);
}

#[test]
fn a_set_finds_the_nth_record_of_its_pass_and_replaces_its_body() {
    let set = Set::parse("00:01.000:Ant::take_packet#1=aa0f", session_ms).unwrap();
    assert_eq!(
        set,
        Set {
            at_ms: 1_000,
            of: Of::Method(packet()),
            nth: 1,
            answer: Answer::Body(vec![0xaa, 0x0f]),
        }
    );
    let (altered, resolved) = super::set(&recorded(), &set).unwrap();
    assert_eq!(resolved, 1_000);
    assert_eq!(
        bodies(&altered),
        [
            (1_000, vec![1]),
            (1_000, vec![0xaa, 0x0f]),
            (2_000, vec![3])
        ]
    );
}

#[test]
fn a_pass_with_too_few_records_passes_the_set_on_to_the_next_one() {
    let set = Set::parse("00:01.500:Ant::take_packet#1=drop", session_ms).unwrap();
    assert_eq!(
        super::set(&recorded(), &set),
        Err("no pass at or after 00:01.500 holds record #1 of Ant::take_packet".into())
    );
}

#[test]
fn a_set_names_what_it_cannot_read() {
    assert!(Set::parse("00:01.000:Ant::take_packet=zz", session_ms).is_err());
    assert!(Set::parse("00:01.000 Ant::take_packet=drop", session_ms).is_err());
    assert_eq!(
        Set::parse("1s:Ant::sing=drop", session_ms),
        Err("--set: no method Ant::sing".into())
    );
    assert_eq!(
        Set::parse("1s:ant.sing=drop", session_ms),
        Err(
            "--set: no kind \"ant.sing\"; kinds read as trace timeline names them, such as ant.request"
                .into()
        )
    );
}

#[test]
fn a_set_names_its_records_as_trace_timeline_names_their_kind() {
    let encoded = |value: u64| {
        let mut body = vec![0u8; 16];
        let mut w = firmware_services::trace::wire::Writer::new(&mut body, 0);
        firmware_services::trace::wire::Wire::put(&value, &mut w);
        let len = w.written().unwrap();
        body.truncate(len);
        body
    };
    let entropy = |value: u64| -> Event<'static> {
        Event::Call(trace::Call {
            method: crate::twin::script::named("ReadWrite", "entropy").unwrap(),
            now_ms: 0,
            body: Box::leak(encoded(value).into_boxed_slice()),
        })
    };
    let events = [
        Event::Pass { now_ms: 1_000 },
        call(&[1]),
        entropy(5),
        entropy(6),
        Event::Pass { now_ms: 2_000 },
        entropy(7),
    ];
    let trace = encode(&header(), events);
    let set = Set::parse("00:01.000:entropy#1=drop", session_ms).unwrap();
    assert_eq!(set.of, Of::Kind(Kind::Entropy));
    let (altered, resolved) = super::set(&trace, &set).unwrap();
    assert_eq!(resolved, 1_000);
    assert_eq!(
        bodies(&altered),
        [(1_000, vec![1]), (1_000, encoded(5)), (2_000, encoded(7))],
        "the second entropy record of the pass went"
    );
    let request = Set::parse("1:02:03.5:ant.request=drop", session_ms).unwrap();
    assert_eq!(
        (request.at_ms, request.of),
        (3_723_500, Of::Kind(Kind::AntRequest))
    );
}
