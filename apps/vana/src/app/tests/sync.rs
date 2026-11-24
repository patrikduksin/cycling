use super::phone::{Phone, Radio2, Transport, attach, header};
use super::power::{Switch, home, pass};
use super::presentation::Screen;
use super::recording::{FlakyBulk, tick_with};
use super::*;
use crate::app::format::{Event as Wipe, Format, Stage};
use crate::app::navigation::To;
use crate::app::power::{AUTO_OFF_MS, Sheet};
use crate::ride::current::Request;
use crate::ride::log::Status;
use crate::screens::chrome::Tone;
use crate::screens::done::Kept;
use crate::screens::power::Sheet as View;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::HEART_RATE;
use device_api::server::Method;
use std::string::String;

const MEMORY_SECTORS: usize = 64;
const HEART_72: [u8; 8] = [0, 0, 0, 0, 0, 0, 1, 72];
const START: &str = "start=1792249200";

fn in_memory() -> Journal {
    Journal::volatile(MEMORY_SECTORS)
}

fn cold(journal: &mut Journal, radio: &mut Radio) -> Runtime {
    let mut runtime = ready_runtime(journal, radio);
    attach(&mut runtime);
    radio.connected(HEART_RATE, 0);
    runtime
}

fn start(runtime: &mut Runtime, journal: &mut Journal, radio: &mut Radio, mut now: u64) -> u64 {
    runtime.navigation.go(To::Page(Page::Ride));
    runtime.input_received(press(Role::Act), now, now, radio);
    let asked = now;
    while runtime.ride.internal().status() != Status::Recording {
        tick(runtime, journal, radio, now);
        now += 50;
        let status = runtime.ride.internal().status();
        assert!(now < asked + 5_000, "the ride never started: {status:?}");
    }
    now
}

fn ride(runtime: &mut Runtime, journal: &mut Journal, radio: &mut Radio, now: u64) -> u64 {
    let mut now = start(runtime, journal, radio, now);
    let end = now + 9_000;
    while now < end {
        radio.data(HEART_RATE, HEART_72, now);
        tick(runtime, journal, radio, now);
        now += 250;
    }
    runtime.input_received(press(Role::Act), now, now, radio);
    for step in 0..4 {
        tick(runtime, journal, radio, now + step * 50);
    }
    runtime.input_received(press(Role::Back), now + 400, now + 400, radio);
    runtime.input_received(press(Role::Act), now + 800, now + 800, radio);
    for step in 0..20 {
        tick(runtime, journal, radio, now + 800 + step * 100);
    }
    assert!(!runtime.recorder_busy());
    assert_eq!(runtime.broken(), None);
    now + 3_000
}

