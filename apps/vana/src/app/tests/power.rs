//! Turning off: the confirm sheet, the idle policy and the device hand-over.
use super::presentation::Screen;
use super::*;
use crate::app::power::Sheet;
use crate::ride::log::Status;
use crate::screens::workout::Page;
use crate::sensors::ant::device_type::HEART_RATE;
use device_api::power::{Operation, PowerControl, State, Status as PowerStatus};

/// Device power control that records requests and answers as told.
struct Switch {
    status: PowerStatus,
    accept: bool,
    requests: Vec<Operation>,
}
impl Switch {
    fn new(accept: bool) -> Self {
        Self {
            status: PowerStatus::IDLE,
            accept,
            requests: Vec::new(),
        }
    }
}
impl PowerControl for Switch {
    fn capabilities(&self) -> device_api::power::Capabilities {
        device_api::power::Capabilities::UNSUPPORTED
    }
    fn status(&self) -> PowerStatus {
        self.status
    }
    fn request(&mut self, operation: Operation) -> Result<(), device_api::observation::Error> {
        self.requests.push(operation);
        if !self.accept {
            return Err(device_api::observation::Error::Unavailable);
        }
        self.status = PowerStatus {
            sequence: self.status.sequence + 1,
            operation: Some(operation),
            state: State::Requested,
            ready: false,
            ..self.status
        };
        Ok(())
    }
}

/// The label's box is lit exactly where the label draws, in any colour.
fn lit(screen: &Screen, left: usize, top: usize, scale: usize, label: &[u8]) -> bool {
    use firmware_shell::rendering::text::text;
    (top..top + 7 * scale).all(|y| {
        (left..left + label.len() * 6 * scale)
            .all(|x| (screen.panel.pixel(x, y) != 0) == text(x, y, left, top, scale, label))
    })
}

/// Companion power status zero: USB charging, as stock reads it.
struct Plugged;
impl device_api::input::InputObservation for Plugged {
    fn snapshot(&self, now: u64) -> Option<device_api::input::InputSnapshot> {
        Some(device_api::input::InputSnapshot {
            battery: Observation::Unavailable,
            power: Observation::Fresh {
                value: 0,
                received_ms: now,
            },
            button_counts: [0; 3],
            touch_available: false,
            touch_errors: 0,
            companion_valid: 0,
            companion_bad_crc: 0,
            uart_errors: 0,
            input_lost: 0,
        })
    }
}

/// One VANA tick with the given companion power observation.
fn tick_with(
    runtime: &mut Runtime,
    journal: &mut Journal,
    radio: &mut Radio,
    input: &impl device_api::input::InputObservation,
    now: u64,
) {
    runtime.tick(
        journal,
        now,
        radio,
        &mut Absent,
        Observations {
            position: &Absent,
            network_online: false,
            input,
            physical: no_physical_sensors(),
        },
    );
}

/// A ready runtime showing Home after its brand flash.
fn home(journal: &mut Journal, radio: &mut Radio, screen: &mut Screen) -> Runtime {
    let mut runtime = ready_runtime(journal, radio);
    screen.present(&mut runtime, radio, 0);
    screen.present(&mut runtime, radio, crate::app::presentation::BRAND_MS);
    assert_eq!(runtime.navigation.page, Page::Home);
    runtime
}

/// One composition pass: input already delivered, then frame, then power.
fn pass(runtime: &mut Runtime, screen: &mut Screen, radio: &Radio, switch: &mut Switch, now: u64) {
    screen.shell.tick(now);
    screen.present(runtime, radio, now);
    runtime.power(&mut screen.shell, switch, now);
}

