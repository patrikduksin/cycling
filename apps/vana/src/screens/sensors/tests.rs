use super::render::{AMBER, CYAN, GREEN, WHITE};
use super::*;
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity};
use device_api::input::{Button, Input};
use firmware_shell::rendering::text::text;

fn press(button: Button) -> Input {
    Input::Button { button, code: 1 }
}
fn indicator_has_color(menu: &Menu, color: u16) -> bool {
    (59..66).any(|y| (38..45).any(|x| menu.pixel(x, y) == color))
}
fn peer(number: u16) -> Discovery {
    Discovery {
        identity: Identity {
            device_type: 11,
            device_number: number,
            transmission_type: 1,
        },
        rssi: -40,
        seen_ms: 0,
    }
}
fn nearby(peers: &[Discovery]) -> [Option<Discovery>; DISCOVERY_CAPACITY] {
    core::array::from_fn(|index| peers.get(index).copied())
}
fn saved(tiles: &[(Identity, Connection)]) -> [Option<Selected>; CHANNEL_CAPACITY] {
    core::array::from_fn(|slot| {
        tiles.get(slot).map(|&(identity, connection)| Selected {
            identity,
            connection,
        })
    })
}
fn shows(menu: &Menu, left: usize, top: usize, color: u16, label: &[u8]) -> bool {
    (top..top + 14)
        .all(|y| (12..228).all(|x| (menu.pixel(x, y) == color) == text(x, y, left, top, 2, label)))
}

#[test]
fn runtime_scan_messages_preserve_focused_identity_and_connection_feedback() {
    let mut menu = Menu::new();
    menu.refresh(
        nearby(&[peer(10), peer(20)]),
        [None; CHANNEL_CAPACITY],
        true,
    );
    menu.input(press(Button::BottomLeft), 0);
    let mut baseline = [0; 216 * 44];
    for y in 0..44 {
        for x in 0..216 {
            baseline[y * 216 + x] = menu.pixel(12 + x, 238 + y);
        }
    }
    for message in [
        MenuMessage::Scanning,
        MenuMessage::ScanDone,
        MenuMessage::ScanCancelled,
        MenuMessage::StartingScan,
        MenuMessage::SearchPending,
        MenuMessage::StoppingScan,
        MenuMessage::FinishingScan,
    ] {
        menu.set_message(message);
        for y in 0..44 {
            for x in 0..216 {
                assert_eq!(
                    menu.pixel(12 + x, 238 + y),
                    baseline[y * 216 + x],
                    "{message:?} at {x},{y}"
                );
            }
        }
    }
    assert_eq!(
        menu.input(press(Button::BottomRight), 400),
        Some(Action::Connect(peer(10).identity))
    );
    // The runtime saves the choice and shows it as a tile.
    menu.refresh(
        nearby(&[peer(10), peer(20)]),
        saved(&[(peer(10).identity, Connection::Searching)]),
        false,
    );
    menu.focus_type(11);
    menu.set_message(MenuMessage::ScanCancelled);
    assert!(shows(&menu, 12, 267, CYAN, b"WAIT / 10"));
    menu.set_message(MenuMessage::RadioUnavailable);
    assert!(shows(&menu, 12, 267, CYAN, b"WAIT / 10"));
    assert!(shows(&menu, 12, 36, AMBER, b"RADIO UNAVAILABLE"));
}

#[test]
fn saved_sensor_status_is_honest_about_searching_and_retrying() {
    let identity = peer(10).identity;
    for (connection, color, label) in [
        (Connection::Live, GREEN, &b"LIVE DATA"[..]),
        (Connection::Searching, CYAN, b"SEARCHING"),
        (Connection::NotFound, AMBER, b"NOT FOUND YET"),
        (Connection::WaitingForRadio, CYAN, b"WAITING FOR RADIO"),
        (Connection::Replacing, CYAN, b"REPLACING"),
        (Connection::DataLost, AMBER, b"DATA LOST"),
        (Connection::WaitingForData, AMBER, b"WAITING FOR DATA"),
        (Connection::RadioUnavailable, AMBER, b"RADIO UNAVAILABLE"),
        (Connection::Unsupported, AMBER, b"TYPE UNSUPPORTED"),
    ] {
        let mut menu = Menu::new();
        menu.refresh(
            [None; DISCOVERY_CAPACITY],
            saved(&[(identity, connection)]),
            false,
        );
        menu.focus_type(11);
        assert!(indicator_has_color(&menu, color), "{connection:?}");
        assert!(shows(&menu, 12, 267, color, label), "{connection:?}");
    }
}

