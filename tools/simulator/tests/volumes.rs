use shell_simulator::bulk::Presence;
use shell_simulator::sim::{Sim, seconds};

fn field<'a>(status: &'a str, name: &str) -> &'a str {
    status
        .split_whitespace()
        .find_map(|f| f.strip_prefix(name).and_then(|f| f.strip_prefix('=')))
        .unwrap_or_else(|| panic!("{name} in {status}"))
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
