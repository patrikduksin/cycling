use super::layout::{CALIBRATION_BASE, FOCUS_BOTTOM, HINT_BASE, Target};
use super::menu::{Mine, SETTLE_MS, View};
use super::render::{AMBER, CYAN, GREEN};
use super::*;
use crate::screens::canvas::Shape;
use crate::screens::chrome::{ACCENT, CONTENT, HEIGHT, RED, Rect, SECONDARY, TITLE, WHITE, WIDTH};
use crate::screens::cue::{Corner, RADIUS};
use crate::sensors::ant::{Battery, BatteryStatus, device_type};
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity};
use device_api::input::{Button, Input};

const SCREEN: Rect = Rect {
    left: 0,
    top: 0,
    right: WIDTH,
    bottom: HEIGHT,
};

fn press(button: Button) -> Input {
    Input::Button { button, code: 1 }
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

    fn press(&mut self, menu: &mut Menu, button: Button) -> Option<Action> {
        self.now += SETTLE_MS;
        menu.input(press(button), self.now)
    }

    /// MOVE until `kind`'s sensor is focused, or ADD SENSOR's row for `None`.
    fn focus(&mut self, menu: &mut Menu, kind: Option<u8>) {
        for _ in 0..24 {
            if menu.diagnostics().focus.map(|f| f.device_type) == kind {
                return;
            }
            self.press(menu, Button::BottomLeft);
        }
        panic!("{kind:?} is not reachable");
    }
}

fn page(menu: &Menu) -> Page {
    menu.diagnostics().page
}

/// The run reading exactly `text`, if the page shows it.
fn run(menu: &Menu, text: &str) -> Option<crate::screens::canvas::Run> {
    menu.canvas().find(text).copied()
}

#[test]
fn opening_with_nothing_saved_goes_straight_to_nearby_and_searches() {
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), [None; CHANNEL_CAPACITY], false);
    assert_eq!(menu.open(), Some(Action::Scan));
    assert_eq!(page(&menu), Page::Nearby);
    // NEARBY takes the TITLE face SENSORS has, in its place.
    let title = run(&menu, "NEARBY").expect("NEARBY titles the page");
    assert!(core::ptr::eq(title.line.face(), &TITLE));
    assert!(!menu.canvas().shows("ADD SENSOR"));
    // With a sensor saved, SENSORS opens on its first row and nothing searches.
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
    assert!(core::ptr::eq(sensors.line.face(), &TITLE));
    assert!(menu.canvas().shows("HEART RATE"));
    assert!(menu.canvas().shows("ADD SENSOR"));
}

#[test]
fn only_nearby_searches() {
    let tiles = [
        (strap(1), Connection::Live),
        (identity(device_type::POWER, 2), Connection::Searching),
    ];
    let mut menu = opened(&[peer(10)], &tiles);
    let mut rider = Rider::new();
    // Walking SENSORS, opening and leaving its detail pages never searches.
    for _ in 0..6 {
        assert_eq!(rider.press(&mut menu, Button::BottomLeft), None);
    }
    rider.focus(&mut menu, Some(device_type::HEART_RATE));
    assert_eq!(rider.press(&mut menu, Button::BottomRight), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(rider.press(&mut menu, Button::TopLeft), None);
    rider.focus(&mut menu, None);
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Scan)
    );
    assert_eq!(page(&menu), Page::Nearby);
    // On the page, a search in progress is not asked for twice.
    menu.search_started();
    menu.refresh([None; DISCOVERY_CAPACITY], saved(&tiles), true);
    assert_eq!(rider.press(&mut menu, Button::BottomRight), None);
    // Leaving stops it; coming back asks again, even before it has stopped.
    assert_eq!(
        rider.press(&mut menu, Button::TopLeft),
        Some(Action::StopScan)
    );
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Scan)
    );
    // Once it ends, ACT on an empty list or the refresh mark searches again.
    menu.search_started();
    menu.refresh([None; DISCOVERY_CAPACITY], saved(&tiles), false);
    assert_eq!(menu.diagnostics().focus, None);
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Scan)
    );
    let refresh = menu
        .canvas()
        .shapes
        .iter()
        .find_map(|shape| match *shape {
            Shape::Icon {
                icon, left, top, ..
            } if core::ptr::eq(icon, &crate::screens::icons::REFRESH) => Some((left + 9, top + 9)),
            _ => None,
        })
        .expect("the refresh mark");
    assert_eq!(
        menu.tap(refresh.0, refresh.1, rider.now),
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
        rider.press(&mut menu, Button::BottomLeft);
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
    // A detail page returns to its own row.
    rider.focus(&mut menu, Some(POWER));
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(page(&menu), Page::Detail);
    rider.press(&mut menu, Button::TopLeft);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus.map(|f| f.device_type), Some(POWER));
    assert_eq!(rider.press(&mut menu, Button::TopLeft), Some(Action::Ride));
}

