use super::menu::{Mine, SETTLE_MS, View};
use super::*;
use crate::compose::Flags;
use crate::compose::builtin::{Laid, ScreenId};
use crate::compose::registry::{NAV_HEADER, SENSOR_FACTS, SENSOR_LIST, SENSOR_SEARCH};
use crate::compose::tiles::sensor_list;
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::Rect;
use crate::screens::cue::{Cue, Icon};
use crate::screens::theme::{DARK, LIGHT, Tone};
use crate::screens::{C606, Device, panels};
use crate::sensors::ant::{Battery, BatteryStatus, device_type};
use crate::sensors::saved::SAVED_CAPACITY;
use device_api::ant::{DISCOVERY_CAPACITY, Discovery, Identity};
use device_api::input::Role;
use firmware_shell::input::Input;

fn press(button: Role) -> Input {
    Input::Press {
        role: button,
        code: 1,
    }
}

fn identity(device_type: u8, device_number: u16) -> Identity {
    Identity {
        device_type,
        device_number,
        transmission_type: 1,
    }
}

fn peer(number: u16) -> Discovery {
    Discovery {
        identity: identity(device_type::POWER, number),
        rssi: -40,
        seen_ms: 0,
    }
}

fn strap(number: u16) -> Identity {
    identity(device_type::HEART_RATE, number)
}

fn strap_found(number: u16) -> Discovery {
    Discovery {
        identity: strap(number),
        ..peer(number)
    }
}

fn nearby(peers: &[Discovery]) -> [Option<Discovery>; DISCOVERY_CAPACITY] {
    core::array::from_fn(|index| peers.get(index).copied())
}

fn saved(tiles: &[(Identity, Connection)]) -> [Option<Selected>; SAVED_CAPACITY] {
    core::array::from_fn(|slot| {
        tiles.get(slot).map(|&(identity, connection)| Selected {
            identity,
            connection,
            battery: None,
            product: Default::default(),
            reading: None,
        })
    })
}

/// A menu showing `tiles` and `peers`, opened as the app opens it.
fn opened(peers: &[Discovery], tiles: &[(Identity, Connection)]) -> Menu {
    let mut menu = Menu::new();
    menu.refresh(nearby(peers), saved(tiles), false);
    menu.open();
    menu
}

/// Presses a button at a fresh moment, past the debounce and the detail
/// page's settling time.
struct Rider {
    now: u64,
}

impl Rider {
    fn new() -> Self {
        Self { now: 10_000 }
    }

    fn press(&mut self, menu: &mut Menu, button: Role) -> Option<Action> {
        self.now += SETTLE_MS;
        menu.input(press(button), self.now)
    }

    /// MOVE until `kind`'s sensor is focused, or ADD SENSOR's row for `None`.
    fn focus(&mut self, menu: &mut Menu, kind: Option<u8>) {
        for _ in 0..24 {
            if menu.diagnostics().focus.map(|f| f.device_type) == kind {
                return;
            }
            self.press(menu, Role::Move);
        }
        panic!("{kind:?} is not reachable");
    }
}

fn page(menu: &Menu) -> Page {
    menu.diagnostics().page
}

fn run(menu: &Menu, text: &str) -> Option<crate::screens::canvas::Run> {
    canvas(menu).find(text).copied()
}

/// The page shown, drawn from its tree on the C606's panel.
fn canvas(menu: &Menu) -> Canvas {
    canvas_on(menu, &C606)
}

fn canvas_on(menu: &Menu, device: &'static Device) -> Canvas {
    let mut view = menu.frame(device);
    view.prepare();
    view.canvas().clone()
}

fn cues(menu: &Menu) -> std::vec::Vec<(Role, Icon)> {
    let cues = menu.frame(&C606).prepare().cues().to_vec();
    cues.iter().map(|cue| (cue.role, cue.icon)).collect()
}

fn cues_on(menu: &Menu, device: &'static Device) -> std::vec::Vec<Cue> {
    menu.frame(device).prepare().cues().to_vec()
}

fn laid(menu: &Menu, device: &'static Device) -> Laid {
    let laid = Laid::new(menu.screen(), device, Flags::NONE, None);
    laid.expect("each sensor page has a tree")
}

fn tap(menu: &mut Menu, device: &'static Device, x: usize, y: usize, now: u64) -> Option<Action> {
    let laid = laid(menu, device);
    menu.tap_at(&laid, device, x, y, now)
}

fn middle(rect: Rect) -> (usize, usize) {
    (rect.center_x(), rect.top.midpoint(rect.bottom))
}

fn list(menu: &Menu) -> List {
    match menu.screen() {
        ScreenId::SensorsNearby => List::Nearby,
        _ => List::Sensors,
    }
}

fn shown(menu: &Menu, device: &'static Device) -> std::vec::Vec<(usize, Row, Rect)> {
    let laid = laid(menu, device);
    let rect = laid.box_of(SENSOR_LIST).expect("a list");
    sensor_list::placed(menu, list(menu), rect, &device.scaled).to_vec()
}

fn row(menu: &Menu, device: &'static Device, index: usize) -> Option<Rect> {
    let rows = shown(menu, device);
    rows.iter()
        .find(|(at, ..)| *at == index)
        .map(|(.., rect)| *rect)
}

fn focused(menu: &Menu, device: &'static Device) -> Rect {
    let index = menu.focus(list(menu)).expect("a focused row");
    row(menu, device, index).expect("the focused row shows")
}

fn search(menu: &Menu) -> (std::string::String, Tone, usize) {
    let search = menu.search();
    (search.text.as_str().into(), search.tone, search.added)
}

