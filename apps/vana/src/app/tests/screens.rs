//! The rider's screens through the phone session and their journal.
use super::phone::{Radio2, Transport, attach};
use super::*;
use crate::app::Overrides;
use crate::compose::bench::MAIN_BYTES;
use crate::compose::builtin::{ScreenId, main_tree};
use crate::compose::data::Metric;
use crate::compose::node::{Node, Size};
use crate::compose::parse::VERSION;
use crate::compose::registry::{self, VALUE};
use crate::testing::{JournalFlash, main_with};
use device_api::server::Method;
use device_api::storage::Region;
use firmware_services::storage::Journal as ScreensJournal;
use std::string::String;

const MAIN: u8 = ScreenId::RideMain.key();

struct Device {
    runtime: Runtime,
    journal: Journal,
    transport: Transport,
    radio: Radio2,
    flash: JournalFlash,
    now: u64,
}

impl Device {
    fn on(flash: JournalFlash) -> Self {
        let mut runtime = booted(Profile::HeartRate);
        attach(&mut runtime);
        runtime.attach_screens(std::boxed::Box::leak(
            std::boxed::Box::new(Overrides::new()),
        ));
        runtime.open_phone(PhoneMode::Home);
        let mut device = Self {
            runtime,
            journal: Journal::erased(),
            transport: Transport::default(),
            radio: Radio2::default(),
            flash,
            now: 0,
        };
        device.step();
        device
    }

    /// One pass's phone and screens steps, as `pass` runs them.
    fn step(&mut self) {
        self.now += 20;
        self.runtime.serve_phone(
            &mut self.transport,
            &mut self.radio,
            &mut self.journal,
            &mut MustNotWrite,
            self.now,
        );
        self.runtime.serve_screens(&mut self.flash);
    }

    fn request(&mut self, method: Method, path: &str, body: &[u8]) -> (u16, String) {
        let token = String::from(self.runtime.phone_status().unwrap().token.text());
        let (path, query) = path.split_once('?').unwrap_or((path, ""));
        self.transport.upload(method, path, &token, body, 40);
        if let Some(request) = self.transport.request.as_mut() {
            request.query.push_str(query).unwrap();
        }
        let mut response = Vec::new();
        for _ in 0..2000 {
            self.step();
            response.append(&mut self.transport.pending);
            if self.transport.finished {
                let text = String::from_utf8(response).unwrap();
                let status = text[9..12].parse().unwrap();
                let (_, body) = text.split_once("\r\n\r\n").unwrap();
                return (status, String::from(body));
            }
        }
        panic!("{method:?} {path} never finished");
    }

    fn restarted(self) -> Self {
        Self::on(self.flash)
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|byte| std::format!("{byte:02x}"))
        .collect()
}

/// The main page's built-in for the rider's lower line, as stored.
fn builtin_main(runtime: &Runtime) -> Vec<u8> {
    main_tree(runtime.preferences.options.main_slots)
        .iter()
        .flat_map(|node| node.bytes())
        .collect()
}

fn shown_main(runtime: &Runtime) -> Option<Vec<u8>> {
    let screen = runtime.screens().screen(MAIN)?;
    Some(
        screen
            .tree()
            .nodes()
            .iter()
            .flat_map(|node| node.bytes())
            .collect(),
    )
}

