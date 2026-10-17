use super::*;
use crate::screens::sensors::MenuMessage;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::{HEART_RATE, RADAR};
use device_api::display::Geometry;
use firmware_shell::rendering::text::text;
use firmware_shell::shell::Shell;
use std::cell::RefCell;
use std::rc::Rc;
use std::vec;

const WIDTH: usize = 240;
const HEIGHT: usize = 320;

/// A panel whose last frame stays readable after the shell takes ownership.
#[derive(Clone)]
struct Panel(Rc<RefCell<Vec<u16>>>);
impl device_api::display::Display for Panel {
    fn geometry(&self) -> Geometry {
        Geometry {
            width: WIDTH,
            height: HEIGHT,
        }
    }
    fn submit(
        &mut self,
        mut row: impl FnMut(usize, &mut [u16]),
    ) -> Result<(), device_api::observation::Error> {
        let mut frame = self.0.borrow_mut();
        for (y, pixels) in frame.as_chunks_mut::<WIDTH>().0.iter_mut().enumerate() {
            row(y, pixels);
        }
        Ok(())
    }
}
impl Panel {
    fn pixel(&self, x: usize, y: usize) -> u16 {
        self.0.borrow()[y * WIDTH + x]
    }
    /// Pixels in the label's box are white exactly where the label draws.
    fn shows(&self, left: usize, top: usize, scale: usize, label: &[u8]) -> bool {
        (top..top + 7 * scale).all(|y| {
            (left..left + label.len() * 6 * scale)
                .all(|x| (self.pixel(x, y) == 0xffff) == text(x, y, left, top, scale, label))
        })
    }
}

struct Keys;
impl device_api::input::InputSource for Keys {
    fn controls(&self) -> device_api::input::Controls {
        device_api::input::Controls {
            touch: Availability::Unsupported,
            buttons: &[Button::TopLeft, Button::BottomLeft, Button::BottomRight],
        }
    }
    fn take_edge(&mut self) -> Option<device_api::input::Edge> {
        None
    }
}

struct NoPower;
impl device_api::power::Power for NoPower {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        None
    }
    fn brightness(&mut self, _: u8) -> Result<(), device_api::observation::Error> {
        Err(device_api::observation::Error::Unsupported)
    }
}

struct NoFlash;
impl device_api::storage::OwnedFlash for NoFlash {
    type Error = ();
    fn availability(&self, _: device_api::storage::Region) -> Availability {
        Availability::Unsupported
    }
    fn geometry(&self, _: device_api::storage::Region) -> device_api::storage::Geometry {
        device_api::storage::Geometry {
            capacity: 0,
            program_size: 1,
            erase_size: 1,
        }
    }
    fn read(&mut self, _: device_api::storage::Region, _: usize, _: &mut [u8]) -> Result<(), ()> {
        Err(())
    }
    fn program(&mut self, _: device_api::storage::Region, _: usize, _: &[u8]) -> Result<(), ()> {
        Err(())
    }
    fn erase(&mut self, _: device_api::storage::Region, _: usize, _: usize) -> Result<(), ()> {
        Err(())
    }
}

struct Screen {
    panel: Panel,
    shell: Shell<Panel, Keys, NoPower, NoFlash>,
}
impl Screen {
    fn new() -> Self {
        let panel = Panel(Rc::new(RefCell::new(vec![0; WIDTH * HEIGHT])));
        let shell = Shell::new(panel.clone(), Keys, NoPower, NoFlash, 0).expect("display is ready");
        Self { panel, shell }
    }

    /// Whether this call drew a frame.
    fn present(&mut self, runtime: &mut Runtime, radio: &Radio, now: u64) -> bool {
        let before = self.shell.display_submissions;
        runtime.present(&mut self.shell, now, radio, &Absent);
        self.shell.display_submissions != before
    }
}

#[test]
fn frames_follow_page_cadence_and_stop_once_suspended() {
    let mut screen = Screen::new();
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut radio = Radio::new();
    // The animated boot splash redraws twice a second.
    assert!(screen.present(&mut runtime, &radio, 0));
    assert!(!screen.present(&mut runtime, &radio, 499));
    assert!(screen.present(&mut runtime, &radio, 500));
    // The last splash frame hands over to Home, which redraws once a second.
    assert!(screen.present(&mut runtime, &radio, 6500));
    assert!(runtime.navigation.page == Page::Home);
    assert!(screen.present(&mut runtime, &radio, 7000));
    assert!(!screen.present(&mut runtime, &radio, 7999));
    // Input redraws immediately.
    runtime.input(press(Button::BottomLeft), 7999, &mut radio);
    assert!(screen.present(&mut runtime, &radio, 7999));
    runtime.set_startup_status(device_api::sensors::Startup::Probing);
    assert!(screen.present(&mut runtime, &radio, 8000));
    assert_eq!(
        runtime.menu.diagnostics().message,
        Some(MenuMessage::WaitingForCompanion)
    );
    // An unchanged status does not force another frame.
    runtime.set_startup_status(device_api::sensors::Startup::Probing);
    assert!(!screen.present(&mut runtime, &radio, 8001));

    runtime.suspend_display();
    assert!(!runtime.input_active());
    runtime.input(press(Button::BottomLeft), 9000, &mut radio);
    assert_eq!(
        runtime.navigation.home_cursor, 1,
        "input is ignored while suspended"
    );
    assert!(!screen.present(&mut runtime, &radio, 9000));
    assert!(!screen.present(&mut runtime, &radio, 20_000));
}