/// NEARBY's line under its title, as text.
fn search_line(menu: &Menu) -> (std::string::String, u16) {
    let (text, color) = menu.state.search_line();
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
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Scan)
    );
    menu.search_started();
    menu.refresh(nearby(&found), saved(&tiles), true);
    // The first sensor found takes the focus; MOVE steps through and wraps.
    assert_eq!(menu.diagnostics().focus, Some(speed));
    for want in [cadence, radar, speed] {
        rider.press(&mut menu, Button::BottomLeft);
        assert_eq!(menu.diagnostics().focus, Some(want));
    }
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Connect(speed))
    );
    // The runtime saves it, which stops the search, and says it did.
    tiles.push((speed, Connection::WaitingForRadio));
    menu.refresh(nearby(&found), saved(&tiles), false);
    menu.added(speed);
    menu.set_message(MenuMessage::ScanCancelled);
    assert_eq!(page(&menu), Page::Nearby);
    assert_eq!(search_line(&menu), ("SPEED ADDED".into(), GREEN));
    // It leaves the list; the others stay and connect without a new search.
    assert!(!menu.canvas().shows("ID 10"));
    assert_eq!(menu.diagnostics().focus, Some(cadence));
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Connect(cadence))
    );
    tiles.push((cadence, Connection::WaitingForRadio));
    menu.refresh(nearby(&found), saved(&tiles), false);
    menu.added(cadence);
    assert_eq!(search_line(&menu), ("CADENCE ADDED".into(), GREEN));
    assert_eq!(menu.diagnostics().focus, Some(radar));
    // A sensor that could not be saved is no news; a problem replaces it.
    menu.added(radar);
    assert_eq!(search_line(&menu), ("CADENCE ADDED".into(), GREEN));
    menu.set_message(MenuMessage::ScanFailed);
    assert_eq!(search_line(&menu).1, AMBER);
    // BACK shows SENSORS on the sensor added last.
    assert_eq!(
        rider.press(&mut menu, Button::TopLeft),
        Some(Action::StopScan)
    );
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, Some(cadence));
    // A fresh visit forgets it: BACK returns to the ADD SENSOR row.
    rider.focus(&mut menu, None);
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Scan)
    );
    assert_eq!(search_line(&menu).0, "FIND NEARBY");
    rider.press(&mut menu, Button::TopLeft);
    assert_eq!(menu.diagnostics().focus, None);
}

#[test]
fn back_from_nearby_returns_to_sensors_or_leaves_when_nothing_is_saved() {
    let mut rider = Rider::new();
    let mut menu = opened(&[peer(10)], &[(strap(1), Connection::Live)]);
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(
        rider.press(&mut menu, Button::TopLeft),
        Some(Action::StopScan)
    );
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, None, "back on ADD SENSOR's row");
    let mut empty = opened(&[peer(10)], &[]);
    assert_eq!(page(&empty), Page::Nearby);
    assert_eq!(rider.press(&mut empty, Button::TopLeft), Some(Action::Ride));
}

