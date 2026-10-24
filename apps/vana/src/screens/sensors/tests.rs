use super::layout::{CALIBRATION_TOP, DETAIL_BUTTONS, POWER_DETAIL_BUTTONS};
use super::render::{AMBER, CYAN, GREEN, HEADING_TOP, STATUS_TOP, TITLE_TOP, WHITE};
use super::*;
use crate::screens::chrome::{Align, BODY, CAPTION, CARD, Label, Line, RAISED, VALUE};
use device_api::ant::{CHANNEL_CAPACITY, DISCOVERY_CAPACITY, Discovery, Identity};
use device_api::input::{Button, Input};
use firmware_shell::rendering::font::Face;

/// The menu draws `text` exactly where a label of it is solid, and nowhere
/// else in its box.
fn shows(
    menu: &Menu,
    face: &'static Face,
    text: &[u8],
    x: usize,
    top: usize,
    align: Align,
    color: u16,
) -> bool {
    let label = Label::new(Line::<24>::new(face, text), x, top, align, color);
    (top..label.bottom()).all(|y| {
        (label.left..label.right()).all(|x| match label.line.coverage(x - label.left, y - top) {
            15 => menu.pixel(x, y) == color,
            0 => menu.pixel(x, y) != color,
            _ => true,
        })
    })
}