#[test]
fn replacement_disambiguates_maximum_identity_without_overlapping_numbers() {
    let current = Identity {
        device_type: 11,
        device_number: u16::MAX,
        transmission_type: 254,
    };
    let replacement = Identity {
        transmission_type: 255,
        ..current
    };
    let mut menu = Menu::new();
    let mut discovery = peer(u16::MAX);
    discovery.identity = replacement;
    menu.refresh(
        nearby(&[discovery]),
        saved(&[(current, Connection::NotFound)]),
        false,
    );
    menu.focus_type(11);
    menu.input(press(Button::BottomLeft), 0);
    assert_eq!(menu.input(press(Button::BottomRight), 400), None);
    for y in 0..320 {
        for x in 0..240 {
            let _ = menu.pixel(x, y);
        }
    }
    // Each side reserves a 14px number line and a separate transmission line.
    for y in 160..191 {
        for x in 12..100 {
            assert_eq!(
                menu.pixel(x, y) == WHITE,
                text(x, y, 24, 160, 2, b"65535") || text(x, y, 30, 177, 2, b"T254"),
                "{x},{y}"
            );
        }
    }
}

#[test]
fn paper_grid_shows_ten_selected_and_eight_nearby_without_scrolling() {
    let tiles: [_; CHANNEL_CAPACITY] = core::array::from_fn(|n| {
        (
            Identity {
                device_type: n as u8 + 1,
                ..peer(n as u16 + 1).identity
            },
            Connection::Searching,
        )
    });
    let mut menu = Menu::new();
    menu.refresh(
        core::array::from_fn(|n| Some(peer(100 + n as u16))),
        saved(&tiles),
        false,
    );
    assert_eq!(menu.pixel(231, 125), 0x3186);
    assert_eq!(menu.pixel(228, 223), 0x3186);
    menu.input(press(Button::BottomLeft), 0);
    assert_eq!(menu.pixel(8, 56), CYAN);
    assert_eq!(menu.pixel(10, 58), 0x0945);
}

#[test]
fn new_search_reclaims_discovery_slots_and_preserves_selected_focus() {
    let mut menu = Menu::new();
    let selected = Identity {
        device_type: 40,
        ..peer(90).identity
    };
    let tiles = saved(&[(selected, Connection::Searching)]);
    let old = core::array::from_fn(|index| Some(peer(index as u16 + 1)));
    menu.refresh(old, tiles, false);
    menu.focus_type(40);
    let selected_focus = menu.diagnostics().cursor;
    menu.search_started();
    menu.refresh(nearby(&[peer(99)]), tiles, true);
    assert_eq!(menu.selected_identity(40), Some(selected));
    assert_eq!(menu.diagnostics().cursor, selected_focus);
    menu.input(press(Button::BottomLeft), 400);
    assert_eq!(
        menu.input(press(Button::BottomRight), 800),
        Some(Action::Connect(peer(99).identity))
    );
}

#[test]
fn identities_with_equal_device_numbers_have_distinct_available_suffixes() {
    let mut second = peer(10);
    second.identity.transmission_type = 2;
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10), second]), [None; CHANNEL_CAPACITY], false);
    menu.input(press(Button::BottomLeft), 0);
    let mut first_status = [0; 216 * 14];
    for y in 0..14 {
        for x in 0..216 {
            first_status[y * 216 + x] = menu.pixel(12 + x, 267 + y);
        }
    }
    menu.input(press(Button::BottomLeft), 400);
    assert!(
        (0..14).any(|y| (0..216).any(|x| first_status[y * 216 + x] != menu.pixel(12 + x, 267 + y)))
    );
}