#[test]
fn a_sensor_of_a_saved_type_asks_before_it_replaces() {
    let current = identity(device_type::POWER, 1);
    let mut menu = opened(&[peer(20)], &[(current, Connection::Live)]);
    let mut rider = Rider::new();
    rider.focus(&mut menu, None);
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(rider.press(&mut menu, Button::BottomRight), None);
    assert_eq!(page(&menu), Page::Replace);
    assert!(menu.canvas().shows("REPLACE SENSOR"));
    assert!(menu.canvas().shows("1") && menu.canvas().shows("20"));
    // MOVE does nothing; BACK keeps the saved one.
    assert_eq!(rider.press(&mut menu, Button::BottomLeft), None);
    assert_eq!(page(&menu), Page::Replace);
    assert_eq!(rider.press(&mut menu, Button::TopLeft), None);
    assert_eq!(page(&menu), Page::Nearby);
    assert_eq!(menu.selected_identity(device_type::POWER), Some(current));
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
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
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(rider.press(&mut menu, Button::BottomRight), None);
    assert_eq!(menu.diagnostics().message, Some(MenuMessage::SlotsFull));
    assert_eq!(search_line(&menu), ("SENSOR SLOTS FULL".into(), AMBER));
    // A sensor the latest search no longer reports is listed, not connected.
    let mut menu = opened(&[peer(10)], &[]);
    menu.refresh([None; DISCOVERY_CAPACITY], [None; CHANNEL_CAPACITY], false);
    assert!(menu.canvas().shows("NOT SEEN"));
    assert_eq!(rider.press(&mut menu, Button::BottomRight), None);
    assert_eq!(menu.diagnostics().message, Some(MenuMessage::NotSeen));
}

#[test]
fn the_search_line_says_how_the_search_goes() {
    let mut menu = opened(&[], &[]);
    let line = |menu: &Menu| {
        let (text, color) = menu.state.search_line();
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
    // Save trouble outlasts discovery messages.
    menu.set_notice(Some(MenuMessage::SavedUnreadable));
    menu.set_message(MenuMessage::Scanning);
    assert_eq!(line(&menu), (b"NOT SAVED".to_vec(), AMBER));
}

#[test]
fn sensors_names_trouble_with_the_radio_or_saving_in_its_title() {
    let mut menu = opened(&[], &[(strap(1), Connection::Live)]);
    // Search progress belongs to NEARBY.
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
    // Only a battery that needs changing shows, after a dot, and turns a
    // live sensor's whole line its colour; one too long to sit beside LIVE
    // takes its place.
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
        let canvas = menu.canvas();
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
    // A state too long for the battery leaves it out rather than cut it.
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

/// The screen box a row target covers.
fn focused_rect(menu: &Menu) -> Rect {
    let focus = match menu.state.view {
        View::Mine(row) => Target::Mine(row),
        View::Nearby(Some(index)) => Target::Nearby(index),
        view => panic!("{view:?} has no rows"),
    };
    (0..HEIGHT)
        .flat_map(|y| (0..WIDTH).map(move |x| (x, y)))
        .filter(|&(x, y)| menu.layout.target(x, y) == Some(focus))
        .fold(None, |rect: Option<Rect>, (x, y)| {
            let r = rect.unwrap_or(Rect {
                left: x,
                top: y,
                right: x + 1,
                bottom: y + 1,
            });
            Some(Rect {
                left: r.left.min(x),
                top: r.top.min(y),
                right: r.right.max(x + 1),
                bottom: r.bottom.max(y + 1),
            })
        })
        .expect("the focused row is shown")
}

#[test]
fn ten_saved_sensors_scroll_so_the_focus_stays_in_view() {
    let tiles: [_; CHANNEL_CAPACITY] =
        core::array::from_fn(|n| (identity(n as u8 + 1, n as u16), Connection::Live));
    let mut menu = opened(&[], &tiles);
    let mut rider = Rider::new();
    for step in 0..12 {
        let rect = focused_rect(&menu);
        assert!(
            rect.top >= CONTENT.top && rect.bottom <= FOCUS_BOTTOM,
            "step {step}: {rect:?}"
        );
        assert_eq!(
            menu.frame().pixel(rect.left + 1, rect.top + 24),
            ACCENT,
            "step {step}: ring"
        );
        rider.press(&mut menu, Button::BottomLeft);
    }
    // The last row scrolled the title away; wrapping brings it back.
    rider.focus(&mut menu, None);
    assert!(!menu.canvas().shows("SENSORS"));
    rider.press(&mut menu, Button::BottomLeft);
    assert!(menu.canvas().shows("SENSORS"));
    // A tap lands on the row where it is drawn, scrolled or not.
    rider.focus(&mut menu, None);
    let add = focused_rect(&menu);
    assert_eq!(menu.tap(120, add.top + 20, rider.now), Some(Action::Scan));
}

#[test]
fn eight_nearby_sensors_scroll_so_the_focus_stays_in_view() {
    let found: std::vec::Vec<_> = (0..8).map(|n| peer(100 + n)).collect();
    let mut menu = opened(&found, &[]);
    let mut rider = Rider::new();
    for step in 0..10 {
        let rect = focused_rect(&menu);
        assert!(
            rect.top >= CONTENT.top && rect.bottom <= FOCUS_BOTTOM,
            "step {step}: {rect:?}"
        );
        rider.press(&mut menu, Button::BottomLeft);
    }
}

#[test]
fn a_power_meter_detail_waits_a_moment_then_act_zeroes_and_move_asks_to_forget() {
    let meter = identity(device_type::POWER, 5512);
    let mut menu = opened(&[], &[(meter, Connection::Live)]);
    let opened_at = 20_000;
    assert_eq!(menu.input(press(Button::BottomRight), opened_at), None);
    assert_eq!(page(&menu), Page::Detail);
    let cues: std::vec::Vec<_> = menu
        .layout
        .cues
        .iter()
        .map(|c| (c.corner, c.icon))
        .collect();
    use crate::screens::cue::Icon;
    assert_eq!(
        cues,
        [
            (Corner::TopLeft, Icon::Back),
            (Corner::BottomLeft, Icon::Trash),
            (Corner::BottomRight, Icon::Calibrate)
        ]
    );
    // The second of two quick ACT presses never zeroes the meter.
    assert_eq!(
        menu.input(press(Button::BottomRight), opened_at + 400),
        None
    );
    assert_eq!(
        menu.input(press(Button::BottomRight), opened_at + 990),
        None
    );
    assert_eq!(
        menu.input(press(Button::BottomRight), opened_at + SETTLE_MS + 400),
        Some(Action::Calibrate)
    );
    assert_eq!(page(&menu), Page::Detail);
    let mut rider = Rider {
        now: opened_at + SETTLE_MS + 400,
    };
    assert_eq!(rider.press(&mut menu, Button::BottomLeft), None);
    assert_eq!(page(&menu), Page::Forget);
    assert!(menu.canvas().shows("FORGET") && menu.canvas().shows("SENSOR?"));
    assert!(menu.canvas().shows("POWER METER 5512"));
    // MOVE does nothing here; BACK returns to the detail page.
    assert_eq!(rider.press(&mut menu, Button::BottomLeft), None);
    assert_eq!(rider.press(&mut menu, Button::TopLeft), None);
    assert_eq!(page(&menu), Page::Detail);
    rider.press(&mut menu, Button::BottomLeft);
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Disconnect(device_type::POWER))
    );
    assert_eq!(page(&menu), Page::Sensors);
}

