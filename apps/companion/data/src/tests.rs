use super::link::tests::{pairing, Device};
use super::link::Pairing;
use super::shapes::{Bike, ContractValue, Fit, Joined, Link, Network, Outcome as Done, Pairing as Paired, Ride, Rides, Today, Wifi};
use super::{Vana, PAIRING};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use exact_runner::{Answer, DataError, DataSource, Outcome, Response, Store, Value};
use serde_json::{json, Value as Json};

const NOW_MS: u64 = 1_795_500_720_000;

fn module_reply(v: Json) -> Outcome {
    Outcome::Response(Response { status: 200, headers: Vec::new(), body: v.to_string().into_bytes() })
}

/// One request the device heard, read back: method, path, body, and when the phone sent it.
fn heard(plain: &[u8], native: &Json) -> Json {
    let text = String::from_utf8_lossy(plain);
    let (head, body) = text.split_once("\r\n\r\n").unwrap();
    let mut line = head.lines().next().unwrap().split(' ');
    json!({ "method": line.next(), "path": line.next(), "body": body, "after_ms": native["after_ms"] })
}

/// The phone's side under test: the data source, its secrets, and a device
/// model behind the Swift module's pipe.
struct Bench {
    vana: Vana,
    store: Store,
    device: Device,
    heard: Vec<Json>,
    natives: Vec<Json>,
    saved: Option<Json>,
}

impl Bench {
    fn unpaired() -> Bench {
        let store = Store::new(Vana::default().grants(), []);
        Bench { vana: Vana::default(), store, device: Device::new(pairing()), heard: Vec::new(), natives: Vec::new(), saved: None }
    }

    fn paired_as(pairing: &Pairing) -> Bench {
        let store = Store::new(Vana::default().grants(), [(PAIRING.to_string(), pairing.link().into_bytes())]);
        Bench { store, ..Bench::unpaired() }
    }

    fn connected() -> Bench {
        let mut bench = Bench::paired_as(&pairing());
        let answer = bench.run("connect", &[], &[TIME_SET]);
        assert_eq!(answer, Ok(Answer::Now(Done { ok: true, text: "Connected to VANA in 840 ms".into() }.to_value())));
        bench.heard.clear();
        bench
    }

    /// What the Swift module does with one request: `reply` is the device's HTTP response.
    fn swift(&mut self, request: &Json, reply: &str) -> Outcome {
        self.natives.push(request.clone());
        match request["op"].as_str().unwrap() {
            "connect" => {
                assert_eq!(request["id"], "c6060001");
                module_reply(json!({ "ok": true, "name": "VANA", "psm": 129, "ms": 840, "random": B64.encode([0xa0; 16]) }))
            }
            "exchange" => {
                let send = B64.decode(request["send"].as_str().unwrap()).unwrap();
                let at = if request["until"] == "frame" { 0 } else { self.device.sent + 1 };
                let mut plain = Vec::new();
                let frames = self.device.receive(&send, |p| {
                    plain = p.to_vec();
                    reply.as_bytes().to_vec()
                });
                if request["until"] == "end" {
                    self.heard.push(heard(&plain, request));
                }
                module_reply(json!({ "at": at, "frames": B64.encode(frames), "unix_ms": NOW_MS }))
            }
            "disconnect" => module_reply(json!({ "ok": true })),
            op => panic!("unexpected op {op}"),
        }
    }

    /// Answer `source`, playing one device reply per HTTP request, until it answers now.
    fn run(&mut self, source: &str, args: &[Value], replies: &[&str]) -> Result<Answer, DataError> {
        let mut answer = self.vana.answer(&mut self.store, source, args);
        let mut replies = replies.iter();
        loop {
            let Ok(Answer::Later(request)) = &answer else {
                return answer;
            };
            let outcome = match &request.storage {
                Some(payload) => {
                    self.saved = Some(serde_json::from_slice(payload).unwrap());
                    Outcome::Storage(b"null".to_vec())
                }
                None => {
                    let native: Json = serde_json::from_slice(&request.body).unwrap();
                    let reply = if native["until"] == "end" { replies.next().expect("a reply for each request") } else { &"" };
                    self.swift(&native, reply)
                }
            };
            answer = self.vana.parse(&mut self.store, source, args, outcome);
        }
    }

