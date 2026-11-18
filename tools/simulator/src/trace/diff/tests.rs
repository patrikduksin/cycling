use super::*;
use crate::trace::surgery::encode;

fn traced(page: &'static str, body: &'static [u8]) -> Vec<u8> {
    framed(page, body, 0xAB, false)
}

fn framed(page: &'static str, body: &'static [u8], frame: u32, redacted: bool) -> Vec<u8> {
    let header = trace::Header {
        build: [0; 20],
        device: "c606",
        boot: 1,
        terrain: firmware_services::trace::origin::Found::Absent,
    };
    let method = crate::twin::script::named("Ant", "take_packet").unwrap();
    let events = [
        Event::Session {
            memory: firmware_services::trace::origin::Memory::ALL,
            redacted,
        },
        Event::Pass { now_ms: 1_000 },
        Event::Page("home"),
        Event::Panel(1),
        Event::Pass { now_ms: 2_000 },
        Event::Call(trace::Call {
            method,
            now_ms: 2_000,
            body,
        }),
        Event::Page(page),
        Event::Panel(frame),
    ];
    encode(&header, events)
}

#[test]
fn traces_that_hold_the_same_entries_do_not_part() {
    let a = traced("ride/main", &[1]);
    assert_eq!(
        diff(&a, &a),
        Ok(Diff {
            seen: None,
            panel: None,
            world: None,
            exclusions: Vec::new(),
        })
    );
}

#[test]
fn the_first_seen_entry_and_the_first_world_record_that_differ_are_named() {
    let diffed = diff(&traced("ride/main", &[1]), &traced("ride/climb", &[2])).unwrap();
    assert_eq!(
        diffed.seen,
        Some(Difference {
            at_ms: 2_000,
            a: Some("page ride/main".into()),
            b: Some("page ride/climb".into()),
        })
    );
    assert_eq!(
        diffed.world,
        Some(Difference {
            at_ms: 2_000,
            a: Some("Ant::take_packet 01".into()),
            b: Some("Ant::take_packet 02".into()),
        })
    );
    assert_eq!(
        diffed.to_string(),
        "seen: first differs at 00:02.000: page ride/main | page ride/climb\n\
         panel: the same\n\
         world: first differs at 00:02.000: Ant::take_packet 01 | Ant::take_packet 02\n"
    );
}

#[test]
fn a_frame_that_differs_is_named_with_its_pass_when_nothing_else_does() {
    let diffed = diff(
        &framed("ride/main", &[1], 0xAB, false),
        &framed("ride/main", &[1], 0xCD, false),
    )
    .unwrap();
    assert_eq!((diffed.seen, diffed.world), (None, None));
    assert_eq!(
        diffed.panel,
        Some(Difference {
            at_ms: 2_000,
            a: Some("frame 000000ab".into()),
            b: Some("frame 000000cd".into()),
        })
    );
}

#[test]
fn the_phone_pages_frames_are_left_out_by_name_when_a_trace_replaced_its_credentials() {
    let diffed = diff(
        &framed("phone", &[1], 0xAB, true),
        &framed("phone", &[1], 0xCD, false),
    )
    .unwrap();
    assert_eq!(diffed.panel, None);
    assert_eq!(
        diffed.exclusions,
        [
            "1 phone-page frames of a: a trace replaced the credentials they draw",
            "1 phone-page frames of b: a trace replaced the credentials they draw",
        ]
    );
    assert!(diffed.to_string().ends_with(
        "left out: 1 phone-page frames of b: a trace replaced the credentials they draw\n"
    ));
}
