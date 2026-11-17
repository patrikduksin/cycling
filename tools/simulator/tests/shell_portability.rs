#[cfg(test)]
mod tests {
    use device_api::input::Button;
    use device_api::input::Input;
    use device_api::input::Point;
    use device_api::observation::Availability;
    use device_api::observation::Observation;
    use device_api::positioning::Positioning;
    use firmware_shell::rendering::row::Row;
    use firmware_shell::shell::Drawn;
    use firmware_shell::shell::Picture;
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
        assert_eq!(
            shell.settings_source,
            firmware_shell::preferences::Source::Unavailable
        );
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

    #[test]
    fn a_frame_counts_as_shown_when_any_pixel_changed() {
        let display = DisplayDevice::new(240, 320);
        let input = InputDevice::new(&[Button::TopLeft], Availability::Ready);
        let power = PowerDevice::new(Availability::Ready);
        let mut shell = Shell::new(display, input, power, Memory::default(), 0).unwrap();
        let draw = |shell: &mut Shell<_, _, _, _>, flipped: &[usize]| {
            shell.draw_rows(|y, pixels| {
                for (x, pixel) in pixels.iter_mut().enumerate() {
                    *pixel = (y * 240 + x) as u16;
                    if flipped.contains(&(y * 240 + x)) {
                        *pixel ^= 0x8000;
                    }
                }
            });
            shell.take_frame()
        };
        let first = draw(&mut shell, &[]);
        assert!(first.is_some(), "the first frame is a change");
        assert_eq!(draw(&mut shell, &[]), None, "the same pixels again");
        let flipped = draw(&mut shell, &[1, 3]);
        assert!(
            flipped.is_some() && flipped != first,
            "the top bit of two odd pixels"
        );
        let other = draw(&mut shell, &[241, 76_799]);
        assert!(other.is_some() && other != flipped, "two pixels far apart");
        shell.forget_frame();
        assert_eq!(
            draw(&mut shell, &[241, 76_799]),
            other,
            "a change once forgotten"
        );
    }