#[test]
fn any_other_sensor_detail_forgets_on_act_and_ignores_move() {
    let mut menu = opened(&[], &[(strap(100), Connection::Live)]);
    let mut rider = Rider::new();
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(page(&menu), Page::Detail);
    let corners: std::vec::Vec<_> = menu.cues().iter().map(|c| c.corner).collect();
    assert_eq!(corners, [Corner::TopLeft, Corner::BottomRight]);
    // ACT straight after opening waits, as on a power meter.
    assert_eq!(
        menu.input(press(Button::BottomRight), rider.now + 400),
        None
    );
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(rider.press(&mut menu, Button::BottomLeft), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(rider.press(&mut menu, Button::BottomRight), None);
    assert_eq!(page(&menu), Page::Forget);
    assert!(menu.canvas().shows("HEART RATE 100"));
    assert_eq!(
        rider.press(&mut menu, Button::BottomRight),
        Some(Action::Disconnect(device_type::HEART_RATE))
    );
    // The runtime removed it: the next row takes the focus.
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
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(page(&menu), Page::Detail);
    let mut left = saved(&tiles);
    left[0] = None;
    menu.refresh([None; DISCOVERY_CAPACITY], left, false);
    assert_eq!(page(&menu), Page::Sensors);
    assert_eq!(menu.diagnostics().focus, Some(tiles[1].0));
}

#[test]
fn taps_act_on_rows_and_cues_as_their_buttons_do() {
    let meter = identity(device_type::POWER, 2);
    let mut menu = opened(
        &[peer(10)],
        &[(strap(1), Connection::Live), (meter, Connection::Live)],
    );
    let now = 50_000;
    assert_eq!(menu.tap(120, 2, now), None, "the status bar is no target");
    let power_row = MINE_ROW_MIDDLE + super::layout::MINE_PITCH;
    assert_eq!(menu.tap(120, power_row, now), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(menu.diagnostics().focus, Some(meter));
    let view = menu.touch_view();
    // The calibrate cue waits as ACT does, then zeroes.
    let act = (WIDTH - 8, HEIGHT - 8);
    assert_eq!(menu.tap(act.0, act.1, now + 200), None);
    assert_eq!(
        menu.tap(act.0, act.1, now + SETTLE_MS),
        Some(Action::Calibrate)
    );
    assert_eq!(
        menu.tap(120, 160, now + SETTLE_MS),
        None,
        "content is no target"
    );
    assert_eq!(menu.tap(8, HEIGHT - 8, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Forget);
    assert_ne!(
        menu.touch_view(),
        view,
        "a contact never moves between pages"
    );
    assert_eq!(menu.tap(8, 8, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Detail);
    assert_eq!(menu.tap(8, 8, now + SETTLE_MS), None);
    assert_eq!(page(&menu), Page::Sensors);
    // ADD SENSOR's row opens it, and a nearby card asks to replace the meter.
    let add_row = MINE_ROW_MIDDLE + 2 * super::layout::MINE_PITCH;
    assert_eq!(menu.tap(120, add_row, now), Some(Action::Scan));
    assert_eq!(page(&menu), Page::Nearby);
    let first = super::layout::NEARBY_TOP + 24;
    assert_eq!(menu.tap(120, first, now), None);
    assert_eq!(page(&menu), Page::Replace);
    assert_eq!(
        menu.tap(WIDTH - 8, HEIGHT - 8, now),
        Some(Action::Connect(peer(10).identity))
    );
}

const MINE_ROW_MIDDLE: usize = super::layout::MINE_TOP + 24;

#[test]
fn cancel_holds_act_until_move_and_debounce_drops_repeats() {
    let mut menu = opened(&[], &[(strap(1), Connection::Live)]);
    for code in [4, 5] {
        let held = Input::Button {
            button: Button::BottomRight,
            code,
        };
        assert_eq!(menu.input(held, 0), None);
    }
    assert_eq!(page(&menu), Page::Sensors);
    menu.input(Input::Cancel, 400);
    assert!(menu.canvas().shows("MOVE FOCUS FIRST"));
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    assert_eq!(page(&menu), Page::Sensors);
    // MOVE confirms the focus; ACT within the debounce is dropped.
    menu.input(press(Button::BottomLeft), 1_200);
    assert!(menu.canvas().shows("SENSORS"));
    assert_eq!(menu.input(press(Button::BottomRight), 1_549), None);
    assert_eq!(page(&menu), Page::Sensors);
    // Back stays usable after input loss.
    menu.input(Input::Cancel, 2_000);
    assert_eq!(
        menu.input(press(Button::TopLeft), 2_400),
        Some(Action::Ride)
    );
}

#[test]
fn equal_device_numbers_show_their_transmission_type() {
    let mut second = peer(10);
    second.identity.transmission_type = 2;
    let menu = opened(&[peer(10), second], &[]);
    assert!(menu.canvas().shows("ID 10.1"));
    assert!(menu.canvas().shows("ID 10.2"));
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
    rider.press(&mut menu, Button::BottomRight);
    rider.press(&mut menu, Button::BottomRight);
    assert_eq!(page(&menu), Page::Replace);
    let canvas = menu.canvas();
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
        Rider::new().press(&mut menu, Button::BottomRight);
        assert_eq!(page(&menu), Page::Detail);
        assert_eq!(run(&menu, text).map(|r| r.color), Some(color), "{text}");
        assert_eq!(run(&menu, "V").is_some(), millivolts.is_some(), "{text}");
        assert!(menu.canvas().shows("BATTERY") && menu.canvas().shows("100"));
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
        Rider::new().press(&mut menu, Button::BottomRight);
        menu.set_calibration(Some(note));
        assert_eq!(note.text().as_str(), text);
        let line = run(&menu, text).unwrap_or_else(|| panic!("{text}"));
        assert_eq!(line.color, color);
        assert_eq!(
            line.top + usize::from(line.line.face().top + line.line.face().cap),
            CALIBRATION_BASE
        );
        if let Some(hint) = note.hint() {
            let hint = run(&menu, hint).expect("hint");
            assert_eq!(hint.color, SECONDARY);
            assert_eq!(
                hint.top + usize::from(hint.line.face().top + hint.line.face().cap),
                HINT_BASE
            );
        }
        menu.canvas().assert_laid_out(SCREEN, &[text], text);
        assert_clear_of_cues(&menu, text);
    }
    let mut strap_menu = opened(&[], &[(strap(1), Connection::Live)]);
    Rider::new().press(&mut strap_menu, Button::BottomRight);
    strap_menu.set_calibration(Some(CalibrationNote::Calibrating));
    assert!(!strap_menu.canvas().shows("CALIBRATING..."));
}

/// Nothing a page shows comes within reach of a cue's quarter disc.
#[track_caller]
fn assert_clear_of_cues(menu: &Menu, what: &str) {
    let canvas = menu.canvas();
    let texts = canvas
        .runs
        .iter()
        .filter_map(crate::screens::chrome::layout::text)
        .map(|(_, rect)| rect);
    let boxes: std::vec::Vec<Rect> = texts
        .chain(canvas.shapes.iter().map(Shape::bounds))
        .collect();
    for cue in menu.cues() {
        let (cx, cy) = match cue.corner {
            Corner::TopLeft => (0, 0),
            Corner::BottomLeft => (0, HEIGHT),
            Corner::BottomRight => (WIDTH, HEIGHT),
        };
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

/// Every page state the sweep and the reference test visit: ten saved
/// sensors with the longest names, numbers and states, eight nearby.
fn crowded() -> std::vec::Vec<(std::string::String, Menu)> {
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
            rider.press(&mut menu, Button::BottomRight);
            states.push((std::format!("detail {step}"), menu_copy(&menu)));
            menu.set_calibration(Some(CalibrationNote::Failed));
            states.push((std::format!("detail, failed {step}"), menu_copy(&menu)));
            menu.set_calibration(None);
            if menu.state.calibratable(match menu.state.view {
                View::Detail(slot) => slot,
                _ => unreachable!(),
            }) {
                rider.press(&mut menu, Button::BottomLeft);
            } else {
                rider.press(&mut menu, Button::BottomRight);
            }
            states.push((std::format!("forget {step}"), menu_copy(&menu)));
            rider.press(&mut menu, Button::TopLeft);
            rider.press(&mut menu, Button::TopLeft);
        }
        rider.press(&mut menu, Button::BottomLeft);
    }
    // NEARBY lists only sensors not saved, so give it eight new ones.
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
        rider.press(&mut add, Button::BottomLeft);
    }
    let mut replace = Menu::new();
    replace.refresh(
        found,
        saved(&[(identity(kinds[0], 1), Connection::Live)]),
        false,
    );
    replace.open();
    rider.focus(&mut replace, None);
    rider.press(&mut replace, Button::BottomRight);
    rider.press(&mut replace, Button::BottomRight);
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
    copy.prepare_layout();
    copy
}

#[test]
fn crowded_sensor_pages_never_overlap_clip_or_drop_anything() {
    for (what, menu) in crowded() {
        let canvas = menu.canvas();
        assert!(
            canvas.runs.len() < 28 && canvas.shapes.len() < 24,
            "{what}: canvas full"
        );
        // The longest names and states show whole where their rows are in view.
        let want: &[&str] = match (what.as_str(), menu.state.view) {
            ("sensors 0", _) => &["POWER METER", "BIKE LIGHTS", "RADIO UNAVAILABLE"],
            ("add 0", _) => &["NOT SAVED", "POWER METER", "BIKE LIGHTS", "ID 65535"],
            (_, View::Detail(_)) => &["BATTERY", "ID"],
            (_, View::Forget(_)) => &["FORGET", "SENSOR?"],
            (_, View::Replace(_)) => &["REPLACE SENSOR", "CURRENT", "NEW"],
            _ => &[],
        };
        canvas.assert_laid_out(SCREEN, want, &what);
        if full_screen(menu.cues()) {
            assert_clear_of_cues(&menu, &what);
        } else {
            let rect = focused_rect(&menu);
            assert!(
                rect.top >= CONTENT.top && rect.bottom <= FOCUS_BOTTOM,
                "{what}"
            );
        }
    }
}

#[test]
fn painted_rows_match_the_per_pixel_reference() {
    for (what, menu) in crowded() {
        let frame = menu.frame();
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                let (got, want) = (frame.pixel(x, y), frame.reference_pixel(x, y));
                assert_eq!(got, want, "{what} at {x},{y}: {got:#06x} != {want:#06x}");
            }
        }
    }
}