fn unsaved(listed: &str) -> Vec<String> {
    listed
        .split(r#"{"id":""#)
        .skip(1)
        .filter(|ride| ride[..ride.find('}').unwrap()].contains(r#""unsaved":true"#))
        .map(|ride| String::from(&ride[..ride.find('"').unwrap()]))
        .collect()
}

fn session<'a>(
    runtime: &'a mut Runtime,
    journal: &'a mut Journal,
    now: u64,
) -> (Phone<'a>, String) {
    runtime.open_phone(PhoneMode::Home);
    let mut phone = Phone {
        runtime,
        journal,
        transport: Transport::default(),
        radio: Radio2::default(),
        now,
    };
    phone.step();
    let token = phone.token();
    (phone, token)
}

fn listed(phone: &mut Phone<'_>, token: &str) -> String {
    for _ in 0..100 {
        let (_, body) = phone.get(Method::Get, "/api/activities", "", token);
        let listed = String::from_utf8(body).unwrap();
        if listed.contains(r#""indexing":false"#) {
            return listed;
        }
    }
    panic!("the list never finished indexing");
}

fn export(phone: &mut Phone<'_>, token: &str, id: &str) -> (u32, u16) {
    let path = std::format!("/api/activities/{id}.fit");
    let (head, fit) = phone.get(Method::Get, &path, START, token);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let receipt = header(&head, "X-Receipt").expect(&head).parse().unwrap();
    let crc = u16::from_le_bytes([fit[fit.len() - 2], fit[fit.len() - 1]]);
    assert_eq!(firmware_services::fit::crc16(0, &fit[..fit.len() - 2]), crc);
    (receipt, crc)
}

fn acknowledge(phone: &mut Phone<'_>, token: &str, id: &str, receipt: u32, crc: u16) -> u16 {
    let query = std::format!("activity={id}&receipt={receipt}&crc={crc}");
    let (head, _) = phone.get(Method::Post, "/api/synced", &query, token);
    head[9..12].parse().unwrap()
}

fn close(mut phone: Phone<'_>, token: &str) -> u64 {
    phone.get(Method::Post, "/api/close", "", token);
    phone.step();
    phone.now
}

fn sync(runtime: &mut Runtime, journal: &mut Journal, now: u64) -> u64 {
    let (mut phone, token) = session(runtime, journal, now);
    for id in unsaved(&listed(&mut phone, &token)) {
        let (receipt, crc) = export(&mut phone, &token, &id);
        assert_eq!(acknowledge(&mut phone, &token, &id, receipt, crc), 204);
    }
    assert_eq!(unsaved(&listed(&mut phone, &token)), Vec::<String>::new());
    close(phone, &token)
}

fn toast(runtime: &mut Runtime, now: u64) -> Option<(String, Tone)> {
    let toast = runtime.toast(now)?;
    Some((
        String::from_utf8(toast.text().to_vec()).unwrap(),
        toast.tone,
    ))
}

fn starts(runtime: &Runtime) -> bool {
    runtime.ride.admits_start(runtime.ride.medium())
}

fn clear(runtime: &mut Runtime, journal: &mut Journal, now: u64) -> &'static str {
    let bound = runtime.ride.internal().next_slot();
    let command = std::format!("RIDE CLEAR CONFIRM {bound}");
    runtime.command(
        &command,
        journal,
        &mut MustNotWrite,
        now,
        &mut String::new(),
    )
}

fn wipe_stages(runtime: &mut Runtime, journal: &mut Journal, now: u64) {
    journal.0.fill(0xff);
    for _ in 0..10 {
        let Format::Running { stage, scope, .. } = runtime.format else {
            return advance(runtime, &mut Radio::new(), now);
        };
        runtime.format_event(match stage {
            Stage::RecordIntent => Wipe::IntentRecorded(scope),
            _ => Wipe::Wrote,
        });
    }
    panic!("the wipe never ended: {:?}", runtime.format);
}

fn wipe(runtime: &mut Runtime, journal: &mut Journal, now: u64) {
    wipe_stages(runtime, journal, now);
    let mut status = String::new();
    runtime.command("RIDE STATUS", journal, &mut MustNotWrite, now, &mut status);
    assert!(!runtime.recorder_busy(), "{status}");
}

fn clear_rides(phone: &mut Phone<'_>, token: &str) -> (u16, String) {
    let (head, body) = phone.get(Method::Delete, "/api/activities", "", token);
    (
        head[9..12].parse().unwrap(),
        String::from_utf8(body).unwrap(),
    )
}

fn clear_state(phone: &mut Phone<'_>, token: &str) -> String {
    let (_, body) = phone.get(Method::Get, "/api/status", "", token);
    let status = String::from_utf8(body).unwrap();
    let at = status.find(r#""clear":"#).expect(&status) + r#""clear":"#.len();
    String::from(status[at..].split(',').next().unwrap())
}

fn rider_presses(phone: &mut Phone<'_>, role: Role) {
    phone.now += 2_000;
    let at = phone.now;
    phone
        .runtime
        .input_received(press(role), at, at, &mut Radio::new());
}

fn ride_count(listed: &str) -> usize {
    listed.matches(r#"{"id":""#).count()
}

#[test]
fn a_cold_volatile_boot_reaches_ready_and_protects_nothing() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let runtime = cold(&mut journal, &mut radio);
    assert!(runtime.ride.volatile(Medium::Internal));
    assert_eq!(runtime.ride.internal().slots(), Some(MEMORY_SECTORS * 16));
    assert!(!runtime.holds_unsynced());
}

#[test]
fn a_volatile_ride_is_protected_from_its_start_and_never_called_saved() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let mut now = start(&mut runtime, &mut journal, &mut radio, 1_000);
    assert!(runtime.holds_unsynced(), "protected from its start");
    assert_eq!(runtime.ride.internal().written_samples(), 0);
    for _ in 0..40 {
        radio.data(HEART_RATE, HEART_72, now);
        tick(&mut runtime, &mut journal, &mut radio, now);
        now += 250;
    }
    assert_eq!(
        toast(&mut runtime, now),
        Some(("NOT SAVED".into(), Tone::Warn)),
        "the ride pages' sticky toast"
    );
    runtime.input_received(press(Role::Act), now, now, &mut radio);
    for step in 0..4 {
        tick(&mut runtime, &mut journal, &mut radio, now + step * 50);
    }
    runtime.input_received(press(Role::Back), now + 400, now + 400, &mut radio);
    assert_eq!(runtime.navigation.page(), Page::EndRide);
    runtime.input_received(press(Role::Act), now + 1_200, now + 1_200, &mut radio);
    for step in 0..20 {
        tick(
            &mut runtime,
            &mut journal,
            &mut radio,
            now + 1_200 + step * 100,
        );
    }
    now += 4_000;
    assert_eq!(runtime.navigation.page(), Page::Done);
    runtime.refresh_done();
    assert_eq!(runtime.done.kept(), Kept::Unsynced, "never called saved");
    assert_eq!(runtime.broken(), None);

    sync(&mut runtime, &mut journal, now);
    assert!(!runtime.holds_unsynced());
}

#[test]
fn a_durable_ride_is_saved_and_can_be_discarded() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    start(&mut runtime, &mut journal, &mut radio, 1_000);
    assert!(!runtime.holds_unsynced());
    assert!(runtime.can_discard());
}