    fn link(&mut self, state: &str) -> Value {
        self.vana.answer(&mut self.store, "link", &[]).unwrap();
        let status = module_reply(json!({ "state": state, "detail": "", "name": "VANA", "psm": 129 }));
        let Ok(Answer::Now(v)) = self.vana.parse(&mut self.store, "link", &[], status) else { panic!() };
        v
    }
}

const TIME_SET: &str = "HTTP/1.1 204 No Content\r\nContent-Length: 0\r\n\r\n";

fn ask(source: &str, args: &[Value], raw: &str) -> Result<Answer, DataError> {
    Bench::connected().run(source, args, &[raw])
}

/// Drive one Wi-Fi write through the device's replies; the requests it sent and its answer.
fn fly(source: &str, args: &[Value], replies: &[String]) -> (Vec<Json>, Result<Answer, DataError>) {
    let mut bench = Bench::connected();
    let replies: Vec<&str> = replies.iter().map(String::as_str).collect();
    let answer = bench.run(source, args, &replies);
    (bench.heard, answer)
}

#[test]
fn connecting_finds_the_paired_id_shakes_hands_and_sets_the_clock() {
    let mut bench = Bench::paired_as(&pairing());
    let opening = Link { state: "opening".into(), detail: String::new(), name: "VANA".into(), psm: 129.0 };
    assert_eq!(bench.link("open"), opening.to_value(), "an open channel without a session is not connected");
    let answer = bench.run("connect", &[], &[TIME_SET]);
    assert_eq!(answer, Ok(Answer::Now(Done { ok: true, text: "Connected to VANA in 840 ms".into() }.to_value())));
    assert_eq!(bench.natives[1]["send"], B64.encode([&[16, 0][..], &[0xa0; 16]].concat()), "the hello is nonce_p, framed");
    assert_eq!(bench.heard[0]["method"], "PUT");
    assert_eq!(bench.heard[0]["path"], format!("/api/time?unix_ms={NOW_MS}"));
    let open = Link { state: "open".into(), detail: String::new(), name: "VANA".into(), psm: 129.0 };
    assert_eq!(bench.link("open"), open.to_value());
}

#[test]
fn connecting_needs_a_pairing() {
    let answer = Bench::unpaired().vana.answer(&mut Store::new(Vana::default().grants(), []), "connect", &[]);
    assert_eq!(answer, Err(DataError::Unavailable("Pair this iPhone with the bike computer first".into())));
}

#[test]
fn a_bike_computer_that_closes_on_the_first_sealed_frame_asks_to_pair_again() {
    let mut bench = Bench::paired_as(&pairing());
    let mut answer = bench.vana.answer(&mut bench.store, "connect", &[]);
    for _ in 0..2 {
        let Ok(Answer::Later(request)) = &answer else { panic!("{answer:?}") };
        let outcome = bench.swift(&serde_json::from_slice(&request.body).unwrap(), "");
        answer = bench.vana.parse(&mut bench.store, "connect", &[], outcome);
    }
    let closed = Outcome::Response(Response { status: 500, headers: Vec::new(), body: b"channel closed".to_vec() });
    let Ok(Answer::Later(close)) = bench.vana.parse(&mut bench.store, "connect", &[], closed) else { panic!() };
    assert_eq!(serde_json::from_slice::<Json>(&close.body).unwrap()["op"], "disconnect");
    assert_eq!(
        bench.vana.parse(&mut bench.store, "connect", &[], module_reply(json!({ "ok": true }))),
        Err(DataError::Unavailable("The bike computer refused this iPhone (channel closed). Pair it again.".into()))
    );
}

