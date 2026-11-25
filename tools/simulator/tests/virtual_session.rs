use shell_simulator::session::{COMPANION_BOOT_MS, RESTART_PROBE_MS, Session};

use std::path::PathBuf;
use std::sync::atomic::AtomicU32;
use std::sync::atomic::Ordering;
static NEXT: AtomicU32 = AtomicU32::new(0);
struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        Self(std::env::temp_dir().join(format!(
            "cycling-virtual-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        )))
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn command(s: &mut Session, text: &str) -> (&'static str, String) {
    let r = firmware_console::protocol::parse(format!("CMD 1 {text}").as_bytes()).unwrap();
    let mut out = String::new();
    let status = s.execute(r, &mut out);
    (status, out)
}
fn fixture(s: &mut Session, text: &str) {
    assert_eq!(s.fixture(text, &mut String::new()), "OK");
}
#[test]
fn shared_settings_survive_restart_and_torn_update_preserves_previous() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    assert_eq!(command(&mut s, "BRIGHTNESS 61").0, "OK");
    assert_eq!(command(&mut s, "SAVE").0, "OK");
    let boot = s.boot;
    s.restart().unwrap();
    assert_ne!(s.boot, boot);
    assert!(command(&mut s, "SETTINGS").1.contains("brightness=61"));
    fixture(&mut s, "FIXTURE STORAGE_FAIL 1 12");
    command(&mut s, "BRIGHTNESS 42");
    assert_eq!(command(&mut s, "SAVE").0, "FAILED");
    s.restart().unwrap();
    assert!(command(&mut s, "SETTINGS").1.contains("brightness=61"));
    assert_eq!(
        std::fs::read(t.0.join("data.bin")).unwrap(),
        vec![0xff; 1024 * 1024]
    );
}
#[test]
fn deterministic_observations_use_production_freshness_loss_and_missing() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    assert!(
        command(&mut s, "POSITION")
            .1
            .contains("transport=unavailable")
    );
    fixture(
        &mut s,
        "FIXTURE POSITION NMEA $GPRMC,123519,A,4807.038,N,01131.000,E,022.4,084.4,230394,003.1,W*6A",
    );
    assert!(
        command(&mut s, "POSITION")
            .1
            .contains("transport=receiving")
    );
    fixture(&mut s, "FIXTURE ADVANCE 4000");
    assert!(command(&mut s, "POSITION").1.contains("transport=silent"));
    fixture(&mut s, "FIXTURE POSITION LOSS");
    assert!(command(&mut s, "POSITION").1.contains("transport=failed"));
    fixture(&mut s, "FIXTURE POSITION UNSUPPORTED");
    assert_eq!(command(&mut s, "POSITION").0, "UNAVAILABLE");
    // The station takes a reconnect with nothing saved, and fails it as the
    // C606's does.
    assert_eq!(command(&mut s, "WIFI RECONNECT").0, "ACCEPTED");
    fixture(&mut s, "FIXTURE ADVANCE 4000");
    assert!(command(&mut s, "WIFI").1.contains("error=unconfigured"));
}
#[test]
fn display_pattern_capture_is_current_submission_and_restart_invalidates_session() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    let id = (u64::from(s.boot) << 32) | 1;
    command(&mut s, "HARNESS OPEN 1 1000");
    command(&mut s, "DISPLAY f800");
    assert_eq!(
        command(&mut s, &format!("HARNESS {id} CAPTURE START 1 100")).0,
        "ACCEPTED"
    );
    let status = command(&mut s, &format!("HARNESS {id} CAPTURE STATUS"));
    assert!(status.1.contains("captured=1"));
    let pixels = &s.shell.harness.buffer.as_ref().unwrap()[..32 * 24 * 2];
    assert!(pixels.as_chunks::<2>().0.iter().all(|p| *p == [0, 0xf8]));
    s.restart().unwrap();
    assert_eq!(
        command(&mut s, &format!("HARNESS {id} PING")).0,
        "STALE_SESSION"
    );
}
#[test]
fn sdk_production_handlers_record_recover_export_with_persistent_media() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    fixture(&mut s, "FIXTURE ADVANCE 3000");
    let status = command(&mut s, "RIDE STATUS");
    assert!(status.1.contains("state=ready"), "{status:?}");
    let start = command(&mut s, "RIDE START");
    assert_eq!(start.0, "ACCEPTED", "{start:?}");
    fixture(&mut s, "FIXTURE ADVANCE 1000");
    command(&mut s, "RIDE STATUS");
    fixture(&mut s, "FIXTURE BLE CONNECT");
    fixture(&mut s, "FIXTURE BLE HEART 88");
    assert!(command(&mut s, "RIDE SENSORS").1.contains("heart=Some(88)"));
    fixture(&mut s, "FIXTURE ADVANCE 1200");
    s.restart().unwrap();
    fixture(&mut s, "FIXTURE ADVANCE 5000");
    let status = command(&mut s, "RIDE STATUS");
    assert!(status.1.contains("rides=1"), "{status:?}");
    assert_eq!(command(&mut s, "EXPORT INFO").0, "OK");
}

