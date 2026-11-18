use device_api::input::{Fixture, Fixtures, Input};
use device_api::observation::Availability;
use firmware_shell::shell::Shell;
use shell_simulator::devices::DisplayDevice;
use shell_simulator::devices::InputDevice;
use shell_simulator::devices::Memory;
use shell_simulator::devices::PowerDevice;
fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--video") {
        let mut failed = false;
        for dir in &args[2..] {
            match shell_simulator::recording::render_video(std::path::Path::new(dir)) {
                Ok(file) => println!("{dir}/{file}"),
                Err(error) => {
                    eprintln!("{dir}: {error}");
                    failed = true;
                }
            }
        }
        std::process::exit(i32::from(failed));
    }
    if args.iter().any(|a| a == "--terminal") {
        let value = |name: &str| {
            args.iter()
                .position(|a| a == name)
                .and_then(|i| args.get(i + 1))
        };
        let Some(root) = value("--state-dir") else {
            eprintln!("--terminal requires an explicit disposable --state-dir");
            std::process::exit(2);
        };
        let width = value("--width")
            .map(|s| s.parse().expect("invalid width"))
            .unwrap_or(240);
        let height = value("--height")
            .map(|s| s.parse().expect("invalid height"))
            .unwrap_or(320);
        let restart_probe_ms = value("--restart-probe-ms")
            .map(|s| s.parse().expect("invalid restart probe"))
            .unwrap_or(0);
        if let Err(error) = shell_simulator::session::terminal(
            std::path::Path::new(root),
            width,
            height,
            restart_probe_ms,
        ) {
            eprintln!("virtual terminal: {error}");
            std::process::exit(1);
        }
        return;
    }

    for (width, height, id) in [(240, 320, 0), (128, 64, 3)] {
        let display = DisplayDevice::new(width, height);
        let buttons = Fixtures::new(&[Fixture { id, role: None }]);
        let input = InputDevice::new(buttons, Availability::Unsupported);
        let power = PowerDevice::new(Availability::Unsupported);
        let memory = Memory {
            available: Availability::Unsupported,
            ..Memory::default()
        };
        let journal = memory.journal();
        let mut shell =
            Shell::new(display.clone(), input.clone(), power, memory, journal, 0).unwrap();
        shell.observe_position(&device_api::Absent, 0);
        shell.tick(0);
        shell.present();
        let lit = display
            .0
            .borrow()
            .pixels
            .iter()
            .filter(|v| **v != 0)
            .count();
        input.push(10, Input::Button { id, code: 1 });
        shell.tick(10);
        shell.present();
        println!(
            "geometry={width}x{height} control={:?} first_lit_pixels={lit} screen={:?} submissions={} power=unsupported storage={} positioning=unsupported ble=unsupported ant=unsupported network=unsupported",
            id,
            shell.foreground,
            display.0.borrow().submissions,
            shell.settings_source.name()
        );
    }
}