#[test]
fn idle_policy_needs_quiet_input_and_sensors_and_no_ride_or_usb() {
    let idle = |input_idle_ms, quiet_ms| Idle {
        busy: false,
        usb_powered: false,
        input_idle_ms,
        quiet_ms,
    };
    let warn_at = AUTO_OFF_MS - AUTO_OFF_WARNING_MS;
    assert_eq!(auto_off(idle(warn_at - 1, AUTO_OFF_MS)), AutoOff::Awake);
    assert_eq!(
        auto_off(idle(warn_at, AUTO_OFF_MS)),
        AutoOff::Warning { seconds: 30 }
    );
    assert_eq!(
        auto_off(idle(AUTO_OFF_MS - 1, AUTO_OFF_MS)),
        AutoOff::Warning { seconds: 1 }
    );
    assert_eq!(auto_off(idle(AUTO_OFF_MS, AUTO_OFF_MS)), AutoOff::Due);
    // Recent sensor data or movement keeps VANA on even without input.
    assert_eq!(auto_off(idle(AUTO_OFF_MS, 1_000)), AutoOff::Awake);
    let due = idle(AUTO_OFF_MS, AUTO_OFF_MS);
    assert_eq!(auto_off(Idle { busy: true, ..due }), AutoOff::Awake);
    assert_eq!(
        auto_off(Idle {
            usb_powered: true,
            ..due
        }),
        AutoOff::Awake
    );
}

#[test]
fn back_on_home_asks_then_act_turns_off_after_the_last_frame() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);

    runtime.input(press(Button::TopLeft), 1_000, &mut radio);
    assert_eq!(runtime.power.sheet, Sheet::Confirm { since: 1_000 });
    pass(&mut runtime, &mut screen, &radio, &mut switch, 1_000);
    assert!(screen.panel.shows(24, 104, 4, b"TURN OFF"));
    assert!(lit(&screen, 8, 8, 2, b"BACK"));

    // BACK returns to Home without turning off.
    runtime.input(press(Button::TopLeft), 1_500, &mut radio);
    assert_eq!(runtime.power.sheet, Sheet::Closed);
    pass(&mut runtime, &mut screen, &radio, &mut switch, 1_500);
    assert!(lit(&screen, 24, 50, 2, b"READY TO RIDE"));

    runtime.input(press(Button::TopLeft), 2_500, &mut radio);
    runtime.input(press(Button::BottomRight), 3_000, &mut radio);
    assert_eq!(runtime.power.sheet, super::super::power::TURNING_OFF);
    // The request waits until "TURNING OFF" reached the panel.
    runtime.power(&mut screen.shell, &mut switch, 3_000);
    assert!(switch.requests.is_empty());
    pass(&mut runtime, &mut screen, &radio, &mut switch, 3_010);
    assert!(screen.panel.shows(21, 150, 3, b"TURNING OFF"));
    assert_eq!(switch.requests, [Operation::Shutdown]);
    assert!(runtime.turning_off());
    // Nothing else is drawn or requested while the device turns off.
    assert!(!screen.present(&mut runtime, &radio, 5_000));
    runtime.power(&mut screen.shell, &mut switch, 5_000);
    assert_eq!(switch.requests.len(), 1);
}

#[test]
fn a_refused_or_failed_turn_off_says_so_on_home() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut refusing = Switch::new(false);
    runtime.input(press(Button::TopLeft), 1_000, &mut radio);
    runtime.input(press(Button::BottomRight), 1_500, &mut radio);
    pass(&mut runtime, &mut screen, &radio, &mut refusing, 1_500);
    assert_eq!(runtime.power.sheet, Sheet::Closed);
    pass(&mut runtime, &mut screen, &radio, &mut refusing, 1_600);
    assert!(lit(&screen, 24, 50, 2, b"CANNOT TURN OFF"));

    // An accepted request the device later abandons also comes back.
    let mut switch = Switch::new(true);
    runtime.input(press(Button::TopLeft), 10_000, &mut radio);
    runtime.input(press(Button::BottomRight), 10_500, &mut radio);
    pass(&mut runtime, &mut screen, &radio, &mut switch, 10_500);
    assert!(runtime.turning_off());
    switch.status.state = State::Failed;
    switch.status.ready = true;
    runtime.power(&mut screen.shell, &mut switch, 12_000);
    assert!(!runtime.turning_off());
    assert_eq!(
        runtime.power.notice(12_000),
        Some(crate::app::power::Notice::Failed)
    );
}

