//! Phone companion: a session that lets one phone browse and download rides,
//! and load a course to follow, over the device's own Wi-Fi access point (or
//! the home network), with no app to install.
//!
//! A session has a random WPA2 password and a random token. The phone joins
//! the access point (QR code 1), opens `http://192.168.4.1/?t=<token>` (QR
//! code 2) and the page keeps the token as a cookie. Every `/api` request
//! must carry it. The session ends when the rider leaves the screen, after
//! `IDLE_MS` without requests, or when a ride starts; then the access point
//! stops and the station returns to the home network.
//!
//! The device serves the pre-gzipped web client, small JSON and FIT files
//! streamed from the ride journal through `ride::fit`, two passes per file so
//! `Content-Length` is exact. It takes two uploads. A GPX course, `PUT
//! /api/course`, is read as it streams in through `ride::course::gpx` into
//! the course beside the breadcrumb and then stored on FILES. A workout, `PUT
//! /api/workouts`, is one codec file (`workout::codec`): at most
//! `ENCODED_MAX` bytes gathered in `Work`, then decoded, validated and stored
//! under `workouts/` by `Library::store`, the library the picker lists. `GET`
//! lists the uploads and `DELETE ?file=` removes one. Each service step
//! does a bounded amount of media reads, reads at most `UPLOAD_READS` pieces
//! of a body and never writes more output than the transport has room for.

use super::Runtime;
use super::recording::Medium;
use crate::ride::course::{self, gpx};
use crate::ride::fit::{self, Export, Flow, Plan, Refusal};
use crate::ride::index::{Activity, Index};
use crate::ride::log::Slot;
use crate::ride::rider::Rider;
use crate::ride::storage::RideStorage;
use crate::workout::codec::{DecodeError, ENCODED_MAX};
use crate::workout::library::{self, Library, LoadError, StoreError};
use core::fmt::Write;
use device_api::bulk::ReadWrite;
use device_api::connectivity::Text;
use device_api::server::{
    AccessPoint, AccessPointConfig, AccessPointState, Method, Request, Server,
};
use firmware_services::files;
use firmware_services::fit::{Counter, Sink};
use firmware_services::http::{Head, query_value};

/// A session without requests ends after this long.
pub const IDLE_MS: u64 = 10 * 60 * 1000;
/// Slots read per service step while indexing or sizing a file.
const READS_PER_STEP: usize = 48;
/// Pieces of an upload's body read per service step.
const UPLOAD_READS: usize = 8;
const UPLOAD_PIECE: usize = 512;
/// Room for a response: a head, and the workouts list of 8 whose names
/// need the most escapes.
const TEXT: usize = 1280;
/// Room kept for a response's head before its body is written.
const HEAD_GAP: usize = 160;
/// The web client, built by `apps/vana/web/build.py`.
const INDEX_HTML_GZ: &[u8] = include_bytes!("../../web/dist/index.html.gz");
/// Readable characters for passwords, upper case as the screen shows
/// them: no 0/O or 1/I.
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// Where the phone reaches the device.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The device's own access point, for rides away from home.
    AccessPoint,
    /// The home network the station is on.
    Home,
}

impl Mode {
    pub const fn name(self) -> &'static str {
        match self {
            Self::AccessPoint => "access_point",
            Self::Home => "home",
        }
    }
}

/// What the Connect phone screen and the terminal show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Status {
    pub mode: Mode,
    pub ssid: Text<32>,
    pub password: Text<63>,
    pub token: Text<32>,
    /// The address to open, once the network is up.
    pub address: Option<[u8; 4]>,
    pub access_point: AccessPointState,
    pub requests: u32,
    pub downloads: u32,
}

impl Status {
    /// `WIFI:` join text for the camera QR code.
    pub fn join_text(&self) -> heapless::String<128> {
        let mut text = heapless::String::new();
        let _ = write!(
            text,
            "WIFI:T:WPA;S:{};P:{};;",
            self.ssid.text(),
            self.password.text()
        );
        text
    }

    /// The page to open, once the address is known.
    pub fn url(&self) -> Option<heapless::String<64>> {
        let [a, b, c, d] = self.address?;
        let mut text = heapless::String::new();
        let _ = write!(text, "http://{a}.{b}.{c}.{d}/?t={}", self.token.text());
        Some(text)
    }
}

/// Why a session closed, for the screen and terminal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Closed {
    Rider,
    Idle,
    Ride,
    /// The access point could not start.
    Radio,
    /// Home mode without a home network.
    Offline,
}

impl Closed {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Rider => "rider",
            Self::Idle => "idle",
            Self::Ride => "ride",
            Self::Radio => "radio",
            Self::Offline => "offline",
        }
    }
}

struct Session {
    status: Status,
    last_request_ms: u64,
    /// The access point start was accepted.
    started: bool,
}

#[derive(Clone)]
enum FitPhase {
    Sizing { export: Export, counter: Counter },
    Writing { export: Export },
}

#[derive(Clone)]
struct FitJob {
    medium: Medium,
    ride_id: u32,
    /// The body the calories use, read once so both passes agree.
    rider: Rider,
    next_slot: usize,
    start_slot: usize,
    start_utc_ms: Option<u64>,
    phase: FitPhase,
    name: heapless::String<48>,
}

// One instance lives in the runtime; boxing would need an allocator.
#[allow(clippy::large_enum_variant)]
enum Job {
    Idle,
    /// Bytes of `Phone::text` from `at`.
    Text {
        id: u32,
        at: usize,
    },
    Static {
        id: u32,
        data: &'static [u8],
        at: usize,
    },
    /// The activity list: the head is sent, then rides newest first.
    Activities {
        id: u32,
        /// Rides of each medium sent so far.
        sent: [usize; 2],
        first: bool,
    },
    Fit {
        id: u32,
        job: FitJob,
    },
    /// A course streaming in: `received` of the request's body bytes.
    Course {
        id: u32,
        received: u32,
        gpx: gpx::Gpx,
    },
    /// A workout gathering in `Work::workout`: `received` of the request's
    /// body bytes, which are kept only up to `ENCODED_MAX`.
    Workout {
        id: u32,
        received: u32,
    },
}

#[derive(Default)]
pub(super) struct Phone {
    session: Option<Session>,
    /// A session to open (`Some(mode)`) or close (`None`), from the screen
    /// or terminal, applied on the next service step.
    wanted: Option<Option<Mode>>,
    closed: Option<Closed>,
    /// The access point stop is still to be requested.
    stopping: bool,
    last_id: u32,
    /// Ride indexes and the response in progress, kept by the device
    /// outside the runtime (see [`Runtime::attach_phone`]).
    work: Option<&'static mut Work>,
}