#[test]
fn opening_with_nothing_saved_goes_straight_to_nearby_and_searches() {
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), [None; SAVED_CAPACITY], false);
    assert_eq!(menu.open(), Some(Action::Scan));
    assert_eq!(page(&menu), Page::Nearby);
    let title = run(&menu, "Nearby").expect("Nearby titles the page");
    let heading = C606.scaled.face(crate::screens::theme::LAYOUT.face.heading);
    assert!(core::ptr::eq(title.line.face(), heading));
    assert!(!canvas(&menu).shows("Add sensor"));
    let mut menu = Menu::new();
    menu.refresh(
        nearby(&[peer(10)]),
        saved(&[(strap(1), Connection::Live)]),
        false,
    );
    assert_eq!(menu.open(), None);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, Some(strap(1)));
    let sensors = run(&menu, "Sensors").expect("Sensors titles the page");
    assert_eq!((sensors.left, sensors.top), (title.left, title.top));
    assert!(canvas(&menu).shows("Heart rate"));
    assert!(canvas(&menu).shows("Add sensor"));
}

#[test]
fn only_nearby_searches_and_a_tap_on_its_line_searches_again() {
    let tiles = [
        (strap(1), Connection::Live),
        (identity(device_type::POWER, 2), Connection::Searching),
    ];
    let mut menu = opened(&[peer(10)], &tiles);
    let mut rider = Rider::new();
    for _ in 0..6 {
        assert_eq!(rider.press(&mut menu, Role::Move), None);
    }
    rider.focus(&mut menu, Some(device_type::HEART_RATE));
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(rider.press(&mut menu, Role::Back), None);
    rider.focus(&mut menu, None);
    assert_eq!(rider.press(&mut menu, Role::Act), Some(Action::Scan));
    assert_eq!(page(&menu), Page::Nearby);
    menu.search_started();
    menu.refresh([None; DISCOVERY_CAPACITY], saved(&tiles), true);
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(rider.press(&mut menu, Role::Back), Some(Action::StopScan));
    assert_eq!(rider.press(&mut menu, Role::Act), Some(Action::Scan));
    menu.search_started();
    menu.refresh([None; DISCOVERY_CAPACITY], saved(&tiles), false);
    assert_eq!(menu.diagnostics().focus, None);
    assert_eq!(rider.press(&mut menu, Role::Act), Some(Action::Scan));
    let line = laid(&menu, &C606).box_of(SENSOR_SEARCH).expect("the line");
    let (x, y) = middle(line);
    assert_eq!(tap(&mut menu, &C606, x, y, rider.now), Some(Action::Scan));
}

#[test]
fn move_walks_the_rows_and_wraps_and_back_leaves() {
    let tiles = [
        (strap(1), Connection::Live),
        (identity(device_type::POWER, 2), Connection::NotFound),
        (identity(device_type::SPEED, 3), Connection::Searching),
    ];
    let mut menu = opened(&[], &tiles);
    let mut rider = Rider::new();
    let mut seen = std::vec::Vec::new();
    for _ in 0..5 {
        seen.push(menu.diagnostics().focus.map(|f| f.device_type));
        rider.press(&mut menu, Role::Move);
    }
    use device_type::{HEART_RATE, POWER, SPEED};
    let walk = [
        Some(HEART_RATE),
        Some(POWER),
        Some(SPEED),
        None,
        Some(HEART_RATE),
    ];
    assert_eq!(seen, walk);
    rider.focus(&mut menu, Some(POWER));
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Detail);
    rider.press(&mut menu, Role::Back);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus.map(|f| f.device_type), Some(POWER));
    assert_eq!(rider.press(&mut menu, Role::Back), Some(Action::Ride));
}

#[test]
fn nearby_stays_open_after_an_add_so_more_sensors_pair_from_one_search() {
    let speed = identity(device_type::SPEED, 10);
    let cadence = identity(device_type::CADENCE, 20);
    let radar = identity(device_type::RADAR, 30);
    let found = [speed, cadence, radar].map(|identity| Discovery {
        identity,
        rssi: -40,
        seen_ms: 0,
    });
    let mut tiles = std::vec![(strap(1), Connection::Live)];
    let mut menu = opened(&found, &tiles);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    assert_eq!(rider.press(&mut menu, Role::Act), Some(Action::Scan));
    menu.search_started();
    menu.refresh(nearby(&found), saved(&tiles), true);
    assert_eq!(menu.diagnostics().focus, Some(speed));
    for want in [cadence, radar, speed] {
        rider.press(&mut menu, Role::Move);
        assert_eq!(menu.diagnostics().focus, Some(want));
    }
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Connect(speed))
    );
    tiles.push((speed, Connection::WaitingForRadio));
    menu.refresh(nearby(&found), saved(&tiles), false);
    menu.added(speed);
    menu.set_message(MenuMessage::ScanCancelled);
    assert_eq!(page(&menu), Page::Nearby);
    assert_eq!(search(&menu).2, 1);
    assert!(canvas(&menu).shows("1 ADDED"));
    assert!(!canvas(&menu).shows("ID 10 · ANT+"));
    assert_eq!(menu.diagnostics().focus, Some(cadence));
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Connect(cadence))
    );
    tiles.push((cadence, Connection::WaitingForRadio));
    menu.refresh(nearby(&found), saved(&tiles), false);
    menu.added(cadence);
    assert_eq!(search(&menu).2, 2);
    assert_eq!(menu.diagnostics().focus, Some(radar));
    menu.added(radar);
    assert_eq!(search(&menu).2, 2, "not saved");
    menu.set_message(MenuMessage::ScanFailed);
    assert_eq!(search(&menu).1, Tone::Warn);
    assert_eq!(rider.press(&mut menu, Role::Back), Some(Action::StopScan));
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, Some(cadence));
    rider.focus(&mut menu, None);
    assert_eq!(rider.press(&mut menu, Role::Act), Some(Action::Scan));
    assert_eq!(search(&menu).0, "TAP TO SEARCH");
    rider.press(&mut menu, Role::Back);
    assert_eq!(menu.diagnostics().focus, None);
}