#[test]
fn same_media_cannot_be_owned_twice_and_malformed_fixture_does_not_mutate() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    assert!(Session::new(&t.0, 32, 24).is_err());
    assert_eq!(
        s.fixture("FIXTURE BLE CONNECT trailing", &mut String::new()),
        "INVALID"
    );
    assert!(command(&mut s, "BLE").1.contains("link=off"));
    s.restart().unwrap();
    drop(s);
    assert!(Session::new(&t.0, 32, 24).is_ok());
}

#[test]
fn connectivity_updates_preserve_persisted_preferences_and_existing_data() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    let existing = vec![0x5a; 1024 * 1024];
    std::fs::write(t.0.join("data.bin"), &existing).unwrap();
    command(&mut s, "BRIGHTNESS 61");
    command(&mut s, "SAVE");
    command(&mut s, "BRIGHTNESS 42");
    assert_eq!(
        command(&mut s, "WIFI CONFIG WPA2 54657374 70617373776f7264").0,
        "ACCEPTED"
    );
    assert!(command(&mut s, "WIFI").1.contains("saved=true"));
    assert!(command(&mut s, "SETTINGS").1.contains("brightness=42"));
    s.restart().unwrap();
    assert!(command(&mut s, "SETTINGS").1.contains("brightness=61"));
    assert!(command(&mut s, "WIFI").1.contains("saved=true"));
    assert_eq!(s.shell.settings.wifi.unwrap().ssid.text(), "Test");
    fixture(&mut s, "FIXTURE STORAGE_FAIL 1 12");
    assert_eq!(command(&mut s, "WIFI FORGET").0, "FAILED");
    s.restart().unwrap();
    assert!(command(&mut s, "WIFI").1.contains("saved=true"));
    assert_eq!(command(&mut s, "WIFI FORGET").0, "ACCEPTED");
    s.restart().unwrap();
    assert!(command(&mut s, "WIFI").1.contains("saved=false"));
    assert_eq!(std::fs::read(t.0.join("data.bin")).unwrap(), existing);
}

#[test]
fn runtime_sensor_change_clears_old_readings_and_survives_restart() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    assert_eq!(command(&mut s, "BLE SELECT HRS 54657374 -").0, "ACCEPTED");
    fixture(&mut s, "FIXTURE BLE CONNECT");
    fixture(&mut s, "FIXTURE BLE HEART 88");
    assert!(command(&mut s, "RIDE SENSORS").1.contains("heart=Some(88)"));
    assert_eq!(command(&mut s, "BLE SELECT CSC 54657374 -").0, "ACCEPTED");
    assert!(command(&mut s, "RIDE SENSORS").1.contains("heart=None"));
    s.restart().unwrap();
    assert!(command(&mut s, "BLE").1.contains("profile=2"));
    assert_eq!(s.shell.settings.ble.unwrap().service, 0x1816);
    assert_eq!(command(&mut s, "BLE FORGET").0, "ACCEPTED");
    s.restart().unwrap();
    assert!(command(&mut s, "BLE").1.contains("saved=false"));
}

