use core::cell::RefCell;
use device_api::ble_transport::{Ble, Operation, Selection};
use device_api::bulk::ReadWrite;
use device_api::connectivity::Text;
use device_api::input::Button;
use firmware_services::trace::Event;
use firmware_services::trace::frozen::{FrozenBle, Record, Recorder, Sink};
use firmware_services::{files, store, volume};
use shell_simulator::bulk::Presence;
use shell_simulator::compare::{MARKER, Tap};
use shell_simulator::recording::Recording;
use shell_simulator::session::BleFixture;
use shell_simulator::sim::{Sim, minutes, seconds};
use shell_simulator::trace::timeline::read;

fn booted_card_unrecorded() -> Sim {
    let mut sim = Sim::comparing();
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    sim.init_volumes();
    sim.put_file("notes.txt", b"x");
    sim
}

fn ride_then_restart(sim: &mut Sim) {
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(minutes(2));
    sim.stop_ride();
    sim.done();
    sim.power_loss();
    sim.advance(seconds(5));
}

#[test]
fn two_identical_runs_hash_the_same() {
    let run = || {
        let mut sim = booted_card_unrecorded();
        ride_then_restart(&mut sim);
        sim.hashes().unwrap()
    };
    assert_eq!(run(), run());
}

#[test]
fn the_same_press_before_or_after_a_restart_hashes_differently() {
    let run = |before: bool| {
        let mut sim = booted_card_unrecorded();
        for session in [true, false] {
            sim.power_loss();
            sim.advance(seconds(5));
            if before == session {
                sim.report(Button::BottomLeft);
            }
            sim.advance(seconds(5));
        }
        sim.hashes().unwrap()
    };
    assert_ne!(run(true).trace, run(false).trace);
}

#[test]
fn a_press_before_a_restart_changes_the_trace_hash_alone() {
    let run = |press: bool| {
        let mut sim = booted_card_unrecorded();
        if press {
            sim.report(Button::BottomLeft);
        }
        sim.advance(seconds(1));
        sim.power_loss();
        sim.advance(seconds(5));
        sim.hashes().unwrap()
    };
    let (pressed, still) = (run(true), run(false));
    assert_ne!(pressed.trace, still.trace, "the first session's press");
    assert_eq!(
        (pressed.media, pressed.storage),
        (still.media, still.storage)
    );
}

#[derive(Default)]
struct Refusing(u32);

impl Sink for Refusing {
    fn block(&mut self, _: &[u8]) -> bool {
        self.0 += 1;
        false
    }
}

fn refused_blocks_hash(text: &str) -> (String, u32) {
    let tap = Tap::default();
    let mut recorder = Recorder::new(vec![0u8; 64].into_boxed_slice(), Refusing::default());
    recorder.tap(Box::new(tap.clone()));
    for at in 0..8 {
        recorder.record(&Event::Pass { now_ms: at * 1000 });
        recorder.record(&Event::Command(text));
    }
    recorder.flush();
    (tap.hashes(String::new()).trace, recorder.sink().0)
}

#[test]
fn an_event_in_a_block_the_spool_refused_changes_the_trace_hash() {
    let (first, refused) = refused_blocks_hash("one");
    assert!(refused > 1, "every block refused: {refused}");
    assert_ne!(first, refused_blocks_hash("two").0);
}

#[test]
fn an_event_larger_than_any_block_counts_whole() {
    let long = |last: char| format!("{}{last}", "w".repeat(70_000));
    let (a, b) = (long('a'), long('b'));
    assert_ne!(refused_blocks_hash(&a).0, refused_blocks_hash(&b).0);
}

fn ble_calls_hash(calls: &[Operation]) -> String {
    let tap = Tap::default();
    let mut recorder = Recorder::new(vec![0u8; 4096].into_boxed_slice(), Refusing::default());
    recorder.tap(Box::new(tap.clone()));
    let recorder = RefCell::new(recorder);
    let (mut frozen, mut fixture) = (FrozenBle::default(), BleFixture::default());
    let mut ble = frozen.bind(&mut fixture, 0, &recorder);
    for &call in calls {
        let _ = ble.request(call);
    }
    tap.hashes(String::new()).trace
}

