//! Render the sensor page with deterministic fixtures into PPM previews.
use device_api::ant::{Discovery, Identity};
use device_api::input::{Button, Input};
use vana::screens::sensors::{Connection, Menu, MenuMessage, Selected};
fn main() {
    let output = std::env::args().nth(1).expect("output directory");
    std::fs::create_dir_all(&output).unwrap();
    let saved: [Option<Selected>; 10] = std::array::from_fn(|number| {
        let kind = [120, 11, 123, 40, 122, 121, 34, 17, 128, 35][number];
        Some(Selected {
            identity: Identity {
                device_type: kind,
                device_number: number as u16 + 100,
                transmission_type: 1,
            },
            connection: [
                Connection::Live,
                Connection::Searching,
                Connection::NotFound,
            ][number % 3],
            battery: None,
        })
    });
    let discoveries = std::array::from_fn(|index| {
        Some(Discovery {
            identity: Identity {
                device_type: [120, 40, 11, 123, 122, 35, 34, 17][index],
                device_number: 4321 + index as u16,
                transmission_type: 1,
            },
            rssi: -40,
            seen_ms: 0,
        })
    });
    for full in [false, true] {
        let mut menu = Menu::new();
        let mut tiles = saved;
        if !full {
            tiles[3..].fill(None);
        }
        menu.refresh(discoveries, tiles, true);
        menu.set_message(MenuMessage::Scanning);
        menu.input(
            Input::Button {
                button: Button::BottomLeft,
                code: 1,
            },
            400,
        );
        render(&menu, &output, if full { "ten" } else { "three" });
    }
    let mut menu = Menu::new();
    menu.refresh(discoveries, saved, false);
    menu.set_message(MenuMessage::ScanDone);
    let mut now = 0;
    focus(&mut menu, 1, &mut now);
    render(&menu, &output, "focused-heart");
    press(&mut menu, Button::BottomRight, &mut now);
    render(&menu, &output, "detail");
    press(&mut menu, Button::TopLeft, &mut now);
    focus(&mut menu, 11, &mut now);
    render(&menu, &output, "nearby-heart");
    press(&mut menu, Button::BottomRight, &mut now);
    render(&menu, &output, "replacement");
    press(&mut menu, Button::TopLeft, &mut now);
    focus(&mut menu, 12, &mut now);
    render(&menu, &output, "nearby-radar");
    focus(&mut menu, 0, &mut now);
    render(&menu, &output, "refresh");
    focus(&mut menu, 6, &mut now);
    render(&menu, &output, "combined-sensor");
    focus(&mut menu, 3, &mut now);
    render(&menu, &output, "not-found");
    press(&mut menu, Button::BottomRight, &mut now);
    render(&menu, &output, "not-found-detail");
    press(&mut menu, Button::TopLeft, &mut now);
    let mut empty = Menu::new();
    empty.refresh([None; 8], [None; 10], true);
    render(&empty, &output, "searching");
    empty.refresh([None; 8], [None; 10], false);
    render(&empty, &output, "empty");
    empty.set_message(MenuMessage::RadioUnavailable);
    render(&empty, &output, "unavailable");
}

fn press(menu: &mut Menu, button: Button, now: &mut u64) {
    *now += 400;
    menu.input(Input::Button { button, code: 1 }, *now);
}

fn focus(menu: &mut Menu, cursor: usize, now: &mut u64) {
    for _ in 0..20 {
        if menu.diagnostics().cursor == cursor {
            return;
        }
        press(menu, Button::BottomLeft, now);
    }
    panic!("fixture cursor {cursor} is not reachable");
}

fn render(menu: &Menu, output: &str, name: &str) {
    let started = std::time::Instant::now();
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
    println!("{name}: {} us", started.elapsed().as_micros());
    std::fs::write(format!("{output}/sensors-{name}.ppm"), bytes).unwrap();
}
