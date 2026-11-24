use super::*;
use crate::app::navigation::To;
use crate::screens::sensors::MenuMessage;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::{HEART_RATE, RADAR};
use device_api::display::Geometry;
use firmware_shell::rendering::row::Shown;
use firmware_shell::shell::Shell;
use firmware_shell::storage::Store;
use std::boxed::Box;
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
    fn availability(&self) -> device_api::observation::Availability {
        device_api::observation::Availability::Ready
    }
    fn format(&self) -> device_api::display::Format {
        device_api::display::Format::Rgb565
    }
    fn check(&mut self, _row: impl FnMut(usize, &mut [u16])) {}
    fn geometry(&self) -> Geometry {
        Geometry {
            width: WIDTH,
            height: HEIGHT,
        }
    }
    fn submit(
        &mut self,
        rows: device_api::display::Rows,
        mut row: impl FnMut(usize, &mut [u16]),
    ) -> Result<(), device_api::observation::Error> {
        if self.1.get() > 0 {
            self.1.set(self.1.get() - 1);
            return Err(device_api::observation::Error::Unavailable);
        }
        let mut frame = self.0.borrow_mut();
        let lines = frame.as_chunks_mut::<WIDTH>().0;
        for y in rows.runs(HEIGHT).flatten() {
            row(y, &mut lines[y]);
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
        let device = &crate::screens::C606;
        let laid = crate::compose::builtin::Laid::new(
            sheet.screen(),
            device,
            Default::default(),
            Default::default(),
            None,
        )
        .expect("every sheet has a tree");
        let (canvas, cues) = (
            sheet.canvas(&laid),
            crate::screens::power::Sheet::cues(&laid),
        );
        let look = crate::screens::chrome::Look::PLAIN;
        (0..HEIGHT).all(|y| {
            (0..WIDTH).all(|x| {
                let under = canvas.pixel(look, x, y, Shown(0));
                let shown = cues
                    .iter()
                    .fold(under, |under, cue| cue.pixel(look, x, y, under, device));
                self.pixel(x, y) == shown.0
            })
        })
    }
}

pub(super) struct Keys;
impl device_api::input::InputSource for Keys {
    fn controls(&self) -> device_api::input::Controls {
        device_api::input::Controls {
            touch: Availability::Unsupported,
            buttons: crate::screens::limits::C606_BUTTONS,
        }
    }
    fn take_edge(&mut self) -> Option<device_api::input::Edge> {
        None
    }
}

pub(super) struct Screen {
    pub(super) panel: Panel,
    pub(super) shell: Shell<Panel, Keys, Absent, Absent>,
}
impl Screen {
    pub(super) fn new() -> Self {
        let panel = Panel(
            Rc::new(RefCell::new(vec![0; WIDTH * HEIGHT])),
            Rc::default(),
        );
        let shell = Shell::new(
            panel.clone(),
            Keys,
            Absent,
            Absent,
            Box::leak(Box::new(Store::new(Absent))),
            0,
        )
        .expect("display is ready");
        Self { panel, shell }
    }

    /// Whether this call drew a frame.
    pub(super) fn present(&mut self, runtime: &mut Runtime, radio: &Radio, now: u64) -> bool {
        let before = self.shell.display_submissions;
        runtime.frame(&mut self.shell, now, radio, &Absent);
        self.shell.display_submissions != before
    }
}

#[test]
fn frames_follow_page_cadence_and_stop_once_suspended() {
    let mut screen = Screen::new();
    let mut runtime = booted(Profile::HeartRate);
    let mut radio = Radio::new();
    // The brand flashes from VANA's first frame, whenever the device lets it run.
    assert!(screen.present(&mut runtime, &radio, 3000));
    assert!(runtime.navigation.page() == Page::Boot);
    // The reveal moves at the motion cadence, then Home opens on time.
    assert!(!screen.present(&mut runtime, &radio, 3039));
    let reveal = (3040..3400)
        .step_by(10)
        .filter(|&now| screen.present(&mut runtime, &radio, now))
        .count();
    assert_eq!(reveal, 9, "ten reveal frames in 400 ms");
    assert!(runtime.navigation.page() == Page::Boot);
    assert!(screen.present(&mut runtime, &radio, 3400));
    assert!(runtime.navigation.page() == Page::Home);
    assert!(!screen.present(&mut runtime, &radio, 7000));
    assert!(!screen.present(&mut runtime, &radio, 7999));
    // Input redraws immediately.
    runtime.input_received(press(Role::Move), 7999, 7999, &mut radio);
    assert!(screen.present(&mut runtime, &radio, 7999));
    runtime.set_startup_status(device_api::sensors::Startup::Probing);
    assert!(!screen.present(&mut runtime, &radio, 8000));
    assert_eq!(
        runtime.menu.diagnostics().message,
        Some(MenuMessage::WaitingForCompanion)
    );

    runtime.suspend_display();
    assert!(!runtime.input_active());
    runtime.input_received(press(Role::Move), 9000, 9000, &mut radio);
    assert_eq!(runtime.focus_index(), 0, "input is ignored while suspended");
    assert!(!screen.present(&mut runtime, &radio, 9000));
    assert!(!screen.present(&mut runtime, &radio, 20_000));
}