#[test]
fn ble_requests_the_trace_passes_through_count_in_order() {
    let scan_then_connect = ble_calls_hash(&[Operation::Scan, Operation::Connect]);
    assert_ne!(scan_then_connect, ble_calls_hash(&[Operation::Scan]));
    assert_ne!(
        scan_then_connect,
        ble_calls_hash(&[Operation::Connect, Operation::Scan])
    );
}

#[test]
fn every_part_of_a_ble_selection_counts() {
    let strap = Selection {
        name: Text::new(b"STRAP").unwrap(),
        address: Some([1, 2, 3, 4, 5, 6]),
        service: 0x180d,
        characteristic: 0x2a37,
    };
    let select = |selection| ble_calls_hash(&[Operation::Select(Some(selection))]);
    let other_name = Selection {
        name: Text::new(b"OTHER").unwrap(),
        ..strap
    };
    let other_address = Selection {
        address: Some([1, 2, 3, 4, 5, 7]),
        ..strap
    };
    let other_service = Selection {
        service: 0x1816,
        ..strap
    };
    let hashes = [
        select(strap),
        select(other_name),
        select(other_address),
        select(other_service),
        ble_calls_hash(&[Operation::Select(None)]),
    ];
    for (at, hash) in hashes.iter().enumerate() {
        assert!(!hashes[at + 1..].contains(hash), "selection {at}");
    }
}

#[test]
fn the_devices_card_reads_change_the_storage_hash_and_a_host_write_the_media_hash() {
    let run = |rides: bool, host_read: bool, note: &[u8]| {
        let mut sim = booted_card_unrecorded();
        sim.advance(seconds(2));
        if rides {
            sim.home("RIDES");
        }
        if host_read {
            assert!(sim.file("notes.txt").is_some());
        }
        sim.put_file("notes.txt", note);
        sim.hashes().unwrap()
    };
    let plain = run(false, false, b"x");
    assert_ne!(
        plain.storage,
        run(true, false, b"x").storage,
        "the rides list"
    );
    assert_eq!(plain, run(false, true, b"x"), "a host's read");
    let edited = run(false, false, b"y");
    assert_ne!(plain.media, edited.media, "one byte of a file");
    assert_eq!(
        (&plain.trace, &plain.storage),
        (&edited.trace, &edited.storage)
    );
}

#[test]
fn reading_a_copy_of_the_card_changes_no_hash() {
    let run = |copies: bool| {
        let mut sim = booted_card_unrecorded();
        sim.advance(seconds(2));
        if copies {
            let mut sector = [0u8; 512];
            for mut copy in [sim.device.bulk.copy(), sim.device.bulk.inspect()] {
                copy.owned_read(0, &mut sector).unwrap();
                copy.owned_write(1, &sector).unwrap();
            }
        }
        sim.advance(seconds(2));
        sim.hashes().unwrap()
    };
    assert_eq!(run(false), run(true));
}

fn zero_sectors(sim: &mut Sim, name: &volume::Name, sectors: &[u64]) {
    let mut sector = [0u8; 512];
    let mut card = sim.device.host_bulk();
    let mut vol = volume::open(&mut card, name, &mut sector).unwrap();
    for &at in sectors {
        vol.owned_write(at, &[0u8; 512]).unwrap();
    }
}

#[test]
fn a_files_volume_that_does_not_mount_still_counts_its_bytes() {
    let run = |note: &[u8]| {
        let mut sim = booted_card_unrecorded();
        sim.put_file("notes.txt", note);
        zero_sectors(&mut sim, &volume::FILES, &[0, files::COPY_B]);
        assert_eq!(sim.file("notes.txt"), None, "FILES no longer mounts");
        sim.hashes().unwrap().media
    };
    assert_ne!(run(b"x"), run(b"y"));
}

fn put_synced_records(sim: &mut Sim, records: &[&[u8]]) {
    sim.format_store();
    let mut sector = [0u8; 512];
    let mut card = sim.device.host_bulk();
    let mut vol = volume::open(&mut card, &volume::STORE, &mut sector).unwrap();
    let mut mounted = store::mount(&mut vol, &mut sector).unwrap();
    let name = store::Name::new("notes").unwrap();
    let mut file = mounted.create(&mut vol, &name, None, &mut sector).unwrap();
    for record in records {
        file.append(&mut mounted, &mut vol, record, &mut sector)
            .unwrap();
        file.sync(&mut mounted, &mut vol, &mut sector).unwrap();
    }
}

