use super::render::{AMBER, CYAN, GREEN, WHITE};
use super::*;
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity, LinkState};
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
#[test]
fn runtime_scan_messages_preserve_focused_identity_and_connection_feedback() {
    let mut menu = Menu::new();
    menu.refresh(
        [
            Some(peer(10)),
            Some(peer(20)),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        [None; CHANNEL_CAPACITY],
        true,
        0,
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
    menu.input(press(Button::BottomRight), 400);
    menu.set_message(MenuMessage::ScanCancelled);
    // Pending belongs to the selected peer, not to the cancelled discovery.
    for y in 267..281 {
        for x in 12..228 {
            assert_eq!(
                menu.pixel(x, y) == CYAN,
                text(x, y, 12, 267, 2, b"WAIT / 10")
            );
        }
    }
    menu.set_message(MenuMessage::RadioUnavailable);
    for y in 267..281 {
        for x in 12..228 {
            assert_eq!(
                menu.pixel(x, y) == CYAN,
                text(x, y, 12, 267, 2, b"WAIT / 10")
            );
        }
    }
    for y in 36..50 {
        for x in 12..228 {
            assert_eq!(
                menu.pixel(x, y) == AMBER,
                text(x, y, 12, 36, 2, b"RADIO UNAVAILABLE")
            );
        }
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
    menu.request_pending(current);
    menu.request_failed(current, MenuMessage::RequestFailed);
    let mut discovery = peer(u16::MAX);
    discovery.identity = replacement;
    menu.refresh(
        [Some(discovery), None, None, None, None, None, None, None],
        [None; CHANNEL_CAPACITY],
        false,
        0,
    );
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
    let mut channels = firmware_services::ant::Channels::new();
    for kind in 1..=10 {
        channels
            .connect(
                Identity {
                    device_type: kind,
                    ..peer(u16::from(kind)).identity
                },
                0,
            )
            .unwrap();
    }
    let mut menu = Menu::new();
    menu.refresh(
        core::array::from_fn(|n| Some(peer(100 + n as u16))),
        channels.snapshots(0),
        false,
        0,
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
    menu.request_pending(selected);
    let old = core::array::from_fn(|index| Some(peer(index as u16 + 1)));
    menu.refresh(old, [None; CHANNEL_CAPACITY], false, 0);
    let selected_focus = menu.diagnostics().cursor;
    menu.search_started();
    menu.refresh(
        [Some(peer(99)), None, None, None, None, None, None, None],
        [None; CHANNEL_CAPACITY],
        true,
        400,
    );
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
    menu.refresh(
        [
            Some(peer(10)),
            Some(second),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        [None; CHANNEL_CAPACITY],
        false,
        0,
    );
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
fn admitted_connecting_sensor_keeps_the_pending_indicator() {
    let mut channels = firmware_services::ant::Channels::new();
    channels.connect(peer(10).identity, 0).unwrap();
    let mut menu = Menu::new();
    menu.request_pending(peer(10).identity);
    menu.request_accepted(peer(10).identity);
    menu.refresh([None; DISCOVERY_CAPACITY], channels.snapshots(0), false, 0);
    assert!(indicator_has_color(&menu, CYAN));
}
#[test]
fn rejected_unadmitted_peer_offers_back_instead_of_disconnect() {
    let mut menu = Menu::new();
    menu.request_pending(peer(10).identity);
    menu.request_failed(peer(10).identity, MenuMessage::RequestFailed);
    menu.input(press(Button::BottomRight), 0);
    menu.input(press(Button::BottomLeft), 400);
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    assert_eq!(menu.input(press(Button::TopLeft), 1200), Some(Action::Ride));
}
#[test]
fn pending_admission_cannot_be_overwritten_by_disconnect_or_replacement() {
    let mut menu = Menu::new();
    menu.refresh(
        [
            Some(peer(10)),
            Some(peer(20)),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        [None; CHANNEL_CAPACITY],
        false,
        0,
    );
    menu.request_pending(peer(10).identity);
    menu.input(press(Button::BottomRight), 0);
    assert_eq!(menu.input(press(Button::BottomRight), 400), None);
    menu.input(press(Button::BottomLeft), 800);
    assert_eq!(menu.input(press(Button::BottomRight), 1200), None);
    assert_eq!(menu.input(press(Button::BottomRight), 1600), None);
    assert_eq!(menu.selected_identity(11), Some(peer(10).identity));
}
#[test]
fn same_type_replacement_requires_confirmation_and_retains_failed_intent() {
    let mut channels = firmware_services::ant::Channels::new();
    channels.connect(peer(10).identity, 0).unwrap();
    let mut menu = Menu::new();
    menu.refresh(
        [
            Some(peer(10)),
            Some(peer(20)),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        channels.snapshots(0),
        false,
        0,
    );
    menu.input(press(Button::BottomLeft), 0);
    menu.input(press(Button::BottomLeft), 400);
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    assert_eq!(menu.selected_identity(11), Some(peer(10).identity));
    assert_eq!(
        menu.input(press(Button::BottomRight), 1200),
        Some(Action::Connect(peer(20).identity))
    );
    menu.request_failed(peer(20).identity, MenuMessage::RequestFailed);
    menu.refresh(
        [None; DISCOVERY_CAPACITY],
        channels.snapshots(1600),
        false,
        1600,
    );
    assert_eq!(menu.selected_identity(11), Some(peer(20).identity));
    menu.input(press(Button::BottomRight), 1600);
    assert_eq!(
        menu.input(press(Button::BottomRight), 2000),
        Some(Action::Connect(peer(20).identity))
    );
}
#[test]
fn reordered_discovery_keeps_focus_and_selected_peer_is_not_duplicated() {
    let mut menu = Menu::new();
    menu.refresh(
        [
            Some(peer(10)),
            Some(peer(20)),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        [None; CHANNEL_CAPACITY],
        true,
        0,
    );
    menu.input(press(Button::BottomLeft), 0);
    menu.refresh(
        [
            Some(peer(20)),
            Some(peer(10)),
            None,
            None,
            None,
            None,
            None,
            None,
        ],
        [None; CHANNEL_CAPACITY],
        false,
        400,
    );
    assert_eq!(
        menu.input(press(Button::BottomRight), 400),
        Some(Action::Connect(peer(10).identity))
    );
    menu.request_failed(peer(10).identity, MenuMessage::RequestFailed);
    // Next skips the selected identity's former available tile.
    menu.input(press(Button::BottomLeft), 800);
    assert_eq!(menu.input(press(Button::BottomRight), 1200), None);
    assert_eq!(
        menu.input(press(Button::BottomRight), 1600),
        Some(Action::Connect(peer(20).identity))
    );
}
#[test]
fn connection_indicator_requires_observed_fresh_data_and_failure_is_recoverable() {
    let mut menu = Menu::new();
    menu.request_pending(peer(10).identity);
    assert!(indicator_has_color(&menu, CYAN));
    let mut channels = firmware_services::ant::Channels::new();
    channels.connect(peer(10).identity, 0).unwrap();
    let mut snapshots = channels.snapshots(0);
    let snapshot = snapshots[0].as_mut().unwrap();
    snapshot.link = LinkState::Connected;
    snapshot.stale = false;
    snapshot.age_ms = Some(0);
    // A stale observation before admission cannot acknowledge new intent.
    menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 0);
    assert!(indicator_has_color(&menu, CYAN));
    menu.request_accepted(peer(10).identity);
    menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 400);
    assert!(indicator_has_color(&menu, GREEN));
    snapshots[0].as_mut().unwrap().link = LinkState::TimedOut;
    menu.refresh([None; DISCOVERY_CAPACITY], snapshots, false, 800);
    assert!(indicator_has_color(&menu, AMBER));
    menu.input(press(Button::BottomRight), 800);
    assert_eq!(
        menu.input(press(Button::BottomRight), 1200),
        Some(Action::Connect(peer(10).identity))
    );
}
#[test]
fn cancel_holds_and_debounce_cannot_activate_a_sensor() {
    let mut menu = Menu::new();
    menu.refresh(
        [Some(peer(10)), None, None, None, None, None, None, None],
        [None; CHANNEL_CAPACITY],
        false,
        0,
    );
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
    let mut channels = firmware_services::ant::Channels::new();
    for kind in 1..=10 {
        channels
            .connect(
                Identity {
                    device_type: kind,
                    device_number: u16::from(kind),
                    transmission_type: 1,
                },
                0,
            )
            .unwrap();
    }
    let mut menu = Menu::new();
    menu.refresh(
        [Some(peer(10)), None, None, None, None, None, None, None],
        channels.snapshots(0),
        false,
        0,
    );
    for n in 0..10 {
        menu.input(press(Button::BottomLeft), n * 400);
    }
    menu.input(press(Button::BottomRight), 4000);
    assert_eq!(
        menu.input(press(Button::BottomRight), 4400),
        Some(Action::Disconnect(10))
    );
    menu.input(press(Button::BottomLeft), 4800);
    assert_eq!(menu.input(press(Button::BottomRight), 5200), None);
}
#[test]
fn rejected_disconnect_can_retry_the_disconnect() {
    let mut channels = firmware_services::ant::Channels::new();
    channels.connect(peer(10).identity, 0).unwrap();
    channels.receive(device_api::ant::Event::Connected(peer(10).identity), 0);
    let mut menu = Menu::new();
    menu.refresh([None; DISCOVERY_CAPACITY], channels.snapshots(0), false, 0);
    menu.input(press(Button::BottomLeft), 0);
    menu.input(press(Button::BottomRight), 400);
    assert_eq!(
        menu.input(press(Button::BottomRight), 800),
        Some(Action::Disconnect(11))
    );
    menu.request_failed(peer(10).identity, MenuMessage::Busy);
    menu.input(press(Button::BottomRight), 1200);
    assert_eq!(
        menu.input(press(Button::BottomRight), 1600),
        Some(Action::Disconnect(11))
    );
}
#[test]
fn choosing_available_moves_focus_to_selected_and_back_exits_immediately() {
    let mut menu = Menu::new();
    menu.refresh(
        [Some(peer(10)), None, None, None, None, None, None, None],
        [None; CHANNEL_CAPACITY],
        true,
        0,
    );
    menu.input(press(Button::BottomLeft), 0);
    assert_eq!(
        menu.input(press(Button::BottomRight), 400),
        Some(Action::Connect(peer(10).identity))
    );
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    assert_eq!(menu.input(press(Button::TopLeft), 1200), None);
    assert_eq!(menu.input(press(Button::TopLeft), 1600), Some(Action::Ride));
}
