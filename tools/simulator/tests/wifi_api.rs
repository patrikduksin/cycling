//! A phone puts the device on the home Wi-Fi the way the VANA app does: it
//! scans, picks a network, joins it with a password and forgets it. The
//! saved network outlasts a power cut, and the session's trace replays
//! exactly.
use device_api::server::Method;
use serde_json::{Value, json};
use shell_simulator::replay::first_difference;
use shell_simulator::sim::{Sim, seconds};
use shell_simulator::station::Network;

const PASSWORD: &str = "correct horse";

fn request(sim: &mut Sim, method: Method, path: &str, body: &str) -> (u16, Value) {
    let (status, text) = sim.phone_request(method, path, body.as_bytes());
    let value = match text.as_str() {
        "" => Value::Null,
        text => serde_json::from_str(text).unwrap_or_else(|e| panic!("{path}: {e}: {text}")),
    };
    (status, value)
}

fn wifi(sim: &mut Sim) -> Value {
    let (status, wifi) = request(sim, Method::Get, "/api/wifi", "");
    assert_eq!(status, 200, "{wifi}");
    wifi
}

/// What `GET /api/wifi` says once the operation under way ended.
fn settled(sim: &mut Sim) -> Value {
    for _ in 0..60 {
        let wifi = wifi(sim);
        if wifi["op"]["pending"] == false {
            return wifi;
        }
        sim.advance(500);
    }
    panic!("the Wi-Fi operation never ended: {}", wifi(sim));
}

fn join(sim: &mut Sim, ssid: &str, password: &str) -> (u16, Value) {
    let body = json!({ "ssid": ssid, "password": password }).to_string();
    request(sim, Method::Put, "/api/wifi", &body)
}

fn phone_near_networks() -> Sim {
    let mut sim = Sim::new();
    sim.discard_recording();
    let mut sim = sim.boot();
    let network = |ssid: &str, rssi, password: Option<&str>| Network {
        ssid: ssid.into(),
        rssi,
        password: password.map(Into::into),
    };
    sim.device.sim_mut().wifi.around = vec![
        network("home", -75, Some(PASSWORD)),
        network("café", -50, None),
        network("Mum's \"5G\"", -80, Some("another password")),
        network("home", -60, Some(PASSWORD)),
    ];
    sim.connect_phone();
    sim
}

#[test]
fn a_phone_scans_joins_and_forgets_a_network_that_outlasts_a_power_cut() {
    let mut sim = phone_near_networks();
    let before = wifi(&mut sim);
    assert_eq!(before["configured"], Value::Null);
    assert_eq!(before["state"], "off");
    assert_eq!(before["networks"], json!([]));

    let (status, scan) = request(&mut sim, Method::Post, "/api/wifi/scan", "");
    assert_eq!(status, 202, "{scan}");
    let (status, busy) = request(&mut sim, Method::Post, "/api/wifi/scan", "");
    assert_eq!((status, &busy["error"]), (409, &json!("busy")));
    let scanned = settled(&mut sim);
    assert_eq!(scanned["op"]["seq"], scan["seq"]);
    assert_eq!(
        scanned["networks"],
        json!([
            { "ssid": "café", "rssi": -50, "secure": false },
            { "ssid": "home", "rssi": -60, "secure": true },
            { "ssid": "Mum's \"5G\"", "rssi": -80, "secure": true },
        ])
    );

    let (status, short) = join(&mut sim, "home", "short");
    assert_eq!((status, &short["error"]), (400, &json!("password")));
    let (status, empty) = join(&mut sim, "", PASSWORD);
    assert_eq!((status, &empty["error"]), (400, &json!("ssid")));
    let (status, garbled) = request(&mut sim, Method::Put, "/api/wifi", "{\"ssid\":");
    assert_eq!((status, &garbled["error"]), (400, &json!("json")));
    assert_eq!(wifi(&mut sim)["configured"], Value::Null);

    let (status, joined) = join(&mut sim, "home", PASSWORD);
    assert_eq!(status, 202, "{joined}");
    let joining = wifi(&mut sim);
    assert_eq!(joining["configured"], "home");
    assert_eq!(joining["state"], "joining");
    assert_eq!(joining["op"]["pending"], true);
    let (status, busy) = join(&mut sim, "home", PASSWORD);
    assert_eq!((status, &busy["error"]), (409, &json!("busy")));
    let connected = settled(&mut sim);
    assert_eq!(connected["state"], "connected", "{connected}");
    assert_eq!(connected["online"], true);
    assert_eq!(connected["rssi"], -60);
    assert_eq!(connected["op"]["error"], Value::Null);

    sim.cut_power();
    sim.advance(seconds(5));
    sim.connect_phone();
    let restarted = wifi(&mut sim);
    assert_eq!(restarted["configured"], "home", "{restarted}");

    let (status, forgot) = request(&mut sim, Method::Delete, "/api/wifi", "");
    assert_eq!(status, 202, "{forgot}");
    let forgotten = settled(&mut sim);
    assert_eq!(forgotten["configured"], Value::Null);
    assert_eq!(forgotten["state"], "off");
    let (status, reply) = sim.command("WIFI");
    assert_eq!(status, "OK");
    assert!(reply.contains("saved=false"), "{reply}");

    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}

#[test]
fn a_wrong_password_and_a_missing_network_each_say_so() {
    let mut sim = phone_near_networks();
    let (status, _) = join(&mut sim, "home", "not the password");
    assert_eq!(status, 202);
    let refused = settled(&mut sim);
    assert_eq!(refused["state"], "failed", "{refused}");
    assert_eq!(refused["op"]["error"], "wrong_password");
    assert_eq!(refused["online"], false);

    let (status, _) = join(&mut sim, "next door", PASSWORD);
    assert_eq!(status, 202);
    let missing = settled(&mut sim);
    assert_eq!(missing["state"], "failed", "{missing}");
    assert_eq!(missing["op"]["error"], "network_not_found");
}

#[test]
fn wifi_routes_need_the_session_token() {
    let mut sim = phone_near_networks();
    sim.disconnect_phone();
    sim.connect_phone();
    let token = sim.device.sdk.phone_status().unwrap().token;
    assert!(!token.is_empty());
    // A request without the token is refused before it reaches the station.
    let server = &mut sim.device.sim_mut().phone_server;
    server.ask(
        Method::Put,
        "/api/wifi",
        "",
        br#"{"ssid":"home","password":"password"}"#,
    );
    for _ in 0..50 {
        sim.advance(100);
        if let Some(response) = sim.device.sim_mut().phone_server.response() {
            let text = String::from_utf8_lossy(response);
            assert!(text.starts_with("HTTP/1.1 401"), "{text}");
            assert_eq!(wifi(&mut sim)["configured"], Value::Null);
            return;
        }
    }
    panic!("no answer");
}

#[test]
fn over_bluetooth_a_phone_scans_and_cannot_join() {
    let mut sim = phone_near_networks();
    sim.disconnect_phone();
    let (status, reply) = sim.command("PHONE START BLE");
    assert_eq!(status, "ACCEPTED", "PHONE START BLE: {reply}");
    sim.advance(seconds(1));
    let (status, scan) = request(&mut sim, Method::Post, "/api/wifi/scan", "");
    assert_eq!(status, 202, "{scan}");
    let scanned = settled(&mut sim);
    assert_eq!(scanned["networks"][0]["ssid"], "café", "{scanned}");
    let (status, refused) = join(&mut sim, "home", PASSWORD);
    assert_eq!((status, &refused["error"]), (403, &json!("read_only")));
    assert_eq!(wifi(&mut sim)["configured"], Value::Null);
}
