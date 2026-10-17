//! The phone session through `serve_phone`, with an in-memory transport.
use super::*;
use crate::sensors::ant::device_type::HEART_RATE;
use device_api::server::{
    AccessPoint, AccessPointConfig, AccessPointState, Method, Reach, Request, Server,
};
use std::string::String;

const HEART_72: [u8; 8] = [0, 0, 0, 0, 0, 0, 1, 72];
/// Like the device's exchange buffer.
const ROOM: usize = 4096;

/// One request at a time; the test drains what VANA wrote after each step.
#[derive(Default)]
struct Transport {
    listening: bool,
    request: Option<Request>,
    pending: Vec<u8>,
    finished: bool,
    next_id: u32,
}

impl Transport {
    fn ask(&mut self, method: Method, path: &str, query: &str, session: &str) {
        self.next_id += 1;
        let mut request = Request::new(method);
        request.id = self.next_id;
        request.path.push_str(path).unwrap();
        request.query.push_str(query).unwrap();
        request.session.push_str(session).unwrap();
        self.request = Some(request);
        self.pending.clear();
        self.finished = false;
    }
}

impl Server for Transport {
    fn listen(&mut self, on: bool) {
        self.listening = on;
    }
    fn request(&self) -> Option<Request> {
        self.request.clone().filter(|_| !self.finished)
    }
    fn room(&self) -> usize {
        // The test drains between steps, so room is the whole buffer less
        // this step's writes.
        ROOM.saturating_sub(self.pending.len() % ROOM)
    }
    fn write(&mut self, id: u32, bytes: &[u8]) -> usize {
        if self.finished || self.request.as_ref().is_none_or(|r| r.id != id) {
            return 0;
        }
        let n = bytes.len().min(self.room());
        self.pending.extend_from_slice(&bytes[..n]);
        n
    }
    fn finish(&mut self, id: u32) {
        if self.request.as_ref().is_some_and(|r| r.id == id) {
            self.finished = true;
        }
    }
}

#[derive(Default)]
struct Radio2 {
    started: Option<AccessPointConfig>,
    stops: u32,
    word: u32,
}

impl AccessPoint for Radio2 {
    fn start(&mut self, config: AccessPointConfig) -> Result<(), device_api::observation::Error> {
        self.started = Some(config);
        Ok(())
    }
    fn stop(&mut self) -> Result<(), device_api::observation::Error> {
        self.started = None;
        self.stops += 1;
        Ok(())
    }
    fn state(&self) -> AccessPointState {
        if self.started.is_some() {
            AccessPointState::Up { clients: 1 }
        } else {
            AccessPointState::Off
        }
    }
    fn reach(&self) -> Reach {
        Reach {
            access_point: self.started.map(|_| [192, 168, 4, 1]),
            station: Some([10, 0, 0, 7]),
        }
    }
    fn entropy(&mut self) -> u32 {
        self.word = self
            .word
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        self.word
    }
}

/// A runtime with one saved ride of about nine seconds in its journal.
fn saved_ride(journal: &mut Journal) -> (Runtime, super::Radio, u64) {
    let mut radio = super::Radio::new();
    let mut runtime = ready_runtime(journal, &mut radio);
    runtime.navigation.page = crate::screens::workout::Page::Ride;
    radio.connected(HEART_RATE, 0);
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    let mut now = 1000;
    while now < 10_000 {
        radio.data(HEART_RATE, HEART_72, now);
        tick(&mut runtime, journal, &mut radio, now);
        now += 250;
    }
    runtime.input(press(Button::BottomRight), now, &mut radio);
    for step in 0..4 {
        tick(&mut runtime, journal, &mut radio, now + step * 50);
    }
    runtime.input(press(Button::TopLeft), now + 400, &mut radio);
    runtime.input(press(Button::BottomRight), now + 800, &mut radio);
    for step in 0..20 {
        tick(&mut runtime, journal, &mut radio, now + 800 + step * 100);
    }
    assert!(!runtime.recording());
    (runtime, radio, now + 3000)
}

struct Phone<'a> {
    runtime: &'a mut Runtime,
    journal: &'a mut Journal,
    transport: Transport,
    radio: Radio2,
    now: u64,
}