#[test]
fn turning_off_is_out_of_reach_while_a_ride_is_open() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.navigation.page = Page::Ride;
    runtime.input(press(Button::BottomRight), 1_000, &mut radio);
    for step in 0..20 {
        tick(&mut runtime, &mut journal, &mut radio, 1_000 + step * 100);
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Recording);
    // BACK during a recording ride does not leave the ride.
    runtime.input(press(Button::TopLeft), 4_000, &mut radio);
    assert_eq!(runtime.navigation.page, Page::Ride);
    // Even asked directly, the sheet refuses an open ride.
    runtime.open_power_sheet(4_500);
    assert_eq!(runtime.power.sheet, Sheet::Closed);
    // An idle open ride never turns off either.
    let mut screen = Screen::new();
    let mut switch = Switch::new(true);
    runtime.power(&mut screen.shell, &mut switch, AUTO_OFF_MS * 2);
    assert_eq!(runtime.power.auto, AutoOff::Awake);
    assert!(switch.requests.is_empty());
}

#[test]
fn holding_top_left_through_power_on_does_not_ask_to_turn_off() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    screen.present(&mut runtime, &radio, 0);
    // The rider still holds TopLeft from turning on: reports every 100 ms.
    for at in (100..3_000).step_by(100) {
        runtime.input(press(Button::TopLeft), at, &mut radio);
        assert_eq!(runtime.power.sheet, Sheet::Closed, "at {at} ms");
    }
    assert_eq!(
        runtime.navigation.page,
        Page::Home,
        "the press skipped the brand"
    );
    // A fresh press after release asks.
    runtime.input(press(Button::TopLeft), 3_500, &mut radio);
    assert!(matches!(runtime.power.sheet, Sheet::Confirm { .. }));
}

#[test]
fn an_unanswered_sheet_closes_and_an_idle_device_warns_then_turns_off() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    runtime.input(press(Button::TopLeft), 1_000, &mut radio);
    pass(&mut runtime, &mut screen, &radio, &mut switch, 8_999);
    assert!(matches!(runtime.power.sheet, Sheet::Confirm { .. }));
    pass(&mut runtime, &mut screen, &radio, &mut switch, 9_000);
    assert_eq!(runtime.power.sheet, Sheet::Closed);

    // Shell input clock started at 0; sensors have been quiet since boot.
    let warn = AUTO_OFF_MS - AUTO_OFF_WARNING_MS;
    tick(&mut runtime, &mut journal, &mut radio, warn);
    pass(&mut runtime, &mut screen, &radio, &mut switch, warn);
    assert_eq!(runtime.power.auto, AutoOff::Warning { seconds: 30 });
    assert!(screen.panel.pixel(0, 0) == 0 && switch.requests.is_empty());
    pass(&mut runtime, &mut screen, &radio, &mut switch, AUTO_OFF_MS);
    pass(
        &mut runtime,
        &mut screen,
        &radio,
        &mut switch,
        AUTO_OFF_MS + 10,
    );
    assert_eq!(switch.requests, [Operation::Shutdown]);
}

#[test]
fn sensor_data_and_usb_power_keep_vana_on() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    radio.connected(HEART_RATE, 0);
    let late = AUTO_OFF_MS - 1_000;
    radio.data(HEART_RATE, [0, 0, 0, 0, 0, 0, 1, 72], late);
    tick(&mut runtime, &mut journal, &mut radio, late);
    pass(
        &mut runtime,
        &mut screen,
        &radio,
        &mut switch,
        AUTO_OFF_MS + 60_000,
    );
    assert_eq!(runtime.power.auto, AutoOff::Awake);

    // On USB, a quiet idle device stays on.
    let now = AUTO_OFF_MS * 3;
    runtime.tick(
        &mut journal,
        now,
        &mut radio,
        &mut Absent,
        Observations {
            position: &Absent,
            network_online: false,
            input: &Plugged,
            physical: no_physical_sensors(),
        },
    );
    pass(&mut runtime, &mut screen, &radio, &mut switch, now);
    assert_eq!(runtime.power.auto, AutoOff::Awake);
    assert!(switch.requests.is_empty());
}

