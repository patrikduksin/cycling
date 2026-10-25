use super::*;
use crate::screens::sensors::MenuMessage;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::{HEART_RATE, RADAR};
use device_api::display::Geometry;
use firmware_shell::shell::Shell;
use std::cell::RefCell;
use std::rc::Rc;
use std::vec;

const WIDTH: usize = 240;
const HEIGHT: usize = 320;

/// A panel whose last frame stays readable after the shell takes ownership.
/// The second field rejects that many submissions before accepting again.
#[derive(Clone)]
pub(super) struct Panel(Rc<RefCell<Vec<u16>>>, pub(super) Rc<core::cell::Cell<u32>>);
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
        if self.1.get() > 0 {
            self.1.set(self.1.get() - 1);
            return Err(device_api::observation::Error::Unavailable);
        }
        let mut frame = self.0.borrow_mut();
        for (y, pixels) in frame.as_chunks_mut::<WIDTH>().0.iter_mut().enumerate() {
            row(y, pixels);
        }
        Ok(())
    }
}
impl Panel {
    pub(super) fn pixel(&self, x: usize, y: usize) -> u16 {
        self.0.borrow()[y * WIDTH + x]
    }
    /// The whole frame is `sheet`, pixel for pixel.
    pub(super) fn shows_sheet(&self, sheet: crate::screens::power::Sheet) -> bool {
        let canvas = sheet.canvas();
        (0..HEIGHT).all(|y| (0..WIDTH).all(|x| self.pixel(x, y) == canvas.pixel(x, y, 0)))
    }
}

pub(super) struct Keys;
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

pub(super) struct NoPower;
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

pub(super) struct NoFlash;
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

pub(super) struct Screen {
    pub(super) panel: Panel,
    pub(super) shell: Shell<Panel, Keys, NoPower, NoFlash>,
}
impl Screen {
    pub(super) fn new() -> Self {
        let panel = Panel(
            Rc::new(RefCell::new(vec![0; WIDTH * HEIGHT])),
            Rc::default(),
        );
        let shell = Shell::new(panel.clone(), Keys, NoPower, NoFlash, 0).expect("display is ready");
        Self { panel, shell }
    }