#[test]
fn train_shows_the_connection_check_then_the_ride_page() {
    let mut screen = Screen::new();
    let mut radio = Radio::new();
    let heart = radio.connected(HEART_RATE, 0);
    let mut runtime = restored(&[heart]);
    radio.data(HEART_RATE, [0; 8], 6000);
    assert!(screen.present(&mut runtime, &radio, 6500));
    runtime.input(press(Button::BottomRight), 7000, &mut radio);
    assert!(runtime.navigation.page == Page::Preflight);

    assert!(screen.present(&mut runtime, &radio, 7000));
    assert!(screen.panel.shows(10, 36 + 2 * 27, 1, b"SPEED --"));
    assert!(
        screen
            .panel
            .shows(10, 36 + 3 * 27, 1, b"HEART 42 CONNECTED")
    );
    assert!(screen.panel.shows(10, 36 + 6 * 27, 2, b"GPS OFF"));

    // Pages advance when a frame is drawn.
    assert!(screen.present(&mut runtime, &radio, 9499));
    assert!(runtime.navigation.page == Page::Preflight);
    assert!(screen.present(&mut runtime, &radio, 10_499));
    assert!(runtime.navigation.page == Page::Ride);
}

#[test]
fn radar_alerts_sound_only_on_the_ride_page() {
    use device_api::sound::{Pattern, Snapshot, Sound};
    struct Speaker(Vec<u8>);
    impl Sound for Speaker {
        fn patterns(&self) -> &'static [Pattern] {
            const fn pattern(id: u8, nominal_ms: u16) -> Pattern {
                Pattern {
                    id,
                    frequency_hz: 2000,
                    nominal_ms,
                    guard_ms: 0,
                }
            }
            const PATTERNS: [Pattern; 3] = [pattern(1, 100), pattern(2, 300), pattern(3, 600)];
            &PATTERNS
        }
        fn snapshot(&self) -> Snapshot {
            unimplemented!("alerts only play patterns")
        }
        fn play(&mut self, id: u8, _: u64) -> Result<(), device_api::observation::Error> {
            self.0.push(id);
            Ok(())
        }
        fn stop(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
            Ok(())
        }
    }
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    let mut speaker = Speaker(Vec::new());
    radio.connected(RADAR, 0);
    // One target 31 m behind, closing at 3 m/s.
    let close = [0x30, 1, 0, 10, 0, 0, 1, 0];
    radio.data(RADAR, close, 1000);
    tick(&mut runtime, &mut journal, &mut radio, 1000);
    runtime.alert(&mut speaker, 1000);
    assert!(speaker.0.is_empty(), "no alert away from the ride page");
    runtime.navigation.page = Page::Ride;
    radio.data(RADAR, close, 1250);
    tick(&mut runtime, &mut journal, &mut radio, 1250);
    runtime.alert(&mut speaker, 1250);
    assert_eq!(speaker.0, [2], "the longest pattern that fits");
}

#[test]
fn a_touch_lifted_after_leaving_settings_does_not_swallow_the_next_tap() {
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut radio = Radio::new();
    let touch = |x, y| Input::Touch(device_api::input::Point { x, y });
    runtime.navigation.page = Page::Settings;
    // A finger on FTP edits it, and stays down while TopLeft finishes and
    // TopLeft leaves Settings; it lifts on Home.
    runtime.input(touch(120, 86), 1000, &mut radio);
    assert!(runtime.navigation.settings_editing);
    runtime.input(press(Button::TopLeft), 2000, &mut radio);
    runtime.input(press(Button::TopLeft), 3000, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
    runtime.input(Input::Release, 3500, &mut radio);
    // Back in Settings, the first tap counts.
    runtime.navigation.page = Page::Settings;
    runtime.input(touch(120, 164), 4000, &mut radio);
    assert_eq!(runtime.navigation.settings_field, 1);
    assert!(runtime.navigation.settings_editing);
}