    struct Solid {
        color: u16,
        hashes_color: bool,
    }
    impl core::hash::Hash for Solid {
        fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
            if self.hashes_color {
                self.color.hash(state);
            }
        }
    }
    impl Picture for Solid {
        const SIZE: (usize, usize) = (240, 320);
        fn paint(&self, row: &mut Row) {
            row.fill(self.color);
        }
    }
    fn solid(color: u16) -> Solid {
        Solid {
            color,
            hashes_color: true,
        }
    }

    #[test]
    fn a_picture_on_the_panel_is_sent_again_only_when_something_else_drew() {
        let display = DisplayDevice::new(240, 320);
        let input = InputDevice::new(&[Button::TopLeft], Availability::Ready);
        let power = PowerDevice::new(Availability::Ready);
        let mut shell =
            Shell::new(display.clone(), input.clone(), power, Memory::default(), 0).unwrap();
        shell.set_app_active(true);
        let submissions = || display.0.borrow().submissions;
        assert_eq!(shell.draw(&solid(1), 0), Drawn::Sent, "the first picture");
        assert_eq!(
            shell.draw(&solid(1), 10),
            Drawn::Unchanged,
            "the same again"
        );
        assert_eq!(shell.draw(&solid(2), 20), Drawn::Sent, "another picture");
        shell.fill(7);
        assert_eq!(
            shell.draw(&solid(2), 30),
            Drawn::Sent,
            "after another frame"
        );
        shell.forget_panel();
        assert_eq!(shell.draw(&solid(2), 40), Drawn::Sent, "once forgotten");
        display.0.borrow_mut().fail_next = true;
        assert_eq!(shell.draw(&solid(3), 50), Drawn::Failed);
        assert_eq!(
            shell.draw(&solid(3), 60),
            Drawn::Sent,
            "after a failed frame"
        );
        input.push(
            70,
            Input::Button {
                button: Button::TopLeft,
                code: 1,
            },
        );
        shell.tick(70);
        let before = submissions();
        assert_eq!(
            shell.draw(&solid(3), 70),
            Drawn::Sent,
            "a press waits for a frame"
        );
        assert_eq!(shell.draw(&solid(3), 80), Drawn::Unchanged, "once answered");
        assert_eq!(submissions(), before + 1);
        assert!(display.0.borrow().pixels.iter().all(|&pixel| pixel == 3));
    }

    #[derive(Hash)]
    struct Split {
        top: u16,
        bottom: u16,
        split: usize,
    }
    impl Picture for Split {
        const SIZE: (usize, usize) = (240, 320);
        fn paint(&self, row: &mut Row) {
            row.fill(if row.y() < self.split {
                self.top
            } else {
                self.bottom
            });
        }
        fn parts(&self, parts: &mut firmware_shell::shell::Parts) {
            parts.add(0..self.split, &self.top);
            parts.add(self.split..320, &self.bottom);
        }
    }

    #[test]
    fn a_frame_sends_the_bands_whose_parts_changed_and_digests_the_whole_panel() {
        let shell = |display: &DisplayDevice| {
            let input = InputDevice::new(&[Button::TopLeft], Availability::Ready);
            let power = PowerDevice::new(Availability::Ready);
            let mut shell =
                Shell::new(display.clone(), input, power, Memory::default(), 0).unwrap();
            shell.set_app_active(true);
            shell
        };
        let display = DisplayDevice::new(240, 320);
        let mut panel = shell(&display);
        let split = |top, bottom, split| Split { top, bottom, split };
        assert_eq!(panel.draw(&split(1, 2, 10), 0), Drawn::Sent);
        {
            let mut frame = display.0.borrow_mut();
            frame.probe = Some(shell_simulator::probe::Probe::new(240, &frame.pixels));
        }
        assert_eq!(panel.draw(&split(3, 2, 10), 10), Drawn::Sent);
        assert_eq!(panel.draw(&split(3, 2, 10), 20), Drawn::Unchanged);
        assert_eq!(panel.draw(&split(3, 2, 14), 30), Drawn::Sent);
        assert_eq!(panel.draw(&split(3, 4, 14), 40), Drawn::Sent);
        let rows: Vec<usize> = display
            .0
            .borrow_mut()
            .probe
            .take()
            .unwrap()
            .changes
            .iter()
            .map(|change| change.rows)
            .collect();
        assert_eq!(
            rows,
            [12, 8, 308],
            "the bands the top touches; the moved split's; the bottom's"
        );
        let whole = DisplayDevice::new(240, 320);
        let mut fresh = shell(&whole);
        assert_eq!(fresh.draw(&split(3, 4, 14), 0), Drawn::Sent);
        assert_eq!(
            panel.take_frame(),
            fresh.take_frame(),
            "the panel's digest, whichever rows were sent"
        );
    }

    #[test]
    fn nothing_saves_settings_from_a_format_erase_until_the_restart() {
        use firmware_shell::preferences::{AppPreferences, Source};
        let display = DisplayDevice::new(240, 320);
        let input = InputDevice::new(&[Button::TopLeft], Availability::Ready);
        let power = PowerDevice::new(Availability::Ready);
        let mut shell = Shell::new(display, input, power, Memory::default(), 0).unwrap();
        assert!(shell.save());
        assert_eq!(shell.settings_source, Source::Current);
        assert!(shell.erase_settings());
        assert_eq!(shell.settings_source, Source::Foreign);
        let app = AppPreferences::new(&[1]).unwrap();
        assert!(!shell.save(), "an erased journal stays blank");
        assert!(!shell.save_app_preferences(app));
        assert!(!shell.save_connectivity());
        assert!(shell.settings_error);
    }

    #[test]
    #[should_panic(expected = "a skipped frame hid a change")]
    fn the_simulated_panel_catches_a_picture_whose_key_misses_what_it_paints() {
        let display = DisplayDevice::new(240, 320);
        let input = InputDevice::new(&[Button::TopLeft], Availability::Ready);
        let power = PowerDevice::new(Availability::Ready);
        let mut shell = Shell::new(display, input, power, Memory::default(), 0).unwrap();
        let lying = |color| Solid {
            color,
            hashes_color: false,
        };
        assert_eq!(shell.draw(&lying(1), 0), Drawn::Sent);
        shell.draw(&lying(2), 10);
    }
}
