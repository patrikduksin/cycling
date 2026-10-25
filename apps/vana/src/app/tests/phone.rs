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
    /// The body still to be read, and the most each `read` hands over.
    body: Vec<u8>,
    piece: usize,
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
        self.body.clear();
        self.pending.clear();
        self.finished = false;
    }

    fn upload(&mut self, method: Method, path: &str, session: &str, body: &[u8], piece: usize) {
        self.ask(method, path, "", session);
        if let Some(request) = self.request.as_mut() {
            request.body = body.len() as u32;
        }
        self.body = body.to_vec();
        self.piece = piece.max(1);
    }
}

impl Server for Transport {
    fn listen(&mut self, on: bool) {
        self.listening = on;
    }
    fn read(&mut self, id: u32, out: &mut [u8]) -> usize {
        if self.finished || self.request.as_ref().is_none_or(|r| r.id != id) {
            return 0;
        }
        let n = out.len().min(self.piece).min(self.body.len());
        out[..n].copy_from_slice(&self.body[..n]);
        self.body.drain(..n);
        n
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
    clients: u8,
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
            AccessPointState::Up {
                clients: self.clients,
            }
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

fn phone_lines(runtime: &mut Runtime) -> (Vec<String>, Option<usize>) {
    let mut view =
        crate::screens::workout::View::new(crate::screens::workout::Page::Phone, 0, None);
    runtime.phone_view(&mut view);
    let lines = view
        .lines()
        .iter()
        .map(|line| String::from(line.as_str()))
        .collect();
    (lines, view.qr().map(|code| code.qr.size()))
}

#[test]
fn connect_phone_opens_from_settings_and_moves_to_the_page_code_once_a_phone_joins() {
    use crate::screens::phone::{NAME, NOTE, SECRET, STEP, TITLE_LINE};
    use crate::screens::settings::Row;
    use crate::screens::workout::Page;
    let mut journal = Journal::erased();
    let (mut runtime, mut radio, now) = saved_ride(&mut journal);
    attach(&mut runtime);
    runtime.navigation.page = Page::Settings;
    runtime.navigation.settings_field = Row::Phone.index();
    runtime.input(press(Button::BottomRight), now + 1600, &mut radio);
    assert_eq!(runtime.navigation.page, Page::Phone);
    let (lines, code) = phone_lines(&mut runtime);
    assert_eq!(lines[TITLE_LINE], "STARTING WI-FI");
    assert!(code.is_none());

    let mut phone = Phone {
        runtime: &mut runtime,
        journal: &mut journal,
        transport: Transport::default(),
        radio: Radio2::default(),
        now: now + 1600,
    };
    phone.step();
    phone.step();
    let status = phone.runtime.phone_status().unwrap();
    let (lines, code) = phone_lines(phone.runtime);
    assert_eq!(lines[STEP], "STEP 1 OF 2");
    assert_eq!(lines[TITLE_LINE], "JOIN VANA WI-FI");
    assert_eq!(lines[NAME], status.ssid.text());
    assert_eq!(lines[SECRET].replace(' ', ""), status.password.text());
    assert_eq!(lines[NOTE], "SCAN WITH THE CAMERA");
    assert!(code.is_some());

    // A phone joins: the page code shows by itself, once.
    phone.radio.clients = 1;
    phone.step();
    let (lines, code) = phone_lines(phone.runtime);
    assert_eq!(lines[STEP], "STEP 2 OF 2");
    assert_eq!(lines[NAME], "192.168.4.1");
    assert!(code.is_some());
    // The rider can go back to the join code for a second phone.
    phone
        .runtime
        .input(press(Button::BottomLeft), phone.now + 400, &mut radio);
    let (lines, _) = phone_lines(phone.runtime);
    assert_eq!(lines[STEP], "STEP 1 OF 2");
    assert_eq!(lines[NOTE], "1 JOINED");

    // BACK ends the session and the access point stops.
    phone
        .runtime
        .input(press(Button::TopLeft), phone.now + 800, &mut radio);
    assert_eq!(phone.runtime.navigation.page, Page::Home);
    phone.step();
    phone.step();
    assert!(phone.runtime.phone_status().is_none());
    assert_eq!(phone.radio.stops, 1);
}

#[test]
fn connect_phone_says_why_a_session_ended_and_a_tap_tries_again() {
    use crate::screens::phone::{NOTE, PROBLEM, TITLE_LINE};
    let mut journal = Journal::erased();
    let (mut runtime, _, now) = saved_ride(&mut journal);
    attach(&mut runtime);
    runtime.enter_phone();
    let mut phone = Phone {
        runtime: &mut runtime,
        journal: &mut journal,
        transport: Transport::default(),
        radio: Radio2::default(),
        now,
    };
    phone.step();
    phone.now += crate::app::phone::IDLE_MS + 1;
    phone.step();
    let (lines, code) = phone_lines(phone.runtime);
    assert_eq!(lines[TITLE_LINE], "ENDED AFTER 10 MIN IDLE");
    assert_eq!(lines[NOTE], "TAP TO TRY AGAIN");
    assert!(!lines[PROBLEM].is_empty() && code.is_none());
    phone.runtime.phone_touch();
    phone.step();
    assert!(phone.runtime.phone_status().is_some(), "a new session");
}

/// A phone session with a memory card whose FILES store is formatted, and
/// room for a course.
struct Upload {
    runtime: Runtime,
    journal: Journal,
    transport: Transport,
    radio: Radio2,
    files: crate::workout::library::tests::Region,
    now: u64,
}

impl Upload {
    fn new() -> Self {
        let mut journal = Journal::erased();
        let (mut runtime, _, now) = saved_ride(&mut journal);
        attach(&mut runtime);
        attach_course_room(&mut runtime);
        runtime.open_phone(PhoneMode::Home);
        let mut upload = Self {
            runtime,
            journal,
            transport: Transport::default(),
            radio: Radio2::default(),
            files: crate::workout::library::tests::formatted(),
            now,
        };
        upload.step();
        upload
    }

    fn step(&mut self) {
        self.now += 20;
        self.runtime.serve_phone(
            &mut self.transport,
            &mut self.radio,
            &mut self.journal,
            &mut self.files,
            self.now,
        );
    }

    fn token(&self) -> String {
        String::from(self.runtime.phone_status().unwrap().token.text())
    }

    /// Send `body` in reads of at most `piece` bytes: the status and the
    /// response body.
    fn send(&mut self, method: Method, body: &[u8], piece: usize) -> (u16, String) {
        self.request(method, "/api/course", "", body, piece)
    }

    fn request(
        &mut self,
        method: Method,
        path: &str,
        query: &str,
        body: &[u8],
        piece: usize,
    ) -> (u16, String) {
        let token = self.token();
        self.transport.upload(method, path, &token, body, piece);
        if let Some(request) = self.transport.request.as_mut() {
            request.query.push_str(query).unwrap();
        }
        let mut response = Vec::new();
        for _ in 0..200_000 {
            self.step();
            response.append(&mut self.transport.pending);
            if self.transport.finished {
                let text = String::from_utf8(response).unwrap();
                let status = text[9..12].parse().unwrap();
                let (_, body) = text.split_once("\r\n\r\n").unwrap();
                return (status, String::from(body));
            }
        }
        panic!("the upload never finished");
    }

    fn course(&mut self) -> String {
        self.send(Method::Get, b"", 1).1
    }

    /// Points of the course in memory.
    fn points(&self) -> Vec<crate::ride::course::Point> {
        self.runtime
            .course()
            .map_or(Vec::new(), |course| course.points().to_vec())
    }

    /// Points of the stored course, read as a restart would.
    fn stored(&mut self) -> Vec<crate::ride::course::Point> {
        let mut course = std::boxed::Box::new(crate::ride::course::Course::empty());
        crate::ride::course::load(&mut self.files, &mut course).unwrap();
        course.points().to_vec()
    }

    /// The turns stored beside the stored course, if its own.
    fn stored_turns(&mut self) -> Option<usize> {
        use crate::ride::course::{self, stored};
        let mut course = std::boxed::Box::new(course::Course::empty());
        course::load(&mut self.files, &mut course).unwrap();
        let mut turns = std::vec![crate::app::Turn::default(); crate::app::COURSE_TURNS];
        let found = stored::load(&mut self.files, course::key(&course), &mut turns).unwrap();
        found.map(|found| found.count)
    }
}

fn attach_course_room(runtime: &mut Runtime) {
    use std::boxed::Box;
    let crumbs = Vec::leak(std::vec![crate::app::Crumb::default(); 64]);
    let line = Vec::leak(std::vec![crate::app::Crumb::default(); crate::app::COURSE_POINTS]);
    let turns = Vec::leak(std::vec![crate::app::Turn::default(); crate::app::COURSE_TURNS]);
    let map = Box::leak(Box::new(crate::app::MapFrame::default()));
    let course = Box::leak(Box::new(crate::ride::course::Course::empty()));
    let finder = Box::leak(Box::new(crate::app::CourseFinder::new()));
    let back = crate::app::BackToStart::new(crumbs, line, turns, map, course, finder);
    runtime.attach_back(Box::leak(Box::new(back)));
}

/// A GPX loop of `points` fixes, about 10 m apart, about Santiago.
fn loop_gpx(name: &str, points: usize) -> String {
    let mut gpx = std::format!(
        "<?xml version=\"1.0\"?>\n<gpx version=\"1.1\"><trk><name>{name}</name><trkseg>\n"
    );
    for i in 0..points {
        let turn = i as f64 / points as f64 * core::f64::consts::TAU;
        let (lat, lon) = (-33.45 + 0.004 * turn.sin(), -70.66 + 0.005 * turn.cos());
        gpx.push_str(&std::format!(
            "<trkpt lat=\"{lat:.7}\" lon=\"{lon:.7}\"><ele>530</ele></trkpt>\n"
        ));
    }
    gpx.push_str("</trkseg></trk></gpx>\n");
    gpx
}

#[test]
fn a_phone_loads_a_course_that_is_kept_on_the_card() {
    let mut phone = Upload::new();
    assert!(phone.course().contains(r#""loaded":false"#));
    let gpx = loop_gpx("Cerro loop", 300);
    let (status, body) = phone.send(Method::Put, gpx.as_bytes(), 97);
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""name":"CERRO LOOP""#), "{body}");
    assert!(body.contains(r#""points":300"#), "{body}");
    let points = phone.points();
    assert_eq!(points.len(), 300);
    assert_eq!(phone.stored(), points, "the card keeps what memory holds");
    assert_eq!(
        phone.stored_turns(),
        Some(0),
        "its turns beside it: none on a loop"
    );
    let listed = phone.course();
    assert!(
        listed.contains(r#""store":"kept","loaded":true"#),
        "{listed}"
    );

    // After a restart the course is read back from the card.
    let mut runtime = Runtime::new(crate::sensors::ble_profile::Profile::HeartRate);
    attach_course_room(&mut runtime);
    runtime.service_course(&mut phone.files, 0);
    let restarted = runtime.course().unwrap().points().to_vec();
    assert_eq!(restarted, points);
}

#[test]
fn a_file_that_is_not_a_course_leaves_the_stored_one() {
    let mut phone = Upload::new();
    let gpx = loop_gpx("First", 50);
    assert_eq!(phone.send(Method::Put, gpx.as_bytes(), 512).0, 200);
    let first = phone.stored();
    let off_earth = gpx.replacen("lat=\"-33.", "lat=\"-93.", 1);
    let bad = [
        (off_earth.as_str(), 422, "coordinates"),
        ("<gpx><wpt lat=\"1\" lon=\"1\"/></gpx>", 422, "no_route"),
        ("", 400, "empty"),
    ];
    for (body, want, error) in bad {
        let (status, reply) = phone.send(Method::Put, body.as_bytes(), 64);
        assert_eq!(status, want, "{reply}");
        assert!(reply.contains(error), "{reply}");
        // The course in memory is read back from the card.
        phone.runtime.service_course(&mut phone.files, phone.now);
        assert_eq!(phone.points(), first);
        assert_eq!(phone.stored(), first);
    }
    // An upload cut off by the phone leaves the stored course too.
    let token = phone.token();
    let second = loop_gpx("Second", 400);
    phone
        .transport
        .upload(Method::Put, "/api/course", &token, second.as_bytes(), 300);
    for _ in 0..5 {
        phone.step();
    }
    phone.transport.ask(Method::Get, "/api/status", "", &token);
    for _ in 0..20 {
        phone.step();
    }
    phone.runtime.service_course(&mut phone.files, phone.now);
    assert_eq!(phone.points(), first);
}

#[test]
fn the_phone_clears_the_course_from_memory_and_the_card() {
    let mut phone = Upload::new();
    let gpx = loop_gpx("Gone", 40);
    assert_eq!(phone.send(Method::Put, gpx.as_bytes(), 200).0, 200);
    assert_eq!(phone.stored_turns(), Some(0));
    assert_eq!(phone.send(Method::Delete, b"", 1).0, 204);
    assert!(phone.points().is_empty());
    assert!(phone.stored().is_empty());
    assert_eq!(phone.stored_turns(), None);
    assert!(phone.course().contains(r#""loaded":false"#));
    // Clearing again is no error.
    assert_eq!(phone.send(Method::Delete, b"", 1).0, 204);
}

#[test]
fn without_a_formatted_card_no_course_is_kept() {
    let mut phone = Upload::new();
    phone.files = crate::workout::library::tests::Region::default();
    let gpx = loop_gpx("Nowhere", 40);
    let (status, body) = phone.send(Method::Put, gpx.as_bytes(), 512);
    assert_eq!(status, 503, "{body}");
    assert!(body.contains("unavailable"), "{body}");
    phone.runtime.service_course(&mut phone.files, phone.now);
    assert!(phone.points().is_empty());
}

/// Hostile uploads: damaged GPX and random bytes, read in pieces of any
/// size. Nothing panics, every answer is a status the page knows, and memory
/// then holds exactly the stored course.
#[test]
fn hostile_uploads_never_panic_and_memory_matches_the_card() {
    use crate::testing::Rng;
    let mut phone = Upload::new();
    let good = loop_gpx("Fuzz", 120).into_bytes();
    let (mut kept, mut refused) = (0, 0);
    for seed in 0..300 {
        let mut rng = Rng::new(seed);
        let mut body = match rng.below(3) {
            0 => rng.bytes(3_000),
            _ => {
                let mut bytes = good.clone();
                for _ in 0..rng.range(1, 4) {
                    bytes = rng.mutate(bytes);
                }
                bytes
            }
        };
        if body.is_empty() {
            body.push(b'<');
        }
        let piece = rng.range(1, 700) as usize;
        let (status, reply) = phone.send(Method::Put, &body, piece);
        match status {
            200 => kept += 1,
            422 => refused += 1,
            other => panic!("seed {seed}: {other} {reply}"),
        }
        phone.runtime.service_course(&mut phone.files, phone.now);
        assert_eq!(phone.points(), phone.stored(), "seed {seed}");
    }
    assert!(kept > 20 && refused > 50, "{kept} kept, {refused} refused");
}

/// A slow upload keeps the session open: body progress counts as activity,
/// even when the whole upload takes longer than the idle timeout.
#[test]
fn a_slow_upload_is_not_an_idle_session() {
    let mut phone = Upload::new();
    let gpx = loop_gpx("Slow", 300);
    let token = phone.token();
    phone
        .transport
        .upload(Method::Put, "/api/course", &token, gpx.as_bytes(), 64);
    let mut response = Vec::new();
    let mut minutes = 0;
    while !phone.transport.finished {
        // A minute between service steps: about 512 bytes a minute.
        phone.now += 60_000;
        minutes += 1;
        phone.step();
        response.append(&mut phone.transport.pending);
        assert!(
            phone.runtime.phone_status().is_some(),
            "closed after {minutes} min"
        );
    }
    assert!(minutes > 15, "{minutes} min");
    assert!(
        String::from_utf8(response)
            .unwrap()
            .starts_with("HTTP/1.1 200")
    );
}

fn workout_bytes(name: &str, minutes: u16) -> Vec<u8> {
    use crate::workout::model::{Intensity, Length, Name, Step, Target, Workout};
    let steps = [
        Step::new(
            Intensity::Warmup,
            Length::Time(minutes * 60),
            Target::Ftp { low: 50, high: 65 },
        ),
        Step::new(
            Intensity::Work,
            Length::Time(minutes * 60),
            Target::Ftp { low: 88, high: 94 },
        ),
    ];
    let mut workout = Workout::new("", &steps, &[], None);
    workout.name = Name::new(name);
    crate::workout::codec::encode(&workout).to_vec()
}

impl Upload {
    fn put_workout(&mut self, bytes: &[u8], piece: usize) -> (u16, String) {
        self.request(Method::Put, "/api/workouts", "", bytes, piece)
    }

    /// The files under `workouts/`, each of which must decode.
    fn stored_workouts(&mut self) -> Vec<String> {
        use crate::workout::library::FOLDER;
        let mut sector = [0; 512];
        let mut store = firmware_services::volume::open(
            &mut self.files,
            &firmware_services::volume::FILES,
            &mut sector,
        )
        .unwrap();
        let dir = firmware_services::files::mount(&mut store, &mut sector).unwrap();
        let mut entries = Vec::new();
        dir.list(&mut store, &mut sector, |entry| {
            if entry.name.as_str().starts_with(FOLDER) {
                entries.push(*entry);
            }
        })
        .unwrap();
        let mut names = Vec::new();
        for entry in entries {
            let mut bytes = std::vec![0; entry.length as usize];
            dir.read(&mut store, &entry, 0, &mut bytes, &mut sector)
                .unwrap();
            assert_eq!(
                firmware_services::checksum::crc32(&bytes),
                entry.crc,
                "{}",
                entry.name.as_str()
            );
            crate::workout::codec::decode(&bytes)
                .unwrap_or_else(|e| panic!("{} holds {e:?}", entry.name.as_str()));
            names.push(String::from(entry.name.as_str()));
        }
        names
    }
}

#[test]
fn a_phone_uploads_lists_replaces_and_deletes_a_workout() {
    let mut phone = Upload::new();
    let (status, body) = phone.request(Method::Get, "/api/workouts", "", b"", 1);
    assert_eq!(
        (status, body.contains(r#""uploads":[]"#)),
        (200, true),
        "{body}"
    );
    let bytes = workout_bytes("Over \"Unders\"", 10);
    let (status, body) = phone.put_workout(&bytes, 37);
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""name":"OVER \"UNDERS\"","minutes":20,"steps":2"#),
        "{body}"
    );
    assert!(body.contains(r#""max":8"#), "{body}");
    let stored = phone.stored_workouts();
    assert_eq!(stored.len(), 1);
    let file = stored[0].strip_prefix("workouts/").unwrap();
    assert!(
        file.starts_with("over-unders-") && file.ends_with(".vw"),
        "{file}"
    );
    // The picker lists it, and the chosen workout stays chosen.
    assert_eq!(phone.runtime.training.library.uploads().len(), 1);
    // The same name replaces the file; another name is another file.
    let longer = workout_bytes("Over \"Unders\"", 15);
    assert_eq!(phone.put_workout(&longer, 512).0, 200);
    assert_eq!(phone.stored_workouts(), stored);
    assert!(
        phone
            .put_workout(&workout_bytes("Tempo", 20), 512)
            .1
            .contains("TEMPO")
    );
    assert_eq!(phone.stored_workouts().len(), 2);
    let query = std::format!("file={file}");
    let (status, _) = phone.request(Method::Delete, "/api/workouts", &query, b"", 1);
    assert_eq!(status, 204);
    assert_eq!(phone.stored_workouts().len(), 1);
    let (status, _) = phone.request(Method::Delete, "/api/workouts", &query, b"", 1);
    assert_eq!(status, 404, "gone already");
    for odd in [
        "file=../course.dat",
        "file=course.dat",
        "file=",
        "",
        "file=%00",
    ] {
        let (status, _) = phone.request(Method::Delete, "/api/workouts", odd, b"", 1);
        assert_eq!(status, 404, "{odd}");
    }
    assert_eq!(phone.stored_workouts().len(), 1);
}

/// The list of eight workouts whose names need the most escaping still fits
/// its response and ends whole.
#[test]
fn the_list_of_eight_worst_case_names_is_whole() {
    let mut phone = Upload::new();
    for index in 0..8 {
        let name = std::format!("{index}{}", "\"".repeat(19));
        assert_eq!(phone.put_workout(&workout_bytes(&name, 10), 512).0, 200);
    }
    let (status, body) = phone.request(Method::Get, "/api/workouts", "", b"", 1);
    assert_eq!(status, 200);
    assert!(body.ends_with("]}"), "{body}");
    assert_eq!(body.matches(r#""file":"#).count(), 8, "{body}");
}

/// A file the host left that the device cannot read takes one of the eight
/// places; the page lists it so it can be deleted, and nothing else can.
#[test]
fn an_unreadable_file_is_listed_and_deleted_from_the_phone() {
    let mut phone = Upload::new();
    for index in 0..7 {
        let (status, _) = phone.put_workout(&workout_bytes(&std::format!("Set {index}"), 10), 512);
        assert_eq!(status, 200);
    }
    crate::workout::library::tests::put(
        &mut phone.files,
        "workouts/zz-broken.vw",
        b"not a workout",
    );
    let (_, body) = phone.request(Method::Get, "/api/workouts", "", b"", 1);
    assert!(
        body.contains(r#""unlisted":[{"file":"zz-broken.vw"}]"#),
        "{body}"
    );
    assert_eq!(body.matches(r#""name":"#).count(), 7, "{body}");
    let (status, body) = phone.put_workout(&workout_bytes("Set 7", 10), 512);
    assert_eq!((status, body.as_str()), (409, r#"{"error":"full"}"#));
    for odd in [
        "file=zz-broken.vw.bak",
        "file=../course.dat",
        "file=course.dat",
    ] {
        assert_eq!(
            phone
                .request(Method::Delete, "/api/workouts", odd, b"", 1)
                .0,
            404
        );
    }
    let (status, _) = phone.request(Method::Delete, "/api/workouts", "file=zz-broken.vw", b"", 1);
    assert_eq!(status, 204);
    assert_eq!(phone.stored_workouts().len(), 7);
    assert_eq!(phone.put_workout(&workout_bytes("Set 7", 10), 512).0, 200);
    let (_, body) = phone.request(Method::Get, "/api/workouts", "", b"", 1);
    assert!(body.contains(r#""unlisted":[]"#), "{body}");
}

#[test]
fn a_ninth_workout_is_refused_and_a_stored_one_can_be_replaced() {
    use crate::workout::library::UPLOADS;
    let mut phone = Upload::new();
    for index in 0..UPLOADS {
        let (status, body) =
            phone.put_workout(&workout_bytes(&std::format!("Set {index}"), 10), 512);
        assert_eq!(status, 200, "{body}");
    }
    let before = phone.stored_workouts();
    let (status, body) = phone.put_workout(&workout_bytes("Set 8", 10), 512);
    assert_eq!((status, body.as_str()), (409, r#"{"error":"full"}"#));
    assert_eq!(phone.stored_workouts(), before);
    assert_eq!(phone.put_workout(&workout_bytes("Set 3", 30), 512).0, 200);
    assert_eq!(phone.stored_workouts().len(), UPLOADS);
    // Room made by a delete is room again.
    let file = before[0].strip_prefix("workouts/").unwrap();
    let query = std::format!("file={file}");
    assert_eq!(
        phone
            .request(Method::Delete, "/api/workouts", &query, b"", 1)
            .0,
        204
    );
    assert_eq!(phone.put_workout(&workout_bytes("Set 8", 10), 512).0, 200);
}

#[test]
fn a_workout_the_card_cannot_keep_says_why_and_stores_nothing() {
    let mut phone = Upload::new();
    let good = workout_bytes("Good", 10);
    assert_eq!(phone.put_workout(&good, 512).0, 200);
    let kept = phone.stored_workouts();
    let mut wrong_crc = good.clone();
    wrong_crc[7] ^= 1;
    let mut text = good.clone();
    text[0] = b'X';
    let cases: [(&[u8], u16, &str); 5] = [
        (b"", 400, "empty"),
        (b"{\"name\":\"JSON is not the file\"}", 422, "format"),
        (&wrong_crc, 422, "damaged"),
        (&text, 422, "format"),
        (&good[..good.len() - 1], 422, "damaged"),
    ];
    for (body, want, error) in cases {
        let (status, reply) = phone.put_workout(body, 100);
        assert_eq!(status, want, "{reply}");
        assert!(reply.contains(error), "{reply}");
        assert_eq!(phone.stored_workouts(), kept);
    }
    // More than any workout holds is read to its end and refused.
    let (status, reply) = phone.put_workout(&std::vec![7; 3000], 700);
    assert_eq!(
        (status, reply.contains("too_large")),
        (413, true),
        "{reply}"
    );
    // A blank name is no name.
    let (status, _) = phone.put_workout(&workout_bytes("   ", 10), 512);
    assert_eq!(status, 422);
    assert_eq!(phone.stored_workouts(), kept);
    phone.files = crate::workout::library::tests::Region::default();
    let (status, reply) = phone.put_workout(&good, 512);
    assert_eq!(
        (status, reply.contains("unavailable")),
        (503, true),
        "{reply}"
    );
}

/// Hostile uploads: damaged workouts (half with their CRC made right again,
/// so the checks behind it run) and random bytes, in pieces of any size.
/// Nothing panics, every answer is a status the page knows, and every file
/// under `workouts/` is a whole valid workout, at most eight.
#[test]
fn hostile_workout_uploads_never_panic_and_leave_only_valid_files() {
    use crate::testing::Rng;
    use firmware_services::checksum::crc32;
    let mut phone = Upload::new();
    let good = workout_bytes("Fuzz", 12);
    let (mut kept, mut refused) = (0, 0);
    for seed in 0..400 {
        let mut rng = Rng::new(seed);
        let mut body = match rng.below(4) {
            0 => rng.bytes(700),
            _ => {
                let mut bytes = good.clone();
                for _ in 0..rng.range(1, 4) {
                    bytes = rng.mutate(bytes);
                }
                if rng.below(2) == 0 && bytes.len() > 8 {
                    let end = bytes.len() - 4;
                    let crc = crc32(&bytes[..end]).to_le_bytes();
                    bytes[end..].copy_from_slice(&crc);
                }
                bytes
            }
        };
        if body.is_empty() {
            body.push(b'V');
        }
        let piece = rng.range(1, 600) as usize;
        let (status, reply) = phone.put_workout(&body, piece);
        match status {
            200 => kept += 1,
            400 | 409 | 413 | 422 => refused += 1,
            other => panic!("seed {seed}: {other} {reply}"),
        }
        assert!(phone.stored_workouts().len() <= 8, "seed {seed}");
        assert_eq!(
            phone.runtime.training.library.uploads().len(),
            phone.stored_workouts().len().min(8),
            "seed {seed}"
        );
    }
    assert!(kept >= 1 && refused > 200, "{kept} kept, {refused} refused");
}