#[test]
fn gps_movement_is_measured_across_the_antimeridian_without_overflow() {
    use crate::app::power::moved;
    // 0.0002° either side of ±180° is about 44 m apart, not a full turn.
    assert!(moved((0, 1_799_998_000), (0, -1_799_998_000)));
    assert!(!moved((0, 1_799_999_000), (0, -1_799_999_000)));
    // Extreme inputs neither panic nor wrap.
    assert!(moved((i32::MAX, i32::MAX), (i32::MIN, i32::MIN)));
    assert!(!moved((i32::MIN, i32::MIN), (i32::MIN, i32::MIN)));
}

#[test]
fn unplugging_usb_starts_a_full_idle_period_with_its_warning() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    let unplugged = AUTO_OFF_MS * 2;
    tick_with(&mut runtime, &mut journal, &mut radio, &Plugged, unplugged);
    // On battery again: nothing happens at once, the warning comes first.
    tick(&mut runtime, &mut journal, &mut radio, unplugged + 2_000);
    pass(
        &mut runtime,
        &mut screen,
        &radio,
        &mut switch,
        unplugged + 2_000,
    );
    assert_eq!(runtime.power.auto, AutoOff::Awake);
    let warn = unplugged + AUTO_OFF_MS - AUTO_OFF_WARNING_MS;
    pass(&mut runtime, &mut screen, &radio, &mut switch, warn);
    assert_eq!(runtime.power.auto, AutoOff::Warning { seconds: 30 });
    assert!(switch.requests.is_empty());
}

static CLOCK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[test]
fn press_to_frame_latency_includes_the_time_spent_drawing() {
    use core::sync::atomic::Ordering;
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    runtime.set_clock(|| CLOCK.load(Ordering::Relaxed));
    runtime.navigation.page = Page::Ride;
    runtime.input(press(Button::BottomLeft), 10_000, &mut radio);
    // The pass started at 10 000 ms; the frame finished 800 ms later.
    CLOCK.store(10_800, Ordering::Relaxed);
    assert!(screen.present(&mut runtime, &radio, 10_000));
    let mut report = std::string::String::new();
    runtime.timing.report(&mut report);
    assert!(report.contains("press_frame_last_ms=800 "), "{report}");
    assert!(report.contains("press_frame_max_ms=800 "), "{report}");
}

#[test]
fn a_live_radar_keeps_vana_on() {
    use crate::sensors::ant::device_type::RADAR;
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    radio.connected(RADAR, 0);
    // A radar with nothing behind still sends pages; only radar data arrives.
    let clear = [0x30, 0, 0, 0, 0, 0, 0, 0];
    let late = AUTO_OFF_MS - 1_000;
    radio.data(RADAR, clear, late);
    tick(&mut runtime, &mut journal, &mut radio, late);
    pass(
        &mut runtime,
        &mut screen,
        &radio,
        &mut switch,
        AUTO_OFF_MS + 60_000,
    );
    assert_eq!(runtime.power.auto, AutoOff::Awake);
    assert!(switch.requests.is_empty());
}