#[test]
fn foreground_app_receives_only_forwarded_edges_and_transitions_clear_queue() {
    use device_api::input::{Fixture, Fixtures, Input, Role};
    use device_api::observation::Availability;
    use firmware_shell::input::Input as App;
    use firmware_shell::shell::Screen;
    use firmware_shell::shell::Shell;
    use shell_simulator::devices::DisplayDevice;
    use shell_simulator::devices::InputDevice;
    use shell_simulator::devices::Memory;
    use shell_simulator::devices::PowerDevice;
    let input = InputDevice::new(
        Fixtures::new(&[Fixture {
            id: 0,
            role: Some(Role::Back),
            edge: device_api::input::Side::Left,
            at: 0,
        }]),
        Availability::Ready,
    );
    let mut shell = Shell::new(
        DisplayDevice::new(32, 24),
        input.clone(),
        PowerDevice::new(Availability::Ready),
        Memory::default(),
        Memory::default().journal(),
        0,
    )
    .unwrap();
    let click = Input::Button { id: 0, code: 1 };
    let press = App::Press {
        role: Role::Back,
        code: 1,
    };
    shell.settings.dim_timeout_secs = 1;
    shell.set_app_active(true);
    shell.tick(1000);
    assert!(shell.dimmed());
    input.push(1001, click);
    shell.tick(1001);
    assert!(shell.take_app_input().is_none(), "wake click is consumed");
    input.push(1002, click);
    shell.tick(1002);
    assert_eq!(shell.foreground, Screen::Status);
    assert_eq!(shell.take_app_input().unwrap().input, press);
    assert!(shell.take_app_input().is_none());
    input.push(1003, Input::Release);
    shell.tick(1003);
    shell.set_app_active(true);
    assert_eq!(shell.take_app_input().unwrap().input, App::Release);
    input.push(1004, click);
    shell.tick(1004);
    shell.set_app_active(false);
    assert!(shell.take_app_input().is_none());
    input.push(1005, click);
    shell.tick(1005);
    assert_eq!(shell.foreground, Screen::Blank);
    assert!(shell.take_app_input().is_none());
    shell.set_app_active(true);
    assert!(shell.take_app_input().is_none());
}