    /// Whether this call drew a frame.
    pub(super) fn present(&mut self, runtime: &mut Runtime, radio: &Radio, now: u64) -> bool {
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
    // The brand flashes from VANA's first frame, whenever the device lets it run.
    assert!(screen.present(&mut runtime, &radio, 3000));
    assert!(runtime.navigation.page == Page::Boot);
    // The reveal moves at the motion cadence, then Home opens on time.
    assert!(!screen.present(&mut runtime, &radio, 3039));
    let reveal = (3040..3400)
        .step_by(10)
        .filter(|&now| screen.present(&mut runtime, &radio, now))
        .count();
    assert_eq!(reveal, 9, "ten reveal frames in 400 ms");
    assert!(runtime.navigation.page == Page::Boot);
    // Home opens as the flash ends and then redraws once a second.
    assert!(screen.present(&mut runtime, &radio, 3400));
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
fn ride_opens_ready_which_waits_for_act_to_start() {
    let mut screen = Screen::new();
    let mut radio = Radio::new();
    let heart = radio.connected(HEART_RATE, 0);
    let mut runtime = restored(&[heart]);
    radio.data(HEART_RATE, [0; 8], 6000);
    assert!(screen.present(&mut runtime, &radio, 6000));
    assert!(screen.present(&mut runtime, &radio, 6500));
    assert!(runtime.navigation.page == Page::Home);
    runtime.input(press(Button::BottomRight), 7000, &mut radio);
    assert!(runtime.navigation.page == Page::Ready);

    assert!(screen.present(&mut runtime, &radio, 7000));
    let readiness = runtime.readiness(
        crate::screens::chrome::Gps::Off,
        Some(crate::screens::chrome::Battery {
            percent: 80,
            ..Default::default()
        }),
        7000,
    );
    let title = |line: crate::screens::ready::Line| {
        std::string::String::from(readiness.checks[line as usize].title.as_str())
    };
    use crate::screens::ready::Line;
    assert_eq!(title(Line::Gps), "NO GPS");
    assert_eq!(title(Line::Battery), "BATTERY 80%");
    assert_eq!(title(Line::Sensors), "1 SENSOR LIVE");
    assert_eq!(readiness.checks[Line::Sensors as usize].hint.as_str(), "HR");
    // The panel shows the Ready screen as the view lays it out.
    let mut view = crate::screens::workout::View::new(Page::Ready, 0, None);
    view.show_ready(runtime.readiness(crate::screens::chrome::Gps::Off, None, 7000));
    view.prepare();
    let content = crate::screens::chrome::CONTENT.top..crate::screens::chrome::TOAST.top;
    assert!(
        content
            .into_iter()
            .all(|y| (0..WIDTH).all(|x| screen.panel.pixel(x, y) == view.pixel(x, y)))
    );

    // Ready stays until the rider acts: no timer moves on.
    assert!(screen.present(&mut runtime, &radio, 30_000));
    assert!(runtime.navigation.page == Page::Ready);
    runtime.input(press(Button::TopLeft), 30_500, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
}

#[test]
fn radar_beeps_sound_only_on_the_ride_page() {
    use device_api::sound::{Pattern, Snapshot, Sound, State};
    /// Played ids, and when the buzzer is free again (500 ms per sound).
    struct Speaker(Vec<u8>, u64);
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
            Snapshot {
                state: State::Elapsed,
                pattern: None,
                requested_ms: 0,
                submitted_ms: None,
                until_ms: self.1,
            }
        }
        fn play(&mut self, id: u8, now: u64) -> Result<(), device_api::observation::Error> {
            if now < self.1 {
                return Err(device_api::observation::Error::Unavailable);
            }
            self.0.push(id);
            self.1 = now + 500;
            Ok(())
        }
        fn stop(&mut self, _: u64) -> Result<(), device_api::observation::Error> {
            Ok(())
        }
    }
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    let mut speaker = Speaker(Vec::new(), 0);
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

    // The road empties, then a chime holds the buzzer when the next car
    // appears: the alarm is not spent on a refused sound, and plays once the
    // buzzer is free.
    speaker.0.clear();
    radio.data(RADAR, [0x30, 0, 0, 0, 0, 0, 0, 0], 6000);
    tick(&mut runtime, &mut journal, &mut radio, 6000);
    runtime.alert(&mut speaker, 6000);
    assert!(speaker.0.is_empty(), "an empty road is quiet");
    runtime.navigation.chime = Some(crate::app::cues::Cue::Lap);
    runtime.alert(&mut speaker, 7000);
    assert_eq!(speaker.0, [1], "the lap chime");
    radio.data(RADAR, [0x30, 2, 0, 10, 0, 0, 1, 0], 7050);
    tick(&mut runtime, &mut journal, &mut radio, 7050);
    runtime.alert(&mut speaker, 7050);
    assert_eq!(speaker.0, [1], "the buzzer is busy");
    radio.data(RADAR, [0x30, 2, 0, 10, 0, 0, 1, 0], 7500);
    tick(&mut runtime, &mut journal, &mut radio, 7500);
    runtime.alert(&mut speaker, 7500);
    assert_eq!(speaker.0, [1, 2], "the alarm, once the buzzer is free");
}

#[test]
fn a_touch_lifted_after_leaving_settings_does_not_swallow_the_next_tap() {
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut radio = Radio::new();
    let touch = |x, y| Input::Touch(device_api::input::Point { x, y });
    runtime.navigation.page = Page::Settings;
    // A tap on FTP edits it. A finger on the minus pad lowers it once and
    // stays down while TopLeft finishes and TopLeft leaves Settings; it
    // lifts on Home.
    runtime.input(touch(120, 86), 1000, &mut radio);
    runtime.input(Input::Release, 1050, &mut radio);
    assert!(runtime.navigation.settings_editing);
    runtime.input(touch(60, 237), 1500, &mut radio);
    assert_eq!(runtime.preferences.profile.ftp_watts, 210);
    runtime.input(press(Button::TopLeft), 2000, &mut radio);
    runtime.input(press(Button::TopLeft), 3000, &mut radio);
    assert!(runtime.navigation.page == Page::Home);
    runtime.input(Input::Release, 3500, &mut radio);
    // Back in Settings, the first tap counts.
    runtime.navigation.page = Page::Settings;
    runtime.input(touch(120, 140), 4000, &mut radio);
    runtime.input(Input::Release, 4050, &mut radio);
    assert_eq!(runtime.navigation.settings_field, 1);
    assert!(runtime.navigation.settings_editing);
}

#[test]
fn an_ant_sensor_stands_for_the_ble_role_in_the_order_the_ride_prefers() {
    use super::super::presentation::ant_stands_for as stands;
    use crate::screens::sensors::Glance;
    use crate::sensors::ant::device_type::{CADENCE, SPEED, SPEED_CADENCE};
    let ant = |kind, live| Glance::ble(kind, live, None);
    for kind in [CADENCE, SPEED_CADENCE] {
        assert!(stands(CADENCE, false, true, &[ant(kind, true)]));
        assert!(
            !stands(CADENCE, true, true, &[ant(kind, true)]),
            "BLE first"
        );
        assert!(
            !stands(CADENCE, false, true, &[ant(kind, false)]),
            "not live"
        );
        assert!(
            !stands(CADENCE, false, false, &[ant(kind, true)]),
            "live but never turned, such as a combined sensor without a magnet"
        );
    }
    assert!(!stands(CADENCE, false, true, &[ant(SPEED, true)]));
    assert!(!stands(CADENCE, false, true, &[]));
    assert!(stands(HEART_RATE, true, false, &[ant(HEART_RATE, true)]));
    assert!(!stands(HEART_RATE, false, true, &[ant(CADENCE, true)]));
}