#[test]
fn back_from_nearby_returns_to_sensors_or_leaves_when_nothing_is_saved() {
    let mut rider = Rider::new();
    let mut menu = opened(&[peer(10)], &[(strap(1), Connection::Live)]);
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    assert_eq!(rider.press(&mut menu, Role::Back), Some(Action::StopScan));
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, None, "back on ADD SENSOR's row");
    let mut empty = opened(&[peer(10)], &[]);
    assert_eq!(page(&empty), Page::Nearby);
    assert_eq!(rider.press(&mut empty, Role::Back), Some(Action::Ride));
}

#[test]
fn sensors_added_from_one_search_fold_into_short_rows_then_say_new_on_sensors() {
    let current = identity(device_type::POWER, 1);
    let found = [peer(20), strap_found(30), peer(40)];
    let mut menu = opened(&found, &[(current, Connection::Live)]);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Nearby);
    assert!(canvas(&menu).shows("TAP TO ADD ONE OR MORE"));
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Connect(peer(20).identity))
    );
    let mut tiles = std::vec![
        (current, Connection::Live),
        (peer(20).identity, Connection::Live)
    ];
    menu.refresh(nearby(&found), saved(&tiles), true);
    menu.added(peer(20).identity);
    assert_eq!(page(&menu), Page::Nearby, "NEARBY stays open");
    assert_eq!(menu.diagnostics().focus, Some(strap(30)), "focus moves on");
    assert_eq!(search(&menu), ("SEARCHING".into(), Tone::Info, 1));
    assert!(canvas(&menu).shows("BACK WHEN DONE"));
    let rows = shown(&menu, &C606);
    assert!(matches!(rows[0].1, Row::Added(s) if s.identity == peer(20).identity));
    assert!(
        rows[0].2.height() < rows[1].2.height(),
        "an added row is short"
    );
    assert_eq!(menu.focus(List::Nearby), Some(1), "past the added row");
    assert!(canvas(&menu).shows("Live"));
    let check = crate::screens::icons::Symbol::CheckDisc;
    let check = C606.scaled.art.symbol(check);
    assert!(canvas(&menu).shapes.iter().any(|shape| matches!(
        shape,
        Shape::Icon { icon, color, .. } if core::ptr::eq(icon.0, check) && *color == DARK.good
    )));
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Connect(strap(30)))
    );
    tiles.push((strap(30), Connection::Live));
    menu.refresh(nearby(&found), saved(&tiles), true);
    menu.added(strap(30));
    assert_eq!(menu.diagnostics().focus, Some(peer(40).identity));
    assert_eq!(search(&menu).2, 2);
    assert_eq!(menu.added_peers(), [peer(20).identity, strap(30)]);
    assert_eq!(rider.press(&mut menu, Role::Back), Some(Action::StopScan));
    assert_eq!(menu.diagnostics().focus, Some(strap(30)));
    let news = menu
        .rows(List::Sensors)
        .filter(|row| matches!(row, Row::Saved { new: true, .. }))
        .count();
    assert_eq!(news, 2);
    assert!(canvas(&menu).shows("New · Live"));
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    assert!(menu.added_peers().is_empty());
}

#[test]
fn nearby_lists_the_strongest_signal_first() {
    let at = |number, rssi| Discovery {
        rssi,
        ..peer(number)
    };
    let found = [at(1, -85), at(2, -50), at(3, -75), at(4, -52)];
    let mut menu = opened(&found, &[]);
    let order: std::vec::Vec<_> = menu
        .rows(List::Nearby)
        .map(|row| match row {
            Row::Found { identity, bars, .. } => (identity.device_number, bars),
            other => panic!("{other:?}"),
        })
        .collect();
    assert_eq!(order, [(2, 4), (4, 4), (3, 2), (1, 1)]);
    assert_eq!(
        menu.diagnostics().focus,
        Some(identity(device_type::POWER, 2))
    );
    let mut rider = Rider::new();
    rider.press(&mut menu, Role::Move);
    assert_eq!(
        menu.diagnostics().focus,
        Some(identity(device_type::POWER, 4))
    );
}

#[test]
fn full_slots_and_stale_discoveries_say_why_nothing_connects() {
    let tiles: [_; SAVED_CAPACITY] =
        core::array::from_fn(|n| (identity(n as u8 + 20, n as u16), Connection::Live));
    let mut menu = opened(&[peer(10)], &tiles);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    assert!(menu.full());
    assert_eq!(search(&menu).0, "FULL - FORGET ONE");
    assert_eq!(search(&menu).1, Tone::Warn);
    assert_eq!(menu.hint(), "BACK, THEN FORGET ONE");
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(menu.diagnostics().message, Some(MenuMessage::SlotsFull));
    assert_eq!(search(&menu).0, "FULL - FORGET ONE");
    let mut menu = opened(&[peer(10)], &[]);
    menu.refresh([None; DISCOVERY_CAPACITY], [None; SAVED_CAPACITY], false);
    assert!(canvas(&menu).shows("Not seen"));
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(menu.diagnostics().message, Some(MenuMessage::NotSeen));
}