#[test]
fn a_rejected_turning_off_frame_is_drawn_again_before_shutdown() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    runtime.input(press(Button::TopLeft), 1_000, &mut radio);
    runtime.input(press(Button::BottomRight), 1_500, &mut radio);
    // The panel rejects the first TURNING OFF frame: no shutdown yet.
    screen.panel.1.set(1);
    pass(&mut runtime, &mut screen, &radio, &mut switch, 1_500);
    assert!(screen.shell.display_error);
    assert!(
        switch.requests.is_empty(),
        "shut down with the old screen visible"
    );
    assert!(!screen.panel.shows(21, 150, 3, b"TURNING OFF"));
    // The retry follows at once and, once accepted, hands over.
    pass(&mut runtime, &mut screen, &radio, &mut switch, 1_510);
    assert!(screen.panel.shows(21, 150, 3, b"TURNING OFF"));
    assert_eq!(switch.requests, [Operation::Shutdown]);
}

#[test]
fn a_panel_that_keeps_rejecting_frames_still_turns_off() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    runtime.input(press(Button::TopLeft), 1_000, &mut radio);
    runtime.input(press(Button::BottomRight), 1_500, &mut radio);
    screen.panel.1.set(u32::MAX);
    for step in 0..2 {
        pass(
            &mut runtime,
            &mut screen,
            &radio,
            &mut switch,
            1_500 + step * 10,
        );
        assert!(switch.requests.is_empty(), "attempt {step}");
    }
    // Staying on would only drain the battery.
    pass(&mut runtime, &mut screen, &radio, &mut switch, 1_520);
    assert_eq!(switch.requests, [Operation::Shutdown]);
}

static WRITE_CLOCK: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

#[test]
fn recording_is_confirmed_after_the_start_write_finishes() {
    use core::sync::atomic::Ordering;
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    runtime.set_clock(|| WRITE_CLOCK.load(Ordering::Relaxed));
    runtime.navigation.page = Page::Ride;
    WRITE_CLOCK.store(1_000, Ordering::Relaxed);
    runtime.input(press(Button::BottomRight), 1_000, &mut radio);
    // The pass starts at 1 000 ms; the start write and commit end 300 ms later.
    WRITE_CLOCK.store(1_300, Ordering::Relaxed);
    let mut step = 0;
    while runtime.ride.recorder.status() != Status::Recording {
        assert!(step < 50, "never recording");
        tick(&mut runtime, &mut journal, &mut radio, 1_000);
        step += 1;
    }
    let mut report = std::string::String::new();
    runtime.timing.report(&mut report);
    assert!(report.contains("ride_press_ms=1000 "), "{report}");
    assert!(report.contains("recording_ms=1300 "), "{report}");
}

#[test]
fn storage_is_ready_when_its_scan_finishes_not_when_vana_first_ticks() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = Runtime::new(Profile::HeartRate);
    // The journal is scanned while the companion still starts.
    let mut now = 100;
    while runtime.ride.recorder.status() == Status::Scanning {
        assert!(
            now < 100 + crate::ride::log::SECTORS as u64 * 10,
            "scan never ended"
        );
        runtime.prepare(&mut journal, now);
        now += 5;
    }
    let scanned = now - 5;
    // VANA's first full tick comes later, once the companion is ready.
    tick(&mut runtime, &mut journal, &mut radio, 3_300);
    let mut report = std::string::String::new();
    runtime.timing.report(&mut report);
    assert!(
        report.contains(&std::format!("storage_ms={scanned} ")),
        "{report}"
    );
}

#[test]
fn turning_off_while_the_terminal_owns_the_panel_is_still_bounded() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    // A terminal DISPLAY command took the panel; VANA draws nothing.
    runtime.suspend_display();
    let late = AUTO_OFF_MS + 1_000;
    for step in 0..2 {
        pass(
            &mut runtime,
            &mut screen,
            &radio,
            &mut switch,
            late + step * 10,
        );
        assert!(switch.requests.is_empty(), "pass {step}");
    }
    pass(&mut runtime, &mut screen, &radio, &mut switch, late + 20);
    assert_eq!(switch.requests, [Operation::Shutdown]);
}

