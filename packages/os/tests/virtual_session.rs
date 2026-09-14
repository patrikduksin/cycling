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

#[cfg(feature = "cycling")]
#[test]
fn foundation_runtime_deadline_commits_stop_without_peers_and_preserves_existing_prefix() {
    use cycling_os::sdk::ride_log::{self, Entry, Kind, Slot, Source};
    let t = Temp::new();
    let mut s = Session::new(&t.0, 32, 24).unwrap();
    let data_path = t.0.join("data.bin");
    let mut bytes = std::fs::read(&data_path).unwrap();
    // A real committed ride and an occupied unknown slot must survive the whole capture.
    for (index, kind) in [Kind::Start, Kind::Finish].into_iter().enumerate() {
        let mut slot = ride_log::encode(&Entry::event(
            kind,
            Source::Live,
            7,
            index as u32,
            1000 * index as u64,
        ));
        slot.0[252..].copy_from_slice(&ride_log::commit_word().0);
        assert!(ride_log::decode(&slot).is_some());
        bytes[index * 256..(index + 1) * 256].copy_from_slice(&slot.0);
    }
    bytes[512..768].fill(0x5a);
    let original_prefix = bytes[..768].to_vec();
    std::fs::write(&data_path, bytes).unwrap();
    s.advance(3000).unwrap();
    assert!(command(&mut s, "RIDE STATUS").1.contains("rides=1"));
    assert_eq!(
        command(&mut s, "FOUNDATION LOG INFO"),
        ("OK", "INFO 1 256 3 idle".into())
    );
    assert_eq!(command(&mut s, "FOUNDATION LOG START 300").0, "ACCEPTED");
    assert!(s.sdk.recording());
    // This uses the production runtime's scan, deadline, acquisition and writer.
    for _ in 0..600 {
        if command(&mut s, "FOUNDATION LOG STATUS")
            .1
            .contains("status: Recording")
        {
            break;
        }
        s.advance(10).unwrap();
    }
    let recording = command(&mut s, "FOUNDATION LOG STATUS").1;
    assert!(recording.contains("status: Recording"), "{recording}");
    assert!(recording.contains("required_slots: 608"), "{recording}");
    assert!(
        recording.contains("requested_seconds=Some(300)"),
        "{recording}"
    );
    let first_recording_ms = s.now;
    for delta in [120_000, 120_000, 59_000] {
        s.advance(delta).unwrap();
    }
    assert!(
        command(&mut s, "FOUNDATION LOG STATUS")
            .1
            .contains("status: Recording")
    );
    // No STOP command is sent. Crossing the production deadline must flush a stop record.
    s.advance(2000).unwrap();
    let stopped = command(&mut s, "FOUNDATION LOG STATUS").1;
    assert!(stopped.contains("status: Stopped"), "{stopped}");
    assert!(stopped.contains("error: None"), "{stopped}");
    assert!(!s.sdk.recording());
    let info = command(&mut s, "FOUNDATION LOG INFO");
    assert_eq!(info.0, "OK");
    let upper: usize = info.1.split_whitespace().nth(3).unwrap().parse().unwrap();
    let after = std::fs::read(&data_path).unwrap();
    assert_eq!(&after[..original_prefix.len()], original_prefix.as_slice());
    let appended = after[768..upper * 256].as_chunks::<256>().0;
    assert!(
        appended.len() > 590,
        "five minutes must retain GPS/environment progress"
    );
    assert!(
        appended.len() <= 608,
        "no-peer capture must fit its reservation"
    );
    for slot in appended {
        assert_eq!(&slot[..4], b"ANT1");
        assert_eq!(&slot[252..], &ride_log::commit_word().0);
        assert_eq!(
            u32::from_le_bytes(slot[248..252].try_into().unwrap()),
            ride_log::transport_checksum(&slot[..248])
        );
        assert!(
            matches!(slot[5], 2 | 6 | 7),
            "no ANT packets or links without selected peers"
        );
    }
    assert_eq!(appended.iter().filter(|slot| slot[5] == 2).count(), 1);
    let stop = appended.last().unwrap();
    assert_eq!(stop[5], 2);
    let stop_ms = u64::from_le_bytes(stop[16..24].try_into().unwrap());
    assert!((299_900..=300_100).contains(&stop_ms.saturating_sub(first_recording_ms)));
    assert!(after[upper * 256..].iter().all(|byte| *byte == 0xff));
    // No later tick can keep appending after automatic stop, and restart preserves export.
    s.advance(5000).unwrap();
    assert_eq!(std::fs::read(&data_path).unwrap(), after);
    s.restart().unwrap();
    s.advance(3000).unwrap();
    assert_eq!(
        command(&mut s, "FOUNDATION LOG INFO").1,
        format!("INFO 1 256 {upper} idle")
    );
    assert_eq!(std::fs::read(&data_path).unwrap(), after);
    assert!(ride_log::decode(&Slot(after[..256].try_into().unwrap())).is_some());
}