#[test]
fn the_search_line_says_how_the_search_goes() {
    let mut menu = opened(&[], &[]);
    let line = |menu: &Menu| {
        let (text, tone, _) = search(menu);
        (text, tone)
    };
    assert_eq!(line(&menu), ("TAP TO SEARCH".into(), Tone::Sub));
    menu.refresh([None; DISCOVERY_CAPACITY], [None; SAVED_CAPACITY], true);
    menu.set_message(MenuMessage::Scanning);
    assert_eq!(line(&menu), ("SEARCHING".into(), Tone::Info));
    assert!(menu.search().wake && canvas(&menu).shows("WAKE A SENSOR"));
    menu.refresh([None; DISCOVERY_CAPACITY], [None; SAVED_CAPACITY], false);
    menu.set_message(MenuMessage::ScanDone);
    assert_eq!(line(&menu), ("NO SENSORS FOUND".into(), Tone::Sub));
    menu.refresh(nearby(&[peer(1)]), [None; SAVED_CAPACITY], false);
    assert_eq!(line(&menu), ("SCAN DONE - PICK".into(), Tone::Info));
    menu.set_message(MenuMessage::ScanFailed);
    assert_eq!(line(&menu), ("SCAN FAILED - RETRY".into(), Tone::Warn));
    menu.set_notice(Some(MenuMessage::SavedUnreadable));
    menu.set_message(MenuMessage::Scanning);
    assert_eq!(line(&menu), ("NOT SAVED".into(), Tone::Warn));
}

#[test]
fn sensors_names_trouble_with_the_radio_or_saving_in_its_title() {
    let mut menu = opened(&[], &[(strap(1), Connection::Live)]);
    menu.set_message(MenuMessage::ScanFailed);
    assert_eq!(run(&menu, "Sensors").map(|r| r.color), Some(DARK.text));
    menu.set_message(MenuMessage::RadioUnavailable);
    let trouble = run(&menu, "RADIO UNAVAILABLE").map(|r| r.color);
    assert_eq!(trouble, Some(DARK.warn));
    menu.set_notice(Some(MenuMessage::NotSaved));
    assert_eq!(run(&menu, "NOT SAVED").map(|r| r.color), Some(DARK.warn));
}

#[test]
fn rows_name_each_state_in_its_tone_and_a_battery_after_it() {
    let cases = [
        (Connection::Live, "Live", DARK.good),
        (Connection::Searching, "Searching", DARK.warn),
        (Connection::NotFound, "Not found yet", DARK.warn),
        (Connection::WaitingForRadio, "Waiting for radio", DARK.warn),
        (Connection::Switching, "Switching", DARK.warn),
        (Connection::NotInRange, "Not in range", DARK.sub),
        (Connection::Standby, "In range", DARK.sub),
        (Connection::DataLost, "Data lost", DARK.warn),
        (Connection::WaitingForData, "Waiting for data", DARK.warn),
        (Connection::RadioUnavailable, "Radio unavailable", DARK.bad),
        (Connection::Unsupported, "Type unsupported", DARK.bad),
    ];
    for (connection, text, color) in cases {
        let menu = opened(&[], &[(strap(1), connection)]);
        assert_eq!(run(&menu, text).map(|r| r.color), Some(color), "{text}");
    }
    let batteries = [
        (BatteryStatus::New, Some(3_100), "Live · 3.1 V", DARK.good),
        (BatteryStatus::Good, Some(2_949), "Live · 2.9 V", DARK.good),
        (BatteryStatus::Ok, None, "Live", DARK.good),
        (BatteryStatus::Low, Some(2_400), "Battery Low", DARK.warn),
        (BatteryStatus::Critical, None, "Battery Critical", DARK.bad),
    ];
    for (status, millivolts, text, color) in batteries {
        let mut tiles = saved(&[(strap(1), Connection::Live)]);
        tiles[0].as_mut().unwrap().battery = Some(Battery {
            status,
            millivolts,
            identifier: 0,
        });
        let mut menu = Menu::new();
        menu.refresh([None; DISCOVERY_CAPACITY], tiles, false);
        menu.open();
        assert_eq!(run(&menu, text).map(|r| r.color), Some(color), "{text}");
    }
}