#[test]
fn where_a_store_record_ends_counts() {
    let run = |records: &[&[u8]]| {
        let mut sim = booted_card_unrecorded();
        put_synced_records(&mut sim, records);
        assert_eq!(sim.store_file("notes").as_deref(), Some(&b"abc"[..]));
        sim.hashes().unwrap().media
    };
    assert_ne!(run(&[b"ab", b"c"]), run(&[b"a", b"bc"]));
}

#[test]
fn a_store_that_does_not_mount_still_counts_its_bytes() {
    let run = |record: &[u8]| {
        let mut sim = booted_card_unrecorded();
        put_synced_records(&mut sim, &[record]);
        zero_sectors(&mut sim, &volume::STORE, &[0, store::COPY_B]);
        assert_eq!(sim.store_file("notes"), None, "STORE no longer mounts");
        sim.hashes().unwrap().media
    };
    assert_ne!(run(b"one"), run(b"two"));
}

const RECORD_PAYLOAD_BYTE: usize = 100;

fn damage_record(sim: &mut Sim, record: u64, byte: u8) {
    let mut sector = [0u8; 512];
    let mut card = sim.device.host_bulk();
    let mut vol = volume::open(&mut card, &volume::STORE, &mut sector).unwrap();
    let mounted = store::mount(&mut vol, &mut sector).unwrap();
    let name = store::Name::new("notes").unwrap();
    let mut first = None;
    mounted
        .chunk_starts(&mut vol, &name, &mut sector, |at| {
            first.get_or_insert(at);
        })
        .unwrap();
    let at = first.unwrap() + record;
    vol.owned_read(at, &mut sector).unwrap();
    sector[RECORD_PAYLOAD_BYTE] = byte;
    vol.owned_write(at, &sector).unwrap();
}

#[test]
fn the_bytes_of_a_store_record_lost_at_the_end_count() {
    let run = |byte: u8| {
        let mut sim = booted_card_unrecorded();
        put_synced_records(&mut sim, &[b"one", b"two"]);
        damage_record(&mut sim, 1, byte);
        assert_eq!(sim.store_file("notes").as_deref(), Some(&b"one"[..]));
        sim.hashes().unwrap().media
    };
    assert_ne!(run(0x11), run(0x22));
}

#[test]
fn the_bytes_of_a_store_record_lost_in_a_hole_count() {
    let run = |byte: u8| {
        let mut sim = booted_card_unrecorded();
        put_synced_records(&mut sim, &[b"one", b"two", b"three"]);
        damage_record(&mut sim, 0, byte);
        assert_eq!(sim.store_file("notes"), None, "a hole");
        sim.hashes().unwrap().media
    };
    assert_ne!(run(0x11), run(0x22));
}

fn log_line(level: log::Level, module: &'static str, text: &str) {
    log::logger().log(
        &log::Record::builder()
            .level(level)
            .target("sdk.test")
            .module_path_static(Some(module))
            .args(format_args!("{text}"))
            .build(),
    );
}

fn after_logging(lines: &[(log::Level, &str)]) -> String {
    let mut sim = booted_card_unrecorded();
    sim.advance(seconds(1));
    for &(level, text) in lines {
        log_line(level, "vana::app", text);
    }
    sim.advance(seconds(1));
    sim.hashes().unwrap().trace
}

#[test]
fn vana_info_lines_count_in_the_trace_hash_and_others_do_not() {
    let run = |module: Option<&'static str>| {
        let mut sim = booted_card_unrecorded();
        if let Some(module) = module {
            log_line(log::Level::Info, module, "turns_scanned");
        }
        sim.advance(seconds(1));
        sim.hashes().unwrap().trace
    };
    let none = run(None);
    assert_ne!(none, run(Some("vana::app")));
    assert_eq!(none, run(Some("firmware_shell::shell")));
}

#[test]
fn lines_of_either_level_count_in_the_order_they_were_logged() {
    let (info, warn) = ((log::Level::Info, "info"), (log::Level::Warn, "warn"));
    assert_ne!(after_logging(&[info, warn]), after_logging(&[warn, info]));
}