#[test]
fn a_reply_sealed_with_another_key_closes_the_channel() {
    let mut bench = Bench::connected();
    bench.vana.answer(&mut bench.store, "bike", &[]).unwrap();
    let forged = module_reply(json!({ "at": bench.device.sent + 1, "frames": B64.encode([&[32, 0][..], &[0u8; 32]].concat()) }));
    let Ok(Answer::Later(close)) = bench.vana.parse(&mut bench.store, "bike", &[], forged) else { panic!() };
    assert_eq!(serde_json::from_slice::<Json>(&close.body).unwrap()["op"], "disconnect");
    assert_eq!(
        bench.vana.parse(&mut bench.store, "bike", &[], module_reply(json!({ "ok": true }))),
        Err(DataError::Unavailable("the bike computer's reply did not authenticate".into()))
    );
    assert!(bench.vana.answer(&mut bench.store, "wifi", &[]).is_err(), "the broken session is not used again");
}

#[test]
fn a_join_without_an_encrypted_session_never_sends_the_password() {
    let mut bench = Bench::paired_as(&pairing());
    let answer = bench.vana.answer(&mut bench.store, "join", &join_args());
    assert_eq!(answer, Err(DataError::Unavailable("Not connected to the bike computer".into())));
    assert!(bench.natives.is_empty());
}

#[test]
fn the_password_crosses_the_air_only_sealed() {
    let mut bench = Bench::connected();
    let Ok(Answer::Later(request)) = bench.vana.answer(&mut bench.store, "join", &join_args()) else { panic!() };
    let native: Json = serde_json::from_slice(&request.body).unwrap();
    let send = B64.decode(native["send"].as_str().unwrap()).unwrap();
    assert!(!send.windows(13).any(|w| w == b"correct horse"));
}

#[test]
fn pairing_keeps_the_link_and_forgetting_drops_it() {
    let mut bench = Bench::unpaired();
    let unpaired = Paired { paired: false, id: String::new() };
    assert_eq!(bench.vana.answer(&mut bench.store, "pairing", &[]), Ok(Answer::Now(unpaired.to_value())));

    let link = pairing().link();
    let answer = bench.run("pair", &[Value::str(&format!(" {link} "))], &[]);
    assert_eq!(answer, Ok(Answer::Now(Done { ok: true, text: "Paired".into() }.to_value())));
    assert_eq!(bench.store.get(PAIRING), Some(link.as_str()));
    let paired = Paired { paired: true, id: "c6060001".into() };
    assert_eq!(bench.vana.answer(&mut bench.store, "pairing", &[]), Ok(Answer::Now(paired.to_value())));

    let answer = bench.run("unpair", &[], &[]);
    assert_eq!(answer, Ok(Answer::Now(Done { ok: true, text: "Forgot the bike computer".into() }.to_value())));
    assert_eq!(bench.store.get(PAIRING), None);
    assert_eq!(bench.vana.answer(&mut bench.store, "pairing", &[]), Ok(Answer::Now(unpaired.to_value())));
}

#[test]
fn a_link_that_is_not_a_pairing_is_refused() {
    let answer = Bench::unpaired().run("pair", &[Value::str("https://cycling.computer")], &[]);
    assert_eq!(answer, Err(DataError::Unavailable("That is not a VANA pairing link.".into())));
}

