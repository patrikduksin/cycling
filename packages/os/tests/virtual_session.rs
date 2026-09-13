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
