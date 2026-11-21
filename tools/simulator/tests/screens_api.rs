//! A phone changes the main ride page over the device's Wi-Fi the way a
//! companion app would: it reads the screens and the catalogue, edits a
//! tree from what they say, dry-runs it, saves it and reads it back after
//! a power cut. The trace of the session replays exactly.
use device_api::input::Role;
use device_api::server::Method;
use serde_json::Value;
use shell_simulator::replay::first_difference;
use shell_simulator::sim::{Sim, seconds};

/// The main ride page's key in the record.
const MAIN: u8 = vana::compose::builtin::ScreenId::RideMain.key();

fn json(sim: &mut Sim, method: Method, path: &str, body: &[u8]) -> (u16, Value) {
    let (status, text) = sim.phone_request(method, path, body);
    let value = serde_json::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}: {text}"));
    (status, value)
}

fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|at| u8::from_str_radix(&text[at..at + 2], 16).unwrap())
        .collect()
}

fn main_page(screens: &Value) -> &Value {
    screens["screens"]
        .as_array()
        .unwrap()
        .iter()
        .find(|screen| screen["name"] == "ride/main")
        .expect("ride/main is a named screen")
}

/// The main page as the phone reads it with the lower line's cadence
/// changed to power: the revision it was read at, its nodes and the index
/// of the changed one.
fn power_for_cadence(sim: &mut Sim) -> (u64, Vec<u8>, usize) {
    let (status, screens) = json(sim, Method::Get, "/api/screens", b"");
    assert_eq!(status, 200, "{screens}");
    let base = screens["revision"].as_u64().unwrap();
    let main = main_page(&screens);
    assert_eq!(main["override"], false);
    let mut nodes = unhex(main["nodes"].as_str().unwrap());

    let (status, catalogue) = json(sim, Method::Get, "/api/screens/catalogue", b"");
    assert_eq!(status, 200, "{catalogue}");
    let leaf = catalogue["kinds"]["leaf"].as_u64().unwrap() as u8;
    let hero = catalogue["kinds"]["hero"].as_u64().unwrap() as u8;
    for tile in catalogue["tiles"].as_array().unwrap() {
        assert!(!tile["forms"].as_array().unwrap().is_empty(), "{tile}");
    }
    // A reading's setting names its metric in the low four bits.
    let (reading, cadence) = tile(&catalogue, "reading", "CADENCE");
    let (_, power) = tile(&catalogue, "reading", "POWER");
    let index = nodes
        .chunks(4)
        .position(|node| node[0] & !hero == leaf + reading && node[1] & 0x0f == cadence & 0x0f)
        .expect("the lower line shows cadence");
    let arg = &mut nodes[index * 4 + 1];
    *arg = *arg & 0xf0 | power & 0x0f;
    (base, nodes, index)
}

/// The catalogue's `(id, arg)` for tile `name` showing `label`.
fn tile(catalogue: &Value, name: &str, label: &str) -> (u8, u8) {
    let tile = catalogue["tiles"]
        .as_array()
        .unwrap()
        .iter()
        .find(|tile| tile["tile"] == name && tile["label"] == label)
        .unwrap_or_else(|| panic!("no {name}({label}) in the catalogue"));
    (
        tile["id"].as_u64().unwrap() as u8,
        tile["arg"].as_u64().unwrap() as u8,
    )
}

