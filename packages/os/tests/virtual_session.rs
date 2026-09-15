#![cfg(feature = "simulator")]
use cycling_os::{simulator::session::Session, terminal_protocol};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU32, Ordering},
};
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
    let r = terminal_protocol::parse(format!("CMD 1 {text}").as_bytes()).unwrap();
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
    assert_eq!(command(&mut s, "WIFI RECONNECT").0, "UNSUPPORTED");
}
#[cfg(feature = "debug-harness")]
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
#[cfg(not(feature = "debug-harness"))]
#[test]
fn disabled_discovery_keeps_ordinary_commands_and_no_instrumentation() {
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    let caps = command(&mut s, "HARNESS CAPS");
    assert_eq!(caps.0, "OK");
    assert!(caps.1.contains("input=unsupported capture=unsupported"));
    assert_eq!(command(&mut s, "HARNESS OPEN 1 1000").0, "UNSUPPORTED");
    assert_eq!(command(&mut s, "DISPLAY f800").0, "OK");
}
#[cfg(feature = "cycling")]
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

#[cfg(feature = "cycling")]
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
    use cycling_os::{
        capabilities::{Availability, Button, Input},
        shell::{Screen, Shell},
        simulator::{DisplayDevice, InputDevice, Memory, PowerDevice},
    };
    let input = InputDevice::new(&[Button::TopLeft], Availability::Ready);
    let mut shell = Shell::new(
        DisplayDevice::new(32, 24),
        input.clone(),
        PowerDevice::new(Availability::Ready),
        Memory::default(),
        0,
    )
    .unwrap();
    let click = Input::Button {
        button: Button::TopLeft,
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
    assert_eq!(shell.take_app_input().unwrap().input, click);
    assert!(shell.take_app_input().is_none());
    input.push(1003, Input::Release);
    shell.tick(1003);
    shell.set_app_active(true);
    assert_eq!(shell.take_app_input().unwrap().input, Input::Release);
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

#[cfg(feature = "cycling")]
#[test]
fn sdk_boot_exposes_physical_menu_without_a_usb_command() {
    let t = Temp::new();
    let s = Session::new(&t.0, 32, 24).unwrap();
    assert!(
        s.sdk.input_active(),
        "SDK boot must accept physical menu input before any USB command"
    );
}
#[cfg(feature = "cycling")]
#[test]
fn vana_buttons_pause_resume_and_save_without_overwriting_previous_workout() {
    use cycling_os::{
        capabilities::{Button, Input},
        sdk::ride_log::{self, Kind},
        simulator::session::NoAnt,
    };
    fn present(s: &mut Session) {
        s.sdk.present(&mut s.shell, s.now, &NoAnt, &s.position);
    }
    fn press(s: &mut Session, button: Button) {
        s.advance(400).unwrap();
        s.sdk
            .input(Input::Button { button, code: 1 }, s.now, &mut NoAnt);
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
    present(&mut s); // Splash completes into the TRAIN/SENSORS menu.
    state(&mut s, "ready");
    press(&mut s, Button::BottomRight); // TRAIN -> preflight.
    s.advance(2600).unwrap();
    present(&mut s); // Preflight -> ready workout.
    assert!(!s.sdk.recording());
    press(&mut s, Button::BottomLeft);
    state(&mut s, "recording");
    s.advance(5200).unwrap();
    press(&mut s, Button::BottomLeft);
    let paused = active(&state(&mut s, "paused"));
    s.advance(5000).unwrap();
    assert_eq!(active(&state(&mut s, "paused")), paused);
    press(&mut s, Button::BottomLeft);
    s.advance(2100).unwrap();
    assert!(active(&state(&mut s, "recording")) > paused);
    press(&mut s, Button::BottomRight); // Arm stop; a single press must not save.
    state(&mut s, "recording");
    press(&mut s, Button::BottomRight);
    let saved_duration = active(&state(&mut s, "saved"));
    assert!(!s.sdk.recording());
    let saved = std::fs::read(t.0.join("data.bin")).unwrap();
    let info = command(&mut s, "EXPORT INFO");
    assert_eq!(info.0, "OK");
    let upper: usize = info.1.split_whitespace().nth(3).unwrap().parse().unwrap();
    let prefix = &saved[..upper * ride_log::SLOT_SIZE];
    let entries: Vec<_> = prefix
        .as_chunks::<{ ride_log::SLOT_SIZE }>()
        .0
        .iter()
        .map(|bytes| ride_log::decode(&ride_log::Slot(*bytes)).unwrap())
        .collect();
    assert_eq!(entries.first().unwrap().kind, Kind::Start);
    assert!(entries.iter().any(|e| e.kind == Kind::Pause));
    assert!(entries.iter().any(|e| e.kind == Kind::Resume));
    assert!(entries.iter().any(|e| e.kind == Kind::Samples));
    assert_eq!(entries.last().unwrap().kind, Kind::Finish);
    assert_eq!(entries.last().unwrap().active_ms, saved_duration);

    // A later workout appends after the completed one, including across restart.
    press(&mut s, Button::BottomLeft);
    s.advance(1500).unwrap();
    press(&mut s, Button::BottomRight);
    press(&mut s, Button::BottomRight);
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