#[test]
fn a_line_counts_at_the_quiet_pass_it_was_logged_in() {
    use vana::app::{STANDBY_DARK_PASS_MS, STANDBY_LIT_MS};
    let run = |quiet_passes: u64| {
        let mut sim = Sim::comparing().boot();
        sim.discard_recording();
        sim.set_battery(64, true);
        sim.advance(seconds(2));
        sim.press(Button::TopLeft);
        sim.press(Button::BottomRight);
        sim.advance(seconds(2));
        assert!(sim.is_off(), "turned off into charging standby");
        sim.advance(STANDBY_LIT_MS + seconds(65));
        sim.advance(STANDBY_DARK_PASS_MS * quiet_passes);
        log_line(log::Level::Info, "vana::app", "dark");
        sim.advance(STANDBY_DARK_PASS_MS * (3 - quiet_passes));
        sim.hashes().unwrap().trace
    };
    assert_ne!(run(1), run(2));
}

#[test]
fn a_warning_past_the_ones_the_trace_keeps_still_counts() {
    let run = |last: &str| {
        let mut lines: Vec<_> = (0..64)
            .map(|n| (log::Level::Warn, format!("line {n}")))
            .collect();
        lines.push((log::Level::Warn, last.to_string()));
        let lines: Vec<_> = lines.iter().map(|(l, t)| (*l, t.as_str())).collect();
        after_logging(&lines)
    };
    assert_ne!(run("a"), run("b"));
}

#[test]
fn lines_logged_after_the_last_pass_count() {
    let run = |level: Option<log::Level>| {
        let mut sim = booted_card_unrecorded();
        sim.advance(seconds(1));
        if let Some(level) = level {
            log_line(level, "vana::app", "late");
        }
        sim.hashes().unwrap().trace
    };
    let none = run(None);
    assert_ne!(none, run(Some(log::Level::Info)), "an info line");
    assert_ne!(none, run(Some(log::Level::Warn)), "a warning");
}

#[test]
fn a_warning_waiting_at_a_power_cut_stays_with_its_session() {
    let run = |before: bool| {
        let mut sim = Sim::comparing().boot();
        sim.discard_recording();
        sim.advance(seconds(1));
        if before {
            log_line(log::Level::Warn, "vana::app", "around the cut");
        }
        sim.cut_power();
        if !before {
            log_line(log::Level::Warn, "vana::app", "around the cut");
        }
        sim.advance(seconds(5));
        let timeline = read(&sim.trace()).expect("the trace decodes");
        let carried = timeline
            .entries
            .iter()
            .any(|entry| entry.text.contains("around the cut"));
        (carried, sim.hashes().unwrap().trace)
    };
    let ((carried_before, before), (carried_after, after)) = (run(true), run(false));
    assert!(!carried_before, "the next session's trace has the warning");
    assert!(carried_after);
    assert_ne!(before, after);
}

#[test]
fn a_warning_waiting_at_a_power_cut_stays_with_its_session_without_comparison() {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.advance(seconds(1));
    log_line(log::Level::Warn, "vana::app", "before the cut");
    sim.cut_power();
    sim.advance(seconds(5));
    let timeline = read(&sim.trace()).expect("the trace decodes");
    assert!(
        !timeline
            .entries
            .iter()
            .any(|entry| entry.text.contains("before the cut")),
        "the next session's trace has the warning"
    );
}

#[test]
fn a_passing_recorded_scenario_writes_its_hashes() {
    let run = std::env::temp_dir().join(format!("sim-compare-hashes-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&run);
    let mut sim = Sim::comparing();
    sim.attach_recording(Recording::at(
        run.join("scenario"),
        "scenario",
        "c606",
        240,
        320,
    ));
    let mut sim = sim.boot();
    let hashes = sim.hashes().unwrap();
    drop(sim);
    let written = std::fs::read_to_string(run.join("scenario/hashes.json"));
    let marked = run.join(MARKER).exists();
    std::fs::remove_dir_all(&run).unwrap();
    assert_eq!(written.unwrap(), hashes.json());
    assert!(marked, "the run says it compared");
}