#[test]
fn a_live_row_shows_its_reading_and_one_not_in_use_shows_dashes() {
    let meters = [
        (identity(device_type::POWER, 1), Connection::Live),
        (identity(device_type::POWER, 2), Connection::NotInRange),
    ];
    let menu = opened(&[], &meters);
    let canvas = canvas(&menu);
    let readings: std::vec::Vec<_> = canvas
        .runs
        .iter()
        .filter(|run| run.line.text() == b"--")
        .map(|run| run.color)
        .collect();
    assert_eq!(readings, [DARK.sub, DARK.sub], "no power reading yet");
    let units = canvas.runs.iter().filter(|run| run.line.text() == b"W");
    assert_eq!(units.count(), 0, "dashes stand without a unit");
    let mut tiles = saved(&meters);
    tiles[0].as_mut().unwrap().reading = Some(250);
    let mut live = Menu::new();
    live.refresh([None; DISCOVERY_CAPACITY], tiles, false);
    live.open();
    let canvas = self::canvas(&live);
    let (number, unit) = (canvas.find("250").unwrap(), canvas.find("W").unwrap());
    assert_eq!(number.color, DARK.power);
    assert_eq!(unit.color, DARK.sub);
    assert_eq!(
        number.top + number.line.face().fit_height(),
        unit.top + unit.line.face().fit_height(),
        "one baseline"
    );
    assert!(number.left < unit.left, "the unit after the number");
    let names: std::vec::Vec<_> = menu
        .rows(List::Sensors)
        .filter_map(|row| match row {
            Row::Saved { sensor, .. } => {
                Some(std::string::String::from(menu.row_name(sensor).as_str()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(names, ["Power 1", "Power 2"], "two of a kind read apart");
}

#[test]
fn ten_saved_sensors_scroll_so_the_focus_stays_in_view() {
    let tiles: [_; SAVED_CAPACITY] =
        core::array::from_fn(|n| (identity(n as u8 + 1, n as u16), Connection::Live));
    let mut menu = opened(&[], &tiles);
    let mut rider = Rider::new();
    for step in 0..12 {
        let rect = focused(&menu, &C606);
        let pixel = menu
            .frame(&C606)
            .pixel(rect.left + 1, rect.top.midpoint(rect.bottom));
        assert_eq!(pixel, DARK.focus, "step {step}: ring");
        rider.press(&mut menu, Role::Move);
    }
    rider.focus(&mut menu, None);
    assert!(canvas(&menu).shows("Sensors"));
    let (x, y) = middle(focused(&menu, &C606));
    assert_eq!(tap(&mut menu, &C606, x, y, rider.now), Some(Action::Scan));
}

#[test]
fn eight_nearby_sensors_scroll_so_the_focus_stays_in_view() {
    let found: std::vec::Vec<_> = (0..8).map(|n| peer(100 + n)).collect();
    let mut menu = opened(&found, &[]);
    let mut rider = Rider::new();
    for _ in 0..10 {
        focused(&menu, &C606);
        rider.press(&mut menu, Role::Move);
    }
}

#[test]
fn a_power_meter_page_waits_a_moment_then_act_zeroes_and_move_asks_to_forget() {
    let meter = identity(device_type::POWER, 5512);
    let mut menu = opened(&[], &[(meter, Connection::Live)]);
    let opened_at = 20_000;
    assert_eq!(menu.input(press(Role::Act), opened_at), None);
    assert_eq!(page(&menu), Page::Detail);
    assert!(canvas(&menu).shows("Power meter"));
    assert_eq!(
        cues(&menu),
        [(Role::Act, Icon::Calibrate), (Role::Move, Icon::Trash)]
    );
    assert_eq!(menu.input(press(Role::Act), opened_at + 400), None);
    assert_eq!(menu.input(press(Role::Act), opened_at + 990), None);
    assert_eq!(
        menu.input(press(Role::Act), opened_at + SETTLE_MS + 400),
        Some(Action::Calibrate(meter))
    );
    assert_eq!(page(&menu), Page::Detail);
    let mut rider = Rider {
        now: opened_at + SETTLE_MS + 400,
    };
    assert_eq!(rider.press(&mut menu, Role::Move), None);
    assert_eq!(page(&menu), Page::Forget);
    assert!(canvas(&menu).shows("Forget power meter?"));
    assert!(canvas(&menu).shows("5512 · ANT+"));
    assert_eq!(
        cues(&menu),
        [(Role::Act, Icon::Trash), (Role::Move, Icon::Back)]
    );
    assert_eq!(rider.press(&mut menu, Role::Move), None);
    assert_eq!(page(&menu), Page::Detail, "MOVE goes back");
    rider.press(&mut menu, Role::Move);
    assert_eq!(rider.press(&mut menu, Role::Back), None);
    assert_eq!(page(&menu), Page::Detail, "BACK goes back");
    rider.press(&mut menu, Role::Move);
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Forget(meter))
    );
    assert_eq!(page(&menu), Page::Sensors);
}

#[test]
fn any_other_sensor_page_forgets_from_move_or_act() {
    let mut menu = opened(&[], &[(strap(100), Connection::Live)]);
    let mut rider = Rider::new();
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(cues(&menu), [(Role::Move, Icon::Trash)]);
    assert_eq!(menu.input(press(Role::Act), rider.now + 400), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(rider.press(&mut menu, Role::Move), None);
    assert_eq!(page(&menu), Page::Forget);
    assert!(canvas(&menu).shows("Forget heart rate?"));
    rider.press(&mut menu, Role::Back);
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(page(&menu), Page::Forget);
    assert_eq!(menu.input(press(Role::Act), rider.now + 400), None);
    assert_eq!(page(&menu), Page::Forget);
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Forget(strap(100)))
    );
    menu.refresh([None; DISCOVERY_CAPACITY], [None; SAVED_CAPACITY], false);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, None);
}

#[test]
fn a_sensor_removed_under_its_page_returns_to_sensors() {
    let tiles = [
        (strap(1), Connection::Live),
        (identity(device_type::POWER, 2), Connection::Live),
    ];
    let mut menu = opened(&[], &tiles);
    let mut rider = Rider::new();
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Detail);
    let mut left = saved(&tiles);
    left[0] = None;
    menu.refresh([None; DISCOVERY_CAPACITY], left, false);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, Some(tiles[1].0));
}

#[test]
fn taps_act_on_rows_buttons_titles_and_cues_as_their_keys_do() {
    for device in panels::ALL {
        taps_act_on(device);
    }
}

