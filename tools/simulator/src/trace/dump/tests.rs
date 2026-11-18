use super::*;
use std::path::Path;

fn fixture(name: &str) -> Vec<u8> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("tests/traces/{name}.trace"));
    std::fs::read(path).unwrap()
}

fn events(trace: &[u8]) -> Vec<String> {
    let (_, events) = trace::decode(trace).unwrap();
    events.map(|event| format!("{event:?}")).collect()
}

#[test]
fn a_dump_undumps_to_the_same_records_under_the_same_header() {
    for name in ["laps-and-a-menu", "power-dropout-after-descent"] {
        let trace = fixture(name);
        let undumped = undump(&dump(&trace).unwrap()).unwrap();
        assert_eq!(undumped.stopped, None, "{name}");
        assert_eq!(
            trace::decode(&undumped.trace).unwrap().0,
            trace::decode(&trace).unwrap().0
        );
        assert_eq!(events(&undumped.trace), events(&trace), "{name}");
    }
}

fn lines(trace: &[u8]) -> Vec<Value> {
    (dump(trace).unwrap().lines())
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn joined(lines: &[Value]) -> String {
    lines.iter().map(|line| format!("{line}\n")).collect()
}

#[test]
fn a_record_this_build_decodes_otherwise_ends_the_trace_before_its_pass() {
    let trace = fixture("laps-and-a-menu");
    let mut lines = lines(&trace);
    let at = (lines.iter())
        .rposition(|line| {
            line["value"]
                .as_str()
                .is_some_and(|v| v.starts_with("AntPacket"))
        })
        .expect("a packet");
    let pass_ms = lines[at]["at"].as_u64().unwrap();
    lines[at]["value"] = "AntPacket(another build's packet)".into();
    let undumped = undump(&joined(&lines)).unwrap();
    let stopped = undumped.stopped.expect("the packet is not carried");
    assert_eq!(stopped.at_ms, pass_ms);
    assert!(
        stopped.why.contains("another build's packet"),
        "{}",
        stopped.why
    );
    let (_, events) = trace::decode(&undumped.trace).unwrap();
    let last = events.filter_map(|e| match e {
        Event::Pass { now_ms } => Some(now_ms),
        _ => None,
    });
    assert!(
        last.max().unwrap() < pass_ms,
        "the trace ends before that pass"
    );
}

#[test]
fn a_method_whose_signature_changed_is_not_carried() {
    let trace = fixture("laps-and-a-menu");
    let mut lines = lines(&trace);
    let first_call = lines
        .iter()
        .position(|line| line["call"].is_string())
        .unwrap();
    let name = lines[first_call]["call"].as_str().unwrap().to_string();
    lines[1]["methods"][&name] = "Observe(Unkeyed) [] -> Geometry".into();
    let stopped = undump(&joined(&lines)).unwrap().stopped.unwrap();
    assert_eq!(stopped.record, name);
    assert!(stopped.why.contains("its signature was"), "{}", stopped.why);
}

#[test]
fn a_dump_from_before_format_7_keeps_every_part_of_a_read_on_its_read() {
    let trace = fixture("laps-and-a-menu");
    let mut lines = lines(&trace);
    for line in lines.iter_mut().filter(|line| line["media"].is_string()) {
        let object = line.as_object_mut().unwrap();
        object.remove("continues");
        object.remove("each");
    }
    let undumped = undump(&joined(&lines)).unwrap();
    let parts = |trace: &[u8]| -> Vec<bool> {
        let (_, events) = trace::decode(trace).unwrap();
        events
            .filter_map(|e| match e {
                Event::Call(call) => Access::of(&call),
                _ => None,
            })
            .map(|access| access.continues)
            .collect()
    };
    let (recorded, derived) = (parts(&trace), parts(&undumped.trace));
    assert!(
        recorded.iter().filter(|c| **c).count() > 0,
        "the fixture reads more than a part at once"
    );
    assert_eq!(recorded.len(), derived.len());
    let lost = recorded
        .iter()
        .zip(&derived)
        .filter(|(r, d)| **r && !**d)
        .count();
    assert_eq!(lost, 0, "a part that went on its read is joined to it");
}

#[test]
fn another_builds_call_neither_build_decodes_is_not_carried() {
    let trace = fixture("laps-and-a-menu");
    let mut lines = lines(&trace);
    assert!(undump(&joined(&lines)).unwrap().same_build);
    lines[0]["dumped_by"] = "0".repeat(40).into();
    let undecoded = (lines.iter())
        .position(|line| line["call"].is_string() && line["value"].is_null())
        .expect("a call no build decodes");
    let name = lines[undecoded]["call"].as_str().unwrap().to_string();
    let stopped = undump(&joined(&lines)).unwrap().stopped.unwrap();
    assert_eq!(stopped.record, name);
    assert!(
        stopped.why.contains("neither build reads its value"),
        "{}",
        stopped.why
    );
}

/// Two methods of one signature, by name and id.
fn twins() -> [(String, MethodId); 2] {
    let all: Vec<(String, String)> = capabilities()
        .flat_map(|table| {
            (table.methods.iter()).map(|m| (format!("{}::{}", table.name, m.name), signature(m)))
        })
        .collect();
    let (a, b) = (all.iter().enumerate())
        .find_map(|(i, a)| Some((a, all[i + 1..].iter().find(|b| b.1 == a.1)?)))
        .expect("two methods share a signature");
    [a, b].map(|(name, _)| {
        let (capability, method) = name.split_once("::").unwrap();
        (name.clone(), named(capability, method).unwrap())
    })
}

/// A trace whose `Repeated` and `TooLarge` name `method` by its id.
fn repeated_and_too_large(method: MethodId) -> Vec<u8> {
    let header = Header {
        build: [0; 20],
        device: "c606",
        boot: 1,
        terrain: firmware_services::trace::origin::Found::Absent,
    };
    super::super::surgery::encode(
        &header,
        [
            Event::Pass { now_ms: 10 },
            Event::Repeated { method, count: 3 },
            Event::TooLarge {
                capability: method.capability,
                operation: method.operation,
            },
            Event::Pass { now_ms: 20 },
        ],
    )
}

#[test]
fn a_repeated_or_too_large_record_names_its_method_and_undumps_to_its_id_here() {
    let [(name, id), (other, other_id)] = twins();
    let trace = repeated_and_too_large(id);
    let mut lines = lines(&trace);
    let at = |key: &str| (lines.iter()).position(|l| l[key].is_string()).unwrap();
    let (repeated, too_large) = (at("repeated"), at("too_large"));
    assert_eq!(lines[repeated]["repeated"], name.as_str());
    assert_eq!(lines[repeated]["count"], 3);
    assert_eq!(lines[too_large]["too_large"], name.as_str());
    let undumped = undump(&joined(&lines)).unwrap();
    assert_eq!(undumped.stopped, None);
    assert_eq!(events(&undumped.trace), events(&trace));

    // A build that numbers its methods otherwise reads the name, not the
    // number: here, the dump names another method of the same signature.
    lines[repeated]["repeated"] = other.as_str().into();
    lines[too_large]["too_large"] = other.as_str().into();
    let renamed = undump(&joined(&lines)).unwrap();
    assert_eq!(renamed.stopped, None);
    let (_, events) = trace::decode(&renamed.trace).unwrap();
    let carried: Vec<Event<'_>> = events
        .filter(|e| matches!(e, Event::Repeated { .. } | Event::TooLarge { .. }))
        .collect();
    assert_eq!(
        carried,
        [
            Event::Repeated {
                method: other_id,
                count: 3
            },
            Event::TooLarge {
                capability: other_id.capability,
                operation: other_id.operation
            },
        ]
    );
}

#[test]
fn a_repeated_or_too_large_record_of_a_gone_or_changed_method_is_not_carried() {
    let [(name, id), _] = twins();
    let trace = repeated_and_too_large(id);
    for key in ["repeated", "too_large"] {
        let mut gone = lines(&trace);
        let at = (gone.iter()).position(|l| l[key].is_string()).unwrap();
        gone[at][key] = "Ant::gone".into();
        let stopped = undump(&joined(&gone)).unwrap().stopped.expect(key);
        assert_eq!((stopped.record.as_str(), stopped.at_ms), ("Ant::gone", 10));
        assert!(stopped.why.contains("no such method"), "{}", stopped.why);

        let mut changed = lines(&trace);
        changed[1]["methods"][&name] = "Observe(Unkeyed) [] -> Geometry".into();
        let stopped = undump(&joined(&changed)).unwrap().stopped.expect(key);
        assert_eq!(stopped.record, name);
        assert!(stopped.why.contains("its signature was"), "{}", stopped.why);
    }
}

#[test]
fn a_repeated_record_an_older_dump_kept_by_number_is_not_carried() {
    let [(_, id), _] = twins();
    let trace = repeated_and_too_large(id);
    let mut lines = lines(&trace);
    let at = (lines.iter())
        .position(|l| l["repeated"].is_string())
        .unwrap();
    let mut record = [0u8; 32];
    let event = Event::Repeated {
        method: id,
        count: 3,
    };
    let len = put_record(&event, 10, &mut record).unwrap();
    lines[at] = json!({ "record": hex(&record[..len]), "at": 10, "value": format!("{event:?}") });
    let stopped = undump(&joined(&lines)).unwrap().stopped.unwrap();
    assert!(stopped.why.contains("by a number"), "{}", stopped.why);
}
