//! The owned MMC region's volumes: migration from the whole-region ride log,
//! and terrain and files installed by the host beside the rides.
#![cfg(feature = "cycling")]
use shell_simulator::bulk::Presence;
use shell_simulator::sim::{Sim, seconds};

fn field<'a>(status: &'a str, name: &str) -> &'a str {
    status
        .split_whitespace()
        .find_map(|f| f.strip_prefix(name).and_then(|f| f.strip_prefix('=')))
        .unwrap_or_else(|| panic!("{name} in {status}"))
}

#[test]
fn the_ride_log_from_before_volumes_is_reported_until_initialized_again() {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    // The bulk store as the whole-region log left it, four test rides on it.
    {
        use firmware_services::sector_log;
        let mut sector = [0u8; 512];
        let initial = vana::ride::bulk::encode_catalog(&Default::default());
        let mut log = sector_log::format(
            &mut sim.device.bulk,
            vana::ride::bulk::TAG,
            1,
            &initial,
            &mut sector,
        )
        .unwrap();
        for _ in 0..4 {
            log.append(&mut sim.device.bulk, &[7; 128], &mut sector)
                .unwrap();
        }
    }
    let writes = sim.device.bulk.writes();
    let mut sim = sim.boot();
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert_eq!(field(&status, "state"), "needs_init");
    assert_eq!(field(&status, "layout"), "legacy");
    let (status, volumes) = sim.command("VOLUME STATUS");
    assert_eq!(
        (status, volumes.as_str()),
        ("OK", "layout=legacy sectors=5777407")
    );
    sim.advance(seconds(10));
    assert_eq!(sim.device.bulk.writes(), writes, "nothing written over it");

    sim.init_bulk();
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert_eq!(field(&status, "layout"), "volumes");
    assert_eq!(field(&status, "rides"), "0");
    let (_, volumes) = sim.command("VOLUME STATUS");
    assert!(volumes.starts_with("layout=volumes"), "{volumes}");
    assert!(volumes.contains("rides=2048+2097152:log"), "{volumes}");
}

fn files_entries(sim: &mut Sim) -> usize {
    use firmware_services::{files, volume};
    let mut sector = [0u8; 512];
    let mut store = volume::open(&mut sim.device.bulk, &volume::FILES, &mut sector).unwrap();
    files::mount(&mut store, &mut sector).unwrap().len()
}

/// The 64 facts files older firmware keeps, one per ride, beside the host's
/// files: after boot they are one table with every ride's facts, the entries
/// they took are free for the host, and the host's files are as they were.
#[test]
fn facts_files_from_older_firmware_fold_into_one_table_that_frees_their_entries() {
    use vana::ride::facts::{self, Facts, Record};
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    for host in 0..40 {
        sim.put_file(&format!("host/{host}.bin"), b"host");
    }
    let records: Vec<Record> = (1..=64)
        .map(|ride| Record {
            journal: b'b',
            ride_id: ride,
            started_utc_ms: 1_760_000_000_000 + u64::from(ride) * 86_400_000,
            facts: Facts {
                profile: vana::ride::zones::TrainingProfile::new(200 + ride as u16, 170),
                recovery_bpm: Some(ride as i16),
                ..Facts::default()
            },
        })
        .collect();
    for record in &records {
        let name = facts::file(record.journal, record.ride_id);
        sim.put_file(name.as_str(), &facts::encode(record));
    }
    assert_eq!(files_entries(&mut sim), 104);

    let mut sim = sim.boot();
    sim.advance(seconds(10));
    let table = sim.file(facts::TABLE).expect("the facts table");
    let mut folded: Vec<Record> = facts::decode_table(&table).collect();
    folded.sort_by_key(|record| record.ride_id);
    assert_eq!(
        folded, records,
        "every ride's facts, as the files held them"
    );
    assert_eq!(files_entries(&mut sim), 41, "one entry for 64 rides");
    for host in 0..40 {
        assert_eq!(
            sim.file(&format!("host/{host}.bin")).as_deref(),
            Some(&b"host"[..])
        );
    }
    for host in 40..120 {
        sim.put_file(&format!("host/{host}.bin"), b"host");
    }
    assert_eq!(
        files_entries(&mut sim),
        121,
        "the freed entries hold host files"
    );
}

#[test]
fn terrain_and_files_installed_by_the_host_live_beside_bulk_rides() {
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    let pack: Vec<u8> = (0..100_000u32).map(|i| (i % 253) as u8).collect();
    sim.install_terrain(b"CHILE-TERRAIN-V1", &pack);
    sim.put_file("workouts/sweet-spot.json", b"{\"steps\":[]}");
    sim.put_file("settings/phone.json", b"{}");
    let (_, terrain) = sim.command("TERRAIN STATUS");
    assert_eq!(field(&terrain, "state"), "ready");
    assert_eq!(field(&terrain, "length"), "100000");
    assert_eq!(field(&terrain, "tag"), "CHILE-TERRAIN-V1");
    let (_, files) = sim.command("FILES LIST");
    assert!(files.starts_with("workouts/sweet-spot.json:12:"), "{files}");
    assert!(files.contains(" settings/phone.json:2:"), "{files}");

    // A ride on the MMC, a power loss, and everything is still there.
    sim.start_ride();
    sim.advance(seconds(30));
    sim.power_loss();
    let (_, status) = sim.command("RIDE BULK STATUS");
    assert_eq!(field(&status, "state"), "recovered");
    assert_eq!(
        sim.file("workouts/sweet-spot.json").as_deref(),
        Some(&b"{\"steps\":[]}"[..])
    );
    let (_, terrain) = sim.command("TERRAIN STATUS");
    assert_eq!(field(&terrain, "state"), "ready");
    let (status, _) = sim.command("FILES DELETE settings/phone.json");
    assert_eq!(status, "OK");
    assert_eq!(sim.file("settings/phone.json"), None);
    assert_eq!(sim.command("VOLUME TRANSFER").0, "UNSUPPORTED");
}