impl Phone {
    fn w(&mut self) -> &mut Work {
        self.work.as_deref_mut().expect("phone work is attached")
    }
}

/// The phone session's larger state: a few KB the device keeps where it
/// costs no task stack, such as external RAM.
pub struct Work {
    indexes: [Index; 2],
    job: Job,
    /// A response, and while a workout uploads, its bytes so far: the
    /// response is queued only once they are used. Every byte here is copied
    /// through the boot stack (`PHONE_WORK`), so it is kept small.
    text: heapless::Vec<u8, TEXT>,
}

impl Work {
    pub const fn new() -> Self {
        Self {
            indexes: [Index::new(), Index::new()],
            job: Job::Idle,
            text: heapless::Vec::new(),
        }
    }
}

impl Default for Work {
    fn default() -> Self {
        Self::new()
    }
}

const MEDIA: [Medium; 2] = [Medium::Internal, Medium::Bulk];

fn medium_letter(medium: Medium) -> char {
    match medium {
        Medium::Internal => 'i',
        Medium::Bulk => 'b',
    }
}

/// `i12-3` is internal ride 3 starting at slot 12.
fn parse_id(id: &str) -> Option<(Medium, usize, u32)> {
    let medium = match id.as_bytes().first()? {
        b'i' => Medium::Internal,
        b'b' => Medium::Bulk,
        _ => return None,
    };
    let (slot, ride) = id[1..].split_once('-')?;
    Some((medium, slot.parse().ok()?, ride.parse().ok()?))
}

fn file_name(ride_id: u32, start_utc_ms: Option<u64>) -> heapless::String<48> {
    let mut name = heapless::String::new();
    match start_utc_ms {
        Some(ms) => {
            let seconds = (ms / 1000) as i64;
            let (year, month, day) = crate::clock::civil(seconds.div_euclid(86_400));
            let minute = seconds.rem_euclid(86_400) / 60;
            let _ = write!(
                name,
                "vana-{year:04}-{month:02}-{day:02}-{:02}{:02}-ride{ride_id}.fit",
                minute / 60,
                minute % 60
            );
        }
        None => {
            let _ = write!(name, "vana-ride{ride_id}.fit");
        }
    }
    name
}

/// Writes straight into the transport, which the caller checked has room.
/// `body` as a complete JSON response in `text`.
fn queue(text: &mut heapless::Vec<u8, TEXT>, status: u16, body: &str) {
    text.clear();
    let head = Head::new(status)
        .header("Content-Type", "application/json")
        .header("Cache-Control", "no-store")
        .length(body.len())
        .finish();
    if let Some(head) = head {
        let _ = text.extend_from_slice(head.bytes());
        let _ = text.extend_from_slice(body.as_bytes());
    }
}

struct Out<'a, S: Server> {
    server: &'a mut S,
    id: u32,
    short: bool,
}

impl<S: Server> Sink for Out<'_, S> {
    fn put(&mut self, bytes: &[u8]) {
        if self.server.write(self.id, bytes) != bytes.len() {
            self.short = true;
        }
    }
}

/// `length` symbols of `alphabet` from hardware entropy, one byte each. The
/// modulo bias of 256 over 31 or 16 symbols costs well under a bit.
fn random_text<const N: usize>(
    ap: &mut impl AccessPoint,
    length: usize,
    alphabet: &[u8],
) -> Text<N> {
    let mut bytes = [0u8; 32];
    let mut word = 0u32;
    for (index, byte) in bytes[..length].iter_mut().enumerate() {
        if index % 4 == 0 {
            word = ap.entropy();
        }
        let value = (word >> ((index % 4) * 8)) as u8;
        *byte = alphabet[usize::from(value) % alphabet.len()];
    }
    Text::new(&bytes[..length]).unwrap_or(Text::empty())
}

impl Runtime {
    /// Give the phone session its working memory; without it no session
    /// opens.
    pub fn attach_phone(&mut self, work: &'static mut Work) {
        self.phone.work = Some(work);
    }

    /// Open a phone session on the next service step; the screen and
    /// terminal call this.
    pub fn open_phone(&mut self, mode: Mode) {
        self.phone.wanted = Some(Some(mode));
    }

    /// Close the phone session on the next service step.
    pub fn close_phone(&mut self) {
        self.phone.wanted = Some(None);
    }

    pub fn phone_status(&self) -> Option<Status> {
        self.phone.session.as_ref().map(|s| s.status)
    }

    /// Why the last session closed, until the next opens.
    pub fn phone_closed(&self) -> Option<Closed> {
        self.phone.closed
    }

    fn end_session(&mut self, why: Closed) {
        if let Some(session) = self.phone.session.take() {
            self.phone.stopping = session.started;
            self.phone.closed = Some(why);
            self.phone.w().job = Job::Idle;
            log::info!(target: "phone", "VANA_PHONE closed reason={}", why.name());
        }
    }

    fn begin_session(&mut self, mode: Mode, ap: &mut impl AccessPoint, now: u64) {
        if self.phone.session.is_some() || self.phone.work.is_none() {
            return;
        }
        if self.recording() {
            self.phone.closed = Some(Closed::Ride);
            return;
        }
        let mut ssid = heapless::String::<32>::new();
        let suffix: Text<4> = random_text(ap, 4, b"0123456789ABCDEF");
        let _ = write!(ssid, "VANA-{}", suffix.text());
        let status = Status {
            mode,
            ssid: Text::new(ssid.as_bytes()).unwrap_or(Text::empty()),
            password: random_text(ap, 12, ALPHABET),
            token: random_text(ap, 16, b"0123456789abcdef"),
            address: None,
            access_point: AccessPointState::Off,
            requests: 0,
            downloads: 0,
        };
        for index in &mut self.phone.w().indexes {
            index.reset();
        }
        self.phone.closed = None;
        self.phone.session = Some(Session {
            status,
            last_request_ms: now,
            started: false,
        });
        log::info!(target: "phone", "VANA_PHONE open mode={}", mode.name());
    }

