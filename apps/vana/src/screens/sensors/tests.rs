use super::menu::{Mine, SETTLE_MS, View};
use super::render::{AMBER, CYAN, GREEN};
use super::*;
use crate::compose::Flags;
use crate::compose::builtin::{Choices, Laid};
use crate::screens::canvas::{Canvas, Shape};
use crate::screens::chrome::{ACCENT, RED, Rect, SECONDARY, WHITE};
use crate::screens::cue::Cue;
use crate::screens::cue::RADIUS;
use crate::screens::{BASE, Layout, panels};
use crate::sensors::ant::{Battery, BatteryStatus, device_type};
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity};
use device_api::input::Role;
use firmware_shell::input::Input;
use firmware_shell::rendering::font::FaceRole;

const SCREEN: Rect = BASE.scaled.screen();

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

fn nearby(peers: &[Discovery]) -> [Option<Discovery>; DISCOVERY_CAPACITY] {
    core::array::from_fn(|index| peers.get(index).copied())
}

fn saved(tiles: &[(Identity, Connection)]) -> [Option<Selected>; CHANNEL_CAPACITY] {
    core::array::from_fn(|slot| {
        tiles.get(slot).map(|&(identity, connection)| Selected {
            identity,
            connection,
            battery: None,
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
    canvas_on(menu, &BASE)
}

fn canvas_on(menu: &Menu, layout: &'static Layout) -> Canvas {
    let mut view = menu.frame(layout);
    view.prepare();
    view.canvas().clone()
}

fn cues(menu: &Menu) -> std::vec::Vec<Cue> {
    cues_on(menu, &BASE)
}

fn cues_on(menu: &Menu, layout: &'static Layout) -> std::vec::Vec<Cue> {
    menu.frame(layout).prepare().cues().to_vec()
}

fn laid(menu: &Menu, layout: &'static Layout) -> Laid {
    let laid = Laid::new(menu.screen(), layout, Choices::default(), Flags::NONE, None);
    laid.expect("each sensor page has a tree")
}

fn tap(menu: &mut Menu, layout: &'static Layout, x: usize, y: usize, now: u64) -> Option<Action> {
    let laid = laid(menu, layout);
    menu.tap_at(&laid, layout, x, y, now)
}

/// The screen rows card `index` of the list shown covers, scrolled as it
/// shows, and whether it shows whole.
fn card(menu: &Menu, layout: &'static Layout, index: usize) -> (Rect, bool) {
    let laid = laid(menu, layout);
    let page = laid.page().expect("a list");
    let offset = menu.offset(&laid);
    let (top, bottom) = page.span(index);
    let shift = page.top as i32 - offset;
    let rect = Rect {
        top: (top + shift).max(0) as usize,
        bottom: (bottom + shift).max(0) as usize,
        left: page.column.left,
        right: page.column.right,
    };
    (rect, page.whole(index, offset))
}

/// The focused card of the list shown.
fn focused(menu: &Menu, layout: &'static Layout) -> (Rect, bool) {
    let list = match menu.screen() {
        crate::compose::builtin::ScreenId::SensorsNearby => crate::compose::data::ListKind::Nearby,
        _ => crate::compose::data::ListKind::Sensors,
    };
    let index = menu.focus(list).expect("a focused card");
    card(menu, layout, usize::from(index))
}

#[test]
fn opening_with_nothing_saved_goes_straight_to_nearby_and_searches() {
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), [None; CHANNEL_CAPACITY], false);
    assert_eq!(menu.open(), Some(Action::Scan));
    assert_eq!(page(&menu), Page::Nearby);
    let title = run(&menu, "NEARBY").expect("NEARBY titles the page");
    assert!(core::ptr::eq(
        title.line.face(),
        BASE.scaled.face(FaceRole::Title)
    ));
    assert!(!canvas(&menu).shows("ADD SENSOR"));
    let mut menu = Menu::new();
    menu.refresh(
        nearby(&[peer(10)]),
        saved(&[(strap(1), Connection::Live)]),
        false,
    );
    assert_eq!(menu.open(), None);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, Some(strap(1)));
    let sensors = run(&menu, "SENSORS").expect("SENSORS titles the page");
    assert_eq!((sensors.left, sensors.top), (title.left, title.top));
    assert!(core::ptr::eq(
        sensors.line.face(),
        BASE.scaled.face(FaceRole::Title)
    ));
    assert!(canvas(&menu).shows("HEART RATE"));
    assert!(canvas(&menu).shows("ADD SENSOR"));
}

#[test]
fn only_nearby_searches() {
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
    let refresh = canvas(&menu)
        .shapes
        .iter()
        .find_map(|shape| match *shape {
            Shape::Icon {
                icon, left, top, ..
            } if core::ptr::eq(icon.0, &crate::screens::icons::REFRESH) => {
                Some((left + 9, top + 9))
            }
            _ => None,
        })
        .expect("the refresh mark");
    assert_eq!(
        tap(&mut menu, &BASE, refresh.0, refresh.1, rider.now),
        Some(Action::Scan)
    );
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

/// NEARBY's line under its title, as text.
fn search_line(menu: &Menu) -> (std::string::String, u16) {
    let (text, color) = menu.search_line();
    (text.as_str().into(), color)
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
    assert_eq!(search_line(&menu), ("SPEED ADDED".into(), GREEN));
    assert!(!canvas(&menu).shows("ID 10"));
    assert_eq!(menu.diagnostics().focus, Some(cadence));
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Connect(cadence))
    );
    tiles.push((cadence, Connection::WaitingForRadio));
    menu.refresh(nearby(&found), saved(&tiles), false);
    menu.added(cadence);
    assert_eq!(search_line(&menu), ("CADENCE ADDED".into(), GREEN));
    assert_eq!(menu.diagnostics().focus, Some(radar));
    menu.added(radar);
    assert_eq!(search_line(&menu), ("CADENCE ADDED".into(), GREEN));
    menu.set_message(MenuMessage::ScanFailed);
    assert_eq!(search_line(&menu).1, AMBER);
    assert_eq!(rider.press(&mut menu, Role::Back), Some(Action::StopScan));
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, Some(cadence));
    rider.focus(&mut menu, None);
    assert_eq!(rider.press(&mut menu, Role::Act), Some(Action::Scan));
    assert_eq!(search_line(&menu).0, "FIND NEARBY");
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
fn a_sensor_of_a_saved_type_asks_before_it_replaces() {
    let current = identity(device_type::POWER, 1);
    let mut menu = opened(&[peer(20)], &[(current, Connection::Live)]);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(page(&menu), Page::Replace);
    assert!(canvas(&menu).shows("REPLACE SENSOR"));
    assert!(canvas(&menu).shows("1") && canvas(&menu).shows("20"));
    assert_eq!(rider.press(&mut menu, Role::Move), None);
    assert_eq!(page(&menu), Page::Replace);
    assert_eq!(rider.press(&mut menu, Role::Back), None);
    assert_eq!(page(&menu), Page::Nearby);
    assert_eq!(menu.selected_identity(device_type::POWER), Some(current));
    rider.press(&mut menu, Role::Act);
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Connect(peer(20).identity))
    );
}