/// The focused sensor's status line.
fn status(menu: &Menu, color: u16, text: &[u8]) -> bool {
    shows(menu, &CAPTION, text, 12, STATUS_TOP, Align::Left, color)
}

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
/// A saved heart rate strap, whose detail page has no calibrate button.
fn strap(number: u16) -> Identity {
    Identity {
        device_type: 120,
        ..peer(number).identity
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
            battery: None,
        })
    })
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
            baseline[y * 216 + x] = menu.pixel(12 + x, TITLE_TOP + y);
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
                    menu.pixel(12 + x, TITLE_TOP + y),
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
    assert!(status(&menu, CYAN, b"WAIT / 10"));
    menu.set_message(MenuMessage::RadioUnavailable);
    assert!(status(&menu, CYAN, b"WAIT / 10"));
    assert!(shows(
        &menu,
        &BODY,
        b"RADIO UNAVAILABLE",
        12,
        HEADING_TOP,
        Align::Left,
        AMBER
    ));
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
        assert!(status(&menu, color, label), "{connection:?}");
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
    // Each side reserves a number line and a separate transmission line.
    assert!(shows(
        &menu,
        &VALUE,
        b"65535",
        60,
        140,
        Align::Center,
        WHITE
    ));
    assert!(shows(
        &menu,
        &CAPTION,
        b"T254",
        60,
        172,
        Align::Center,
        WHITE
    ));
    assert!(shows(
        &menu,
        &CAPTION,
        b"T255",
        180,
        172,
        Align::Center,
        WHITE
    ));
    let number = Label::new(
        Line::<8>::new(&VALUE, b"65535"),
        60,
        140,
        Align::Center,
        WHITE,
    );
    assert!(
        number.bottom() <= 172 && number.right() < 120,
        "numbers do not overlap"
    );
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
    // The last selected and nearby tiles are on screen and unfocused.
    assert_eq!(menu.pixel(194, 110), CARD);
    assert_eq!(menu.pixel(181, 210), CARD);
    menu.input(press(Button::BottomLeft), 0);
    assert_eq!(menu.pixel(28, 56), CYAN, "focus ring");
    assert_eq!(menu.pixel(12, 70), RAISED, "focused fill");
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
            first_status[y * 216 + x] = menu.pixel(12 + x, STATUS_TOP + y);
        }
    }
    menu.input(press(Button::BottomLeft), 400);
    assert!((0..14).any(|y| {
        (0..216).any(|x| first_status[y * 216 + x] != menu.pixel(12 + x, STATUS_TOP + y))
    }));
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
            saved(&[(strap(10), connection)]),
            false,
        );
        menu.input(press(Button::BottomLeft), 0);
        assert_eq!(menu.input(press(Button::BottomRight), 400), None);
        let label = |(top, height): (usize, usize), label: &[u8]| {
            let top = top + height / 2 - usize::from(BODY.top) - usize::from(BODY.cap) / 2;
            shows(&menu, &BODY, label, 120, top, Align::Center, WHITE)
        };
        assert!(label(DETAIL_BUTTONS[0], b"DISCONNECT"), "{connection:?}");
        assert!(label(DETAIL_BUTTONS[1], b"BACK"), "{connection:?}");
        // Secondary is Back: nothing is removed.
        menu.input(press(Button::BottomLeft), 800);
        assert_eq!(menu.input(press(Button::BottomRight), 1200), None);
        assert_eq!(menu.selected_identity(120), Some(strap(10)));
        // Primary removes the sensor.
        assert_eq!(menu.input(press(Button::BottomRight), 1600), None);
        assert_eq!(
            menu.input(press(Button::BottomRight), 2000),
            Some(Action::Disconnect(120))
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

#[test]
fn taps_focus_and_activate_what_is_under_the_finger() {
    let mut menu = Menu::new();
    menu.refresh(nearby(&[peer(10)]), [None; CHANNEL_CAPACITY], false);
    // Empty space does nothing; the search button searches.
    assert_eq!(menu.tap(120, 100), None);
    assert_eq!(menu.tap(218, 140), Some(Action::Scan));
    // The search margin takes only spare space: an empty slot, never a tile.
    assert_eq!(
        menu.tap(200, 158),
        Some(Action::Scan),
        "an empty slot is spare"
    );
    let mut crowded = Menu::new();
    crowded.refresh(
        nearby(&[peer(10), peer(11), peer(12), peer(13)]),
        [None; CHANNEL_CAPACITY],
        false,
    );
    assert_eq!(
        crowded.tap(200, 158),
        Some(Action::Connect(peer(13).identity))
    );
    // Lost input asks for a confirmed focus; a tap is one.
    menu.input(Input::Cancel, 100);
    assert_eq!(menu.input(press(Button::BottomRight), 600), None);
    assert_eq!(menu.tap(30, 170), Some(Action::Connect(peer(10).identity)));
    // Once saved, tapping its tile opens the detail; Back closes it.
    let tiles = saved(&[(strap(10), Connection::Searching)]);
    menu.refresh(nearby(&[peer(10)]), tiles, false);
    assert_eq!(menu.tap(20, 70), None);
    assert!(matches!(menu.view, menu::View::Detail(0)));
    assert_eq!(menu.tap(120, 275), None);
    assert!(matches!(menu.view, menu::View::Tiles));
    assert_eq!(menu.tap(20, 70), None);
    assert_eq!(menu.tap(120, 236), Some(Action::Disconnect(120)));
}

#[test]
fn home_glance_lists_selected_sensors_with_their_state() {
    let mut menu = Menu::new();
    assert!(menu.glance().is_empty());
    let tiles = saved(&[(peer(10).identity, Connection::Searching)]);
    menu.refresh([None; DISCOVERY_CAPACITY], tiles, false);
    let glance = menu.glance();
    assert_eq!(glance.len(), 1);
    assert_eq!(glance[0].kind, 11);
    assert!(!glance[0].live);
    assert_eq!(glance[0].color, CYAN);
}

/// Every painted row of the menu equals the per-pixel reference.
#[track_caller]
fn assert_paints_reference(menu: &Menu, what: &str) {
    for y in 0..320 {
        for x in 0..240 {
            let (got, want) = (menu.pixel(x, y), menu.reference_pixel(x, y));
            assert_eq!(got, want, "{what} at {x},{y}: {got:#06x} != {want:#06x}");
        }
    }
}

#[test]
fn painted_rows_match_the_per_pixel_reference() {
    let mut menu = Menu::new();
    menu.refresh([None; DISCOVERY_CAPACITY], [None; CHANNEL_CAPACITY], true);
    assert_paints_reference(&menu, "empty and scanning");
    let mut replacement = peer(77);
    replacement.identity.transmission_type = 5;
    let mut radar = peer(30);
    radar.identity.device_type = 40;
    let tiles = saved(&[
        (peer(10).identity, Connection::Live),
        (
            Identity {
                device_type: 120,
                ..peer(11).identity
            },
            Connection::Searching,
        ),
        (
            Identity {
                device_type: 121,
                ..peer(12).identity
            },
            Connection::DataLost,
        ),
    ]);
    menu.refresh(nearby(&[peer(20), replacement, radar]), tiles, false);
    assert_paints_reference(&menu, "tiles, search focused");
    menu.set_notice(Some(MenuMessage::NotSaved));
    for step in 0..6 {
        menu.input(press(Button::BottomLeft), 400 * (step + 1));
        assert_paints_reference(&menu, "tiles, focus moving");
    }
    menu.set_notice(None);
    // The first saved sensor's detail, both choices.
    menu.focus_type(11);
    menu.input(press(Button::BottomRight), 4000);
    assert!(matches!(menu.view, menu::View::Detail(_)));
    assert_paints_reference(&menu, "detail");
    menu.input(press(Button::BottomLeft), 4400);
    assert_paints_reference(&menu, "detail, back focused");
    // A nearby strap of the saved type asks to replace it.
    let mut menu = Menu::new();
    let tiles = saved(&[(peer(10).identity, Connection::Live)]);
    menu.refresh(nearby(&[peer(10), replacement]), tiles, false);
    menu.input(press(Button::BottomLeft), 0);
    menu.input(press(Button::BottomLeft), 400);
    assert_eq!(menu.input(press(Button::BottomRight), 800), None);
    assert!(matches!(menu.view, menu::View::Replace(_)));
    assert_paints_reference(&menu, "replace");
    menu.input(press(Button::BottomLeft), 1200);
    assert_paints_reference(&menu, "replace, keep focused");
}

/// The text a whole frame of the menu paints, once per label, as ink boxes.
fn painted_text(menu: &Menu) -> std::vec::Vec<(std::string::String, crate::screens::chrome::Rect)> {
    use super::render::PAINTED;
    PAINTED.with(|painted| painted.borrow_mut().clear());
    for y in 0..320 {
        let mut pixels = [0; 240];
        menu.paint(&mut crate::screens::chrome::Row::new(y, &mut pixels));
    }
    let mut labels = PAINTED.with(|painted| painted.take());
    labels.sort_by_key(|label| (label.top, label.left));
    labels.dedup_by(|a, b| (a.left, a.top, a.line.text()) == (b.left, b.top, b.line.text()));
    labels
        .iter()
        .filter_map(crate::screens::chrome::layout::text)
        .collect()
}

#[test]
fn crowded_sensor_pages_never_overlap_or_clip_text() {
    use crate::screens::chrome::{CONTENT, layout::assert_apart};
    // Ten saved sensors and eight nearby, many of one type with the largest
    // numbers, so every status line names an identity.
    let tiles: [_; CHANNEL_CAPACITY] = core::array::from_fn(|n| {
        let identity = Identity {
            device_type: if n < 5 { 11 } else { n as u8 + 110 },
            device_number: u16::MAX - n as u16,
            transmission_type: 255,
        };
        (identity, Connection::Searching)
    });
    let mut menu = Menu::new();
    let found = core::array::from_fn(|n| {
        let mut discovery = peer(u16::MAX - 20 - n as u16);
        discovery.identity.transmission_type = 255;
        Some(discovery)
    });
    menu.refresh(found, saved(&tiles), true);
    for step in 0..20 {
        let what = std::format!("tiles, focus step {step}");
        let text = painted_text(&menu);
        assert!(text.len() >= 2, "{what}: {text:?}");
        assert_apart(&text, CONTENT, &what);
        menu.input(press(Button::BottomLeft), 400 * step as u64);
    }
    // A saved sensor's detail.
    menu.focus_type(11);
    menu.input(press(Button::BottomRight), 20_000);
    assert_apart(&painted_text(&menu), CONTENT, "detail");
}

#[test]
fn detail_page_shows_the_battery_in_the_colour_of_its_urgency() {
    use crate::screens::chrome::{CONTENT, RED, SECONDARY, layout::assert_apart};
    use crate::sensors::ant::{Battery, BatteryStatus};
    let cases = [
        (
            BatteryStatus::Good,
            Some(2_898),
            &b"GOOD BATTERY  2.9 V"[..],
            SECONDARY,
        ),
        (
            BatteryStatus::Low,
            Some(2_449),
            b"LOW BATTERY  2.4 V",
            AMBER,
        ),
        (
            BatteryStatus::Critical,
            Some(2_100),
            b"CRITICAL BATTERY  2.1 V",
            RED,
        ),
    ];
    for (status, millivolts, text, color) in cases {
        let mut tiles = saved(&[(peer(10).identity, Connection::Live)]);
        tiles[0].as_mut().unwrap().battery = Some(Battery {
            status,
            millivolts,
            identifier: 0,
        });
        let mut menu = Menu::new();
        menu.refresh([None; DISCOVERY_CAPACITY], tiles, false);
        menu.input(press(Button::BottomLeft), 0);
        menu.input(press(Button::BottomRight), 400);
        assert!(matches!(menu.view, menu::View::Detail(_)));
        assert!(
            shows(&menu, &CAPTION, text, 120, 184, Align::Center, color),
            "{status:?}"
        );
        assert_paints_reference(&menu, "detail with a battery");
        assert_apart(&painted_text(&menu), CONTENT, "detail with a battery");
    }
    let mut menu = Menu::new();
    menu.refresh(
        [None; DISCOVERY_CAPACITY],
        saved(&[(peer(10).identity, Connection::Live)]),
        false,
    );
    menu.input(press(Button::BottomLeft), 0);
    menu.input(press(Button::BottomRight), 400);
    assert!(
        !painted_text(&menu)
            .iter()
            .any(|(text, _)| text.contains("BATTERY"))
    );
}

fn power_detail(connection: Connection) -> Menu {
    let mut menu = Menu::new();
    menu.refresh(
        [None; DISCOVERY_CAPACITY],
        saved(&[(peer(10).identity, connection)]),
        false,
    );
    menu.input(press(Button::BottomLeft), 0);
    assert_eq!(menu.input(press(Button::BottomRight), 400), None);
    assert!(matches!(menu.view, menu::View::Detail(_)));
    menu
}

#[test]
fn a_power_meter_detail_opens_on_calibrate_and_offers_it_above_the_rest() {
    let mut menu = power_detail(Connection::Live);
    let label = |menu: &Menu, (top, height): (usize, usize), label: &[u8]| {
        let top = top + height / 2 - usize::from(BODY.top) - usize::from(BODY.cap) / 2;
        shows(menu, &BODY, label, 120, top, Align::Center, WHITE)
    };
    let [calibrate, disconnect, back] = POWER_DETAIL_BUTTONS;
    assert!(label(&menu, calibrate, b"CALIBRATE"));
    assert!(label(&menu, disconnect, b"DISCONNECT"));
    assert!(label(&menu, back, b"BACK"));
    // The page opens on the harmless button; ACT asks and stays on the page.
    assert_eq!(
        menu.input(press(Button::BottomRight), 800),
        Some(Action::Calibrate)
    );
    assert!(matches!(menu.view, menu::View::Detail(_)));
    // MOVE steps down and wraps: disconnect, back, calibrate.
    menu.input(press(Button::BottomLeft), 1200);
    menu.input(press(Button::BottomLeft), 1600);
    assert_eq!(menu.input(press(Button::BottomRight), 2000), None);
    assert!(matches!(menu.view, menu::View::Tiles), "back leaves");
    let mut menu = power_detail(Connection::Live);
    menu.input(press(Button::BottomLeft), 800);
    menu.input(press(Button::BottomLeft), 1200);
    menu.input(press(Button::BottomLeft), 1600);
    assert_eq!(
        menu.input(press(Button::BottomRight), 2000),
        Some(Action::Calibrate)
    );
    let mut menu = power_detail(Connection::Live);
    menu.input(press(Button::BottomLeft), 800);
    assert_eq!(
        menu.input(press(Button::BottomRight), 1200),
        Some(Action::Disconnect(11))
    );
}

#[test]
fn a_power_meter_detail_taps_each_of_its_three_buttons() {
    let middle = |(top, height): (usize, usize)| top + height / 2;
    let [calibrate, disconnect, back] = POWER_DETAIL_BUTTONS;
    let mut menu = power_detail(Connection::Live);
    assert_eq!(menu.tap(120, middle(calibrate)), Some(Action::Calibrate));
    assert_eq!(menu.tap(120, 160), None);
    assert_eq!(menu.tap(120, middle(back)), None);
    assert!(matches!(menu.view, menu::View::Tiles));
    let mut menu = power_detail(Connection::Live);
    assert_eq!(
        menu.tap(120, middle(disconnect)),
        Some(Action::Disconnect(11))
    );
}

#[test]
fn calibration_notes_paint_in_their_colour_and_never_collide() {
    use crate::screens::chrome::{CONTENT, layout::assert_apart};
    let notes = [
        (CalibrationNote::Calibrating, "CALIBRATING...", CYAN),
        (CalibrationNote::Done(512), "CALIBRATED  OFFSET 512", GREEN),
        (
            CalibrationNote::Done(i16::MIN),
            "CALIBRATED  OFFSET -32768",
            GREEN,
        ),
        (CalibrationNote::Failed, "CALIBRATION FAILED", AMBER),
        (CalibrationNote::NotConnected, "NOT CONNECTED", AMBER),
        (CalibrationNote::WhileRiding, "NOT DURING A RIDE", AMBER),
    ];
    for (note, text, color) in notes {
        let mut menu = power_detail(Connection::Live);
        menu.set_calibration(Some(note));
        assert_eq!(note.text().as_str(), text);
        assert!(
            shows(
                &menu,
                &CAPTION,
                text.as_bytes(),
                120,
                CALIBRATION_TOP,
                Align::Center,
                color
            ),
            "{text}"
        );
        assert_paints_reference(&menu, text);
        assert_apart(&painted_text(&menu), CONTENT, text);
    }
}

#[test]
fn a_strap_never_shows_a_calibration_note() {
    let mut menu = Menu::new();
    menu.refresh(
        [None; DISCOVERY_CAPACITY],
        saved(&[(strap(10), Connection::Live)]),
        false,
    );
    menu.set_calibration(Some(CalibrationNote::Calibrating));
    menu.input(press(Button::BottomLeft), 0);
    menu.input(press(Button::BottomRight), 400);
    assert!(
        !painted_text(&menu)
            .iter()
            .any(|(text, _)| text.contains("CALIBRAT"))
    );
}