#[test]
fn detail_offers_only_disconnect_and_back_whatever_the_link_state() {
    for connection in [
        Connection::Live,
        Connection::NotFound,
        Connection::Searching,
    ] {
        let mut menu = Menu::new();
        menu.refresh(
            [None; DISCOVERY_CAPACITY],
            saved(&[(peer(10).identity, connection)]),
            false,
        );
        menu.input(press(Button::BottomLeft), 0);
        assert_eq!(menu.input(press(Button::BottomRight), 400), None);
        let screen: std::vec::Vec<u16> = (0..320)
            .flat_map(|y| (0..240).map(move |x| (x, y)))
            .map(|(x, y)| menu.pixel(x, y))
            .collect();
        let label = |left, top, label: &[u8]| {
            (top..top + 14).any(|y| {
                (0..240).any(|x| text(x, y, left, top, 2, label) && screen[y * 240 + x] == WHITE)
            })
        };
        assert!(label(60, 230, b"DISCONNECT"), "{connection:?}");
        assert!(label(96, 269, b"BACK"), "{connection:?}");
        // Secondary is Back: nothing is removed.
        menu.input(press(Button::BottomLeft), 800);
        assert_eq!(menu.input(press(Button::BottomRight), 1200), None);
        assert_eq!(menu.selected_identity(11), Some(peer(10).identity));
        // Primary removes the sensor.
        assert_eq!(menu.input(press(Button::BottomRight), 1600), None);
        assert_eq!(
            menu.input(press(Button::BottomRight), 2000),
            Some(Action::Disconnect(11))
        );
        assert_eq!(menu.diagnostics().cursor, 0);
    }
}

#[test]
fn removed_tile_leaves_the_page_and_reappears_only_as_a_nearby_sensor() {
    let mut menu = Menu::new();
    let tiles = saved(&[(peer(10).identity, Connection::Live)]);
    menu.refresh(nearby(&[peer(10)]), tiles, false);
    menu.input(press(Button::BottomLeft), 0);
    menu.input(press(Button::BottomRight), 400);
    // The runtime removed it while its detail page was open.
    menu.refresh(nearby(&[peer(10)]), [None; CHANNEL_CAPACITY], false);
    assert_eq!(menu.selected_identity(11), None);
    assert_eq!(menu.diagnostics().cursor, 0);
    menu.input(press(Button::BottomLeft), 800);
    assert_eq!(
        menu.input(press(Button::BottomRight), 1200),
        Some(Action::Connect(peer(10).identity))
    );
}

#[test]
fn same_type_replacement_requires_confirmation() {
    let mut menu = Menu::new();
    let tiles = saved(&[(peer(10).identity, Connection::Live)]);
    menu.refresh(nearby(&[peer(10), peer(20)]), tiles, false);
    menu.input(press(Button::BottomLeft), 0);
    menu.input(press(Button::BottomLeft), 400);
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    assert_eq!(menu.selected_identity(11), Some(peer(10).identity));
    // Keep current returns to the tiles.
    menu.input(press(Button::BottomLeft), 1200);
    assert_eq!(menu.input(press(Button::BottomRight), 1600), None);
    assert_eq!(menu.input(press(Button::BottomRight), 2000), None);
    assert_eq!(
        menu.input(press(Button::BottomRight), 2400),
        Some(Action::Connect(peer(20).identity))
    );
}

#[test]
fn full_slots_refuse_a_new_type_but_allow_replacement() {
    let tiles: [_; CHANNEL_CAPACITY] = core::array::from_fn(|n| {
        (
            Identity {
                device_type: n as u8 + 20,
                ..peer(n as u16 + 1).identity
            },
            Connection::Live,
        )
    });
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), saved(&tiles), false);
    for n in 0..11 {
        menu.input(press(Button::BottomLeft), n * 400);
    }
    assert_eq!(menu.input(press(Button::BottomRight), 4400), None);
    assert_eq!(menu.diagnostics().message, Some(MenuMessage::SlotsFull));
}