#[test]
fn full_slots_and_stale_discoveries_say_why_nothing_connects() {
    let tiles: [_; CHANNEL_CAPACITY] =
        core::array::from_fn(|n| (identity(n as u8 + 20, n as u16), Connection::Live));
    let mut menu = opened(&[peer(10)], &tiles);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(menu.diagnostics().message, Some(MenuMessage::SlotsFull));
    assert_eq!(search_line(&menu), ("SENSOR SLOTS FULL".into(), AMBER));
    let mut menu = opened(&[peer(10)], &[]);
    menu.refresh([None; DISCOVERY_CAPACITY], [None; CHANNEL_CAPACITY], false);
    assert!(canvas(&menu).shows("NOT SEEN"));
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(menu.diagnostics().message, Some(MenuMessage::NotSeen));
}

#[test]
fn the_search_line_says_how_the_search_goes() {
    let mut menu = opened(&[], &[]);
    let line = |menu: &Menu| {
        let (text, color) = menu.search_line();
        (text.as_bytes().to_vec(), color)
    };
    assert_eq!(line(&menu), (b"FIND NEARBY".to_vec(), SECONDARY));
    menu.refresh([None; DISCOVERY_CAPACITY], [None; CHANNEL_CAPACITY], true);
    menu.set_message(MenuMessage::Scanning);
    assert_eq!(line(&menu), (b"SEARCHING...".to_vec(), ACCENT));
    menu.refresh([None; DISCOVERY_CAPACITY], [None; CHANNEL_CAPACITY], false);
    menu.set_message(MenuMessage::ScanDone);
    assert_eq!(line(&menu), (b"NO SENSORS FOUND".to_vec(), SECONDARY));
    menu.refresh(nearby(&[peer(1)]), [None; CHANNEL_CAPACITY], false);
    assert_eq!(line(&menu), (b"SCAN DONE - PICK".to_vec(), SECONDARY));
    menu.set_message(MenuMessage::ScanFailed);
    assert_eq!(line(&menu), (b"SCAN FAILED - RETRY".to_vec(), AMBER));
    menu.set_notice(Some(MenuMessage::SavedUnreadable));
    menu.set_message(MenuMessage::Scanning);
    assert_eq!(line(&menu), (b"NOT SAVED".to_vec(), AMBER));
}

#[test]
fn sensors_names_trouble_with_the_radio_or_saving_in_its_title() {
    let mut menu = opened(&[], &[(strap(1), Connection::Live)]);
    menu.set_message(MenuMessage::ScanFailed);
    assert_eq!(run(&menu, "SENSORS").map(|r| r.color), Some(WHITE));
    menu.set_message(MenuMessage::RadioUnavailable);
    assert_eq!(
        run(&menu, "RADIO UNAVAILABLE").map(|r| r.color),
        Some(AMBER)
    );
    menu.set_notice(Some(MenuMessage::NotSaved));
    assert_eq!(run(&menu, "NOT SAVED").map(|r| r.color), Some(AMBER));
}