#[test]
fn turning_off_and_auto_off_wait_until_the_phone_has_every_ride() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    attach(&mut runtime);
    radio.connected(HEART_RATE, 0);
    let mut switch = Switch::new(true);
    let mut now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    runtime.navigation.go(To::Page(Page::Home));

    runtime.input_received(press(Role::Back), now, now, &mut radio);
    pass(&mut runtime, &mut screen, &radio, &mut switch, now);
    assert!(screen.panel.shows_sheet(View::Confirm { unsaved: 1 }));
    runtime.input_received(press(Role::Act), now + 500, now + 500, &mut radio);
    assert_eq!(runtime.power.sheet, Sheet::Closed, "ACT does not turn off");
    now += 1_000;
    for at in [now, now + AUTO_OFF_MS, now + 2 * AUTO_OFF_MS] {
        tick(&mut runtime, &mut journal, &mut radio, at);
        pass(&mut runtime, &mut screen, &radio, &mut switch, at);
        assert_eq!(runtime.power.sheet, Sheet::Closed, "auto-off refuses");
    }
    assert!(switch.requests.is_empty());
    assert!(runtime.holds_unsynced());

    now = sync(&mut runtime, &mut journal, now + 2 * AUTO_OFF_MS);
    runtime.navigation.go(To::Page(Page::Home));
    runtime.input_received(press(Role::Back), now, now, &mut radio);
    pass(&mut runtime, &mut screen, &radio, &mut switch, now);
    assert!(screen.panel.shows_sheet(View::Confirm { unsaved: 0 }));
    runtime.input_received(press(Role::Act), now + 500, now + 500, &mut radio);
    assert_eq!(runtime.power.sheet, crate::app::power::TURNING_OFF);
}

