//! Render the sensor page with deterministic fixtures into PPM previews.
use device_api::ant::{Discovery, Event, Identity};
use device_api::input::{Button, Input};
use vana::screens::sensors::Menu;
fn main() {
    let output = std::env::args().nth(1).expect("output directory");
    std::fs::create_dir_all(&output).unwrap();
    let mut channels = firmware_services::ant::Channels::new();
    for (number, kind) in [120, 11, 123, 40, 122, 121, 34, 17, 128, 35]
        .into_iter()
        .enumerate()
    {
        let peer = Identity {
            device_type: kind,
            device_number: number as u16 + 100,
            transmission_type: 1,
        };
        channels.connect(peer, 0).unwrap();
        channels.receive(Event::Connected(peer), 0);
        channels.receive(
            Event::Data {
                device_type: kind,
                data: [0; 8],
            },
            0,
        );
    }
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
        let mut snapshots = channels.snapshots(0);
        if !full {
            snapshots[3..].fill(None);
        }
        menu.refresh(discoveries, snapshots, true, 0);
        menu.set_message(b"SCANNING...");
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
    menu.refresh(discoveries, channels.snapshots(0), false, 0);
    menu.set_message(b"SCAN DONE - PICK");
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
    let pending = Identity {
        device_type: 120,
        device_number: 65535,
        transmission_type: 1,
    };
    menu.request_pending(pending);
    render(&menu, &output, "pending");
    menu.request_failed(pending, b"RADIO UNAVAILABLE");
    render(&menu, &output, "failed");
    press(&mut menu, Button::BottomRight, &mut now);
    render(&menu, &output, "failed-detail");
    press(&mut menu, Button::TopLeft, &mut now);
    focus(&mut menu, 11, &mut now);
    press(&mut menu, Button::BottomRight, &mut now);
    render(&menu, &output, "long-identity-replacement");
    let mut empty = Menu::new();
    empty.refresh([None; 8], [None; 10], true, 0);
    render(&empty, &output, "searching");
    empty.refresh([None; 8], [None; 10], false, 0);
    render(&empty, &output, "empty");
    empty.set_message(b"RADIO UNAVAILABLE");
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