#[test]
fn a_phone_changes_ride_main_and_a_restart_shows_it() {
    let mut device = Device::on(JournalFlash::of(Region::Screens));
    let (status, body) = device.request(Method::Get, "/api/screens", b"");
    assert_eq!(status, 200, "{body}");
    assert_eq!(
        body,
        std::format!(
            r#"{{"version":{VERSION},"revision":0,"reset":false,"screens":[{{"name":"ride/main","key":{MAIN},"override":false,"nodes":"{}"}}]}}"#,
            hex(&builtin_main(&device.runtime))
        )
    );

    let power = main_with(Metric::Power);
    let (status, body) = device.request(Method::Put, "/api/screens/ride/main?base=0", &power);
    assert_eq!((status, body.as_str()), (200, r#"{"revision":1}"#));
    assert_eq!(shown_main(&device.runtime).as_deref(), Some(&power[..]));

    let mut device = device.restarted();
    assert_eq!(shown_main(&device.runtime).as_deref(), Some(&power[..]));
    let (_, body) = device.request(Method::Get, "/api/screens", b"");
    assert!(body.contains(r#""revision":1"#), "{body}");
    assert!(
        body.contains(&std::format!(
            r#""override":true,"nodes":"{}""#,
            hex(&power)
        )),
        "{body}"
    );
}

#[test]
fn a_refused_screen_answers_422_with_the_reason_and_writes_nothing() {
    let mut device = Device::on(JournalFlash::of(Region::Screens));
    let put = |device: &mut Device, nodes: &[u8]| {
        device.request(Method::Put, "/api/screens/ride/main?base=0", nodes)
    };
    let mut unknown = MAIN_BYTES.to_vec();
    unknown[4] = 99;
    assert_eq!(
        put(&mut device, &unknown),
        (
            422,
            String::from(
                r#"{"error":"refused","reason":"screen 0: node 1 is of unknown kind 99"}"#
            )
        )
    );
    let (status, body) = put(&mut device, &MAIN_BYTES[..5]);
    assert_eq!(status, 422);
    assert_eq!(
        body,
        r#"{"error":"refused","reason":"screen 0: the record ends early"}"#
    );

    let speed = Node::leaf(VALUE, Metric::Speed as u8, Size::CONTENT).bytes();
    let mut crowded = Node::row(12, Size::grow(1)).bytes().to_vec();
    for _ in 0..12 {
        crowded.extend_from_slice(&speed);
    }
    let (status, body) = put(&mut device, &crowded);
    assert_eq!(status, 422, "{body}");
    assert!(
        body.starts_with(r#"{"error":"refused","reason":"screen 0: node "#)
            && body.contains(" px wide, needs "),
        "{body}"
    );

    assert_eq!(device.flash.mutations, 0, "a refused screen writes nothing");
    assert_eq!(shown_main(&device.runtime), None);
}

#[test]
fn a_change_made_against_an_older_revision_answers_409() {
    let mut device = Device::on(JournalFlash::of(Region::Screens));
    let power = main_with(Metric::Power);
    let heart = main_with(Metric::Heart);
    let path = "/api/screens/ride/main";
    let (status, _) = device.request(Method::Put, &std::format!("{path}?base=0"), &power);
    assert_eq!(status, 200);
    let writes = device.flash.mutations;

    let stale = device.request(Method::Put, &std::format!("{path}?base=0"), &heart);
    assert_eq!(
        stale,
        (409, String::from(r#"{"error":"stale","revision":1}"#))
    );
    let stale = device.request(Method::Delete, &std::format!("{path}?base=0"), b"");
    assert_eq!(stale.0, 409);
    assert_eq!(device.flash.mutations, writes);
    assert_eq!(shown_main(&device.runtime).as_deref(), Some(&power[..]));

    let reset = device.request(Method::Delete, &std::format!("{path}?base=1"), b"");
    assert_eq!(reset, (200, String::from(r#"{"revision":2}"#)));
    assert_eq!(shown_main(&device.runtime), None);
    let (status, body) = device.request(Method::Put, path, &heart);
    assert_eq!((status, body.as_str()), (400, r#"{"error":"base"}"#));
}

#[test]
fn a_dry_run_lays_the_screen_out_under_each_flag_combination_and_writes_nothing() {
    let mut device = Device::on(JournalFlash::of(Region::Screens));
    let power = main_with(Metric::Power);
    let (status, body) =
        device.request(Method::Post, "/api/screens/check?screen=ride/main", &power);
    assert_eq!(status, 200, "{body}");
    assert!(body.starts_with(r#"{"screen":"ride/main","solves":[{"flags":"","leaves":[["#));
    let flags: Vec<&str> = body
        .split(r#""flags":""#)
        .skip(1)
        .map(|rest| rest.split('"').next().unwrap())
        .collect();
    assert_eq!(flags, ["", "COURSE", "ARRIVED", "COURSE+ARRIVED"]);
    assert!(body.ends_with("]}]}"), "{body}");
    assert_eq!(device.flash.mutations, 0);
    assert_eq!(shown_main(&device.runtime), None);

    let (status, _) = device.request(Method::Post, "/api/screens/check?screen=ride/main", &[9; 4]);
    assert_eq!(status, 422);
    let (status, _) = device.request(Method::Post, "/api/screens/check?screen=home", &power);
    assert_eq!(status, 404);
}

#[test]
fn the_catalogue_names_every_tile_with_each_setting_and_its_looks() {
    let mut device = Device::on(JournalFlash::of(Region::Screens));
    let (status, body) = device.request(Method::Get, "/api/screens/catalogue", b"");
    assert_eq!(status, 200, "{body}");
    assert!(
        body.starts_with(&std::format!(
            r#"{{"version":{VERSION},"panel":[240,320],"scale":4,"#
        )),
        "{body}"
    );
    for (component, tile) in registry::every() {
        let named = std::format!(r#""tile":"{}","id":{component},"#, tile.name());
        // A script slot with nothing installed has no looks, so no place.
        assert_eq!(
            body.contains(&named),
            component != registry::SCRIPT,
            "{named}: {body}"
        );
    }
    assert!(
        body.contains(&std::format!(
            r#""tile":"value","id":{VALUE},"arg":{},"label":"SPEED""#,
            Metric::Speed as u8
        )),
        "{body}"
    );
    assert!(body.ends_with("]}]}"), "{body}");
}

#[test]
fn a_record_of_another_catalogue_version_shows_the_builtin_and_is_replaced() {
    let mut flash = JournalFlash::of(Region::Screens);
    let mut older = std::vec![VERSION + 1, 1, MAIN, (MAIN_BYTES.len() / 4) as u8];
    older.extend_from_slice(&main_with(Metric::Power));
    older.push(0);
    ScreensJournal::new(&mut flash, Region::Screens)
        .save(&older)
        .unwrap();

    let mut device = Device::on(flash);
    assert_eq!(shown_main(&device.runtime), None);
    let (_, body) = device.request(Method::Get, "/api/screens", b"");
    assert!(body.contains(r#""revision":1,"reset":true"#), "{body}");
    let heart = main_with(Metric::Heart);
    let saved = device.request(Method::Put, "/api/screens/ride/main?base=1", &heart);
    assert_eq!(saved, (200, String::from(r#"{"revision":2}"#)));
    let device = device.restarted();
    assert_eq!(shown_main(&device.runtime).as_deref(), Some(&heart[..]));
}

#[test]
fn without_its_memory_the_screens_api_answers_503() {
    let mut device = Device::on(JournalFlash::of(Region::Screens));
    device.runtime.overrides = None;
    let (status, body) = device.request(Method::Get, "/api/screens", b"");
    assert_eq!((status, body.as_str()), (503, r#"{"error":"memory"}"#));
    assert_eq!(device.runtime.screens(), &crate::compose::Screens::EMPTY);
}
