use device_api::input::Button;
use shell_simulator::ant::device_type;
use shell_simulator::devices::Model;
use shell_simulator::models::{C606, SMALL, WIDE};
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{NoteKind, Sim, seconds};
use shell_simulator::world::{Side, Vehicle};
use vana::screens::workout::Page;

const KINDS: [&str; 4] = ["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"];

fn world(model: Model) -> Sim {
    let mut sim = Sim::on(model);
    sim.ant_sensor(device_type::HEART_RATE, 4242);
    sim.ant_sensor(device_type::POWER, 1234);
    sim.ant_sensor(device_type::SPEED_CADENCE, 777);
    sim.ant_sensor(device_type::RADAR, 31);
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim
}

fn c606_presses() -> (Vec<(u64, Button)>, u64) {
    let mut sim = world(C606).boot();
    sim.discard_recording();
    sim.pair(&KINDS);
    sim.start_ride();
    let presses = sim
        .notes
        .iter()
        .filter(|note| note.kind == NoteKind::Input)
        .map(|note| {
            let name = note.text.strip_prefix("press ").expect("only presses");
            let button = [
                Button::TopLeft,
                Button::BottomLeft,
                Button::BottomRight,
                Button::Center,
            ]
            .into_iter()
            .find(|button| format!("{button:?}") == name)
            .expect("a button");
            (note.at_ms, button)
        })
        .collect();
    (presses, sim.now())
}

fn ride_main(model: Model) -> Sim {
    let (presses, started) = c606_presses();
    let mut sim = world(model);
    for (at, button) in presses {
        sim.advance(at - sim.now());
        sim.report(button);
    }
    sim.advance(started - sim.now());
    assert_eq!(sim.device.sdk.page(), Page::Ride);
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(20));
    for (range_m, side) in [(90.0, Side::Behind), (150.0, Side::Right)] {
        sim.vehicle(Vehicle {
            range_m,
            closing_kmh: 25.0,
            fast: false,
            side,
        });
    }
    sim.advance(seconds(2));
    sim
}

#[test]
fn the_main_page_fills_a_taller_panel() {
    let mut sim = ride_main(WIDE);
    let screen = sim.screen();
    assert_eq!((screen.width, screen.height), (240, 400));
    for text in ["KM/H", "W", "BPM", "RPM", "KM"] {
        assert!(screen.contains(text), "{text}: {}", screen.text());
    }
    let speed = screen.run("KM/H").expect("speed").y;
    let lower = screen.run("RPM").expect("cadence").y;
    assert!(lower > 320, "the readings spread down the panel: {lower}");
    assert!(speed < lower);
    sim.checkpoint("ride main on wide");
}

#[test]
fn the_main_page_lays_out_at_three_quarters() {
    let mut sim = ride_main(SMALL);
    let screen = sim.screen();
    assert_eq!((screen.width, screen.height), (180, 240));
    let lit = screen.pixels.iter().filter(|&&pixel| pixel != 0).count();
    assert!(lit > 2_000, "the page is drawn: {lit} lit pixels");
    sim.checkpoint("ride main on small");
}
