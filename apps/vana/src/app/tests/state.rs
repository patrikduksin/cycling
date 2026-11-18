use super::*;
use firmware_services::why::{Fields, Value};
use std::boxed::Box;
use std::collections::BTreeSet;
use std::string::String;

fn named(runtime: &Runtime) -> Vec<(&'static str, &'static str, String)> {
    let mut named = Vec::new();
    let mut sink = |owner, field, value: Value<'_>| {
        let value = match value {
            Value::Debug(value) => std::format!("{value:?}"),
            Value::Buffer { len, digest } => std::format!("{len} items {digest:x}"),
        };
        named.push((owner, field, value));
    };
    runtime.state(&mut Fields::new(&mut sink));
    named
}

#[test]
fn the_state_names_every_owner_and_each_field_once() {
    let mut runtime = booted(Profile::default());
    let crumbs = Vec::leak(std::vec![crate::app::Crumb::default(); 64]);
    let line = Vec::leak(std::vec![crate::app::Crumb::default(); crate::app::COURSE_POINTS]);
    let turns = Vec::leak(std::vec![crate::app::Turn::default(); crate::app::COURSE_TURNS]);
    let map = Box::leak(Box::new(crate::app::MapFrame::default()));
    let course = Box::leak(Box::new(crate::ride::course::Course::empty()));
    let finder = Box::leak(Box::new(crate::app::CourseFinder::new()));
    let track = crate::app::RideTrack::new(crumbs, line, turns, map, course, finder);
    runtime.attach_track(Box::leak(Box::new(track)));
    let named = named(&runtime);
    let owners: BTreeSet<&str> = named.iter().map(|(owner, ..)| *owner).collect();
    for owner in [
        "runtime",
        "navigation",
        "ride",
        "recorder_internal",
        "recorder_bulk",
        "follow",
        "breadcrumb",
        "course",
        "readings",
        "calibration",
        "phone",
        "power",
        "cues",
        "track",
    ] {
        assert!(owners.contains(owner), "{owner} missing from {owners:?}");
    }
    let mut seen = BTreeSet::new();
    for (owner, field, _) in &named {
        assert!(seen.insert((owner, field)), "{owner}.{field} named twice");
    }
    let bytes: usize = named.iter().map(|(.., value)| value.len()).sum();
    assert!(bytes < 64 * 1024, "{bytes} bytes of state each pass");
}

#[test]
fn a_phone_session_shows_neither_its_password_nor_its_token() {
    let mut journal = Journal::erased();
    let mut runtime = booted(Profile::default());
    super::phone::attach(&mut runtime);
    runtime.open_phone(PhoneMode::AccessPoint);
    let mut phone = super::phone::Phone {
        runtime: &mut runtime,
        journal: &mut journal,
        transport: super::phone::Transport::default(),
        radio: super::phone::Radio2::default(),
        now: 1_000,
    };
    phone.step();
    phone.step();
    let password = String::from(phone.runtime.phone_status().unwrap().password.text());
    let token = phone.token();
    for (owner, field, value) in named(phone.runtime) {
        for secret in [&password, &token] {
            assert!(
                !value.contains(secret.as_str()),
                "{owner}.{field} shows a secret: {value}"
            );
        }
    }
}