#[test]
fn rows_name_each_state_in_its_colour_and_a_battery_to_change_after_it() {
    let cases = [
        (Connection::Live, "LIVE", GREEN),
        (Connection::Searching, "SEARCHING", CYAN),
        (Connection::NotFound, "NOT FOUND YET", AMBER),
        (Connection::WaitingForRadio, "WAITING FOR RADIO", CYAN),
        (Connection::Replacing, "REPLACING", CYAN),
        (Connection::DataLost, "DATA LOST", AMBER),
        (Connection::WaitingForData, "WAITING FOR DATA", AMBER),
        (Connection::RadioUnavailable, "RADIO UNAVAILABLE", AMBER),
        (Connection::Unsupported, "TYPE UNSUPPORTED", AMBER),
    ];
    for (connection, text, color) in cases {
        let menu = opened(&[], &[(strap(1), connection)]);
        assert_eq!(run(&menu, text).map(|r| r.color), Some(color), "{text}");
    }
    let batteries = [
        (BatteryStatus::New, Some(3_100), None, Some(GREEN)),
        (BatteryStatus::Good, Some(2_949), None, Some(GREEN)),
        (BatteryStatus::Ok, None, None, Some(GREEN)),
        (
            BatteryStatus::Low,
            Some(2_400),
            Some(("BATTERY LOW", AMBER)),
            Some(AMBER),
        ),
        (
            BatteryStatus::Critical,
            Some(2_100),
            Some(("BATTERY CRITICAL", RED)),
            None,
        ),
    ];
    for (status, millivolts, warning, live) in batteries {
        let mut tiles = saved(&[(strap(1), Connection::Live)]);
        tiles[0].as_mut().unwrap().battery = Some(Battery {
            status,
            millivolts,
            identifier: 0,
        });
        let mut menu = Menu::new();
        menu.refresh([None; DISCOVERY_CAPACITY], tiles, false);
        menu.open();
        let state = run(&menu, "LIVE");
        assert_eq!(state.map(|state| state.color), live, "{status:?}");
        let canvas = canvas(&menu);
        let dots = canvas.shapes.iter();
        let dots = dots.filter(|s| matches!(s, Shape::Disc { .. })).count();
        let Some((text, color)) = warning else {
            assert_eq!(dots, 0, "{status:?} adds no dot");
            let texts = canvas.runs.iter().map(|run| run.line.text());
            let names = [&b"SENSORS"[..], b"HEART RATE", b"LIVE", b"ADD SENSOR"];
            assert!(texts.eq(names), "{status:?} adds nothing");
            continue;
        };
        let battery = run(&menu, text).unwrap_or_else(|| panic!("{text}"));
        assert_eq!(battery.color, color, "{text}");
        if let Some(state) = state {
            assert!(battery.left > state.right(), "{text} follows the state");
            assert_eq!(dots, 1, "a dot between them");
        }
    }
    let mut tiles = saved(&[(strap(1), Connection::WaitingForRadio)]);
    tiles[0].as_mut().unwrap().battery = Some(Battery {
        status: BatteryStatus::Critical,
        millivolts: None,
        identifier: 0,
    });
    let mut menu = Menu::new();
    menu.refresh([None; DISCOVERY_CAPACITY], tiles, false);
    menu.open();
    assert!(run(&menu, "WAITING FOR RADIO").is_some());
    assert!(run(&menu, "BATTERY CRITICAL").is_none());
}

#[test]
fn ten_saved_sensors_scroll_so_the_focus_stays_in_view() {
    let tiles: [_; CHANNEL_CAPACITY] =
        core::array::from_fn(|n| (identity(n as u8 + 1, n as u16), Connection::Live));
    let mut menu = opened(&[], &tiles);
    let mut rider = Rider::new();
    let ring = BASE.scaled.unit(3);
    for step in 0..12 {
        let (rect, whole) = focused(&menu, &BASE);
        assert!(whole, "step {step}: {rect:?}");
        let middle = (rect.top + rect.bottom) / 2;
        let pixel = menu.frame(&BASE).pixel(rect.left - ring / 2 - 1, middle);
        assert_eq!(pixel, ACCENT, "step {step}: ring");
        rider.press(&mut menu, Role::Move);
    }
    rider.focus(&mut menu, None);
    assert!(canvas(&menu).shows("SENSORS"));
    let (add, _) = focused(&menu, &BASE);
    let middle = (add.top + add.bottom) / 2;
    assert_eq!(
        tap(&mut menu, &BASE, 120, middle, rider.now),
        Some(Action::Scan)
    );
}

#[test]
fn eight_nearby_sensors_scroll_so_the_focus_stays_in_view() {
    let found: std::vec::Vec<_> = (0..8).map(|n| peer(100 + n)).collect();
    let mut menu = opened(&found, &[]);
    let mut rider = Rider::new();
    for step in 0..10 {
        let (rect, whole) = focused(&menu, &BASE);
        assert!(whole, "step {step}: {rect:?}");
        rider.press(&mut menu, Role::Move);
    }
}

#[test]
fn a_power_meter_detail_waits_a_moment_then_act_zeroes_and_move_asks_to_forget() {
    let meter = identity(device_type::POWER, 5512);
    let mut menu = opened(&[], &[(meter, Connection::Live)]);
    let opened_at = 20_000;
    assert_eq!(menu.input(press(Role::Act), opened_at), None);
    assert_eq!(page(&menu), Page::Detail);
    let cues: std::vec::Vec<_> = cues(&menu).iter().map(|c| (c.role, c.icon)).collect();
    use crate::screens::cue::Icon;
    assert_eq!(
        cues,
        [
            (Role::Act, Icon::Calibrate),
            (Role::Back, Icon::Back),
            (Role::Move, Icon::Trash)
        ]
    );
    assert_eq!(menu.input(press(Role::Act), opened_at + 400), None);
    assert_eq!(menu.input(press(Role::Act), opened_at + 990), None);
    assert_eq!(
        menu.input(press(Role::Act), opened_at + SETTLE_MS + 400),
        Some(Action::Calibrate)
    );
    assert_eq!(page(&menu), Page::Detail);
    let mut rider = Rider {
        now: opened_at + SETTLE_MS + 400,
    };
    assert_eq!(rider.press(&mut menu, Role::Move), None);
    assert_eq!(page(&menu), Page::Forget);
    assert!(canvas(&menu).shows("FORGET") && canvas(&menu).shows("SENSOR?"));
    assert!(canvas(&menu).shows("POWER METER 5512"));
    assert_eq!(rider.press(&mut menu, Role::Move), None);
    assert_eq!(rider.press(&mut menu, Role::Back), None);
    assert_eq!(page(&menu), Page::Detail);
    rider.press(&mut menu, Role::Move);
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Disconnect(device_type::POWER))
    );
    assert_eq!(page(&menu), Page::Sensors);
}