impl Phone<'_> {
    fn step(&mut self) {
        self.now += 20;
        self.runtime.serve_phone(
            &mut self.transport,
            &mut self.radio,
            self.journal,
            &mut NoBulk,
            self.now,
        );
    }

    /// The whole response: head and body.
    fn get(&mut self, method: Method, path: &str, query: &str, session: &str) -> (String, Vec<u8>) {
        self.transport.ask(method, path, query, session);
        let mut response = Vec::new();
        for _ in 0..2000 {
            self.step();
            response.append(&mut self.transport.pending);
            if self.transport.finished {
                let split = response.windows(4).position(|w| w == b"\r\n\r\n").unwrap() + 4;
                let body = response.split_off(split);
                return (String::from_utf8(response).unwrap(), body);
            }
        }
        panic!("{path} never finished");
    }

    fn token(&self) -> String {
        String::from(self.runtime.phone_status().unwrap().token.text())
    }
}

fn attach(runtime: &mut Runtime) {
    runtime.attach_phone(std::boxed::Box::leak(
        std::boxed::Box::new(PhoneWork::new()),
    ));
}

fn header<'a>(head: &'a str, name: &str) -> Option<&'a str> {
    head.lines()
        .find_map(|line| line.strip_prefix(name)?.strip_prefix(": "))
}