    /// Run the phone session: open or close it as asked, keep its network,
    /// index rides and answer the transport's requests in bounded steps.
    pub fn serve_phone(
        &mut self,
        server: &mut impl Server,
        ap: &mut impl AccessPoint,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) {
        match self.phone.wanted.take() {
            Some(Some(mode)) => self.begin_session(mode, ap, now),
            Some(None) => self.end_session(Closed::Rider),
            None => {}
        }
        if let Some(session) = &self.phone.session {
            let idle = now.saturating_sub(session.last_request_ms) > IDLE_MS;
            if self.recording() {
                self.end_session(Closed::Ride);
            } else if idle {
                self.end_session(Closed::Idle);
            }
        }
        let Some(session) = self.phone.session.as_mut() else {
            server.listen(false);
            if self.phone.stopping && ap.stop().is_ok() {
                self.phone.stopping = false;
            }
            return;
        };
        let state = ap.state();
        session.status.access_point = state;
        let reach = ap.reach();
        match session.status.mode {
            Mode::AccessPoint => {
                if !session.started
                    && ap
                        .start(AccessPointConfig {
                            ssid: session.status.ssid,
                            password: session.status.password,
                        })
                        .is_ok()
                {
                    session.started = true;
                }
                session.status.address = reach.access_point;
                if session.started && state == AccessPointState::Failed {
                    self.end_session(Closed::Radio);
                    return;
                }
            }
            Mode::Home => {
                session.status.address = reach.station;
                if reach.station.is_none() {
                    self.end_session(Closed::Offline);
                    return;
                }
            }
        }
        server.listen(true);
        self.index_step(store, bulk);
        self.answer(server, store, bulk, now);
    }

    fn index_step(&mut self, store: &mut impl RideStorage, bulk: &mut impl ReadWrite) {
        let mut budget = READS_PER_STEP;
        for (index, medium) in MEDIA.into_iter().enumerate() {
            let limit = match self.indexable(medium) {
                Readiness::Ready(limit) => limit,
                // Scanning or busy: look again on a later step.
                Readiness::Pending => continue,
                Readiness::Never => {
                    self.phone.w().indexes[index].finish();
                    continue;
                }
            };
            let mut slot = Slot::default();
            while budget > 0 && !self.phone.w().indexes[index].done() {
                let next = self.phone.w().indexes[index].next_slot();
                if next >= limit {
                    self.phone.w().indexes[index].finish();
                    break;
                }
                budget -= 1;
                match self.ride.read_slot(medium, store, bulk, next, &mut slot) {
                    Some(true) => self.phone.w().indexes[index].accept(Some(&slot)),
                    Some(false) => self.phone.w().indexes[index].accept(None),
                    // The medium failed: list what was read.
                    None => self.phone.w().indexes[index].finish(),
                }
            }
        }
    }

    /// Whether a medium's rides can be read now, up to which slot.
    fn indexable(&self, medium: Medium) -> Readiness {
        use crate::ride::log::Status;
        let recorder = self.ride.recorder(medium);
        let known = medium == Medium::Internal || self.ride.bulk_media.log().is_some();
        if known && recorder.exportable() {
            return Readiness::Ready(recorder.next_slot());
        }
        match recorder.status() {
            Status::Unsupported
            | Status::Unavailable
            | Status::Error
            | Status::Corrupt
            | Status::NeedsInit => Readiness::Never,
            _ => Readiness::Pending,
        }
    }

    fn indexing(&self) -> bool {
        self.phone
            .work
            .as_deref()
            .is_some_and(|work| work.indexes.iter().any(|index| !index.done()))
    }

    fn answer(
        &mut self,
        server: &mut impl Server,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) {
        let current = server.request();
        let job_id = match &self.phone.w().job {
            Job::Idle => None,
            Job::Text { id, .. }
            | Job::Static { id, .. }
            | Job::Activities { id, .. }
            | Job::Fit { id, .. }
            | Job::Course { id, .. }
            | Job::Workout { id, .. } => Some(*id),
        };
        if let Some(id) = job_id
            && current.as_ref().is_none_or(|r| r.id != id)
        {
            // The peer went away: drop its response.
            self.phone.w().job = Job::Idle;
        }
        if matches!(self.phone.w().job, Job::Idle)
            && let Some(request) = current
            && request.id != self.phone.last_id
        {
            self.phone.last_id = request.id;
            if let Some(session) = self.phone.session.as_mut() {
                session.last_request_ms = now;
                session.status.requests += 1;
            }
            self.route(&request, bulk);
        }
        self.progress(server, store, bulk, now);
    }

    fn authorized(&self, request: &Request) -> bool {
        self.phone.session.as_ref().is_some_and(|session| {
            let token = session.status.token.bytes();
            let given = request.session.as_bytes();
            // Compare every byte, so timing does not reveal a prefix.
            token.len() == given.len()
                && token
                    .iter()
                    .zip(given)
                    .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                    == 0
        })
    }

    /// Queue a small complete response from `Phone::text`.
    fn reply(&mut self, id: u32, status: u16, json: Option<&str>) {
        let work = self.phone.w();
        queue(&mut work.text, status, json.unwrap_or(""));
        work.job = Job::Text { id, at: 0 };
    }