#[test]
fn any_other_sensor_detail_forgets_on_act_and_ignores_move() {
    let mut menu = opened(&[], &[(strap(100), Connection::Live)]);
    let mut rider = Rider::new();
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Detail);
    let roles: std::vec::Vec<_> = cues(&menu).iter().map(|c| c.role).collect();
    assert_eq!(roles, [Role::Act, Role::Back]);
    assert_eq!(menu.input(press(Role::Act), rider.now + 400), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(rider.press(&mut menu, Role::Move), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(rider.press(&mut menu, Role::Act), None);
    assert_eq!(page(&menu), Page::Forget);
    assert!(canvas(&menu).shows("HEART RATE 100"));
    assert_eq!(menu.input(press(Role::Act), rider.now + 400), None);
    assert_eq!(page(&menu), Page::Forget);
    assert_eq!(
        rider.press(&mut menu, Role::Act),
        Some(Action::Disconnect(device_type::HEART_RATE))
    );
    menu.refresh([None; DISCOVERY_CAPACITY], [None; CHANNEL_CAPACITY], false);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, None);
}

#[test]
fn a_sensor_removed_under_its_detail_page_returns_to_sensors() {
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
fn taps_act_on_rows_and_cues_as_their_buttons_do() {
    for layout in panels::ALL {
        taps_act_on(layout);
    }
}

fn taps_act_on(layout: &'static Layout) {
    let meter = identity(device_type::POWER, 2);
    let mut menu = Menu::new();
    menu.refresh(
        nearby(&[peer(10)]),
        saved(&[(strap(1), Connection::Live), (meter, Connection::Live)]),
        false,
    );
    menu.open();
    let panel = layout.scaled.panel;
    let (middle, right, bottom) = (panel.width / 2, panel.width - 8, panel.height - 8);
    let row = |menu: &Menu, index: usize| {
        let (rect, _) = card(menu, layout, index);
        (rect.top + rect.bottom) / 2
    };
    let now = 50_000;
    let tap = |menu: &mut Menu, x, y, now| tap(menu, layout, x, y, now);
    assert_eq!(
        tap(&mut menu, middle, 2, now),
        None,
        "the status bar is no target"
    );
    let second = row(&menu, 1);
    assert_eq!(tap(&mut menu, middle, second, now), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(menu.diagnostics().focus, Some(meter));
    let view = menu.touch_view();
    assert_eq!(tap(&mut menu, right, bottom, now + 200), None);
    assert_eq!(
        tap(&mut menu, right, bottom, now + SETTLE_MS),
        Some(Action::Calibrate)
    );
    let card = laid(&menu, layout)
        .box_of(crate::compose::registry::SENSOR_ICON)
        .expect("the icon card");
    assert_eq!(
        tap(&mut menu, middle, card.bottom - 1, now + SETTLE_MS),
        None,
        "content is no target"
    );
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(tap(&mut menu, 8, bottom, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Forget);
    assert_ne!(
        menu.touch_view(),
        view,
        "a contact never moves between pages"
    );
    assert_eq!(tap(&mut menu, 8, 8, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(tap(&mut menu, 8, 8, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Sensors);
    let third = row(&menu, 2);
    assert_eq!(tap(&mut menu, middle, third, now), Some(Action::Scan));
    assert_eq!(page(&menu), Page::Nearby);
    let first = row(&menu, 0);
    assert_eq!(tap(&mut menu, middle, first, now), None);
    assert_eq!(page(&menu), Page::Replace);
    assert_eq!(
        tap(&mut menu, right, bottom, now),
        Some(Action::Connect(peer(10).identity))
    );
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
    assert!(canvas(&menu).shows("SENSORS"));
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
    assert!(canvas(&menu).shows("ID 10.1"));
    assert!(canvas(&menu).shows("ID 10.2"));
}

#[test]
fn replacement_tells_maximum_identities_apart_without_overlap() {
    let current = Identity {
        device_type: device_type::POWER,
        device_number: u16::MAX,
        transmission_type: 254,
    };
    let mut discovery = peer(u16::MAX);
    discovery.identity.transmission_type = 255;
    let mut menu = opened(&[discovery], &[(current, Connection::NotFound)]);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Replace);
    let canvas = canvas(&menu);
    canvas.assert_laid_out(
        SCREEN,
        &["65535", "T254", "T255", "CURRENT", "NEW"],
        "replace",
    );
    assert_clear_of_cues(&menu, "replace");
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
    assert_eq!(glance[0].color, CYAN);
}

#[test]
fn the_detail_page_shows_the_battery_in_the_colour_of_its_urgency() {
    let cases = [
        (Some(BatteryStatus::Good), Some(2_898), "2.9", WHITE),
        (Some(BatteryStatus::Low), Some(2_449), "2.4", AMBER),
        (Some(BatteryStatus::Critical), Some(2_100), "2.1", RED),
        (Some(BatteryStatus::Low), None, "LOW", AMBER),
        (None, None, "--", WHITE),
    ];
    for (status, millivolts, text, color) in cases {
        let mut tiles = saved(&[(strap(100), Connection::Live)]);
        tiles[0].as_mut().unwrap().battery = status.map(|status| Battery {
            status,
            millivolts,
            identifier: 0,
        });
        let mut menu = Menu::new();
        menu.refresh([None; DISCOVERY_CAPACITY], tiles, false);
        menu.open();
        Rider::new().press(&mut menu, Role::Act);
        assert_eq!(page(&menu), Page::Detail);
        assert_eq!(run(&menu, text).map(|r| r.color), Some(color), "{text}");
        assert_eq!(run(&menu, "V").is_some(), millivolts.is_some(), "{text}");
        assert!(canvas(&menu).shows("BATTERY") && canvas(&menu).shows("100"));
    }
}

#[test]
fn calibration_notes_show_in_their_colour_only_on_a_power_meter() {
    let notes = [
        (CalibrationNote::Calibrating, "CALIBRATING...", CYAN),
        (CalibrationNote::Done(512), "CALIBRATED OFFSET 512", GREEN),
        (
            CalibrationNote::Done(i16::MIN),
            "CALIBRATED OFFSET -32768",
            GREEN,
        ),
        (CalibrationNote::WaitingForRadio, "WAITING FOR RADIO", CYAN),
        (CalibrationNote::Failed, "CALIBRATION FAILED", AMBER),
        (CalibrationNote::NeedsRestart, "RESTART TO CALIBRATE", AMBER),
        (CalibrationNote::NotConnected, "NOT CONNECTED", AMBER),
        (CalibrationNote::WhileRiding, "NOT DURING A RIDE", AMBER),
    ];
    let meter = identity(device_type::POWER, 5512);
    for (note, text, color) in notes {
        let mut menu = opened(&[], &[(meter, Connection::Live)]);
        Rider::new().press(&mut menu, Role::Act);
        menu.set_calibration(Some(note));
        assert_eq!(note.text().as_str(), text);
        let line = run(&menu, text).unwrap_or_else(|| panic!("{text}"));
        assert_eq!(line.color, color);
        if let Some(hint) = note.hint() {
            let hint = run(&menu, hint).expect("hint");
            assert_eq!(hint.color, SECONDARY);
            assert!(hint.top > line.top, "the hint under the note");
        }
        canvas(&menu).assert_laid_out(SCREEN, &[text], text);
        assert_clear_of_cues(&menu, text);
    }
    let mut strap_menu = opened(&[], &[(strap(1), Connection::Live)]);
    Rider::new().press(&mut strap_menu, Role::Act);
    strap_menu.set_calibration(Some(CalibrationNote::Calibrating));
    assert!(!canvas(&strap_menu).shows("CALIBRATING..."));
}

#[track_caller]
fn assert_clear_of_cues(menu: &Menu, what: &str) {
    assert_clear_of_cues_on(menu, &BASE, what);
}

#[track_caller]
fn assert_clear_of_cues_on(menu: &Menu, layout: &'static Layout, what: &str) {
    let canvas = canvas_on(menu, layout);
    let texts = canvas
        .runs
        .iter()
        .filter_map(crate::screens::chrome::layout::text)
        .map(|(_, rect)| rect);
    let boxes: std::vec::Vec<Rect> = texts
        .chain(canvas.shapes.iter().map(Shape::bounds))
        .collect();
    let panel = layout.scaled.panel;
    for cue in cues_on(menu, layout) {
        let corner = cue.corner(layout);
        let cx = if corner.left() { 0 } else { panel.width };
        let cy = if corner.top() { 0 } else { panel.height };
        for rect in &boxes {
            let dx = cx.clamp(rect.left, rect.right).abs_diff(cx);
            let dy = cy.clamp(rect.top, rect.bottom).abs_diff(cy);
            assert!(
                dx * dx + dy * dy > (RADIUS + 4).pow(2),
                "{what}: {rect:?} near {cue:?}"
            );
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
        Connection::Replacing,
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
        });
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
            menu.set_calibration(Some(CalibrationNote::Failed));
            states.push((std::format!("detail, failed {step}"), menu_copy(&menu)));
            menu.set_calibration(None);
            if menu.state.calibratable(match menu.state.view {
                View::Detail(slot) => slot,
                _ => unreachable!(),
            }) {
                rider.press(&mut menu, Role::Move);
            } else {
                rider.press(&mut menu, Role::Act);
            }
            states.push((std::format!("forget {step}"), menu_copy(&menu)));
            rider.press(&mut menu, Role::Back);
            rider.press(&mut menu, Role::Back);
        }
        rider.press(&mut menu, Role::Move);
    }
    let mut add = Menu::new();
    let fresh: [_; DISCOVERY_CAPACITY] = core::array::from_fn(|n| {
        let mut discovery = peer(u16::MAX - n as u16);
        discovery.identity.device_type = kinds[n];
        discovery.identity.transmission_type = 254;
        Some(discovery)
    });
    add.refresh(fresh, [None; CHANNEL_CAPACITY], true);
    add.open();
    add.set_message(MenuMessage::SavedUnreadable);
    for step in 0..9 {
        states.push((std::format!("add {step}"), menu_copy(&add)));
        rider.press(&mut add, Role::Move);
    }
    let mut replace = Menu::new();
    replace.refresh(
        found,
        saved(&[(identity(kinds[0], 1), Connection::Live)]),
        false,
    );
    replace.open();
    rider.focus(&mut replace, None);
    rider.press(&mut replace, Role::Act);
    rider.press(&mut replace, Role::Act);
    assert_eq!(page(&replace), Page::Replace);
    states.push(("replace".into(), replace));
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
    copy
}

#[test]
fn crowded_sensor_pages_never_overlap_clip_or_drop_anything() {
    crate::screens::check::enable(true);
    for layout in panels::ALL {
        for (what, menu) in crowded() {
            let canvas = canvas_on(&menu, layout);
            let want: &[&str] = match (what.as_str(), menu.state.view) {
                ("sensors 0", _) => &["POWER METER", "BIKE LIGHTS", "RADIO UNAVAILABLE"],
                ("add 0", _) => &["NOT SAVED", "POWER METER", "BIKE LIGHTS", "ID 65535"],
                (_, View::Detail(_)) => &["BATTERY", "ID"],
                (_, View::Forget(_)) => &["FORGET", "SENSOR?"],
                (_, View::Replace(_)) => &["REPLACE SENSOR", "CURRENT", "NEW"],
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
                    layout.scaled.panel
                );
            }
            menu.frame(layout).prepare().check_layout();
            match menu.state.view {
                View::Mine(_) | View::Nearby(Some(_)) => {
                    let (rect, whole) = focused(&menu, layout);
                    assert!(whole, "{what}: {rect:?}");
                }
                View::Nearby(None) => {}
                _ => {
                    canvas.assert_laid_out(layout.scaled.screen(), want, &what);
                    assert_clear_of_cues_on(&menu, layout, &what);
                }
            }
        }
    }
}

#[test]
fn painted_rows_match_the_per_pixel_reference() {
    for layout in panels::ALL {
        for (what, menu) in crowded() {
            let mut frame = menu.frame(layout);
            let scene = frame.prepare();
            let panel = layout.scaled.panel;
            for y in 0..panel.height {
                for x in 0..panel.width {
                    let (got, want) = (scene.pixel(x, y), scene.reference_pixel(x, y).0);
                    assert_eq!(got, want, "{what} at {x},{y}: {got:#06x} != {want:#06x}");
                }
            }
        }
    }
}

/// `nodes` stored as screen `id`'s tree, if accept takes it.
fn stored(
    nodes: &[crate::compose::Node],
    id: crate::compose::builtin::ScreenId,
    layout: &'static Layout,
) -> Option<crate::compose::Screens> {
    let mut bytes = std::vec![
        crate::compose::parse::VERSION,
        1,
        id.key(),
        nodes.len() as u8
    ];
    bytes.extend(nodes.iter().flat_map(|node| node.bytes()));
    bytes.push(0);
    let mut screens = crate::compose::Screens::EMPTY;
    let controls = |key| crate::compose::builtin::ScreenId::controls(key, layout.corners);
    crate::compose::parse(&bytes, &layout.scaled, &controls, &mut screens).ok()?;
    Some(screens)
}

fn stored_laids(
    screens: &crate::compose::Screens,
    menu: &Menu,
    layout: &'static Layout,
) -> crate::compose::builtin::Laids {
    let mut laids =
        crate::compose::builtin::Laids::new(layout, Choices::default(), Flags::NONE, screens);
    laids.show(menu.screen(), layout, screens);
    laids
}

/// The page `menu` shows on `layout`, drawn from `laids`.
fn stored_canvas(
    menu: &Menu,
    laids: &crate::compose::builtin::Laids,
    layout: &'static Layout,
) -> Canvas {
    let mut view = menu.frame(layout);
    view.laids = Some(laids);
    view.prepare();
    view.canvas().clone()
}

#[test]
fn a_tap_acts_on_the_sensor_the_stored_tree_draws() {
    use crate::compose::{Node, Size, data::ListKind, registry::SENSOR_ROW};
    let mut menu = opened(&[peer(10)], &[(strap(1), Connection::Live)]);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Nearby);
    // NEARBY's tree, stored as a scroll of the saved sensors.
    let nodes = [
        Node::scroll(ListKind::Sensors, Size::grow(1)),
        Node::col(1, Size::units(56)),
        Node::leaf(SENSOR_ROW, 0, Size::grow(1)),
    ];
    let screens = stored(&nodes, menu.screen(), &BASE).expect("accepted");
    let laids = stored_laids(&screens, &menu, &BASE);
    let canvas = stored_canvas(&menu, &laids, &BASE);
    assert!(canvas.shows("HEART RATE") && !canvas.shows("POWER METER"));
    let laid = laids.get(menu.screen()).expect("shown");
    let cards = laid.page().expect("a scroll");
    let (top, bottom) = cards.span(0);
    let middle = cards.top + ((top + bottom) / 2) as usize;
    let tapped = menu.tap_at(laid, &BASE, 120, middle, rider.now);
    assert_ne!(tapped, Some(Action::Connect(peer(10).identity)));
    assert_eq!(page(&menu), Page::Detail, "the strap it draws opens");
}

#[test]
fn a_stored_search_line_searches_again_with_no_scroll() {
    use crate::compose::{
        Node, Size,
        registry::{SENSOR_SEARCH, SPACE},
    };
    let mut menu = opened(&[], &[]);
    let nodes = [
        Node::col(3, Size::grow(1)),
        Node::leaf(SPACE, 0, Size::units(48)),
        Node::leaf(SENSOR_SEARCH, 0, Size::CONTENT),
        Node::leaf(SPACE, 0, Size::grow(1)),
    ];
    let screens = stored(&nodes, menu.screen(), &BASE).expect("accepted");
    let laids = stored_laids(&screens, &menu, &BASE);
    assert!(stored_canvas(&menu, &laids, &BASE).shows("FIND NEARBY"));
    let laid = laids.get(menu.screen()).expect("shown");
    let search = laid.box_of(SENSOR_SEARCH).expect("the search line");
    let tapped = menu.tap_at(laid, &BASE, search.right - 4, search.top + 4, 10_000);
    assert_eq!(tapped, Some(Action::Scan));
}

#[test]
fn the_status_bar_covers_no_stored_tile() {
    use crate::compose::{
        Node, Size,
        data::ListKind,
        registry::{SENSOR_ROW, SENSOR_SEARCH},
    };
    let menu = opened(&[], &[]);
    // The search line where the bar would be, and no room kept for it.
    let nodes = [
        Node::col(2, Size::grow(1)),
        Node::leaf(SENSOR_SEARCH, 0, Size::units(22)),
        Node::scroll(ListKind::Nearby, Size::grow(1)),
        Node::col(1, Size::units(56)),
        Node::leaf(SENSOR_ROW, 0, Size::grow(1)),
    ];
    let screens = stored(&nodes, menu.screen(), &BASE).expect("accepted");
    let laids = stored_laids(&screens, &menu, &BASE);
    let laid = laids.get(menu.screen()).expect("shown");
    let search = laid.box_of(SENSOR_SEARCH).expect("the search line");
    assert!(
        search.top < BASE.chrome.status_bar.bottom,
        "it lies in the bar's band"
    );
    let mut view = menu.frame(&BASE);
    view.laids = Some(&laids);
    let look = crate::screens::look::look(false);
    let black = look.ink(crate::screens::chrome::BLACK);
    view.prepare();
    let tile = view.canvas().clone();
    let scene = view.prepare();
    for y in search.top..search.bottom {
        for x in search.left..search.right {
            let drawn = tile.pixel(look, x, y, black).0;
            assert_eq!(scene.pixel(x, y), drawn, "the tile shows at {x},{y}");
        }
    }
}

/// What a sensor tile reads, outside a frame: the menu, and the item of a
/// list it draws.
struct Shown<'m> {
    menu: &'m Menu,
    row: Option<crate::compose::data::Row>,
}