fn taps_act_on(device: &'static Device) {
    use crate::compose::kit::item::Button;
    use crate::compose::registry::ITEM;
    let meter = identity(device_type::POWER, 2);
    let mut menu = Menu::new();
    menu.refresh(
        nearby(&[peer(10)]),
        saved(&[(strap(1), Connection::Live), (meter, Connection::Live)]),
        false,
    );
    menu.open();
    let button = |menu: &Menu, which: Button| {
        let laid = laid(menu, device);
        let rect = laid
            .tiles()
            .find(|(id, arg, _)| *id == ITEM && *arg == which as u8)
            .map(|(.., placed)| placed.rect.rect())
            .unwrap_or_else(|| panic!("{which:?}"));
        middle(rect)
    };
    let title = |menu: &Menu| {
        let laid = laid(menu, device);
        let rect = laid.box_of(NAV_HEADER).expect("a header to tap");
        (rect.left + 12, rect.top.midpoint(rect.bottom))
    };
    let now = 50_000;
    let tap = |menu: &mut Menu, (x, y): (usize, usize), now| tap(menu, device, x, y, now);
    let second = middle(row(&menu, device, 1).expect("the second row"));
    assert_eq!(tap(&mut menu, second, now), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(menu.diagnostics().focus, Some(meter));
    let view = menu.touch_view();
    let zero = button(&menu, Button::Zero);
    assert_eq!(tap(&mut menu, zero, now + 200), None, "it settles first");
    assert_eq!(
        tap(&mut menu, zero, now + SETTLE_MS),
        Some(Action::Calibrate(meter))
    );
    let facts = laid(&menu, device).box_of(SENSOR_FACTS).expect("the facts");
    assert_eq!(tap(&mut menu, middle(facts), now), None, "facts are none");
    assert_eq!(page(&menu), Page::Detail);
    let forget = button(&menu, Button::Forget);
    assert_eq!(tap(&mut menu, forget, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Forget);
    assert_ne!(menu.touch_view(), view, "a contact never moves pages");
    let keep = button(&menu, Button::Keep);
    assert_eq!(tap(&mut menu, keep, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Detail);
    let back = title(&menu);
    assert_eq!(tap(&mut menu, back, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Sensors);
    let add = middle(row(&menu, device, 2).expect("Add sensor"));
    assert_eq!(tap(&mut menu, add, now), Some(Action::Scan));
    assert_eq!(page(&menu), Page::Nearby);
    let first = middle(row(&menu, device, 0).expect("a found sensor"));
    assert_eq!(
        tap(&mut menu, first, now),
        Some(Action::Connect(peer(10).identity))
    );
    menu.refresh(
        nearby(&[peer(10)]),
        saved(&[(strap(1), Connection::Live), (meter, Connection::Live)]),
        false,
    );
    let back = title(&menu);
    assert_eq!(tap(&mut menu, back, now), Some(Action::StopScan));
    let back = title(&menu);
    assert_eq!(tap(&mut menu, back, now), Some(Action::Ride));
}

#[test]
fn cancel_holds_act_until_move_and_debounce_drops_repeats() {
    let mut menu = opened(&[], &[(strap(1), Connection::Live)]);
    for code in [4, 5] {
        let held = Input::Press {
            role: Role::Act,
            code,
        };
        assert_eq!(menu.input(held, 0), None);
    }
    assert_eq!(page(&menu), Page::Sensors);
    menu.input(Input::Cancel, 400);
    assert!(canvas(&menu).shows("MOVE FOCUS FIRST"));
    assert_eq!(menu.input(press(Role::Act), 800), None);
    assert_eq!(page(&menu), Page::Sensors);
    menu.input(press(Role::Move), 1_200);
    assert!(canvas(&menu).shows("Sensors"));
    assert_eq!(menu.input(press(Role::Act), 1_549), None);
    assert_eq!(page(&menu), Page::Sensors);
    menu.input(Input::Cancel, 2_000);
    assert_eq!(menu.input(press(Role::Back), 2_400), Some(Action::Ride));
}

#[test]
fn equal_device_numbers_show_their_transmission_type() {
    let mut second = peer(10);
    second.identity.transmission_type = 2;
    let menu = opened(&[peer(10), second], &[]);
    assert!(canvas(&menu).shows("ID 10.1 · ANT+"));
    assert!(canvas(&menu).shows("ID 10.2 · ANT+"));
}

#[test]
fn home_glance_lists_selected_sensors_with_their_state() {
    let mut menu = Menu::new();
    assert!(menu.glance().is_empty());
    menu.refresh(
        [None; DISCOVERY_CAPACITY],
        saved(&[(peer(10).identity, Connection::Searching)]),
        false,
    );
    let glance = menu.glance();
    assert_eq!(glance.len(), 1);
    assert_eq!(glance[0].kind, device_type::POWER);
    assert!(!glance[0].live);
    assert_eq!(glance[0].color, DARK.warn);
}

#[test]
fn the_page_shows_the_sensor_its_state_its_battery_as_a_glyph_and_its_id() {
    use crate::compose::registry::SENSOR_HEAD;
    let cases = [
        (Some(BatteryStatus::Good), Some(2_898), Some(DARK.text)),
        (Some(BatteryStatus::Low), Some(2_449), Some(DARK.warn)),
        (Some(BatteryStatus::Critical), None, Some(DARK.bad)),
        (None, None, None),
    ];
    for (status, millivolts, fill) in cases {
        let mut tiles = saved(&[(strap(100), Connection::Live)]);
        tiles[0].as_mut().unwrap().battery = status.map(|status| Battery {
            status,
            millivolts,
            identifier: 0,
        });
        tiles[0].as_mut().unwrap().product.manufacturer = Some(123);
        let mut menu = Menu::new();
        menu.refresh([None; DISCOVERY_CAPACITY], tiles, false);
        menu.open();
        Rider::new().press(&mut menu, Role::Act);
        assert_eq!(page(&menu), Page::Detail);
        let canvas = canvas(&menu);
        let header = run(&menu, "Heart rate").expect("the header names the sensor");
        let head = laid(&menu, &C606).box_of(SENSOR_HEAD).expect("the head");
        assert!(header.top < head.top, "the name is in the header");
        assert_eq!(run(&menu, "Live").map(|r| r.color), Some(DARK.good));
        assert_eq!(run(&menu, "--").map(|r| r.color), Some(DARK.sub));
        assert!(canvas.shows("ID 100 · ANT+"));
        for gone in ["Polar", "Status", "Battery", "Link", "2.9 V"] {
            assert!(!canvas.shows(gone), "{gone}");
        }
        let bars = |color: u16| {
            canvas
                .shapes
                .iter()
                .filter(|shape| matches!(shape, Shape::Bar { color: c, .. } if *c == color))
                .count()
        };
        match fill {
            Some(fill) => {
                assert_eq!(bars(DARK.sub), 2, "the glyph's outline and nub");
                assert!(bars(fill) >= 1, "{status:?} fills it in its tone");
            }
            None => assert_eq!(bars(DARK.sub), 0, "no battery reported, no glyph"),
        }
    }
}

#[test]
fn calibration_notes_take_the_status_row_only_on_a_power_meter() {
    let notes = [
        (CalibrationNote::Calibrating, "Zeroing", DARK.info),
        (CalibrationNote::Done(512), "Zeroed, offset 512", DARK.good),
        (
            CalibrationNote::Done(i16::MIN),
            "Zeroed, offset -32768",
            DARK.good,
        ),
        (
            CalibrationNote::WaitingForRadio,
            "Waiting for radio",
            DARK.info,
        ),
        (CalibrationNote::Failed, "Keep cranks still", DARK.warn),
        (CalibrationNote::NeedsRestart, "Restart to zero", DARK.warn),
        (CalibrationNote::NotConnected, "Not connected", DARK.warn),
        (CalibrationNote::WhileRiding, "Not during a ride", DARK.warn),
    ];
    let meter = identity(device_type::POWER, 5512);
    for (note, text, color) in notes {
        let mut menu = opened(&[], &[(meter, Connection::Live)]);
        Rider::new().press(&mut menu, Role::Act);
        menu.set_calibration(menu.shown().map(|s| (s.identity, note)));
        assert_eq!(note.text().as_str(), text);
        assert_eq!(run(&menu, text).map(|r| r.color), Some(color), "{text}");
        assert!(!canvas(&menu).shows("Live"), "{text} stands for the state");
    }
    let mut strap_menu = opened(&[], &[(strap(1), Connection::Live)]);
    Rider::new().press(&mut strap_menu, Role::Act);
    strap_menu.set_calibration(
        strap_menu
            .shown()
            .map(|s| (s.identity, CalibrationNote::Calibrating)),
    );
    assert!(!canvas(&strap_menu).shows("Zeroing"));
}

#[test]
fn the_day_look_writes_words_black_and_keeps_state_colours() {
    let menu = opened(&[], &[(strap(1), Connection::Live)]);
    let mut view = menu.frame(&C606);
    view.light = true;
    view.prepare();
    let canvas = view.canvas();
    let color = |text: &str| canvas.find(text).map(|run| run.color);
    assert_eq!(color("Sensors"), Some(LIGHT.text));
    assert_eq!(color("Heart rate"), Some(LIGHT.text));
    assert_eq!(color("Live"), Some(LIGHT.good));
    assert_eq!(color("Add sensor"), Some(LIGHT.good));
    assert_eq!(view.pixel(2, 300), LIGHT.bg, "the page fills in its look");
}

#[track_caller]
fn assert_clear_of_cues_on(menu: &Menu, device: &'static Device, what: &str) {
    let canvas = canvas_on(menu, device);
    let texts = canvas
        .runs
        .iter()
        .filter_map(crate::screens::chrome::layout::text)
        .map(|(_, rect)| rect);
    let boxes: std::vec::Vec<Rect> = texts
        .chain(canvas.shapes.iter().map(Shape::bounds))
        .collect();
    for cue in cues_on(menu, device) {
        for rect in &boxes {
            assert!(!cue.covers(*rect, device), "{what}: {rect:?} on {cue:?}");
        }
    }
}

pub(in crate::screens) fn crowded() -> std::vec::Vec<(std::string::String, Menu)> {
    let kinds = [
        device_type::POWER,
        device_type::BIKE_LIGHTS,
        device_type::HEART_RATE,
        device_type::SPEED_CADENCE,
        device_type::FITNESS_EQUIPMENT,
        device_type::SHIFTING,
        device_type::RADAR,
        device_type::CADENCE,
        device_type::SPEED,
        7,
    ];
    let connections = [
        Connection::RadioUnavailable,
        Connection::WaitingForRadio,
        Connection::Live,
        Connection::NotFound,
        Connection::WaitingForData,
        Connection::Unsupported,
        Connection::DataLost,
        Connection::Searching,
        // The speed sensor, live, shows the widest speed.
        Connection::Live,
        Connection::NotInRange,
    ];
    let mut tiles = saved(&[]);
    for (slot, &kind) in kinds.iter().enumerate() {
        let identity = Identity {
            device_type: kind,
            device_number: u16::MAX - slot as u16,
            transmission_type: 255,
        };
        tiles[slot] = Some(Selected {
            identity,
            connection: connections[slot % connections.len()],
            battery: Some(Battery {
                status: [BatteryStatus::Critical, BatteryStatus::Good][slot % 2],
                millivolts: (slot % 3 != 0).then_some(3_950),
                identifier: 0,
            }),
            product: crate::sensors::ant::Product {
                manufacturer: Some([263, 289, 115][slot % 3]),
                model: None,
            },
            reading: None,
        });
        let tile = tiles[slot].as_mut().unwrap();
        if tile.connection == Connection::Live {
            tile.reading = Some(i32::MAX);
        }
    }
    let found: [_; DISCOVERY_CAPACITY] = core::array::from_fn(|n| {
        let mut discovery = peer(u16::MAX - n as u16);
        discovery.identity.device_type = kinds[n];
        discovery.identity.transmission_type = 254;
        Some(discovery)
    });
    let mut states = std::vec::Vec::new();
    let mut rider = Rider::new();
    let mut menu = Menu::new();
    menu.refresh(found, tiles, true);
    menu.open();
    menu.set_message(MenuMessage::Scanning);
    for step in 0..11 {
        states.push((std::format!("sensors {step}"), menu_copy(&menu)));
        if let View::Mine(Mine::Saved(_)) = menu.state.view {
            rider.press(&mut menu, Role::Act);
            states.push((std::format!("detail {step}"), menu_copy(&menu)));
            let failed = CalibrationNote::Done(i16::MIN);
            menu.set_calibration(menu.shown().map(|s| (s.identity, failed)));
            states.push((std::format!("detail, zeroed {step}"), menu_copy(&menu)));
            menu.set_calibration(None);
            rider.press(&mut menu, Role::Move);
            states.push((std::format!("forget {step}"), menu_copy(&menu)));
            rider.press(&mut menu, Role::Back);
            rider.press(&mut menu, Role::Back);
        }
        rider.press(&mut menu, Role::Move);
    }
    let mut add = Menu::new();
    add.refresh(found, [None; SAVED_CAPACITY], true);
    add.open();
    add.set_message(MenuMessage::SavedUnreadable);
    for step in 0..9 {
        states.push((std::format!("add {step}"), menu_copy(&add)));
        rider.press(&mut add, Role::Move);
    }
    let found_all = found;
    let mut pairing = Menu::new();
    pairing.refresh(found, [None; SAVED_CAPACITY], true);
    pairing.open();
    let mut paired = saved(&[]);
    for (slot, found) in found.iter().flatten().take(5).enumerate() {
        paired[slot] = Some(Selected {
            identity: found.identity,
            connection: Connection::Live,
            battery: None,
            product: Default::default(),
            reading: None,
        });
        pairing.refresh(found_all, paired, true);
        pairing.added(found.identity);
        states.push((std::format!("paired {slot}"), menu_copy(&pairing)));
    }
    states
}

/// The same menu, prepared afresh, so a test may keep each state.
fn menu_copy(menu: &Menu) -> Menu {
    let mut copy = Menu::new();
    copy.state.discoveries = menu.state.discoveries;
    copy.state.present = menu.state.present;
    copy.state.selected = menu.state.selected;
    copy.state.view = menu.state.view;
    copy.state.scanning = menu.state.scanning;
    copy.state.message = menu.state.message;
    copy.state.notice = menu.state.notice;
    copy.state.calibration = menu.state.calibration;
    copy.state.added = menu.state.added.clone();
    copy
}

#[test]
fn crowded_sensor_pages_never_overlap_clip_or_drop_anything() {
    crate::screens::check::enable(true);
    for device in panels::ALL {
        for (what, menu) in crowded() {
            let canvas = canvas_on(&menu, device);
            let want: &[&str] = match (what.as_str(), menu.state.view) {
                ("sensors 0", _) => &["Sensors", "Power", "Lights", "Radio unavailable"],
                ("add 0", _) => &["NOT SAVED", "Power", "Lights", "ID 65535 · ANT+"],
                (_, View::Detail(_)) => &[],
                (_, View::Forget(_)) => &["FORGET", "Sensor"],
                _ => &[],
            };
            for text in want {
                let shown: std::vec::Vec<_> = canvas
                    .runs
                    .iter()
                    .map(|run| std::string::String::from_utf8_lossy(run.line.text()).into_owned())
                    .collect();
                assert!(
                    canvas.shows(text),
                    "{what} on {:?}: {text} in {shown:?}",
                    device.scaled.panel
                );
            }
            if let View::Detail(_) = menu.state.view {
                let id = canvas
                    .runs
                    .iter()
                    .any(|run| run.line.text().starts_with(b"ID "));
                assert!(id, "{what} on {:?}: its ID", device.scaled.panel);
            }
            menu.frame(device).prepare().check_layout();
            canvas.assert_laid_out_at(&device.scaled, device.scaled.screen(), want, &what);
            assert_clear_of_cues_on(&menu, device, &what);
            if let View::Mine(_) | View::Nearby(Some(_)) = menu.state.view {
                focused(&menu, device);
            }
        }
    }
}

#[test]
fn painted_rows_match_the_per_pixel_reference() {
    for device in panels::ALL {
        for (what, menu) in crowded() {
            let mut frame = menu.frame(device);
            let scene = frame.prepare();
            let panel = device.scaled.panel;
            for y in 0..panel.height {
                for x in 0..panel.width {
                    let (got, want) = (scene.pixel(x, y), scene.reference_pixel(x, y).0);
                    assert_eq!(got, want, "{what} at {x},{y}: {got:#06x} != {want:#06x}");
                }
            }
        }
    }
}
