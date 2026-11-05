//! Render each sensor page with deterministic fixtures into PPM previews.
use device_api::ant::{Discovery, Identity};
use device_api::input::{Button, Input};
use vana::screens::sensors::{CalibrationNote, Connection, Menu, MenuMessage, Page, Selected};
use vana::sensors::ant::{Battery, BatteryStatus, device_type::*};

fn main() {
    let output = std::env::args().nth(1).expect("output directory");
    std::fs::create_dir_all(&output).unwrap();
    let mut now = 0;
    let battery = |status, millivolts| {
        Some(Battery {
            status,
            millivolts,
            identifier: 0,
        })
    };
    let saved = [
        (
            HEART_RATE,
            100,
            Connection::Live,
            battery(BatteryStatus::Good, Some(2_950)),
        ),
        (
            POWER,
            5512,
            Connection::Live,
            battery(BatteryStatus::Ok, Some(2_810)),
        ),
        (SPEED, 2210, Connection::NotFound, None),
        (
            RADAR,
            881,
            Connection::Live,
            battery(BatteryStatus::Good, None),
        ),
        (CADENCE, 3318, Connection::Searching, None),
        (
            SPEED_CADENCE,
            7710,
            Connection::Live,
            battery(BatteryStatus::Low, Some(2_420)),
        ),
        (SHIFTING, 4410, Connection::Live, None),
        (FITNESS_EQUIPMENT, 1201, Connection::WaitingForRadio, None),
        (BIKE_LIGHTS, 6031, Connection::NotFound, None),
        (7, 999, Connection::Live, None),
    ];
    let tiles = |count: usize| -> [Option<Selected>; 10] {
        std::array::from_fn(|slot| {
            let &(kind, number, connection, battery) = saved.get(slot).filter(|_| slot < count)?;
            Some(Selected {
                identity: identity(kind, number),
                connection,
                battery,
            })
        })
    };
    let found = |kinds: &[(u8, u16)]| -> [Option<Discovery>; 8] {
        std::array::from_fn(|index| {
            let &(kind, number) = kinds.get(index)?;
            Some(Discovery {
                identity: identity(kind, number),
                rssi: -40,
                seen_ms: 0,
            })
        })
    };
    let nearby = found(&[
        (HEART_RATE, 4321),
        (RADAR, 9021),
        (POWER, 7788),
        (CADENCE, 2345),
    ]);

    let mut menu = Menu::new();
    menu.refresh(nearby, tiles(3), false);
    menu.open();
    render(&menu, &output, "sensors-three");
    let mut ten = Menu::new();
    ten.refresh(nearby, tiles(10), false);
    ten.open();
    focus(&mut ten, Some(SPEED_CADENCE), &mut now);
    render(&ten, &output, "sensors-ten-scrolled");
    focus(&mut ten, None, &mut now);
    render(&ten, &output, "sensors-ten-add-row");

    // ADD SENSOR searches as it opens; the runtime reports the search.
    focus(&mut menu, None, &mut now);
    press(&mut menu, Button::BottomRight, &mut now);
    menu.search_started();
    menu.refresh(nearby, tiles(1), true);
    menu.set_message(MenuMessage::Scanning);
    render(&menu, &output, "add-searching");
    let mut eight = Menu::new();
    let many = [
        (HEART_RATE, 4321),
        (RADAR, 9021),
        (POWER, 7788),
        (CADENCE, 2345),
        (SPEED, 6020),
        (BIKE_LIGHTS, 3301),
        (SHIFTING, 65535),
        (FITNESS_EQUIPMENT, 1201),
    ];
    eight.refresh(found(&many), tiles(0), false);
    eight.open();
    eight.set_message(MenuMessage::ScanDone);
    focus(&mut eight, Some(SHIFTING), &mut now);
    render(&eight, &output, "add-eight-scrolled");
    let mut empty = Menu::new();
    empty.refresh([None; 8], tiles(0), false);
    empty.open();
    empty.refresh([None; 8], tiles(0), true);
    empty.set_message(MenuMessage::Scanning);
    render(&empty, &output, "add-empty-searching");
    empty.refresh([None; 8], tiles(0), false);
    empty.set_message(MenuMessage::ScanDone);
    render(&empty, &output, "add-none-found");
    empty.set_message(MenuMessage::RadioUnavailable);
    render(&empty, &output, "add-radio-unavailable");

    // A nearby heart strap of the saved type asks to replace it.
    focus(&mut menu, Some(HEART_RATE), &mut now);
    press(&mut menu, Button::BottomRight, &mut now);
    assert_eq!(menu.diagnostics().page, Page::Replace);
    render(&menu, &output, "replace");

    let mut detail = Menu::new();
    detail.refresh(nearby, tiles(3), false);
    detail.open();
    focus(&mut detail, Some(POWER), &mut now);
    press(&mut detail, Button::BottomRight, &mut now);
    render(&detail, &output, "detail-power");
    detail.set_calibration(Some(CalibrationNote::Done(-12)));
    render(&detail, &output, "detail-power-calibrated");
    detail.set_calibration(Some(CalibrationNote::Failed));
    render(&detail, &output, "detail-power-failed");
    detail.set_calibration(None);
    press(&mut detail, Button::BottomLeft, &mut now);
    assert_eq!(detail.diagnostics().page, Page::Forget);
    render(&detail, &output, "forget-power");
    press(&mut detail, Button::TopLeft, &mut now);
    press(&mut detail, Button::TopLeft, &mut now);
    focus(&mut detail, Some(HEART_RATE), &mut now);
    press(&mut detail, Button::BottomRight, &mut now);
    render(&detail, &output, "detail-heart");
    press(&mut detail, Button::TopLeft, &mut now);
    focus(&mut detail, Some(SPEED), &mut now);
    press(&mut detail, Button::BottomRight, &mut now);
    render(&detail, &output, "detail-speed-not-found");
}

fn identity(device_type: u8, device_number: u16) -> Identity {
    Identity {
        device_type,
        device_number,
        transmission_type: 1,
    }
}

fn press(menu: &mut Menu, button: Button, now: &mut u64) {
    *now += 1_200;
    menu.input(Input::Button { button, code: 1 }, *now);
}

/// MOVE until the sensor of this type is focused, or ADD SENSOR's row for `None`.
fn focus(menu: &mut Menu, kind: Option<u8>, now: &mut u64) {
    for _ in 0..20 {
        if menu.diagnostics().focus.map(|f| f.device_type) == kind {
            return;
        }
        press(menu, Button::BottomLeft, now);
    }
    panic!("fixture focus {kind:?} is not reachable");
}

fn render(menu: &Menu, output: &str, name: &str) {
    let mut bytes = b"P6\n240 320\n255\n".to_vec();
    for y in 0..320 {
        for x in 0..240 {
            let p = menu.pixel(x, y);
            bytes.extend_from_slice(&[
                (((p >> 11) & 31) as u32 * 255 / 31) as u8,
                (((p >> 5) & 63) as u32 * 255 / 63) as u8,
                ((p & 31) as u32 * 255 / 31) as u8,
            ]);
        }
    }
    std::fs::write(format!("{output}/{name}.ppm"), bytes).unwrap();
}
