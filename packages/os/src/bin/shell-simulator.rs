use cycling_os::{
    capabilities::{Availability, Button, Input},
    shell::Shell,
    simulator::{DisplayDevice, InputDevice, Memory, PowerDevice},
};
fn main() {
    for (width, height, buttons) in [
        (240, 320, &[Button::TopLeft][..]),
        (128, 64, &[Button::Center][..]),
    ] {
        let display = DisplayDevice::new(width, height);
        let input = InputDevice::new(buttons, Availability::Unsupported);
        let power = PowerDevice::new(Availability::Unsupported);
        let mut shell = Shell::new(
            display.clone(),
            input.clone(),
            power,
            Memory {
                available: Availability::Unsupported,
                ..Memory::default()
            },
            0,
        )
        .unwrap();
        shell.observe_position(&cycling_os::simulator::NoPosition, 0);
        shell.tick(0);
        shell.present();
        let lit = display
            .0
            .borrow()
            .pixels
            .iter()
            .filter(|v| **v != 0)
            .count();
        input.push(
            10,
            Input::Button {
                button: buttons[0],
                code: 1,
            },
        );
        shell.tick(10);
        shell.present();
        println!(
            "geometry={width}x{height} control={:?} first_lit_pixels={lit} screen={:?} submissions={} power=unsupported storage={} positioning=unsupported ble=unsupported ant=unsupported network=unsupported",
            buttons[0],
            shell.foreground,
            display.0.borrow().submissions,
            shell.settings_source
        );
    }
}