#[test]
fn rejected_frames_are_not_milestones_and_do_not_answer_a_press() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = ready_runtime(&mut journal, &mut radio);
    let report = |runtime: &Runtime| {
        let mut out = std::string::String::new();
        runtime.timing.report(&mut out);
        out
    };
    screen.panel.1.set(1);
    assert!(screen.present(&mut runtime, &radio, 100));
    assert!(
        report(&runtime).contains("first_frame_ms=- "),
        "{}",
        report(&runtime)
    );
    runtime.display.redraw();
    assert!(screen.present(&mut runtime, &radio, 150));
    assert!(
        report(&runtime).contains("first_frame_ms=150 "),
        "{}",
        report(&runtime)
    );

    runtime.navigation.page = Page::Ride;
    runtime.input(press(Button::BottomLeft), 1_000, &mut radio);
    screen.panel.1.set(1);
    assert!(screen.present(&mut runtime, &radio, 1_000));
    assert!(
        report(&runtime).contains("press_frame_last_ms=- "),
        "{}",
        report(&runtime)
    );
    // The press is answered by the frame that reached the panel.
    runtime.display.redraw();
    assert!(screen.present(&mut runtime, &radio, 1_200));
    assert!(
        report(&runtime).contains("press_frame_last_ms=200 "),
        "{}",
        report(&runtime)
    );
}

#[test]
fn a_profile_edit_that_cannot_be_saved_is_reported_before_turning_off() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    let mut switch = Switch::new(true);
    // An FTP edit is still settling; this test shell cannot write settings.
    let edited = crate::ride::zones::TrainingProfile::new(225, 185).unwrap();
    runtime.preferences.set(edited, 1_000);
    runtime.input(press(Button::TopLeft), 1_200, &mut radio);
    runtime.input(press(Button::BottomRight), 1_600, &mut radio);
    pass(&mut runtime, &mut screen, &radio, &mut switch, 1_600);
    assert!(switch.requests.is_empty(), "turned off and lost the edit");
    assert_eq!(runtime.power.sheet, Sheet::Closed);
    assert_eq!(
        runtime.power.notice(1_600),
        Some(crate::app::power::Notice::Unsaved)
    );
    pass(&mut runtime, &mut screen, &radio, &mut switch, 1_700);
    assert!(lit(&screen, 24, 50, 2, b"CHANGES NOT SAVED"));
    // Told once, turning off again goes ahead.
    runtime.input(press(Button::TopLeft), 3_000, &mut radio);
    runtime.input(press(Button::BottomRight), 3_400, &mut radio);
    pass(&mut runtime, &mut screen, &radio, &mut switch, 3_400);
    assert_eq!(switch.requests, [Operation::Shutdown]);
}

/// A BLE link that replays queued notifications.
struct Notifying {
    snapshot: device_api::ble_transport::Snapshot,
    packets: std::collections::VecDeque<device_api::ble_transport::Packet>,
}
impl device_api::ble_transport::Ble for Notifying {
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn snapshot(&self) -> device_api::ble_transport::Snapshot {
        self.snapshot
    }
    fn take_packet(&mut self) -> Option<device_api::ble_transport::Packet> {
        self.packets.pop_front()
    }
    fn reconnect(&mut self) -> Result<(), device_api::observation::Error> {
        Ok(())
    }
}

#[test]
fn a_ble_cadence_sensor_reports_its_first_live_data() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut runtime = Runtime::new(Profile::Cadence);
    for _ in 0..crate::ride::log::SECTORS {
        tick(&mut runtime, &mut journal, &mut radio, 0);
    }
    let packet = |sequence: u32, ms: u64, data: [u8; 5]| {
        let mut bytes = [0; 60];
        bytes[..5].copy_from_slice(&data);
        device_api::ble_transport::Packet {
            connection: 1,
            sequence,
            dropped: 0,
            received_ms: ms,
            bytes,
            length: 5,
        }
    };
    let mut ble = Notifying {
        snapshot: device_api::ble_transport::Snapshot {
            link: device_api::ble_transport::Link::Connected,
            connections: 1,
            ..Default::default()
        },
        packets: [
            packet(1, 1_000, [2, 1, 0, 0, 4]),
            packet(2, 2_000, [2, 2, 0, 0, 8]),
        ]
        .into(),
    };
    runtime.tick(
        &mut journal,
        2_000,
        &mut radio,
        &mut ble,
        Observations {
            position: &Absent,
            network_online: false,
            input: &Absent,
            physical: no_physical_sensors(),
        },
    );
    assert_eq!(runtime.sensors.snapshot(2_000).cadence_tenths, Some(600));
    let mut report = std::string::String::new();
    runtime.timing.report(&mut report);
    assert!(report.contains("sensors=1:2000 "), "{report}");
}