#[test]
fn the_phone_confirms_only_the_newest_whole_export() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    let (_, status) = phone.get(Method::Get, "/api/status", "", &token);
    let status = String::from_utf8(status).unwrap();
    assert!(status.contains(r#""volatile":true"#), "{status}");
    let ids = unsaved(&listed(&mut phone, &token));
    assert_eq!(ids.len(), 1);
    let id = &ids[0];

    phone.transport.ask(
        Method::Get,
        &std::format!("/api/activities/{id}.fit"),
        START,
        &token,
    );
    phone.step();
    let head = String::from_utf8_lossy(&phone.transport.pending).into_owned();
    let cut: u32 = header(&head, "X-Receipt").expect(&head).parse().unwrap();
    let (first, crc) = export(&mut phone, &token, id);
    let (newest, same) = export(&mut phone, &token, id);
    assert_eq!(crc, same, "both exports wrote the same file");
    assert!(cut != newest && first != newest);
    assert_eq!(acknowledge(&mut phone, &token, id, cut, crc), 409);
    assert_eq!(
        acknowledge(&mut phone, &token, id, first, crc),
        409,
        "stale"
    );
    assert_eq!(acknowledge(&mut phone, &token, id, newest, crc ^ 1), 409);
    assert!(phone.runtime.holds_unsynced());
    assert_eq!(acknowledge(&mut phone, &token, id, newest, crc), 204);
    assert!(!phone.runtime.holds_unsynced());
    assert_eq!(acknowledge(&mut phone, &token, id, newest, crc), 404);
    assert_eq!(acknowledge(&mut phone, &token, "b0-1", newest, crc), 404);
    let (head, _) = phone.get(Method::Post, "/api/synced", "activity=i0-1", &token);
    assert!(head.starts_with("HTTP/1.1 400"), "{head}");
}

#[test]
fn clear_with_unsaved_rides_asks_and_back_keeps_them_for_a_later_acknowledgment() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    let id = unsaved(&listed(&mut phone, &token)).remove(0);
    let (receipt, crc) = export(&mut phone, &token, &id);
    let now = close(phone, &token);

    assert_eq!(clear(&mut runtime, &mut journal, now), "ACCEPTED");
    assert_eq!(runtime.navigation.page(), Page::LoseRides);
    assert_eq!(runtime.broken(), None);
    runtime.input_received(press(Role::Back), now + 500, now + 500, &mut radio);
    assert_eq!(runtime.navigation.page(), Page::Home);
    assert!(matches!(runtime.ride.host(), Request::Declined(_)));
    assert_eq!(runtime.format, Format::Idle, "nothing was wiped");
    assert!(runtime.holds_unsynced());

    let (mut phone, token) = session(&mut runtime, &mut journal, now + 1_000);
    assert_eq!(acknowledge(&mut phone, &token, &id, receipt, crc), 204);
    close(phone, &token);
    assert!(!runtime.holds_unsynced());
}

#[test]
fn clear_with_unsaved_rides_discards_them_once_the_rider_confirms() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let mut now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    now = ride(&mut runtime, &mut journal, &mut radio, now);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    let id = unsaved(&listed(&mut phone, &token)).remove(0);
    let (receipt, crc) = export(&mut phone, &token, &id);
    let now = close(phone, &token);

    runtime.command(
        "RIDE STATUS",
        &mut journal,
        &mut MustNotWrite,
        now,
        &mut String::new(),
    );
    assert_eq!(clear(&mut runtime, &mut journal, now), "ACCEPTED");
    assert_eq!(runtime.navigation.page(), Page::LoseRides);
    runtime.input_received(press(Role::Act), now + 500, now + 500, &mut radio);
    assert!(!runtime.holds_unsynced(), "the rider discarded them");
    assert!(matches!(
        runtime.format,
        Format::Running { clear: Some(_), .. }
    ));
    assert_eq!(runtime.broken(), None);
    wipe(&mut runtime, &mut journal, now);
    assert_eq!(runtime.ride.internal().status(), Status::Ready);
    assert_eq!(runtime.format, Format::Idle);
    assert_eq!(runtime.navigation.page(), Page::Home);

    let (mut phone, token) = session(&mut runtime, &mut journal, now + 1_000);
    assert_eq!(acknowledge(&mut phone, &token, &id, receipt, crc), 404);
    close(phone, &token);
}

#[test]
fn sixteen_rides_fill_the_set_and_the_phone_saves_them_all() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let mut now = 1_000;
    for _ in 0..16 {
        now = ride(&mut runtime, &mut journal, &mut radio, now);
    }
    assert_eq!(
        runtime.sync.count(),
        16,
        "more than the four catalog summaries"
    );
    runtime.navigation.go(To::Page(Page::Home));
    runtime.power(&mut Screen::new().shell, &mut Switch::new(true), now);
    runtime.open_power_sheet(now);
    assert_eq!(runtime.power.view(), Some(View::Confirm { unsaved: 16 }));
    runtime.power.sheet = Sheet::Closed;
    assert!(!starts(&runtime) && runtime.holds_unsynced());
    sync(&mut runtime, &mut journal, now);
    assert!(!starts(&runtime) && !runtime.holds_unsynced());
}