    fn route(&mut self, request: &Request, bulk: &mut impl ReadWrite) {
        let id = request.id;
        let path = request.path.as_str();
        match (request.method, path) {
            (Method::Get | Method::Head, "/" | "/index.html") => {
                self.phone.w().text.clear();
                let fresh = query_value(&request.query, "t").is_some() && self.authorized(request);
                let mut head = Head::new(200)
                    .header("Content-Type", "text/html; charset=utf-8")
                    .header("Content-Encoding", "gzip")
                    .header("Cache-Control", "no-store")
                    .header("X-Content-Type-Options", "nosniff")
                    .length(if request.method == Method::Head {
                        0
                    } else {
                        INDEX_HTML_GZ.len()
                    });
                if fresh && let Some(session) = &self.phone.session {
                    let mut cookie = heapless::String::<64>::new();
                    let _ = write!(
                        cookie,
                        "vana={}; Path=/; SameSite=Strict; HttpOnly",
                        session.status.token.text()
                    );
                    head = head.header("Set-Cookie", cookie.as_str());
                }
                if let Some(head) = head.finish() {
                    let _ = self.phone.w().text.extend_from_slice(head.bytes());
                }
                // A static body follows the head kept in `text`.
                self.phone.w().job = if request.method == Method::Head {
                    Job::Text { id, at: 0 }
                } else {
                    Job::Static {
                        id,
                        data: INDEX_HTML_GZ,
                        at: 0,
                    }
                };
            }
            (Method::Get, "/favicon.ico") => self.reply(id, 404, None),
            _ if path.starts_with("/api/") && !self.authorized(request) => {
                self.reply(id, 401, Some(r#"{"error":"session"}"#));
            }
            (Method::Get, "/api/status") => self.status_reply(id),
            (Method::Get, "/api/activities") => {
                self.phone.w().job = Job::Activities {
                    id,
                    sent: [0; 2],
                    first: true,
                };
                self.activities_head(id);
            }
            (Method::Get, "/api/course") => self.course_reply(id),
            (Method::Put | Method::Post, "/api/course") => self.start_course(request),
            (Method::Delete, "/api/course") => self.clear_course(id, bulk),
            (Method::Get, "/api/workouts") => self.workouts_reply(id, bulk),
            (Method::Put | Method::Post, "/api/workouts") => self.start_workout(request),
            (Method::Delete, "/api/workouts") => self.delete_workout(request, bulk),
            (Method::Post, "/api/close") => {
                self.reply(id, 204, None);
                self.phone.wanted = Some(None);
            }
            (method, path) if path.starts_with("/api/activities/") => {
                let name = &path["/api/activities/".len()..];
                let fit = name.strip_suffix(".fit");
                match (method, parse_id(fit.unwrap_or(name))) {
                    (_, None) => self.reply(id, 404, Some(r#"{"error":"activity"}"#)),
                    (Method::Get, Some(target)) if fit.is_some() => {
                        let start = query_value(&request.query, "start")
                            .and_then(|v| v.parse::<u64>().ok())
                            .and_then(|s| s.checked_mul(1000));
                        self.start_fit(id, target, start);
                    }
                    (Method::Delete, Some(_)) => self.reply(
                        id,
                        405,
                        Some(
                            r#"{"error":"append_only","detail":"rides are kept until an export is verified and the whole journal is cleared"}"#,
                        ),
                    ),
                    _ => self.reply(id, 405, None),
                }
            }
            (Method::Options, _) => self.reply(id, 204, None),
            _ => self.reply(id, 404, Some(r#"{"error":"not_found"}"#)),
        }
    }

    fn status_reply(&mut self, id: u32) {
        let mut json = heapless::String::<768>::new();
        let session = self.phone.session.as_ref().map(|s| s.status);
        let _ = write!(
            json,
            r#"{{"device":"VANA C606","firmware":"{}","battery":{},"ftp":{},"lthr":{},"mode":"{}","indexing":{},"media":["#,
            env!("CARGO_PKG_VERSION"),
            Maybe(self.readings.live.battery_percent),
            self.preferences.profile.ftp_watts,
            self.preferences.profile.lthr_bpm,
            session.map_or("none", |s| s.mode.name()),
            self.indexing(),
        );
        for (index, medium) in MEDIA.into_iter().enumerate() {
            let recorder = self.ride.recorder(medium);
            let _ = write!(
                json,
                r#"{}{{"medium":"{}","state":"{}","used_slots":{},"capacity_slots":{},"free_s":{}}}"#,
                if index == 0 { "" } else { "," },
                medium.name(),
                recorder.status().name(),
                recorder.next_slot(),
                recorder.slots(),
                recorder.remaining_secs(),
            );
        }
        let _ = json.push_str("]}");
        self.reply(id, 200, Some(json.as_str()));
    }

    fn activities_head(&mut self, _id: u32) {
        self.phone.w().text.clear();
        // Length unknown: the body ends when the connection closes.
        if let Some(head) = Head::new(200)
            .header("Content-Type", "application/json")
            .header("Cache-Control", "no-store")
            .finish()
        {
            let _ = self.phone.w().text.extend_from_slice(head.bytes());
        }
        let mut open = heapless::String::<64>::new();
        let _ = write!(open, r#"{{"indexing":{},"activities":["#, self.indexing());
        let _ = self.phone.w().text.extend_from_slice(open.as_bytes());
    }

    fn activity_json(medium: Medium, ride: &Activity, first: bool) -> heapless::String<320> {
        let mut json = heapless::String::new();
        let _ = write!(
            json,
            r#"{}{{"id":"{}{}-{}","ride":{},"medium":"{}","start":{},"duration_s":{},"distance_m":{},"samples":{},"gps":{},"power":{},"heart":{},"end":"{}"}}"#,
            if first { "" } else { "," },
            medium_letter(medium),
            ride.start_slot,
            ride.ride_id,
            ride.ride_id,
            medium.name(),
            Maybe(ride.start_utc_ms.map(|ms| ms / 1000)),
            ride.active_ms / 1000,
            Maybe(ride.distance_mm.map(|mm| mm / 1000)),
            ride.samples,
            ride.with_position > 0,
            ride.with_power > 0,
            ride.with_heart > 0,
            ride.end.map_or("open", |end| end.name()),
        );
        json
    }

    fn start_fit(
        &mut self,
        id: u32,
        (medium, start_slot, ride_id): (Medium, usize, u32),
        start: Option<u64>,
    ) {
        let index = MEDIA.iter().position(|m| *m == medium).unwrap_or(0);
        let Some(ride) = self.phone.w().indexes[index]
            .find(start_slot, ride_id)
            .copied()
        else {
            let pending = !self.phone.w().indexes[index].done();
            return self.reply(
                id,
                if pending { 503 } else { 404 },
                Some(if pending {
                    r#"{"error":"indexing"}"#
                } else {
                    r#"{"error":"activity"}"#
                }),
            );
        };
        let rider = self.preferences.options.rider;
        let mut counter = Counter::default();
        let export = Export::start(ride_id, rider, None, None, &mut counter);
        self.phone.w().job = Job::Fit {
            id,
            job: FitJob {
                medium,
                ride_id,
                rider,
                next_slot: start_slot,
                start_slot,
                start_utc_ms: start,
                phase: FitPhase::Sizing { export, counter },
                name: file_name(ride_id, ride.start_utc_ms.or(start)),
            },
        };
    }

    /// Advance the current response as far as the transport has room.
    fn progress(
        &mut self,
        server: &mut impl Server,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
        now: u64,
    ) {
        match &mut self.phone.w().job {
            Job::Idle => {}
            Job::Text { id, at } => {
                let (id, sent) = (*id, *at);
                let n = server.write(id, &self.phone.w().text[sent..]);
                let at = sent + n;
                if at >= self.phone.w().text.len() {
                    server.finish(id);
                    self.phone.w().job = Job::Idle;
                } else {
                    self.phone.w().job = Job::Text { id, at };
                }
            }
            Job::Static { id, data, at } => {
                let (id, data, mut at) = (*id, *data, *at);
                // The head first, from `text`, then the body.
                if !self.phone.w().text.is_empty() {
                    let n = server.write(id, &self.phone.w().text);
                    let rest = self.phone.w().text.len() - n;
                    self.phone.w().text.copy_within(n.., 0);
                    self.phone.w().text.truncate(rest);
                    if rest > 0 {
                        return;
                    }
                }
                at += server.write(id, &data[at..]);
                if at >= data.len() {
                    server.finish(id);
                    self.phone.w().job = Job::Idle;
                } else {
                    self.phone.w().job = Job::Static { id, data, at };
                }
            }
            Job::Activities { .. } => self.progress_activities(server),
            Job::Fit { .. } => self.progress_fit(server, store, bulk),
            Job::Course { .. } => self.progress_course(server, bulk, now),
            Job::Workout { .. } => self.progress_workout(server, bulk, now),
        }
    }

    pub(super) fn phone_uploading(&self) -> bool {
        self.phone
            .work
            .as_deref()
            .is_some_and(|work| matches!(work.job, Job::Course { .. }))
    }

    fn course_reply(&mut self, id: u32) {
        let mut json = heapless::String::<128>::new();
        match self.back.as_deref() {
            None => {
                let _ = json.push_str(r#"{"store":"unavailable","loaded":false}"#);
            }
            Some(back) if back.course.is_empty() => {
                let _ = write!(
                    json,
                    r#"{{"store":"{}","loaded":false}}"#,
                    back.kept.store.name()
                );
            }
            Some(back) => {
                // A course name holds no character JSON escapes.
                let _ = write!(
                    json,
                    r#"{{"store":"{}","loaded":true,"name":"{}","points":{},"distance_m":{}}}"#,
                    back.kept.store.name(),
                    back.course.name(),
                    back.course.points().len(),
                    back.course.length_m(),
                );
            }
        }
        self.reply(id, 200, Some(json.as_str()));
    }

    fn start_course(&mut self, request: &Request) {
        let id = request.id;
        if request.body == 0 {
            return self.reply(id, 400, Some(r#"{"error":"empty"}"#));
        }
        let Some(back) = self.back.as_deref_mut() else {
            return self.reply(id, 503, Some(r#"{"error":"memory"}"#));
        };
        // Until the upload is stored the course in memory is not the stored
        // one; if it fails, the stored one is read back.
        back.kept.store = super::course::Store::Unread;
        let mut gpx = gpx::Gpx::new();
        gpx.start(back.course);
        back.course_changed();
        log::info!(target: "phone", "VANA_PHONE course_upload bytes={}", request.body);
        self.phone.w().job = Job::Course {
            id,
            received: 0,
            gpx,
        };
    }

    fn progress_course(&mut self, server: &mut impl Server, bulk: &mut impl ReadWrite, now: u64) {
        let Job::Course {
            id,
            mut received,
            mut gpx,
        } = core::mem::replace(&mut self.phone.w().job, Job::Idle)
        else {
            return;
        };
        let Some(back) = self.back.as_deref_mut() else {
            return self.reply(id, 503, Some(r#"{"error":"memory"}"#));
        };
        let Some(body) = server.request().filter(|r| r.id == id).map(|r| r.body) else {
            return;
        };
        let before = received;
        // A refused file is still read to its end, so the phone gets the
        // answer rather than a connection cut while it sends.
        {
            let mut piece = [0u8; UPLOAD_PIECE];
            for _ in 0..UPLOAD_READS {
                let wanted = (body - received).min(UPLOAD_PIECE as u32) as usize;
                let n = server.read(id, &mut piece[..wanted]);
                if n == 0 {
                    break;
                }
                received += n as u32;
                gpx.feed(&piece[..n], back.course);
            }
        }
        // A long upload is activity, not an idle session.
        if received > before
            && let Some(session) = self.phone.session.as_mut()
        {
            session.last_request_ms = now;
        }
        if received < body {
            self.phone.w().job = Job::Course { id, received, gpx };
            return;
        }
        if let Err(refusal) = gpx.finish(back.course) {
            log::warn!(target: "phone", "VANA_PHONE course_refused reason={refusal:?}");
            return self.reply(
                id,
                422,
                Some(match refusal {
                    gpx::Refusal::Coordinates => r#"{"error":"coordinates","detail":"a point of the track has no real latitude and longitude"}"#,
                    gpx::Refusal::Empty => r#"{"error":"no_route","detail":"the file has no track or route of two points or more"}"#,
                }),
            );
        }
        match course::save(bulk, back.course) {
            Ok(()) => {
                back.kept.store = super::course::Store::Kept;
                back.course_changed();
                log::info!(
                    target: "phone",
                    "VANA_PHONE course_saved points={} distance_m={}",
                    back.course.points().len(),
                    back.course.length_m()
                );
                self.display.redraw();
                self.course_reply(id);
            }
            Err(error) => {
                log::warn!(target: "phone", "VANA_PHONE course_not_saved error={error:?}");
                let (status, json) = match error {
                    course::Error::Full => (507, r#"{"error":"full"}"#),
                    course::Error::Media => (503, r#"{"error":"storage"}"#),
                    _ => (
                        503,
                        r#"{"error":"unavailable","detail":"no formatted memory card to keep a course on"}"#,
                    ),
                };
                self.reply(id, status, Some(json));
            }
        }
    }

    fn clear_course(&mut self, id: u32, bulk: &mut impl ReadWrite) {
        let Some(back) = self.back.as_deref_mut() else {
            return self.reply(id, 503, Some(r#"{"error":"memory"}"#));
        };
        match course::delete(bulk) {
            Ok(()) => {
                back.course.clear();
                back.kept.store = super::course::Store::Kept;
                back.course_changed();
                log::info!(target: "phone", "VANA_PHONE course_cleared");
                self.display.redraw();
                self.reply(id, 204, None);
            }
            Err(error) => {
                log::warn!(target: "phone", "VANA_PHONE course_not_cleared error={error:?}");
                self.reply(id, 503, Some(r#"{"error":"storage"}"#));
            }
        }
    }

    /// The uploaded workouts, listed from FILES now, and the files in the
    /// folder the device does not list.
    fn workouts_reply(&mut self, id: u32, bulk: &mut impl ReadWrite) {
        self.training.list(bulk);
        let (library, error) = (&self.training.library, self.training.list_error);
        let work = self.phone.w();
        let text = &mut work.text;
        text.clear();
        let _ = text.resize(HEAD_GAP, 0);
        let _ = write!(
            text,
            r#"{{"store":"{}","max":{},"skipped":{},"uploads":["#,
            error.map_or("ok", LoadError::name),
            library::UPLOADS,
            library.skipped
        );
        for (index, upload) in library.uploads().iter().enumerate() {
            // The most one entry takes, and the rest of the response.
            if text.capacity() - text.len() < 170 {
                break;
            }
            let file = upload
                .file
                .as_str()
                .strip_prefix(library::FOLDER)
                .unwrap_or("");
            let _ = write!(
                text,
                r#"{}{{"file":"{file}","name":""#,
                if index == 0 { "" } else { "," }
            );
            for byte in upload.name.as_bytes() {
                if matches!(byte, b'"' | b'\\') {
                    let _ = text.push(b'\\');
                }
                let _ = text.push(*byte);
            }
            let _ = write!(
                text,
                r#"","minutes":{},"steps":{}}}"#,
                upload.outline.planned_secs.div_ceil(60),
                upload.outline.played
            );
        }
        let _ = text.extend_from_slice(br#"],"unlisted":["#);
        let mut first = true;
        let _ = library.unlisted(bulk, |name| {
            if text.capacity() - text.len() < 60 {
                return;
            }
            let file = name.as_str().strip_prefix(library::FOLDER).unwrap_or("");
            let _ = write!(
                text,
                r#"{}{{"file":"{file}"}}"#,
                if first { "" } else { "," }
            );
            first = false;
        });
        let _ = text.extend_from_slice(b"]}");
        let body = text.len() - HEAD_GAP;
        let head = Head::new(200)
            .header("Content-Type", "application/json")
            .header("Cache-Control", "no-store")
            .length(body)
            .finish();
        match head {
            Some(head) if head.bytes().len() <= HEAD_GAP => {
                let at = HEAD_GAP - head.bytes().len();
                text[at..HEAD_GAP].copy_from_slice(head.bytes());
                text.copy_within(at.., 0);
                text.truncate(body + head.bytes().len());
            }
            _ => text.clear(),
        }
        work.job = Job::Text { id, at: 0 };
    }

    fn start_workout(&mut self, request: &Request) {
        let id = request.id;
        if self.recording() {
            return self.reply(id, 409, Some(r#"{"error":"ride"}"#));
        }
        if request.body == 0 {
            return self.reply(id, 400, Some(r#"{"error":"empty"}"#));
        }
        log::info!(target: "phone", "VANA_PHONE workout_upload bytes={}", request.body);
        let work = self.phone.w();
        work.text.clear();
        work.job = Job::Workout { id, received: 0 };
    }

    fn progress_workout(&mut self, server: &mut impl Server, bulk: &mut impl ReadWrite, now: u64) {
        let Job::Workout { id, mut received } =
            core::mem::replace(&mut self.phone.w().job, Job::Idle)
        else {
            return;
        };
        let Some(body) = server.request().filter(|r| r.id == id).map(|r| r.body) else {
            return;
        };
        let before = received;
        // A refused file is still read to its end, so the phone gets the
        // answer rather than a connection cut while it sends.
        {
            let mut piece = [0u8; UPLOAD_PIECE];
            for _ in 0..UPLOAD_READS {
                let wanted = (body - received).min(UPLOAD_PIECE as u32) as usize;
                let n = server.read(id, &mut piece[..wanted]);
                if n == 0 {
                    break;
                }
                received += n as u32;
                // Past the largest workout only the count matters.
                let _ = self.phone.w().text.extend_from_slice(&piece[..n]);
            }
        }
        if received > before
            && let Some(session) = self.phone.session.as_mut()
        {
            session.last_request_ms = now;
        }
        if received < body {
            self.phone.w().job = Job::Workout { id, received };
            return;
        }
        if self.recording() {
            return self.reply(id, 409, Some(r#"{"error":"ride"}"#));
        }
        if body as usize > ENCODED_MAX {
            return self.reply(id, 413, Some(r#"{"error":"too_large"}"#));
        }
        let stored = Library::store(bulk, &self.phone.w().text);
        self.training.list(bulk);
        self.display.redraw();
        match stored {
            Ok(file) => {
                log::info!(target: "phone", "VANA_PHONE workout_saved file={}", file.as_str());
                self.workouts_reply(id, bulk);
            }
            Err(error) => {
                log::warn!(target: "phone", "VANA_PHONE workout_not_saved error={error:?}");
                let (status, json) = match error {
                    StoreError::Decode(DecodeError::Magic | DecodeError::Version) => (
                        422,
                        r#"{"error":"format","detail":"not a VANA workout file"}"#,
                    ),
                    StoreError::Decode(DecodeError::Checksum | DecodeError::Length) => (
                        422,
                        r#"{"error":"damaged","detail":"the file is cut short or damaged"}"#,
                    ),
                    StoreError::Decode(_) | StoreError::Unnamed => (
                        422,
                        r#"{"error":"invalid","detail":"the workout is not one VANA can ride"}"#,
                    ),
                    StoreError::Full => (409, r#"{"error":"full"}"#),
                    StoreError::Load(LoadError::Files(files::Error::Full)) => {
                        (507, r#"{"error":"nospace"}"#)
                    }
                    StoreError::Load(
                        LoadError::Volume(_) | LoadError::Files(files::Error::Uninitialized),
                    ) => (
                        503,
                        r#"{"error":"unavailable","detail":"no formatted memory card to keep a workout on"}"#,
                    ),
                    StoreError::Load(_) => (503, r#"{"error":"storage"}"#),
                };
                self.reply(id, status, Some(json));
            }
        }
    }

    fn delete_workout(&mut self, request: &Request, bulk: &mut impl ReadWrite) {
        let id = request.id;
        if self.recording() {
            return self.reply(id, 409, Some(r#"{"error":"ride"}"#));
        }
        let mut path = heapless::String::<{ files::NAME }>::new();
        let wanted = query_value(&request.query, "file")
            .filter(|_| path.push_str(library::FOLDER).is_ok())
            .filter(|file| path.push_str(file).is_ok())
            .and_then(|_| files::Name::new(&path).ok());
        let Some(name) = wanted else {
            return self.reply(id, 404, Some(r#"{"error":"workout"}"#));
        };
        self.training.list(bulk);
        let result = match self.training.library.find(&name) {
            Some(library::Id::Uploaded(index)) => {
                self.delete_upload(usize::from(index), bulk).map(|()| true)
            }
            _ => self.training.library.remove_unlisted(bulk, &name),
        };
        match result {
            Ok(true) => {
                log::info!(target: "phone", "VANA_PHONE workout_deleted");
                self.training.list(bulk);
                self.display.redraw();
                self.reply(id, 204, None);
            }
            Ok(false) => self.reply(id, 404, Some(r#"{"error":"workout"}"#)),
            Err(error) => {
                log::warn!(target: "phone", "VANA_PHONE workout_not_deleted error={error:?}");
                self.reply(id, 503, Some(r#"{"error":"storage"}"#));
            }
        }
    }

    fn progress_activities(&mut self, server: &mut impl Server) {
        let Job::Activities {
            id,
            mut sent,
            mut first,
        } = self.phone.w().job
        else {
            return;
        };
        if !self.phone.w().text.is_empty() {
            let n = server.write(id, &self.phone.w().text);
            let rest = self.phone.w().text.len() - n;
            self.phone.w().text.copy_within(n.., 0);
            self.phone.w().text.truncate(rest);
            if rest > 0 {
                return;
            }
        }
        // Merge both journals newest first: each journal's rides are in
        // recording order, and the later start goes first. Undated rides
        // sort as oldest; on a tie the bulk ride goes first.
        loop {
            let heads: [Option<Activity>; 2] = core::array::from_fn(|medium| {
                self.phone.w().indexes[medium]
                    .rides()
                    .rev()
                    .nth(sent[medium])
                    .copied()
            });
            let Some(medium) = newer(heads.map(|ride| ride.map(|r| r.start_utc_ms))) else {
                break;
            };
            let Some(ride) = heads[medium] else { break };
            let json = Self::activity_json(MEDIA[medium], &ride, first);
            if server.room() < json.len() {
                self.phone.w().job = Job::Activities { id, sent, first };
                return;
            }
            server.write(id, json.as_bytes());
            first = false;
            sent[medium] += 1;
        }
        if server.room() < 2 {
            self.phone.w().job = Job::Activities { id, sent, first };
            return;
        }
        server.write(id, b"]}");
        server.finish(id);
        self.phone.w().job = Job::Idle;
    }

    fn progress_fit(
        &mut self,
        server: &mut impl Server,
        store: &mut impl RideStorage,
        bulk: &mut impl ReadWrite,
    ) {
        let Job::Fit { id, job } = core::mem::replace(&mut self.phone.w().job, Job::Idle) else {
            return;
        };
        let mut job = job;
        let limit = self.ride.recorder(job.medium).next_slot();
        let mut slot = Slot::default();
        let mut budget = READS_PER_STEP;
        match &mut job.phase {
            FitPhase::Sizing { export, counter } => {
                let mut ended = false;
                while budget > 0 && job.next_slot < limit {
                    budget -= 1;
                    match self
                        .ride
                        .read_slot(job.medium, store, bulk, job.next_slot, &mut slot)
                    {
                        Some(true) => {}
                        Some(false) => slot.0.fill(0),
                        None => return self.reply(id, 503, Some(r#"{"error":"storage"}"#)),
                    }
                    job.next_slot += 1;
                    if export.slot(&slot, counter) == Flow::End {
                        ended = true;
                        break;
                    }
                }
                if !ended && job.next_slot < limit {
                    self.phone.w().job = Job::Fit { id, job };
                    return;
                }
                let plan = export.clone().finish(counter);
                let clock = match fit::clock_for(&plan, job.start_utc_ms) {
                    Ok(clock) => clock,
                    Err(refusal) => {
                        return self.reply(
                            id,
                            422,
                            Some(match refusal {
                                Refusal::NoClock => r#"{"error":"no_clock","detail":"this ride has no time; add ?start=<unix seconds>"}"#,
                                Refusal::Empty => r#"{"error":"empty"}"#,
                                Refusal::BadClock => r#"{"error":"bad_clock"}"#,
                            }),
                        );
                    }
                };
                // Nothing was written yet, so the whole buffer is free.
                if !self.fit_head(server, id, &plan, &job.name) {
                    return self.reply(id, 503, Some(r#"{"error":"busy"}"#));
                }
                let mut out = Out {
                    server,
                    id,
                    short: false,
                };
                let export = Export::start(job.ride_id, job.rider, Some(&plan), clock, &mut out);
                job.phase = FitPhase::Writing { export };
                job.next_slot = job.start_slot;
                self.phone.w().job = Job::Fit { id, job };
            }
            FitPhase::Writing { export } => {
                let mut ended = false;
                while budget > 0
                    && job.next_slot < limit
                    && server.room() >= fit::MAX_SLOT_OUTPUT.max(fit::MAX_FINISH_OUTPUT)
                {
                    budget -= 1;
                    match self
                        .ride
                        .read_slot(job.medium, store, bulk, job.next_slot, &mut slot)
                    {
                        Some(true) => {}
                        Some(false) => slot.0.fill(0),
                        // A truncated body: the length tells the client.
                        None => return server.finish(id),
                    }
                    job.next_slot += 1;
                    let mut out = Out {
                        server,
                        id,
                        short: false,
                    };
                    let flow = export.slot(&slot, &mut out);
                    if out.short {
                        return server.finish(id);
                    }
                    if flow == Flow::End {
                        ended = true;
                        break;
                    }
                }
                if (ended || job.next_slot >= limit) && server.room() >= fit::MAX_FINISH_OUTPUT {
                    let mut out = Out {
                        server,
                        id,
                        short: false,
                    };
                    export.clone().finish(&mut out);
                    server.finish(id);
                    if let Some(session) = self.phone.session.as_mut() {
                        session.status.downloads += 1;
                    }
                    log::info!(target: "phone", "VANA_PHONE fit ride={} sent", job.ride_id);
                    return;
                }
                self.phone.w().job = Job::Fit { id, job };
            }
        }
    }

    /// Queue the FIT response head; false when the transport lacks room.
    fn fit_head(&self, server: &mut impl Server, id: u32, plan: &Plan, name: &str) -> bool {
        let mut disposition = heapless::String::<80>::new();
        let _ = write!(disposition, "attachment; filename=\"{name}\"");
        let Some(head) = Head::new(200)
            .header("Content-Type", "application/vnd.ant.fit")
            .header("Content-Disposition", disposition.as_str())
            .header("Cache-Control", "no-store")
            .length(plan.size as usize)
            .finish()
        else {
            return false;
        };
        if server.room() < head.bytes().len() + fit::MAX_START_OUTPUT {
            return false;
        }
        server.write(id, head.bytes()) == head.bytes().len()
    }
}

/// Which journal's next ride (internal, bulk) the list sends: the later
/// start. Undated rides sort as oldest; on a tie the bulk ride goes first.
fn newer(heads: [Option<Option<u64>>; 2]) -> Option<usize> {
    let key = |head: Option<Option<u64>>| head.map(|start| start.unwrap_or(0));
    match (key(heads[0]), key(heads[1])) {
        (None, None) => None,
        (Some(_), None) => Some(0),
        (None, Some(_)) => Some(1),
        (Some(internal), Some(bulk)) => Some(usize::from(bulk >= internal)),
    }
}

enum Readiness {
    Ready(usize),
    Pending,
    Never,
}

impl Runtime {
    /// Open Connect phone: a session on the device's access point, starting
    /// at the join code.
    pub(super) fn enter_phone(&mut self) {
        self.navigation.page = crate::screens::workout::Page::Phone;
        self.navigation.phone_step = 0;
        self.navigation.phone_advanced = false;
        self.open_phone(Mode::AccessPoint);
    }

    /// BACK ends the session and returns where it was opened from; any
    /// other press or a tap on the code switches codes, or tries again after
    /// a session ended.
    pub(super) fn phone_input(&mut self, button: device_api::input::Button, held: bool) {
        use device_api::input::Button;
        match button {
            Button::TopLeft if !held => {
                self.close_phone();
                self.navigation.page = if self.history.phone.take().is_some() {
                    crate::screens::workout::Page::PastRide
                } else {
                    crate::screens::workout::Page::Home
                };
            }
            Button::TopLeft => {}
            _ => self.phone_touch(),
        }
    }

    pub(super) fn phone_touch(&mut self) {
        if self.phone.session.is_none() && self.phone.wanted.is_none() {
            self.enter_phone();
        } else {
            self.navigation.phone_step ^= 1;
        }
    }

    /// The Connect phone lines and code for this frame.
    #[inline(never)]
    pub(super) fn phone_view(&mut self, view: &mut crate::screens::workout::View) {
        use crate::screens::phone::{NAME, NOTE, PROBLEM, SECRET, STEP, TITLE_LINE, place};
        use crate::screens::qr::Qr;
        let (qr, lines) = view.show_phone();
        let Some(status) = self.phone_status() else {
            let reason = match (self.phone.wanted, self.phone.closed) {
                (Some(Some(_)), _) | (_, None) => None,
                (_, Some(closed)) => Some(closed),
            };
            match reason {
                None => {
                    let _ = lines[STEP].push_str("CONNECT PHONE");
                    let _ = lines[TITLE_LINE].push_str("STARTING WI-FI");
                }
                Some(closed) => {
                    let _ = lines[STEP].push_str("PHONE DISCONNECTED");
                    let _ = lines[TITLE_LINE].push_str(match closed {
                        Closed::Rider => "SESSION ENDED",
                        Closed::Idle => "ENDED AFTER 10 MIN IDLE",
                        Closed::Ride => "NOT DURING A RIDE",
                        Closed::Radio => "WI-FI DID NOT START",
                        Closed::Offline => "NO HOME WI-FI",
                    });
                    let _ = lines[NOTE].push_str("TAP TO TRY AGAIN");
                    let _ = lines[PROBLEM].push_str("1");
                }
            }
            return;
        };
        let joined = match status.access_point {
            AccessPointState::Up { clients } => clients,
            _ => 0,
        };
        if status.mode == Mode::AccessPoint && joined > 0 && !self.navigation.phone_advanced {
            self.navigation.phone_advanced = true;
            self.navigation.phone_step = 1;
        }
        let url = status.url();
        let join = status.mode == Mode::AccessPoint && self.navigation.phone_step == 0;
        if join {
            let _ = lines[STEP].push_str("STEP 1 OF 2");
            let _ = lines[TITLE_LINE].push_str("JOIN VANA WI-FI");
            let _ = lines[NAME].push_str(status.ssid.text());
            for (index, chunk) in status.password.bytes().chunks(4).enumerate() {
                if index > 0 {
                    let _ = lines[SECRET].push_str(" ");
                }
                let _ = lines[SECRET].push_str(core::str::from_utf8(chunk).unwrap_or(""));
            }
            if joined > 0 {
                let _ = write!(lines[NOTE], "{joined} JOINED");
            } else {
                let _ = lines[NOTE].push_str("SCAN WITH THE CAMERA");
            }
            *qr = Qr::encode(status.join_text().as_str()).map(place);
            return;
        }
        let _ = lines[STEP].push_str(match status.mode {
            Mode::AccessPoint => "STEP 2 OF 2",
            Mode::Home => "ON HOME WI-FI",
        });
        let _ = lines[TITLE_LINE].push_str("OPEN YOUR RIDES");
        match (&url, status.address) {
            (Some(url), Some([a, b, c, d])) => {
                let _ = write!(lines[NAME], "{a}.{b}.{c}.{d}");
                if status.downloads > 0 {
                    let _ = write!(lines[NOTE], "{} SENT", status.downloads);
                } else {
                    let _ = lines[NOTE].push_str("ENDS AFTER 10 MIN IDLE");
                }
                *qr = Qr::encode(url.as_str()).map(place);
            }
            _ => {
                let _ = lines[NAME].push_str("WAITING FOR WI-FI");
            }
        }
    }
}

/// `null` or the value, for JSON.
struct Maybe<T>(Option<T>);

impl<T: core::fmt::Display> core::fmt::Display for Maybe<T> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match &self.0 {
            Some(value) => value.fmt(f),
            None => f.write_str("null"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_names_and_dates() {
        assert_eq!(parse_id("i12-3"), Some((Medium::Internal, 12, 3)));
        assert_eq!(parse_id("b0-1"), Some((Medium::Bulk, 0, 1)));
        assert_eq!(parse_id("x0-1"), None);
        assert_eq!(parse_id("i12"), None);
        assert_eq!(
            file_name(7, Some(1_792_249_200_000)).as_str(),
            "vana-2026-10-17-1500-ride7.fit"
        );
        assert_eq!(file_name(7, None).as_str(), "vana-ride7.fit");
    }

    #[test]
    fn the_list_merges_journals_newest_first() {
        assert_eq!(newer([None, None]), None);
        assert_eq!(newer([Some(Some(5)), None]), Some(0));
        assert_eq!(newer([None, Some(None)]), Some(1));
        assert_eq!(newer([Some(Some(5)), Some(Some(9))]), Some(1));
        assert_eq!(newer([Some(Some(9)), Some(Some(5))]), Some(0));
        assert_eq!(
            newer([Some(None), Some(Some(5))]),
            Some(1),
            "undated is oldest"
        );
        assert_eq!(newer([Some(Some(5)), Some(Some(5))]), Some(1));
    }
}