#[test]
fn a_failed_journal_scan_is_reported_not_stamped_as_ready() {
    let mut runtime = Runtime::new(Profile::HeartRate);
    let mut now = 100;
    while runtime.ride.recorder.status() == Status::Scanning {
        assert!(now < 10_000, "scan never ended");
        runtime.prepare(&mut Absent, now);
        now += 5;
    }
    assert_eq!(runtime.ride.recorder.status(), Status::Error);
    let mut report = std::string::String::new();
    runtime.timing.report(&mut report);
    assert!(report.contains("storage_ms=- "), "{report}");
    assert!(report.contains("storage_state=error "), "{report}");
}

#[test]
fn every_channel_and_both_ble_kinds_get_a_first_data_milestone() {
    let mut timing = super::super::timing::Timing::default();
    let kinds: std::vec::Vec<u8> = [0, 1]
        .into_iter()
        .chain(110..110 + device_api::ant::CHANNEL_CAPACITY as u8)
        .collect();
    for (at, kind) in kinds.iter().enumerate() {
        timing.sensor(*kind, 1_000 + at as u64);
    }
    let mut report = std::string::String::new();
    timing.report(&mut report);
    for (at, kind) in kinds.iter().enumerate() {
        assert!(
            report.contains(&std::format!("{kind}:{}", 1_000 + at)),
            "{report}"
        );
    }
    assert!(report.contains("sensors_dropped=0 "), "{report}");
    // A kind beyond capacity is counted once, however often it reports.
    timing.sensor(200, 5_000);
    timing.sensor(200, 5_100);
    report.clear();
    timing.report(&mut report);
    assert!(report.contains("sensors_dropped=1 "), "{report}");
}

#[test]
fn a_new_ride_starts_its_press_latency_figures_afresh() {
    let mut timing = super::super::timing::Timing::default();
    timing.ride_press(1_000);
    timing.press(2_000);
    timing.frame(2_900);
    timing.press(3_000);
    timing.frame(3_050);
    let mut report = std::string::String::new();
    timing.report(&mut report);
    assert!(report.contains("press_frame_max_ms=900 "), "{report}");
    // The second ride's start press is measured; the first ride's worst is gone.
    timing.press(10_000);
    timing.ride_press(10_000);
    timing.frame(10_040);
    report.clear();
    timing.report(&mut report);
    assert!(report.contains("press_frame_last_ms=40 "), "{report}");
    assert!(report.contains("press_frame_max_ms=40 "), "{report}");
}

#[test]
fn press_latency_starts_when_the_device_received_the_press() {
    let mut journal = Journal::erased();
    let mut radio = Radio::new();
    let mut screen = Screen::new();
    let mut runtime = home(&mut journal, &mut radio, &mut screen);
    runtime.navigation.page = Page::Ride;
    // Received at 7 100 ms, handled at 7 900 ms behind slow work, drawn at 7 950 ms.
    runtime.input_received(press(Button::BottomLeft), 7_100, 7_900, &mut radio);
    assert!(screen.present(&mut runtime, &radio, 7_950));
    let mut report = std::string::String::new();
    runtime.timing.report(&mut report);
    assert!(report.contains("press_frame_last_ms=850 "), "{report}");
}