#[test]
fn sixteen_synced_rides_need_a_clear_before_the_next() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let mut now = 1_000;
    for _ in 0..16 {
        now = ride(&mut runtime, &mut journal, &mut radio, now);
        now = sync(&mut runtime, &mut journal, now);
    }
    assert!(!starts(&runtime));
    runtime.navigation.go(To::Page(Page::Ride));
    runtime.input_received(press(Role::Act), now, now, &mut radio);
    for step in 0..20 {
        tick(&mut runtime, &mut journal, &mut radio, now + step * 50);
    }
    assert_eq!(
        runtime.ride.internal().status(),
        Status::Saved,
        "no 17th start"
    );
    let mut out = String::new();
    let reply = runtime.command("RIDE START", &mut journal, &mut MustNotWrite, now, &mut out);
    assert_eq!(reply, "STATE");

    now += 2_000;
    runtime.command(
        "RIDE STATUS",
        &mut journal,
        &mut MustNotWrite,
        now,
        &mut out,
    );
    assert_eq!(clear(&mut runtime, &mut journal, now), "ACCEPTED");
    assert_ne!(
        runtime.navigation.page(),
        Page::LoseRides,
        "nothing to lose"
    );
    wipe(&mut runtime, &mut journal, now);
    assert!(starts(&runtime));
    ride(&mut runtime, &mut journal, &mut radio, now + 1_000);
    assert_eq!(runtime.ride.internal().next_ride_id(), 2, "ids start again");
    assert!(runtime.holds_unsynced());
}