#[test]
fn a_phone_on_home_wifi_lists_and_downloads_a_ride() {
    let mut journal = Journal::erased();
    let (mut runtime, _, now) = saved_ride(&mut journal);
    attach(&mut runtime);
    runtime.open_phone(PhoneMode::Home);
    let mut phone = Phone {
        runtime: &mut runtime,
        journal: &mut journal,
        transport: Transport::default(),
        radio: Radio2::default(),
        now,
    };
    phone.step();
    let status = phone.runtime.phone_status().unwrap();
    assert_eq!(status.address, Some([10, 0, 0, 7]));
    assert!(phone.transport.listening);
    assert!(phone.radio.started.is_none(), "home mode keeps the station");
    let token = phone.token();
    assert_eq!(token.len(), 16);
    assert!(status.url().unwrap().ends_with(&token));

    let (head, _) = phone.get(Method::Get, "/api/activities", "", "wrong");
    assert!(head.starts_with("HTTP/1.1 401"), "{head}");

    let (head, page) = phone.get(Method::Get, "/", &std::format!("t={token}"), &token);
    assert!(head.starts_with("HTTP/1.1 200"));
    assert_eq!(header(&head, "Content-Encoding"), Some("gzip"));
    assert!(header(&head, "Set-Cookie").unwrap().contains(&token));
    assert_eq!(page[..2], [0x1f, 0x8b]);

    // Indexing runs while serving; the list fills in.
    let mut listed = String::new();
    for _ in 0..50 {
        let (_, body) = phone.get(Method::Get, "/api/activities", "", &token);
        listed = String::from_utf8(body).unwrap();
        if listed.contains(r#""indexing":false"#) {
            break;
        }
    }
    assert!(listed.contains(r#""indexing":false"#), "{listed}");
    let at = listed.find(r#""id":""#).expect(&listed) + 6;
    let id = &listed[at..at + listed[at..].find('"').unwrap()];
    assert!(id.starts_with('i'), "{id}");
    assert!(listed.contains(r#""heart":true"#), "{listed}");

    let path = std::format!("/api/activities/{id}.fit");
    let (head, _) = phone.get(Method::Get, &path, "", &token);
    assert!(
        head.starts_with("HTTP/1.1 422"),
        "no clock without ?start: {head}"
    );

    let (head, fit) = phone.get(Method::Get, &path, "start=1792249200", &token);
    assert!(head.starts_with("HTTP/1.1 200"), "{head}");
    let length: usize = header(&head, "Content-Length").unwrap().parse().unwrap();
    assert_eq!(fit.len(), length);
    assert!(
        header(&head, "Content-Disposition")
            .unwrap()
            .contains("vana-2026-10-17-1500")
    );
    assert_eq!(&fit[8..12], b".FIT");
    let crc = firmware_services::fit::crc16(0, &fit[..fit.len() - 2]);
    assert_eq!(crc.to_le_bytes(), fit[fit.len() - 2..]);
    assert_eq!(phone.runtime.phone_status().unwrap().downloads, 1);

    let (head, _) = phone.get(Method::Delete, &path, "", &token);
    assert!(head.starts_with("HTTP/1.1 405"), "{head}");

    let (head, _) = phone.get(Method::Post, "/api/close", "", &token);
    assert!(head.starts_with("HTTP/1.1 204"), "{head}");
    phone.step();
    assert!(phone.runtime.phone_status().is_none());
    assert_eq!(phone.runtime.phone_closed(), Some(PhoneClosed::Rider));
    assert!(!phone.transport.listening);
}

#[test]
fn the_access_point_runs_for_the_session_and_stops_after_idle() {
    let mut journal = Journal::erased();
    let (mut runtime, _, now) = saved_ride(&mut journal);
    attach(&mut runtime);
    runtime.open_phone(PhoneMode::AccessPoint);
    let mut phone = Phone {
        runtime: &mut runtime,
        journal: &mut journal,
        transport: Transport::default(),
        radio: Radio2::default(),
        now,
    };
    phone.step();
    let config = phone.radio.started.expect("access point started");
    let status = phone.runtime.phone_status().unwrap();
    assert!(config.ssid.text().starts_with("VANA-"));
    assert_eq!(config.password.text().len(), 12);
    assert_eq!(config.password, status.password);
    phone.step();
    let status = phone.runtime.phone_status().unwrap();
    assert_eq!(status.address, Some([192, 168, 4, 1]));
    assert_eq!(
        status.join_text().as_str(),
        std::format!(
            "WIFI:T:WPA;S:{};P:{};;",
            config.ssid.text(),
            config.password.text()
        )
    );
    assert!(status.url().unwrap().starts_with("http://192.168.4.1/?t="));

    phone.now += crate::app::phone::IDLE_MS + 1;
    phone.step();
    assert_eq!(phone.runtime.phone_closed(), Some(PhoneClosed::Idle));
    phone.step();
    assert_eq!(phone.radio.stops, 1);
    assert!(phone.radio.started.is_none());

    // A new session has new secrets.
    phone.runtime.open_phone(PhoneMode::AccessPoint);
    phone.step();
    assert_ne!(phone.radio.started.unwrap().password, config.password);
}

#[test]
fn a_session_opened_while_the_journal_scans_lists_its_rides() {
    let mut journal = Journal::erased();
    let _ = saved_ride(&mut journal);
    // A restart: the journal is scanned again before rides can be read.
    let mut runtime = Runtime::new(Profile::HeartRate);
    attach(&mut runtime);
    runtime.open_phone(PhoneMode::Home);
    let mut radio = super::Radio::new();
    let mut phone = Phone {
        runtime: &mut runtime,
        journal: &mut journal,
        transport: Transport::default(),
        radio: Radio2::default(),
        now: 0,
    };
    for _ in 0..5 {
        phone.step();
    }
    assert_eq!(
        phone.runtime.ride.internal.status(),
        crate::ride::log::Status::Scanning
    );
    let token = phone.token();
    let (_, body) = phone.get(Method::Get, "/api/activities", "", &token);
    assert!(
        String::from_utf8(body)
            .unwrap()
            .contains(r#""indexing":true"#),
        "still waiting for the scan"
    );
    for _ in 0..SECTORS {
        tick(phone.runtime, phone.journal, &mut radio, phone.now);
    }
    let mut listed = String::new();
    for _ in 0..50 {
        let (_, body) = phone.get(Method::Get, "/api/activities", "", &token);
        listed = String::from_utf8(body).unwrap();
        if listed.contains(r#""indexing":false"#) {
            break;
        }
    }
    assert!(listed.contains(r#""ride":"#), "{listed}");
}

#[test]
fn a_session_does_not_open_during_a_ride() {
    let mut journal = Journal::erased();
    let mut radio = super::Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.page = crate::screens::workout::Page::Ride;
    runtime.input(press(Button::BottomRight), 1000, &mut radio);
    for step in 0..8 {
        tick(&mut runtime, &mut journal, &mut radio, 1000 + step * 250);
    }
    assert!(runtime.recording());
    attach(&mut runtime);
    runtime.open_phone(PhoneMode::AccessPoint);
    let mut access_point = Radio2::default();
    runtime.serve_phone(
        &mut Transport::default(),
        &mut access_point,
        &mut journal,
        &mut NoBulk,
        4000,
    );
    assert!(runtime.phone_status().is_none());
    assert_eq!(runtime.phone_closed(), Some(PhoneClosed::Ride));
    assert!(access_point.started.is_none());
}