#[test]
fn a_new_startup_status_draws_at_once_where_it_shows() {
    let mut screen = Screen::new();
    let mut runtime = booted(Profile::HeartRate);
    let mut radio = Radio::new();
    assert!(screen.present(&mut runtime, &radio, 3000));
    assert!(screen.present(&mut runtime, &radio, 3400));
    runtime.input_received(press(Role::Move), 4000, 4000, &mut radio);
    runtime.input_received(press(Role::Act), 4400, 4400, &mut radio);
    assert!(screen.present(&mut runtime, &radio, 4400));
    assert!(runtime.navigation.page() == Page::Sensors);
    runtime.set_startup_status(device_api::sensors::Startup::Probing);
    assert!(screen.present(&mut runtime, &radio, 4401));
    assert_eq!(
        runtime.menu.diagnostics().message,
        Some(MenuMessage::WaitingForCompanion)
    );
    runtime.set_startup_status(device_api::sensors::Startup::Probing);
    assert!(!screen.present(&mut runtime, &radio, 4402));
}

#[test]
fn act_on_home_starts_the_ride_at_once() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.go(To::Page(Page::Home));
    let mut screen = Screen::new();
    assert!(screen.present(&mut runtime, &radio, 1000));
    runtime.input_received(press(Role::Act), 2000, 2000, &mut radio);
    assert!(runtime.navigation.page() == Page::Ride);
    tick(&mut runtime, &mut journal, &mut radio, 2100);
    assert!(runtime.ride.active().riding());
}

fn start_from_home_planned(stop: usize) -> Runtime {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.training.selected = Some(crate::workout::library::Id::BuiltIn(0));
    runtime.navigation.go(To::Page(Page::Home));
    runtime.focus_stop(stop);
    runtime.input_received(press(Role::Act), 2000, 2000, &mut radio);
    assert!(runtime.navigation.page() == Page::Ride);
    tick(&mut runtime, &mut journal, &mut radio, 2100);
    assert!(runtime.ride.active().riding());
    runtime
}

#[test]
fn home_opens_on_today_s_workout_whose_card_starts_it() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.training.selected = Some(crate::workout::library::Id::BuiltIn(0));
    runtime.navigation.go(To::Page(Page::Home));
    runtime.compose_shown();
    assert_eq!(runtime.focus_index(), 1, "the card, under the widget");
    let runtime = start_from_home_planned(1);
    assert!(runtime.training.player.is_some(), "the workout plays");
    assert!(runtime.workout_pages());
}

#[test]
fn just_ride_rides_free_and_keeps_today_s_workout_planned() {
    let mut runtime = start_from_home_planned(2);
    assert!(runtime.training.player.is_none());
    assert!(!runtime.workout_pages(), "no workout page in the deck");
    runtime.leave_workout();
    assert!(
        runtime.chosen_name().is_some(),
        "still planned once it ends"
    );
}

#[test]
fn a_ridden_workout_is_no_longer_planned_once_its_ride_ends() {
    let mut runtime = start_from_home_planned(1);
    runtime.leave_workout();
    assert!(runtime.chosen_name().is_none());
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
    runtime.navigation.go(To::Page(Page::Ride));
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
    runtime.navigation.chime(crate::app::cues::Cue::Lap);
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
    let mut runtime = booted(Profile::HeartRate);
    let mut radio = Radio::new();
    let touch = |x, y| Input::Touch(device_api::input::Point { x, y });
    runtime.navigation.go(To::Page(Page::Settings));
    // A tap on FTP edits it. A finger on the minus pad lowers it once and
    // stays down while TopLeft finishes and TopLeft leaves Settings; it
    // lifts on Home.
    runtime.input_received(touch(120, 86), 1000, 1000, &mut radio);
    runtime.input_received(Input::Release, 1050, 1050, &mut radio);
    assert!(runtime.navigation.editing());
    runtime.compose_shown();
    let lower = runtime
        .composition
        .laids()
        .get(crate::compose::builtin::ScreenId::Settings)
        .and_then(|laid| laid.box_of(crate::compose::registry::PAD))
        .expect("the editor's minus pad");
    let (x, y) = (
        lower.center_x() as u16,
        (lower.top + lower.bottom) as u16 / 2,
    );
    runtime.input_received(touch(x, y), 1500, 1500, &mut radio);
    assert_eq!(runtime.preferences.profile.ftp_watts, 210);
    runtime.input_received(press(Role::Back), 2000, 2000, &mut radio);
    runtime.input_received(press(Role::Back), 3000, 3000, &mut radio);
    assert!(runtime.navigation.page() == Page::Home);
    runtime.input_received(Input::Release, 3500, 3500, &mut radio);
    // Back in Settings, the first tap counts.
    runtime.navigation.go(To::Page(Page::Settings));
    runtime.input_received(touch(120, 140), 4000, 4000, &mut radio);
    runtime.input_received(Input::Release, 4050, 4050, &mut radio);
    assert_eq!(runtime.navigation.settings_field(), 1);
    assert!(runtime.navigation.editing());
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

#[test]
fn a_brightness_choice_from_the_terminal_holds_a_percent_or_follows_the_light() {
    use firmware_shell::shell::BrightnessChoice;
    let mut screen = Screen::new();
    let mut runtime = booted(Profile::HeartRate);
    let radio = Radio::new();
    screen.present(&mut runtime, &radio, 3000);
    assert!(screen.shell.auto(), "a new rider starts on AUTO");
    for (choice, auto) in [
        (BrightnessChoice::Percent, false),
        (BrightnessChoice::Auto, true),
    ] {
        screen.shell.offer_brightness_choice(choice);
        runtime.save_preferences(&mut screen.shell, 3100);
        screen.present(&mut runtime, &radio, 3100);
        assert_eq!(runtime.preferences.options.auto_brightness, auto);
        assert_eq!(screen.shell.auto(), auto);
    }
}