impl crate::compose::data::Data for Shown<'_> {
    fn metric(&self, _: crate::compose::data::Metric) -> Option<i32> {
        None
    }

    fn zone(&self, _: crate::compose::data::Effort) -> Option<u8> {
        None
    }

    fn list(&self, list: crate::compose::data::ListKind) -> u16 {
        self.menu.count(list)
    }

    fn row(&self) -> Option<crate::compose::data::Row> {
        self.row
    }

    fn sensors(&self) -> Option<&Menu> {
        Some(self.menu)
    }
}

/// Each sensor tile draws its widest words whole in every look it offers,
/// on every panel: accept measures each look on them, so no box it takes
/// cuts a name, a state, an identity or an offset.
#[test]
fn every_sensor_tile_draws_its_widest_words_whole_in_each_look() {
    use crate::compose::data::{ListKind, Row};
    use crate::compose::registry::{
        SENSOR_CELLS, SENSOR_LINE, SENSOR_ROW, SENSOR_SEARCH, SENSOR_SWAP, SENSOR_TITLE, tile,
    };
    use crate::compose::tiles::sensor_line::Says;
    use crate::compose::{Placed, Rect16};
    let widest = Identity {
        device_type: device_type::POWER,
        device_number: u16::MAX,
        transmission_type: u8::MAX,
    };
    let mut twin = widest;
    twin.transmission_type = 1;
    let found = |identity| Discovery {
        identity,
        rssi: -40,
        seen_ms: 0,
    };
    // Every case: a menu, the tile and its argument, the row it draws and
    // the words it must show whole.
    let mut cases: std::vec::Vec<(Menu, u8, u8, Option<Row>, std::string::String)> =
        std::vec::Vec::new();
    for message in MenuMessage::ALL {
        let mut menu = opened(&[], &[(strap(1), Connection::Live)]);
        if MenuMessage::NOTICES.contains(&message) {
            menu.set_notice(Some(message));
        } else {
            menu.set_message(message);
        }
        let (title, _) = menu.title();
        cases.push((
            menu,
            SENSOR_TITLE,
            ListKind::Sensors as u8,
            None,
            title.into(),
        ));
        let mut menu = opened(&[], &[]);
        menu.set_notice(Some(message));
        let (line, _) = menu.search_line();
        cases.push((menu, SENSOR_SEARCH, 0, None, line.as_str().into()));
    }
    for (kind, connection) in NAMES.iter().zip(Connection::ALL.iter().cycle()) {
        let device_type = (0..=u8::MAX)
            .find(|&t| super::kind(t) == *kind)
            .expect("a type");
        let identity = Identity {
            device_type,
            device_number: u16::MAX,
            transmission_type: 255,
        };
        let menu = opened(&[], &[(identity, *connection)]);
        let row = Some(Row {
            list: ListKind::Sensors,
            index: 0,
            focused: false,
        });
        let name = std::string::String::from_utf8_lossy(kind).into_owned();
        cases.push((menu_copy(&menu), SENSOR_ROW, 0, row, name));
        let (state, _) = menu.state.selected[0].expect("saved").link();
        let state = std::string::String::from_utf8_lossy(state).into_owned();
        cases.push((menu, SENSOR_ROW, 0, row, state));
    }
    let nearby_row = Some(Row {
        list: ListKind::Nearby,
        index: 0,
        focused: false,
    });
    let menu = opened(&[found(widest), found(twin)], &[]);
    cases.push((menu, SENSOR_ROW, 0, nearby_row, "ID 65535.255".into()));
    for note in [
        CalibrationNote::Done(i16::MIN),
        CalibrationNote::NeedsRestart,
    ] {
        let mut menu = opened(&[found(twin)], &[(widest, Connection::RadioUnavailable)]);
        let mut rider = Rider::new();
        rider.press(&mut menu, Role::Act);
        menu.set_calibration(Some(note));
        let text = note.text().as_str().into();
        cases.push((
            menu_copy(&menu),
            SENSOR_LINE,
            Says::Calibration as u8,
            None,
            text,
        ));
        cases.push((
            menu_copy(&menu),
            SENSOR_LINE,
            Says::State as u8,
            None,
            "RADIO UNAVAILABLE".into(),
        ));
        cases.push((menu_copy(&menu), SENSOR_CELLS, 0, None, "65535.255".into()));
        rider.press(&mut menu, Role::Move);
        assert_eq!(page(&menu), Page::Forget);
        cases.push((
            menu,
            SENSOR_LINE,
            Says::Forgotten as u8,
            None,
            "POWER METER 65535.255".into(),
        ));
    }
    let mut menu = opened(&[found(widest)], &[(twin, Connection::Live)]);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Role::Act);
    rider.press(&mut menu, Role::Act);
    assert_eq!(page(&menu), Page::Replace);
    cases.push((menu_copy(&menu), SENSOR_SWAP, 0, None, "65535".into()));
    cases.push((menu, SENSOR_SWAP, 0, None, "T255".into()));
    let mut cut = std::vec::Vec::new();
    for layout in panels::ALL {
        let at = &layout.scaled;
        for (menu, component, arg, row, want) in &cases {
            let data = Shown { menu, row: *row };
            for (form, look) in tile(*component).forms(*arg, at).iter().enumerate() {
                let placed = Placed {
                    rect: Rect16 {
                        left: 0,
                        top: 0,
                        right: look.width,
                        bottom: look.height,
                    },
                    node: 0,
                    form: form as u8,
                };
                let mut canvas = Canvas::default();
                tile(*component).prepare(*arg, &placed, at, &data, &mut canvas);
                if !canvas.shows(want) {
                    let name = tile(*component).name();
                    cut.push(std::format!("{name} look {form} on {:?}: {want}", at.panel));
                }
            }
        }
    }
    assert!(cut.is_empty(), "{cut:#?}");
}