#[test]
fn sdk_boot_exposes_physical_menu_without_a_usb_command() {
    let t = Temp::new();
    let s = Session::new(&t.0, 32, 24).unwrap();
    assert!(
        s.sdk.input_active(),
        "SDK boot must accept physical menu input before any USB command"
    );
}
#[test]
fn vana_buttons_pause_resume_and_save_without_overwriting_previous_workout() {
    use device_api::Absent;
    use device_api::input::Role;
    use device_api::observation::Availability;
    use device_api::positioning::{Positioning, Snapshot};
    use firmware_shell::input::Input;
    use vana::ride::log::Kind;
    struct Position(Availability, Option<Snapshot>);
    impl Positioning for Position {
        fn availability(&self) -> Availability {
            self.0
        }
        fn snapshot(&self, _: u64) -> Option<Snapshot> {
            self.1
        }
    }
    fn present(s: &mut Session) {
        let fixture = &s.sim().position;
        let position = Position(fixture.availability(), fixture.snapshot(s.now));
        s.sdk.frame(&mut s.shell, s.now, &Absent, &position);
    }
    fn press(s: &mut Session, button: Role) {
        s.advance(400).unwrap();
        s.sdk.input_received(
            Input::Press {
                role: button,
                code: 1,
            },
            s.now,
            s.now,
            &mut Absent,
        );
        s.advance(100).unwrap();
    }
    fn state(s: &mut Session, expected: &str) -> String {
        let (status, data) = command(s, "RIDE STATUS");
        assert_eq!(status, "OK");
        assert!(data.contains(&format!("state={expected}")), "{data}");
        data
    }
    fn active(data: &str) -> u64 {
        data.split_whitespace()
            .find_map(|p| p.strip_prefix("active_ms="))
            .unwrap()
            .parse()
            .unwrap()
    }
    let t = Temp::new();
    let mut s = Session::new(&t.0, 240, 320).unwrap();
    s.advance(7000).unwrap();
    present(&mut s);
    state(&mut s, "ready");
    assert!(!s.sdk.recorder_busy());
    press(&mut s, Role::Act);
    state(&mut s, "recording");
    s.advance(5200).unwrap();
    press(&mut s, Role::Act);
    let paused = active(&state(&mut s, "paused"));
    s.advance(5000).unwrap();
    assert_eq!(active(&state(&mut s, "paused")), paused);
    press(&mut s, Role::Act);
    s.advance(2100).unwrap();
    assert!(active(&state(&mut s, "recording")) > paused);
    press(&mut s, Role::Back); // Recording, this is a lap, not end ride.
    press(&mut s, Role::Back);
    state(&mut s, "recording");
    press(&mut s, Role::Act); // Only a paused ride ends.
    state(&mut s, "paused");
    press(&mut s, Role::Back); // Asks END RIDE?; a single press must not save.
    state(&mut s, "paused");
    press(&mut s, Role::Act); // Yes: save.
    let saved_duration = active(&state(&mut s, "saved"));
    assert!(!s.sdk.recorder_busy());
    let saved = std::fs::read(t.0.join("data.bin")).unwrap();
    let info = command(&mut s, "EXPORT INFO");
    assert_eq!(info.0, "OK");
    let upper: usize = info.1.split_whitespace().nth(3).unwrap().parse().unwrap();
    let prefix = &saved[..upper * vana::ride::log::SLOT_SIZE];
    let entries: Vec<_> = prefix
        .as_chunks::<{ vana::ride::log::SLOT_SIZE }>()
        .0
        .iter()
        .map(|bytes| vana::ride::log::decode(&vana::ride::log::Slot(*bytes)).unwrap())
        .collect();
    assert_eq!(entries.first().unwrap().kind, Kind::Start);
    assert!(entries.iter().any(|e| e.kind == Kind::Pause));
    assert!(entries.iter().any(|e| e.kind == Kind::Resume));
    assert!(entries.iter().any(|e| e.kind == Kind::Samples));
    assert_eq!(entries.last().unwrap().kind, Kind::Finish);
    assert_eq!(entries.last().unwrap().active_ms, saved_duration);

    // A later workout appends after the completed one, including across restart.
    press(&mut s, Role::Act); // Ride done: any key, Home.
    press(&mut s, Role::Act);
    state(&mut s, "recording");
    s.advance(1500).unwrap();
    press(&mut s, Role::Act);
    state(&mut s, "paused");
    press(&mut s, Role::Back);
    press(&mut s, Role::Act);
    state(&mut s, "saved");
    assert_eq!(
        &std::fs::read(t.0.join("data.bin")).unwrap()[..prefix.len()],
        prefix
    );
    s.restart().unwrap();
    s.advance(5000).unwrap();
    assert!(command(&mut s, "RIDE HISTORY").1.starts_with("count=2"));
    assert_eq!(
        &std::fs::read(t.0.join("data.bin")).unwrap()[..prefix.len()],
        prefix
    );
}

#[test]
fn power_cycle_of_a_running_device_brings_the_companion_back_ready() {
    use device_api::observation::Availability;
    use device_api::sensors::Startup;
    let mut s = Session::device(shell_simulator::models::C606).unwrap();
    s.step(COMPANION_BOOT_MS + 500);
    assert_eq!(
        (s.sim_mut().startup, s.sim_mut().ant.availability),
        (Startup::Ready, Availability::Ready)
    );
    s.power_cycle().unwrap();
    assert_eq!(s.sim_mut().ant.availability, Availability::Initializing);
    s.step(COMPANION_BOOT_MS + 500);
    assert_eq!(
        (s.sim_mut().startup, s.sim_mut().ant.availability),
        (Startup::Ready, Availability::Ready)
    );
}

