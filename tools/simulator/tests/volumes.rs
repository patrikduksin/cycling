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
