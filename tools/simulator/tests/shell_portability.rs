#[cfg(test)]
mod tests {
    use device_api::input::Button;
    use device_api::input::Input;
    use device_api::input::Point;
    use device_api::observation::Availability;
    use device_api::observation::Observation;
    use device_api::positioning::Positioning;
    use firmware_shell::shell::Screen;
    use firmware_shell::shell::Shell;
    use shell_simulator::devices::*;
    #[test]
    fn same_shell_renders_geometries_maps_controls_and_consumes_wake() {
        for (width, height, buttons) in [
            (240, 320, &[Button::TopLeft][..]),
            (128, 64, &[Button::Center][..]),
        ] {
            let display = DisplayDevice::new(width, height);
            let input = InputDevice::new(buttons, Availability::Ready);
            let power = PowerDevice::new(Availability::Ready);
            let mut shell =
                Shell::new(display.clone(), input.clone(), power, Memory::default(), 0).unwrap();
            shell.settings.dim_timeout_secs = 1;
            shell.present();
            assert_eq!(display.0.borrow().pixels[0], 0x0843);
            for color in [0xffb9, 0x07ff, 0xf84e, 0xff60] {
                assert!(
                    display.0.borrow().pixels.contains(&color),
                    "development identity must remain visible at either geometry"
                );
            }
            shell.tick(1000);
            assert!(shell.dimmed());
            input.push(1000, Input::Touch(Point { x: 1, y: 1 }));
            shell.tick(1000);
            input.push(1001, Input::Touch(Point { x: 2, y: 1 }));
            shell.tick(1001);
            input.push(1002, Input::Release);
            shell.tick(1002);
            assert_eq!(shell.routed_events, 0);
            input.push(
                1003,
                Input::Button {
                    button: buttons[0],
                    code: 1,
                },
            );
            shell.tick(1003);
            shell.present();
            assert_eq!(shell.foreground, Screen::Blank);
            assert!(display.0.borrow().pixels.iter().all(|v| *v == 0));
            assert_eq!(display.0.borrow().submissions, 2);
            assert_eq!(shell.effective(), 0);
            shell.tick(10_000);
            assert_eq!(shell.effective(), 0);
            input.push(
                10_001,
                Input::Button {
                    button: buttons[0],
                    code: 1,
                },
            );
            shell.tick(10_001);
            assert_eq!(shell.foreground, Screen::Status);
            assert_eq!(shell.effective(), shell.settings.brightness);
        }
    }
    #[test]
    fn dev_animation_holds_and_respects_dim_blank_and_application() {
        let display = DisplayDevice::new(240, 320);
        let input = InputDevice::new(&[Button::Center], Availability::Ready);
        let mut shell = Shell::new(
            display.clone(),
            input.clone(),
            PowerDevice::new(Availability::Ready),
            Memory::default(),
            0,
        )
        .unwrap();
        shell.settings.dim_timeout_secs = 0;
        shell.tick(0);
        shell.present();
        let still = display.0.borrow().pixels.clone();
        for now in [100, 1000, 3599] {
            shell.tick(now);
            shell.present();
        }
        assert_eq!(shell.display_submissions, 1);
        for now in [3600, 3800, 4000] {
            shell.tick(now);
            shell.present();
            assert_ne!(display.0.borrow().pixels, still);
        }
        shell.tick(4200);
        shell.present();
        assert_eq!(display.0.borrow().pixels, still);
        assert_eq!(shell.display_submissions, 5);
        shell.settings.dim_timeout_secs = 1;
        shell.tick(8400);
        shell.present();
        assert!(shell.dimmed());
        assert_eq!(shell.display_submissions, 5);
        shell.settings.dim_timeout_secs = 0;
        shell.set_app_active(true);
        for now in [8600, 8800, 9000] {
            shell.tick(now);
            shell.present();
        }
        assert_eq!(shell.display_submissions, 5);
        shell.set_app_active(false);
        input.push(
            9000,
            Input::Button {
                button: Button::Center,
                code: 1,
            },
        );
        shell.tick(9200);
        shell.present();
        assert_eq!(shell.foreground, Screen::Blank);
        let submissions = shell.display_submissions;
        for now in [13200, 13400, 13600, 13800] {
            shell.tick(now);
            shell.present();
        }
        assert_eq!(shell.display_submissions, submissions);
        assert!(display.0.borrow().pixels.iter().all(|pixel| *pixel == 0));
    }
    #[test]
    fn lost_input_cancels_hold_optional_absence_and_failures_remain_visible() {
        let display = DisplayDevice::new(64, 64);
        let input = InputDevice::new(&[Button::Center], Availability::Ready);
        let power = PowerDevice::new(Availability::Unsupported);
        let mut shell = Shell::new(
            display.clone(),
            input.clone(),
            power.clone(),
            Memory {
                available: Availability::Unsupported,
                ..Memory::default()
            },
            0,
        )
        .unwrap();
        assert_eq!(shell.settings_source, "unavailable");
        assert!(!shell.settings_error);
        shell.settings.dim_timeout_secs = 1;
        input.push(0, Input::Touch(Point { x: 1, y: 1 }));
        shell.tick(0);
        for t in 1..=17 {
            input.push(t, Input::Release);
        }
        shell.tick(20);
        shell.tick(1020);
        assert!(shell.dimmed());
        assert_eq!(input.edges.borrow().lost, 16);
        assert_eq!(power.0.borrow().writes, 0);
        assert!(!shell.power_error);
        assert!(!shell.save());
        display.0.borrow_mut().fail_next = true;
        shell.present();
        assert!(shell.display_error);
        shell.present();
        assert!(!shell.display_error);
        assert_eq!(display.0.borrow().submissions, 1);
        assert_eq!(NoPosition.availability(), Availability::Unsupported);
        assert!(NoPosition.snapshot(100).is_none());
    }
    #[test]
    fn stale_battery_failed_power_and_background_presentation_are_distinct() {
        let display = DisplayDevice::new(80, 80);
        let input = InputDevice::new(&[Button::Center], Availability::Unsupported);
        let power = PowerDevice::new(Availability::Ready);
        power.0.borrow_mut().battery = Some((50, 3800, 10));
        let mut shell = Shell::new(
            display.clone(),
            input.clone(),
            power.clone(),
            Memory::default(),
            0,
        )
        .unwrap();
        shell.observe_position(&NoPosition, 0);
        assert_eq!(shell.positioning_availability, Availability::Unsupported);
        shell.tick(5011);
        assert!(matches!(
            shell.battery,
            Observation::Stale {
                received_ms: 10,
                ..
            }
        ));
        power.0.borrow_mut().available = Availability::Failed;
        shell.tick(5012);
        assert!(shell.power_error);
        assert_eq!(shell.battery, Observation::Unavailable);
        power.0.borrow_mut().available = Availability::Initializing;
        shell.tick(5013);
        assert_eq!(shell.power_availability, Availability::Initializing);
        assert!(!shell.power_error);
        input.push(
            5014,
            Input::Button {
                button: Button::Center,
                code: 1,
            },
        );
        shell.tick(5014);
        shell.present();
        assert_eq!(shell.foreground, Screen::Blank);
        // Background updates do not steal the foreground or submit frames.
        let before = display.0.borrow().submissions;
        shell.observe_position(&NoPosition, 6000);
        shell.tick(6000);
        shell.present();
        assert_eq!(shell.foreground, Screen::Blank);
        assert_eq!(display.0.borrow().submissions, before);
    }
}