#[test]
fn reordered_discovery_keeps_focus_and_selected_peer_is_not_duplicated() {
    let mut menu = Menu::new();
    menu.refresh(
        nearby(&[peer(10), peer(20)]),
        [None; CHANNEL_CAPACITY],
        true,
    );
    menu.input(press(Button::BottomLeft), 0);
    menu.refresh(
        nearby(&[peer(20), peer(10)]),
        [None; CHANNEL_CAPACITY],
        false,
    );
    assert_eq!(
        menu.input(press(Button::BottomRight), 400),
        Some(Action::Connect(peer(10).identity))
    );
    let tiles = saved(&[(peer(10).identity, Connection::Searching)]);
    menu.refresh(nearby(&[peer(20), peer(10)]), tiles, false);
    menu.focus_type(11);
    // Next skips the selected identity's former available tile.
    menu.input(press(Button::BottomLeft), 800);
    assert_eq!(menu.input(press(Button::BottomRight), 1200), None);
    assert_eq!(
        menu.input(press(Button::BottomRight), 1600),
        Some(Action::Connect(peer(20).identity))
    );
}

#[test]
fn cancel_holds_and_debounce_cannot_activate_a_sensor() {
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), [None; CHANNEL_CAPACITY], false);
    for code in [4, 5] {
        assert_eq!(
            menu.input(
                Input::Button {
                    button: Button::BottomLeft,
                    code
                },
                0
            ),
            None
        );
    }
    menu.input(press(Button::BottomLeft), 0);
    assert_eq!(menu.input(press(Button::BottomRight), 349), None);
    menu.input(Input::Cancel, 400);
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    // Back stays usable even after input loss.
    assert_eq!(menu.input(press(Button::TopLeft), 1200), Some(Action::Ride));
}

#[test]
fn all_ten_selected_slots_are_reachable_without_overflowing_capacity() {
    let tiles: [_; CHANNEL_CAPACITY] = core::array::from_fn(|n| {
        (
            Identity {
                device_type: n as u8 + 1,
                device_number: n as u16 + 1,
                transmission_type: 1,
            },
            Connection::Searching,
        )
    });
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), saved(&tiles), false);
    for n in 0..10 {
        menu.input(press(Button::BottomLeft), n * 400);
    }
    menu.input(press(Button::BottomRight), 4000);
    assert_eq!(
        menu.input(press(Button::BottomRight), 4400),
        Some(Action::Disconnect(10))
    );
}

#[test]
fn choosing_available_moves_focus_to_selected_and_back_exits_immediately() {
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), [None; CHANNEL_CAPACITY], true);
    menu.input(press(Button::BottomLeft), 0);
    assert_eq!(
        menu.input(press(Button::BottomRight), 400),
        Some(Action::Connect(peer(10).identity))
    );
    menu.refresh(
        nearby(&[peer(10)]),
        saved(&[(peer(10).identity, Connection::WaitingForRadio)]),
        true,
    );
    menu.focus_type(11);
    assert_eq!(menu.diagnostics().cursor, 1);
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    assert_eq!(menu.input(press(Button::TopLeft), 1200), None);
    assert_eq!(menu.input(press(Button::TopLeft), 1600), Some(Action::Ride));
}

#[test]
fn save_notice_outlasts_discovery_messages_on_the_page() {
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), [None; CHANNEL_CAPACITY], true);
    menu.set_notice(Some(MenuMessage::SavedUnreadable));
    for message in [
        MenuMessage::StartingScan,
        MenuMessage::Scanning,
        MenuMessage::ScanDone,
        MenuMessage::ScanFailed,
    ] {
        menu.set_message(message);
        assert_eq!(menu.layout.page_feedback.as_bytes(), b"NOT SAVED");
    }
    menu.set_notice(None);
    assert!(
        menu.layout
            .page_feedback
            .as_bytes()
            .starts_with(b"SCAN FAILED")
    );
}
