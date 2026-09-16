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
    let discoveries = [
        Some(Discovery {
            identity: Identity {
                device_type: 120,
                device_number: 4321,
                transmission_type: 1,
            },
            rssi: -40,
            seen_ms: 0,
        }),
        Some(Discovery {
            identity: Identity {
                device_type: 40,
                device_number: 9876,
                transmission_type: 1,
            },
            rssi: -45,
            seen_ms: 0,
        }),
        None,
        None,
        None,
        None,
        None,
        None,
    ];
    for full in [false, true] {
        let mut menu = Menu::new();
        let mut snapshots = channels.snapshots(0);
        if !full {
            snapshots[3..].fill(None);
        }
        menu.refresh(discoveries, snapshots, true, 0);
        menu.input(
            Input::Button {
                button: Button::BottomLeft,
                code: 1,
            },
            400,
        );
        let started = std::time::Instant::now();
        let mut bytes = b"P6\n240 320\n255\n".to_vec();
        for y in 0..320 {
            for x in 0..240 {
                let p = menu.pixel(x, y);
                bytes.extend_from_slice(&[0; 3]);
                let n = bytes.len();
                bytes[n - 3] = (((p >> 11) & 31) as u32 * 255 / 31) as u8;
                bytes[n - 2] = (((p >> 5) & 63) as u32 * 255 / 63) as u8;
                bytes[n - 1] = ((p & 31) as u32 * 255 / 31) as u8;
            }
        }
        println!(
            "{} slots: {} us",
            if full { 10 } else { 3 },
            started.elapsed().as_micros()
        );
        std::fs::write(
            format!(
                "{output}/sensors-{}.ppm",
                if full { "ten" } else { "three" }
            ),
            bytes,
        )
        .unwrap();
    }
}
