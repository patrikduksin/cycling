//! The owned MMC region's volumes: terrain, files and the store installed by
//! the host beside the rides.
use shell_simulator::bulk::Presence;
use shell_simulator::sim::{Sim, seconds};

fn field<'a>(status: &'a str, name: &str) -> &'a str {
    status
        .split_whitespace()
        .find_map(|f| f.strip_prefix(name).and_then(|f| f.strip_prefix('=')))
        .unwrap_or_else(|| panic!("{name} in {status}"))
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

#[test]
fn a_trace_appended_to_the_store_reads_back_after_a_power_loss() {
    use firmware_services::{store, volume};
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    let (_, volumes) = sim.command("VOLUME STATUS");
    assert_eq!(field(&volumes, "store"), "5244928+532479");
    assert_eq!(
        sim.command("STORE STATUS"),
        ("OK", "state=uninitialized".into())
    );
    sim.format_store();
    let trace: Vec<u8> = (0..20_000u32).map(|i| (i * 31 % 251) as u8).collect();
    {
        let mut sector = [0u8; 512];
        let mut vol = volume::open(&mut sim.device.bulk, &volume::STORE, &mut sector).unwrap();
        let mut mounted = store::mount(&mut vol, &mut sector).unwrap();
        let name = store::Name::new("traces/0001").unwrap();
        let rollover = Some(store::Dir::new("traces/").unwrap());
        let mut file = mounted
            .create(&mut vol, &name, rollover, &mut sector)
            .unwrap();
        file.append(&mut mounted, &mut vol, &trace, &mut sector)
            .unwrap();
        file.sync(&mut mounted, &mut vol, &mut sector).unwrap();
    }
    assert_eq!(sim.command("STORE LIST").1, "traces/0001:1:1");
    sim.power_loss();
    assert_eq!(sim.store_file("traces/0001"), Some(trace));
    let own = sim
        .device
        .trace_file()
        .expect("the session records its own trace");
    assert!(own.starts_with("traces/000001-"), "{own}");
    assert_eq!(field(&sim.command("STORE STATUS").1, "files"), "2");
}