#[test]
fn a_search_line_in_a_card_is_refused_so_no_tap_on_it_pairs_the_card() {
    use crate::compose::{
        Node, Size,
        data::ListKind,
        registry::{SENSOR_ROW, SENSOR_SEARCH, SPACE},
    };
    let menu = opened(&[peer(10)], &[]);
    assert_eq!(page(&menu), Page::Nearby);
    let nodes = [
        Node::col(3, Size::grow(1)),
        Node::leaf(SPACE, 0, Size::units(48)),
        Node::scroll(ListKind::Nearby, Size::grow(1)),
        Node::col(2, Size::units(88)),
        Node::leaf(SENSOR_SEARCH, 0, Size::units(24)),
        Node::leaf(SENSOR_ROW, 0, Size::grow(1)),
        Node::leaf(SPACE, 0, Size::units(48)),
    ];
    for layout in panels::ALL {
        assert!(
            stored(&nodes, menu.screen(), layout).is_none(),
            "{:?}",
            layout.scaled.panel
        );
    }
}

/// The status bar paints exactly the rows its tile keeps, so a tile right
/// under a status tile of its own size loses none of its pixels.
#[test]
fn the_status_bar_paints_only_the_rows_its_tile_keeps_on_every_panel() {
    use crate::compose::{
        Node, Size,
        registry::{GAUGE, SPACE, STATUS},
    };
    let menu = opened(&[], &[]);
    let nodes = [
        Node::col(3, Size::grow(1)),
        Node::leaf(STATUS, 0, Size::CONTENT),
        Node::leaf(GAUGE, 0, Size::CONTENT),
        Node::leaf(SPACE, 0, Size::grow(1)),
    ];
    for layout in panels::ALL {
        let panel = layout.scaled.panel;
        let screens = stored(&nodes, menu.screen(), layout).expect("accepted");
        let laids = stored_laids(&screens, &menu, layout);
        let laid = laids.get(menu.screen()).expect("shown");
        let (status, gauge) = (laid.box_of(STATUS).unwrap(), laid.box_of(GAUGE).unwrap());
        assert_eq!(status.bottom, gauge.top, "{panel:?}");
        let mut view = menu.frame(layout);
        view.laids = Some(&laids);
        let look = crate::screens::look::look(false);
        let black = look.ink(crate::screens::chrome::BLACK);
        view.prepare();
        let tile = view.canvas().clone();
        let scene = view.prepare();
        let mut drawn = 0;
        for y in gauge.top..gauge.bottom {
            for x in gauge.left..gauge.right {
                let want = tile.pixel(look, x, y, black).0;
                if want != black.0 {
                    drawn += 1;
                    assert_eq!(scene.pixel(x, y), want, "{panel:?}: the gauge at {x},{y}");
                }
            }
        }
        assert!(drawn > 0, "{panel:?}: the gauge draws");
    }
}