#[test]
fn a_phone_changes_the_main_page_over_wifi_and_it_survives_a_power_cut() {
    let mut sim = Sim::new();
    sim.discard_recording();
    let mut sim = sim.boot();
    sim.connect_phone();

    let (base, nodes, _) = power_for_cadence(&mut sim);
    let (_, catalogue) = json(&mut sim, Method::Get, "/api/screens/catalogue", b"");

    let (status, dry) = json(
        &mut sim,
        Method::Post,
        "/api/screens/check?screen=ride/main",
        &nodes,
    );
    assert_eq!(status, 200, "{dry}");
    let panel = &catalogue["panel"];
    let (width, height) = (panel[0].as_u64().unwrap(), panel[1].as_u64().unwrap());
    let solves = dry["solves"].as_array().unwrap();
    assert_eq!(solves.len(), 16, "{dry}");
    for solve in solves {
        let leaves = solve["leaves"].as_array().unwrap();
        assert!(!leaves.is_empty(), "{solve}");
        for placed in leaves {
            let [_, _, left, top, right, bottom] =
                [0, 1, 2, 3, 4, 5].map(|i| placed[i].as_u64().unwrap());
            assert!(
                left <= right && right <= width && top <= bottom && bottom <= height,
                "{placed}"
            );
        }
    }

    let path = format!("/api/screens/ride/main?base={base}");
    let (status, saved) = json(&mut sim, Method::Put, &path, &nodes);
    assert_eq!(status, 200, "{saved}");
    assert_eq!(saved["revision"].as_u64(), Some(base + 1));
    let (status, stale) = json(&mut sim, Method::Put, &path, &nodes);
    assert_eq!((status, &stale["error"]), (409, &Value::from("stale")));

    sim.cut_power();
    sim.advance(seconds(5));
    let shown = sim
        .device
        .sdk
        .screens()
        .screen(MAIN)
        .expect("the saved page loads at boot");
    let stored: Vec<u8> = shown
        .tree()
        .nodes()
        .iter()
        .flat_map(|node| node.bytes())
        .collect();
    assert_eq!(stored, nodes);
    sim.connect_phone();
    let (_, screens) = json(&mut sim, Method::Get, "/api/screens", b"");
    assert_eq!(screens["revision"].as_u64(), Some(base + 1));
    assert_eq!(main_page(&screens)["override"], true);
    assert_eq!(unhex(main_page(&screens)["nodes"].as_str().unwrap()), nodes);

    let reset = format!("/api/screens/ride/main?base={}", base + 1);
    let (status, _) = json(&mut sim, Method::Delete, &reset, b"");
    assert_eq!(status, 200);
    assert!(sim.device.sdk.screens().screen(MAIN).is_none());

    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}

#[test]
fn a_main_page_the_phone_saved_is_the_one_the_ride_shows() {
    let mut sim = Sim::new();
    sim.discard_recording();
    let mut sim = sim.boot();
    sim.connect_phone();
    let (base, nodes, changed) = power_for_cadence(&mut sim);

    let (status, dry) = json(
        &mut sim,
        Method::Post,
        "/api/screens/check?screen=ride/main",
        &nodes,
    );
    assert_eq!(status, 200, "{dry}");
    let plain = dry["solves"]
        .as_array()
        .unwrap()
        .iter()
        .find(|solve| solve["flags"] == "")
        .unwrap_or_else(|| panic!("a solve with no flags: {dry}"));
    let placed = plain["leaves"]
        .as_array()
        .unwrap()
        .iter()
        .find(|placed| placed[0].as_u64() == Some(changed as u64))
        .unwrap_or_else(|| panic!("node {changed} shows: {plain}"));
    let [left, top, right, bottom] = [2, 3, 4, 5].map(|i| placed[i].as_u64().unwrap() as usize);
    let path = format!("/api/screens/ride/main?base={base}");
    let (status, saved) = json(&mut sim, Method::Put, &path, &nodes);
    assert_eq!(status, 200, "{saved}");

    while sim.vana_page() != "home" {
        sim.press(Role::Back);
        sim.advance(seconds(1));
    }
    sim.start_ride();
    // Past the note that auto-pause is off without a speed.
    sim.advance(seconds(11));
    assert_eq!(sim.vana_page(), "ride");
    let inside: Vec<String> = sim
        .screen()
        .frame()
        .read()
        .into_iter()
        .filter(|run| (left..right).contains(&run.x) && (top..bottom).contains(&run.y))
        .map(|run| run.text)
        .collect();
    assert!(
        inside.iter().any(|text| text == "W"),
        "{inside:?} in {placed}:\n{}",
        sim.screen().text()
    );
    assert!(!inside.iter().any(|text| text == "RPM"), "{inside:?}");

    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}

/// What `cycling screens-wifi` is tested against: the screens and the
/// catalogue as a booted device lists them. Write it again with
/// `SCREENS_API_FIXTURE=write`.
#[test]
fn the_phone_client_fixture_is_what_the_device_lists() {
    let mut sim = Sim::new();
    sim.discard_recording();
    let mut sim = sim.boot();
    sim.connect_phone();
    let (_, screens) = json(&mut sim, Method::Get, "/api/screens", b"");
    let (_, catalogue) = json(&mut sim, Method::Get, "/api/screens/catalogue", b"");
    let listed = serde_json::json!({"screens": screens, "catalogue": catalogue});
    let text = serde_json::to_string(&listed).unwrap() + "\n";
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../devtools/tests/fixtures/screens_api.json");
    if std::env::var_os("SCREENS_API_FIXTURE").is_some_and(|value| value == "write") {
        std::fs::write(&path, &text).unwrap();
    }
    let recorded = std::fs::read_to_string(&path).unwrap_or_default();
    assert!(
        recorded == text,
        "{} is not what the device lists; run with SCREENS_API_FIXTURE=write",
        path.display()
    );
}