#[test]
fn cpu_restart_while_the_companion_boots_lets_it_finish() {
    use device_api::observation::Availability;
    use device_api::sensors::Startup;
    let mut s = Session::device(shell_simulator::models::C606).unwrap();
    s.step(500);
    assert_eq!(s.sim_mut().startup, Startup::Probing);
    s.restart().unwrap();
    assert_eq!(s.sim_mut().ant.availability, Availability::Initializing);
    s.step(COMPANION_BOOT_MS - 400);
    assert_eq!(
        (s.sim_mut().startup, s.sim_mut().ant.availability),
        (Startup::Ready, Availability::Ready)
    );
}

#[test]
fn work_after_a_restart_waits_for_the_companion_probe_and_says_starting() {
    // #137: a restart's harness session was refused as a power transition.
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    s.sim_mut().restart_probe_ms = RESTART_PROBE_MS;
    assert_eq!(command(&mut s, "BRIGHTNESS 61").0, "OK");
    assert_eq!(command(&mut s, "SAVE").0, "OK");
    s.restart().unwrap();
    assert_eq!(command(&mut s, "INFO").0, "OK");
    assert_eq!(
        command(&mut s, "SETTINGS"),
        ("BUSY", "starting companion=probing".into())
    );
    // A refused mutation changed nothing.
    assert_eq!(command(&mut s, "BRIGHTNESS 42").0, "BUSY");
    s.step(RESTART_PROBE_MS);
    let (status, settings) = command(&mut s, "SETTINGS");
    assert_eq!(status, "OK");
    assert!(settings.contains("brightness=61"), "{settings}");
    s.restart().unwrap();
    assert_eq!(command(&mut s, "SETTINGS").0, "BUSY");
}

#[test]
fn an_app_edge_keeps_the_time_the_device_received_it() {
    use device_api::input::{Input, Role};
    use device_api::observation::Availability;
    use firmware_shell::input::Input as App;
    use firmware_shell::shell::Shell;
    use shell_simulator::devices::{DisplayDevice, InputDevice, Memory, PowerDevice};
    let input = InputDevice::new(
        device_api::input::Fixtures::new(&[device_api::input::Fixture {
            id: 1,
            role: Some(device_api::input::Role::Move),
            edge: device_api::input::Side::Left,
            at: 0,
        }]),
        Availability::Ready,
    );
    let mut shell = Shell::new(
        DisplayDevice::new(32, 24),
        input.clone(),
        PowerDevice::new(Availability::Ready),
        Memory::default(),
        Memory::default().journal(),
        0,
    )
    .unwrap();
    shell.set_app_active(true);
    input.push(1_000, Input::Button { id: 1, code: 1 });
    // The shell gets to it late, behind slow work.
    shell.tick(1_800);
    let edge = shell.take_app_input().expect("routed to the app");
    assert_eq!(
        edge.input,
        App::Press {
            role: Role::Move,
            code: 1
        }
    );
    assert_eq!(edge.received_ms, 1_000);
}

#[test]
#[should_panic(expected = "unmet need: touch: bare has none")]
fn a_tap_on_a_model_without_touch_is_refused() {
    let mut sim = shell_simulator::sim::Sim::on(shell_simulator::models::BARE);
    sim.discard_recording();
    sim.tap(120, 160);
}

#[test]
fn a_sized_session_records_a_device_no_model_replays() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    s.step(100);
    let trace = s.trace();
    let (header, _) = firmware_services::trace::decode(&trace).unwrap();
    assert_eq!(header.device, shell_simulator::session::VIRTUAL);
    assert!(shell_simulator::devices::Model::named(header.device).is_none());
}

#[test]
fn a_broken_rule_stops_the_session_naming_the_rule_boot_and_pass() {
    let mut s = Session::device(shell_simulator::models::C606).unwrap();
    for _ in 0..3 {
        s.tick();
    }
    s.restart().unwrap();
    s.tick();
    s.sdk.break_a_rule();
    let panic = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| s.tick())).unwrap_err();
    assert_eq!(
        panic.downcast_ref::<String>().map(String::as_str),
        Some(
            "broken after pass 5 (boot 2): a host operation waits without exactly one owner of its token"
        )
    );
}