#[test]
fn a_ride_downloads_to_a_file_for_the_share_sheet() {
    let args = [Value::str("f12-17"), Value::str("vana-2026-11-24-0712-ride17.fit")];
    let fit: Vec<u8> = (0..3000u32).map(|i| (i % 251) as u8).collect();
    let response =
        [format!("HTTP/1.1 200 OK\r\nContent-Type: application/vnd.ant.fit\r\nContent-Length: {}\r\n\r\n", fit.len()).as_bytes(), &fit]
            .concat();
    let mut bench = Bench::connected();
    let Ok(Answer::Later(request)) = bench.vana.answer(&mut bench.store, "fit", &args) else { panic!() };
    let native: Json = serde_json::from_slice(&request.body).unwrap();
    assert_eq!(native["timeout_ms"], 120_000);
    let send = B64.decode(native["send"].as_str().unwrap()).unwrap();
    let at = bench.device.sent + 1;
    let frames = bench.device.receive(&send, |_| response.clone());
    let reply = module_reply(json!({ "at": at, "frames": B64.encode(frames) }));
    let Ok(Answer::Later(write)) = bench.vana.parse(&mut bench.store, "fit", &args, reply) else { panic!() };
    let write: Json = serde_json::from_slice(write.storage.as_ref().unwrap()).unwrap();
    assert_eq!(write["op"], "fs.writeFile");
    assert_eq!(write["args"]["path"], "app:/cache/vana-2026-11-24-0712-ride17.fit");
    assert_eq!(serde_json::from_value::<Vec<u8>>(write["args"]["bytes"].clone()).unwrap(), fit);
    let want = Fit { path: "app:/cache/vana-2026-11-24-0712-ride17.fit".into() };
    assert_eq!(bench.vana.parse(&mut bench.store, "fit", &args, Outcome::Storage(b"null".to_vec())), Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_short_download_is_not_saved() {
    let args = [Value::str("f12-17"), Value::str("ride.fit")];
    let short = "HTTP/1.1 200 OK\r\nContent-Length: 1048\r\n\r\nFIT";
    assert_eq!(Bench::connected().run("fit", &args, &[short]), Err(DataError::Unavailable("the download was cut short".into())));
}

fn ok(json: &str) -> String {
    format!("HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{json}", json.len())
}

#[test]
fn status_shows_battery_firmware_storage_and_athlete() {
    let raw = ok(
        r#"{"device":"C606","firmware":"0.4.1","volatile":false,"battery":82,"ftp":210,"lthr":176,"mode":"none","indexing":false,"clear":null,"media":[{"medium":"internal","state":"ready","used_slots":3,"capacity_slots":40,"free_s":9000},{"medium":"files","state":"ready","used_slots":1,"capacity_slots":200,"free_s":138000}]}"#,
    );
    let bike = Bike {
        battery: "82%".into(),
        firmware: "0.4.1".into(),
        storage: "38 h of recording left".into(),
        ftp: "210 W".into(),
        lthr: "176 bpm".into(),
    };
    assert_eq!(ask("bike", &[], &raw), Ok(Answer::Now(bike.to_value())));
}

#[test]
fn status_without_a_battery_reading_says_unknown() {
    let raw = ok(
        r#"{"device":"C606","firmware":"0.4.1","volatile":true,"battery":null,"ftp":200,"lthr":170,"mode":"ble","indexing":true,"clear":"kept","media":[{"medium":"internal","state":"ready","used_slots":0,"capacity_slots":4,"free_s":1500}]}"#,
    );
    let Ok(Answer::Now(v)) = ask("bike", &[], &raw) else { panic!() };
    let want = Bike {
        battery: "Unknown".into(),
        firmware: "0.4.1".into(),
        storage: "25 min of recording left".into(),
        ftp: "200 W".into(),
        lthr: "170 bpm".into(),
    };
    assert_eq!(v, want.to_value());
}

#[test]
fn rides_are_dated_in_the_phones_zone_newest_first() {
    // The activity list streams with no Content-Length: the zero frame ends it.
    let raw = "HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nCache-Control: no-store\r\n\r\n".to_string()
        + r#"{"indexing":false,"activities":[{"id":"f12-17","ride":17,"medium":"files","start":1795500720,"duration_s":6130,"distance_m":48312,"samples":6130,"gps":true,"power":true,"heart":false,"end":"stopped","unsaved":false},{"id":"i0-16","ride":16,"medium":"internal","start":null,"duration_s":905,"distance_m":null,"samples":905,"gps":false,"power":false,"heart":true,"end":"open","unsaved":true}]}"#;
    let rides = Rides {
        indexing: false,
        rides: vec![
            Ride {
                id: "f12-17".into(),
                title: "Tue 24 Nov, 07:12".into(),
                subtitle: "1:42:10 · 48.3 km".into(),
                power: true,
                heart: false,
                date: "Tue 24 Nov 2026".into(),
                start: "07:12".into(),
                moving: "1:42:10".into(),
                distance: "48.3 km".into(),
                speed: "28.4 km/h".into(),
                recorded: "GPS, Power".into(),
                file: "vana-2026-11-24-0712-ride17.fit".into(),
            },
            Ride {
                id: "i0-16".into(),
                title: "Ride 16".into(),
                subtitle: "15:05".into(),
                power: false,
                heart: true,
                date: "Unknown".into(),
                start: "Unknown".into(),
                moving: "15:05".into(),
                distance: "Not recorded".into(),
                speed: "Not recorded".into(),
                recorded: "Heart rate".into(),
                file: "vana-ride16.fit".into(),
            },
        ],
    };
    // 1795500720 is 2026-11-24 06:12 UTC; the phone is an hour east (CET).
    assert_eq!(ask("rides", &[Value::Number(60.0)], &raw), Ok(Answer::Now(rides.to_value())));
}

#[test]
fn a_ride_across_midnight_in_utc_takes_the_local_day() {
    let raw = ok(
        r#"{"indexing":true,"activities":[{"id":"f1-2","ride":2,"medium":"files","start":1795558800,"duration_s":59,"distance_m":0,"samples":59,"gps":false,"power":false,"heart":false,"end":"stopped","unsaved":false}]}"#,
    );
    let Ok(Answer::Now(v)) = ask("rides", &[Value::Number(-300.0)], &raw) else { panic!() };
    // 2026-11-24 22:20 UTC is 17:20 five hours west.
    let ride = Ride {
        id: "f1-2".into(),
        title: "Tue 24 Nov, 17:20".into(),
        subtitle: "0:59 · 0.0 km".into(),
        power: false,
        heart: false,
        date: "Tue 24 Nov 2026".into(),
        start: "17:20".into(),
        moving: "0:59".into(),
        distance: "0.0 km".into(),
        speed: "0.0 km/h".into(),
        recorded: "Time only".into(),
        file: "vana-2026-11-24-1720-ride2.fit".into(),
    };
    assert_eq!(v, Rides { indexing: true, rides: vec![ride] }.to_value());
}

#[test]
fn todays_workout_or_none() {
    let planned = ok(r#"{"store":"ok","workout":{"name":"Sweet \"spot\" 3x12","minutes":45,"steps":12}}"#);
    let want = Today { planned: true, name: "Sweet \"spot\" 3x12".into(), detail: "45 min · 12 steps".into() };
    assert_eq!(ask("today", &[], &planned), Ok(Answer::Now(want.to_value())));

    let none = ok(r#"{"store":"ok","workout":null}"#);
    let want = Today { planned: false, name: "No workout today".into(), detail: String::new() };
    assert_eq!(ask("today", &[], &none), Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_refusal_or_broken_reply_fails_the_resource() {
    let refused = "HTTP/1.1 503 Service Unavailable\r\nConnection: close\r\nContent-Length: 20\r\n\r\n{\"error\":\"indexing\"}";
    assert_eq!(ask("rides", &[Value::Number(0.0)], refused), Err(DataError::Unavailable("the bike computer answered 503".into())));
    assert!(matches!(ask("bike", &[], "HTTP/1.1 200 OK\r\n"), Err(DataError::Unavailable(_))));
    assert!(matches!(ask("today", &[], &ok(r#"{"store":"io","workout":null}"#)), Err(DataError::Unavailable(_))));
    let mut bench = Bench::connected();
    bench.vana.answer(&mut bench.store, "bike", &[]).unwrap();
    let channel_closed = Outcome::Response(Response { status: 500, headers: Vec::new(), body: b"channel closed".to_vec() });
    assert_eq!(bench.vana.parse(&mut bench.store, "bike", &[], channel_closed), Err(DataError::Unavailable("channel closed".into())));
}

#[test]
fn average_speed_is_distance_over_moving_time() {
    assert_eq!(super::device::average_speed(Some(36_000), 3600), Some("36.0 km/h".into()));
    assert_eq!(super::device::average_speed(Some(48_312), 6130), Some("28.4 km/h".into()));
    assert_eq!(super::device::average_speed(None, 3600), None);
    assert_eq!(super::device::average_speed(Some(100), 0), None);
}

const STATION: &str = r#"{"configured":"home","state":"connected","online":true,"rssi":-58,"op":{"seq":7,"pending":false,"error":null},"networks":[{"ssid":"home","rssi":-58,"secure":true},{"ssid":"Neighbour 5G","rssi":-67,"secure":true},{"ssid":"Cafe","rssi":-81,"secure":false}]}"#;

#[test]
fn wifi_shows_the_current_network_and_the_scan() {
    let want = Wifi {
        configured: true,
        ssid: "home".into(),
        state: "Connected".into(),
        ink: "vana.green".into(),
        bars: 3.0,
        signal: "Strong signal".into(),
        networks: vec![
            Network { ssid: "home".into(), secure: true, bars: 3.0, signal: "Strong signal".into(), joined: true },
            Network { ssid: "Neighbour 5G".into(), secure: true, bars: 2.0, signal: "Fair signal".into(), joined: false },
            Network { ssid: "Cafe".into(), secure: false, bars: 1.0, signal: "Weak signal".into(), joined: false },
        ],
    };
    assert_eq!(ask("wifi", &[], &ok(STATION)), Ok(Answer::Now(want.to_value())));
}

#[test]
fn wifi_with_nothing_saved_is_not_set_up() {
    let body = r#"{"configured":null,"state":"off","online":false,"rssi":null,"op":{"seq":0,"pending":false,"error":null},"networks":[]}"#;
    let Ok(Answer::Now(v)) = ask("wifi", &[], &ok(body)) else { panic!() };
    let want = Wifi {
        configured: false,
        ssid: String::new(),
        state: "Not Set Up".into(),
        ink: "-exact-secondary-label".into(),
        bars: 0.0,
        signal: String::new(),
        networks: vec![],
    };
    assert_eq!(v, want.to_value());
}

#[test]
fn state_labels_and_signal_bars() {
    use super::wifi::{bars, state_label};
    let labels: Vec<_> = ["connected", "joining", "no_internet", "off", "failed", "idle"].map(state_label).into();
    assert_eq!(labels, ["Connected", "Joining…", "No Internet", "Not Set Up", "Failed", "Not Connected"]);
    assert_eq!([-40, -60, -61, -70, -71, -90].map(bars), [3, 3, 2, 2, 1, 1]);
}

fn accepted(seq: u32) -> String {
    let body = format!(r#"{{"seq":{seq}}}"#);
    format!("HTTP/1.1 202 Accepted\r\nContent-Length: {}\r\n\r\n{body}", body.len())
}

fn station(state: &str, seq: u32, pending: bool, error: Option<&str>) -> String {
    let error = error.map_or("null".into(), |e| format!("\"{e}\""));
    ok(&format!(
        r#"{{"configured":"home","state":"{state}","online":false,"rssi":-60,"op":{{"seq":{seq},"pending":{pending},"error":{error}}},"networks":[]}}"#
    ))
}

const BUSY: &str = "HTTP/1.1 409 Conflict\r\nContent-Length: 16\r\n\r\n{\"error\":\"busy\"}";

fn join_args() -> [Value; 2] {
    [Value::str("home"), Value::str("correct horse")]
}

#[test]
fn a_join_puts_the_network_then_polls_until_the_station_settles() {
    let replies = [accepted(8), station("joining", 8, true, None), station("connected", 8, false, None)];
    let (requests, answer) = fly("join", &join_args(), &replies);
    assert_eq!(requests[0]["method"], "PUT");
    assert_eq!(requests[0]["path"], "/api/wifi");
    assert_eq!(requests[0]["body"], r#"{"password":"correct horse","ssid":"home"}"#);
    assert!(requests[1..].iter().all(|r| r["method"] == "GET" && r["path"] == "/api/wifi" && r["after_ms"] == 1000));
    let want = Joined { outcome: "joined".into(), ssid: "home".into(), text: String::new() };
    assert_eq!(answer, Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_join_reads_only_its_own_operation() {
    // The station still reports the scan before it (seq 7) as settled.
    let replies = [accepted(8), station("connected", 7, false, None), station("no_internet", 8, false, None)];
    let (_, answer) = fly("join", &join_args(), &replies);
    let want = Joined { outcome: "no_internet".into(), ssid: "home".into(), text: "Joined, no internet".into() };
    assert_eq!(answer, Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_wrong_password_and_a_missing_network() {
    let (_, answer) = fly("join", &join_args(), &[accepted(3), station("failed", 3, false, Some("wrong_password"))]);
    let want = Joined { outcome: "wrong_password".into(), ssid: "home".into(), text: String::new() };
    assert_eq!(answer, Ok(Answer::Now(want.to_value())));
    let (_, answer) = fly("join", &join_args(), &[accepted(3), station("failed", 3, false, Some("network_not_found"))]);
    let want = Joined { outcome: "failed".into(), ssid: "home".into(), text: "The bike computer cannot find “home”.".into() };
    assert_eq!(answer, Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_busy_device_is_retried_once() {
    let (requests, answer) = fly("join", &join_args(), &[BUSY.into(), accepted(4), station("connected", 4, false, None)]);
    assert_eq!(requests[1]["method"], "PUT");
    assert_eq!(requests[1]["after_ms"], 1500);
    assert!(matches!(answer, Ok(Answer::Now(_))));
    let (_, answer) = fly("join", &join_args(), &[BUSY.into(), BUSY.into()]);
    let want = Joined { outcome: "failed".into(), ssid: "home".into(), text: "Bike computer is busy".into() };
    assert_eq!(answer, Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_join_gives_up_after_30_seconds() {
    let mut replies = vec![accepted(5)];
    replies.extend(std::iter::repeat_n(station("joining", 5, true, None), 31));
    let (requests, answer) = fly("join", &join_args(), &replies);
    assert_eq!(requests.len(), 32, "the PUT and a poll a second for 31 s");
    let want =
        Joined {
            outcome: "failed".into(), ssid: "home".into(), text: "The bike computer did not join “home” within 30 seconds.".into()
        };
    assert_eq!(answer, Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_refused_password_says_why() {
    let refused = "HTTP/1.1 400 Bad Request\r\nContent-Length: 20\r\n\r\n{\"error\":\"password\"}";
    let (_, answer) = fly("join", &join_args(), &[refused.into()]);
    let want = Joined { outcome: "failed".into(), ssid: "home".into(), text: "The password must be 8 to 63 characters.".into() };
    assert_eq!(answer, Ok(Answer::Now(want.to_value())));
}

#[test]
fn a_scan_and_a_forget_finish_when_their_operation_does() {
    let (requests, answer) = fly("scan", &[], &[accepted(9), station("connected", 9, true, None), station("connected", 9, false, None)]);
    assert_eq!((requests[0]["method"].as_str(), requests[0]["path"].as_str()), (Some("POST"), Some("/api/wifi/scan")));
    assert_eq!(answer, Ok(Answer::Now(Done { ok: true, text: String::new() }.to_value())));
    let (requests, answer) = fly("forget", &[], &[accepted(2), station("off", 2, false, None)]);
    assert_eq!((requests[0]["method"].as_str(), requests[0]["path"].as_str()), (Some("DELETE"), Some("/api/wifi")));
    assert_eq!(answer, Ok(Answer::Now(Done { ok: true, text: "Forgot the network".into() }.to_value())));
}

#[test]
fn sealed_requests_leave_in_the_order_they_were_sealed() {
    let mut bench = Bench::connected();
    for source in ["bike", "rides", "today", "wifi"] {
        let Ok(Answer::Later(request)) = bench.vana.answer(&mut bench.store, source, &[]) else { panic!("{source}") };
        assert_eq!(request.http, exact_runner::HttpScheduling::Ordered, "{source}");
    }
}