#[test]
fn a_continuation_segment_on_a_volatile_journal_is_protected() {
    let mut journal = in_memory();
    let mut bulk = FlakyBulk {
        sectors: std::collections::BTreeMap::new(),
        fail: false,
    };
    let mut runtime = booted(Profile::HeartRate);
    let mut now = 0;
    while runtime.ride.internal().status() == Status::Scanning
        || runtime.ride.bulk().status() == Status::Scanning
    {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    let mut out = String::new();
    runtime.command("RIDE BULK INIT", &mut journal, &mut bulk, now, &mut out);
    while runtime.ride.bulk().status() != Status::Ready {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    runtime.command("RIDE BULK STATUS", &mut journal, &mut bulk, now, &mut out);
    assert_eq!(
        runtime.command("RIDE START", &mut journal, &mut bulk, now, &mut out),
        "ACCEPTED"
    );
    while runtime.ride.bulk().status() != Status::Recording {
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
    }
    assert!(!runtime.holds_unsynced(), "a card ride is durable");
    bulk.fail = true;
    while runtime.ride.internal().status() != Status::Recording {
        now += 250;
        tick_with(&mut runtime, &mut journal, &mut bulk, now);
        assert!(now < 60_000, "the continuation never started");
    }
    assert!(
        runtime.holds_unsynced(),
        "the segment in memory is protected"
    );
    assert_eq!(runtime.broken(), None);
}

#[test]
fn a_volatile_boot_forgets_what_files_kept_about_its_ride_ids() {
    use crate::workout::history::{Record, load, save};
    let old = Record {
        journal: Medium::Internal.journal(),
        ride_id: 1,
        started_utc_ms: None,
        name: crate::workout::model::Name::new("OLD RIDE"),
        test: None,
        summary: Default::default(),
    };
    for (mut journal, forgets) in [(in_memory(), true), (Journal::erased(), false)] {
        let mut card = crate::workout::library::tests::formatted();
        save(&mut card, &old).unwrap();
        let mut runtime = booted(Profile::HeartRate);
        for now in 0..1_000 {
            runtime.tick(
                &mut journal,
                &mut card,
                now,
                &mut Radio::new(),
                &mut Absent,
                Observations {
                    position: &Absent,
                    network_time: Default::default(),
                    input: &Absent,
                    physical: no_physical_sensors(),
                    fitted: FITTED,
                },
            );
        }
        assert_eq!(runtime.ride.internal().status(), Status::Ready);
        let kept = load(&mut card, b'i', 1).unwrap();
        assert_eq!(kept.is_none(), forgets, "volatile boot forgets: {forgets}");
    }
}

#[test]
fn the_page_clears_synced_rides_without_asking() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let mut now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    now = sync(&mut runtime, &mut journal, now);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    assert_eq!(ride_count(&listed(&mut phone, &token)), 1);

    assert_eq!(
        clear_rides(&mut phone, &token),
        (202, r#"{"clear":"clearing"}"#.into())
    );
    assert_ne!(phone.runtime.navigation.page(), Page::LoseRides);
    wipe_stages(phone.runtime, phone.journal, phone.now);
    assert_eq!(clear_state(&mut phone, &token), r#""cleared""#);
    assert_eq!(clear_state(&mut phone, &token), "null", "reported once");
    assert!(matches!(phone.runtime.ride.host(), Request::Idle));
    assert_eq!(ride_count(&listed(&mut phone, &token)), 0, "listed again");
    assert_eq!(phone.runtime.broken(), None);
}

#[test]
fn the_page_clears_unsaved_rides_only_once_the_rider_confirms_on_the_device() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    assert_eq!(unsaved(&listed(&mut phone, &token)).len(), 1);

    assert_eq!(
        clear_rides(&mut phone, &token),
        (202, r#"{"clear":"asking"}"#.into())
    );
    assert_eq!(phone.runtime.navigation.page(), Page::LoseRides);
    assert_eq!(clear_state(&mut phone, &token), r#""asking""#);
    rider_presses(&mut phone, Role::Back);
    assert_eq!(clear_state(&mut phone, &token), r#""kept""#);
    assert_eq!(phone.runtime.format, Format::Idle, "nothing was wiped");
    assert_eq!(unsaved(&listed(&mut phone, &token)).len(), 1);

    assert_eq!(
        clear_rides(&mut phone, &token),
        (202, r#"{"clear":"asking"}"#.into()),
        "the page may ask again"
    );
    assert_eq!(phone.runtime.navigation.page(), Page::LoseRides);
    rider_presses(&mut phone, Role::Act);
    assert!(!phone.runtime.holds_unsynced(), "the rider discarded it");
    wipe_stages(phone.runtime, phone.journal, phone.now);
    assert_eq!(clear_state(&mut phone, &token), r#""cleared""#);
    assert_eq!(ride_count(&listed(&mut phone, &token)), 0);
    assert_eq!(phone.runtime.broken(), None);
}

#[test]
fn the_page_cannot_clear_until_the_host_reads_its_own_clear() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    assert_eq!(clear(&mut runtime, &mut journal, now), "ACCEPTED");
    runtime.input_received(press(Role::Back), now, now, &mut radio);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    listed(&mut phone, &token);
    assert_eq!(
        clear_rides(&mut phone, &token),
        (409, r#"{"error":"busy"}"#.into())
    );
    assert_eq!(
        clear_state(&mut phone, &token),
        "null",
        "not the page's clear"
    );
    assert!(
        matches!(phone.runtime.ride.host(), Request::Declined(_)),
        "left for the host's RIDE STATUS"
    );
}

fn tick_on(
    runtime: &mut Runtime,
    store: &mut impl RideStorage,
    bulk: &mut impl device_api::bulk::ReadWrite,
    now: u64,
) {
    runtime.tick(
        store,
        bulk,
        now,
        &mut Radio::new(),
        &mut Absent,
        Observations {
            position: &Absent,
            network_time: Default::default(),
            input: &Absent,
            physical: no_physical_sensors(),
            fitted: FITTED,
        },
    );
}

#[test]
fn a_device_without_ride_storage_reads_unsupported_not_broken() {
    let mut absent = Absent;
    let mut store = firmware_services::storage::AppStorage::new(&mut absent);
    let mut runtime = booted(Profile::HeartRate);
    for now in 0..10 {
        tick_on(&mut runtime, &mut store, &mut MustNotWrite, now);
    }
    assert_eq!(runtime.ride.internal().status(), Status::Unsupported);
    assert!(!runtime.ride.volatile(Medium::Internal));
    assert!(!runtime.holds_unsynced());
    assert_eq!(runtime.broken(), None);
}

#[test]
fn a_durable_device_refuses_the_pages_clear_and_keeps_its_rides() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    assert_eq!(ride_count(&listed(&mut phone, &token)), 1);
    assert_eq!(
        clear_rides(&mut phone, &token),
        (409, r#"{"error":"durable"}"#.into())
    );
    assert_eq!(phone.runtime.format, Format::Idle);
    assert!(matches!(phone.runtime.ride.host(), Request::Idle));
    assert_eq!(ride_count(&listed(&mut phone, &token)), 1);
}

#[test]
fn the_hosts_clear_ends_a_phone_session() {
    let mut journal = in_memory();
    let mut radio = Radio::new();
    let mut runtime = cold(&mut journal, &mut radio);
    let now = ride(&mut runtime, &mut journal, &mut radio, 1_000);
    let (mut phone, token) = session(&mut runtime, &mut journal, now);
    listed(&mut phone, &token);
    assert_eq!(clear(phone.runtime, phone.journal, phone.now), "ACCEPTED");
    assert_eq!(phone.runtime.navigation.page(), Page::LoseRides);
    phone.step();
    assert!(phone.runtime.phone_status().is_none());
    assert_eq!(phone.runtime.phone_closed(), Some(PhoneClosed::Ride));
}
