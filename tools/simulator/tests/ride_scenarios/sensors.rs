use super::*;

#[test]
fn power_meter_dropout_blanks_watts_and_recovers() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("PWR 3S W", "190");
    sim.checkpoint("power reading");

    // A short loss keeps the channel tracking; readings blank, then resume.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("PWR 3S W", "--");
    sim.checkpoint("power meter out of range");

    sim.set_present(device_type::POWER, POWER, true);
    let waited = sim.wait_for("190", seconds(10)).unwrap();
    assert!(waited < seconds(3), "recovered after {waited} ms");
    sim.expect_field("PWR 3S W", "190");
    sim.checkpoint("power reading recovered");
    sim.advance(seconds(10));
    sim.stop_ride();
    let ride = &sim.rides()[0];
    assert!(ride.samples().any(|s| s.power_watts.is_none()));
    assert!(
        ride.samples()
            .filter(|s| s.power_watts == Some(190))
            .count()
            > 15
    );
}

/// The channel times out while the sensor is away; VANA keeps searching for
/// the saved identity with bounded backoff, so watts return by themselves.
#[test]
fn long_power_meter_loss_reconnects_when_it_returns() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 190.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.expect_field("PWR 3S W", "190");

    // Beyond the companion's search timeout the channel closes.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(40));
    sim.expect_field("PWR 3S W", "--");
    sim.checkpoint("power meter gone for 40 s");

    sim.set_present(device_type::POWER, POWER, true);
    sim.wait_for("190", seconds(30)).unwrap();
    sim.expect_field("PWR 3S W", "190");
    sim.checkpoint("power meter back without visiting sensors");
    sim.advance(seconds(10));
    sim.stop_ride();
    // The gap is recorded as missing power, never bridged with old watts.
    let ride = &sim.rides()[0];
    let gap = ride.samples().filter(|s| s.power_watts.is_none()).count();
    assert!(gap >= 35, "{gap} samples without power");
}

#[test]
fn sensors_wait_for_the_companion_after_power_on() {
    let mut sim = Sim::new();
    sim.ant_sensor(device_type::HEART_RATE, HEART);
    // Ask for sensors as soon as Home appears; the radio is still starting.
    sim.advance(seconds(7));
    sim.home("SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.connect_sensor("HEART RATE");
    sim.checkpoint("heart rate connected");
}

#[test]
fn a_speed_and_cadence_sensor_alone_drives_cadence_and_its_low_alert() {
    let mut sim = rider();
    // The power meter is in range but not saved: only the SPD/C crank speaks.
    sim.pair(&["SPEED/CAD"]);
    sim.option("CADENCE_LOW", "60");
    sim.ride_at(28.0, 0.0, 0.0, 85.0);
    sim.start_ride();
    sim.advance(seconds(20));
    sim.expect_field("CAD RPM", "85");
    sim.expect_no_text("CADENCE LOW");
    sim.checkpoint("cadence from the speed and cadence sensor");
    sim.ride_at(28.0, 0.0, 0.0, 55.0);
    sim.wait_for("CADENCE LOW 55", seconds(30)).unwrap();
    sim.checkpoint("alert: cadence below the limit");
    sim.ride_at(28.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(8));
    sim.expect_field("CAD RPM", "0");
}

#[test]
fn ble_heart_strap_drives_heart_rate_and_drops_out_honestly() {
    ble_heart_strap(&mut rider());
}

#[test]
fn ble_select_and_forget_wait_for_a_pending_ble_operation() {
    let mut sim = rider();
    sim.select_ble_heart_strap();
    let (_, settings) = sim.command("SETTINGS");
    let (_, ble) = sim.command("BLE");
    sim.hold_ble_operation();
    let (status, out) = sim.command("BLE SELECT CSC 4f54484552 -");
    assert_eq!(status, "UNAVAILABLE", "BLE SELECT while pending: {out}");
    let (status, out) = sim.command("BLE FORGET");
    assert_eq!(status, "UNAVAILABLE", "BLE FORGET while pending: {out}");
    assert_eq!(
        sim.command("SETTINGS").1,
        settings,
        "a refused choice changed the settings"
    );
    assert!(ble.contains("saved=true profile=1 "), "{ble}");
    let (_, refused) = sim.command("BLE");
    assert!(refused.contains("saved=true profile=1 "), "{refused}");
    sim.finish_ble_operation();
    let (status, out) = sim.command("BLE FORGET");
    assert_eq!(
        status, "ACCEPTED",
        "BLE FORGET once the operation completed: {out}"
    );
    let (_, forgotten) = sim.command("BLE");
    assert!(forgotten.contains("saved=false profile=0 "), "{forgotten}");
}

#[test]
fn wifi_config_and_forget_wait_for_a_pending_wifi_operation() {
    let mut sim = rider();
    sim.need(Need::Wifi);
    let (status, out) = sim.command("WIFI CONFIG WPA2 54657374 70617373776f7264");
    assert_eq!(status, "ACCEPTED", "WIFI CONFIG: {out}");
    sim.hold_wifi_operation();
    let (_, wifi) = sim.command("WIFI");
    let (status, out) = sim.command("WIFI CONFIG WPA3 4f74686572 70617373776f7264");
    assert_eq!(status, "UNAVAILABLE", "WIFI CONFIG while pending: {out}");
    let (status, out) = sim.command("WIFI FORGET");
    assert_eq!(status, "UNAVAILABLE", "WIFI FORGET while pending: {out}");
    assert_eq!(
        sim.command("WIFI").1,
        wifi,
        "a refused change reached Wi-Fi"
    );
    sim.finish_wifi_operation();
    let (status, out) = sim.command("WIFI FORGET");
    assert_eq!(
        status, "ACCEPTED",
        "WIFI FORGET once the operation completed: {out}"
    );
    assert!(sim.command("WIFI").1.contains("saved=false"));
}

/// Let time pass until the channel of this kind is connected.
#[track_caller]
fn wait_connected(sim: &mut Sim, kind: u8, timeout_ms: u64) {
    let mut waited = 0;
    while link(sim, kind) != Some(LinkState::Connected) {
        assert!(
            waited < timeout_ms,
            "{kind} not connected after {waited} ms: {:?}",
            link(sim, kind)
        );
        sim.advance(100);
        waited += 100;
    }
    sim.note(NoteKind::Check, format!("ANT type {kind} connected"));
}

#[track_caller]
#[test]
fn paired_sensors_reconnect_after_power_loss_without_visiting_sensors() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.advance(seconds(1));
    sim.power_loss();
    sim.checkpoint("home after reboot");
    wait_connected(&mut sim, device_type::HEART_RATE, seconds(5));
    wait_connected(&mut sim, device_type::POWER, seconds(5));
    // Saved identities were asked for directly: no discovery ran.
    assert!(
        sim.device
            .sim_mut()
            .ant
            .log
            .iter()
            .all(|(_, request)| !matches!(request, Request::Scan { .. })),
        "{:?}",
        sim.device.sim_mut().ant.log
    );
    assert_eq!(connects(&sim, device_type::HEART_RATE), [HEART]);
    assert_eq!(connects(&sim, device_type::POWER), [POWER]);

    sim.home("SENSORS");
    sim.expect_text("LIVE");
    sim.checkpoint("sensors after reboot");
    go_home(&mut sim);

    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home after reboot");
    sim.start_ride();
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.advance(seconds(5));
    sim.expect_field("PWR 3S W", "205");
    sim.expect_field("HEART BPM", "138");
    sim.checkpoint("riding after reboot");
}

#[test]
fn sensor_that_wakes_late_connects_after_boot() {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    sim.advance(seconds(1));
    // The strap is not worn yet when the device powers on.
    sim.set_present(device_type::HEART_RATE, HEART, false);
    sim.power_loss();
    sim.advance(seconds(25));
    assert_ne!(
        link(&sim, device_type::HEART_RATE),
        Some(LinkState::Connected)
    );
    sim.home("SENSORS");
    let screen = sim.screen();
    assert!(
        screen.contains("SEARCHING")
            || screen.contains("NOT FOUND YET")
            || screen.contains("WAITING FOR RADIO"),
        "{}",
        screen.text()
    );
    sim.checkpoint("heart strap still asleep");
    go_home(&mut sim);
    sim.advance(seconds(60));
    // Searches back off instead of hammering the radio.
    let searches = connects(&sim, device_type::HEART_RATE).len();
    assert!((3..=8).contains(&searches), "{searches} searches");

    sim.set_present(device_type::HEART_RATE, HEART, true);
    wait_connected(&mut sim, device_type::HEART_RATE, seconds(45));
    sim.start_ride();
    sim.ride_at(20.0, 0.0, 131.0, 0.0);
    sim.advance(seconds(5));
    sim.expect_field("HEART BPM", "131");
    sim.checkpoint("late strap reading");
}

#[test]
fn disconnect_is_remembered_across_reboot() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.home("SENSORS");
    sim.disconnect_sensor("HEART RATE");
    assert_eq!(
        sim.device
            .sdk
            .saved_sensors()
            .count_of(device_type::HEART_RATE),
        0
    );
    sim.expect_no_text("HEART RATE");
    sim.checkpoint("heart rate disconnected");
    // The channel closes once the cancelled discovery has released the radio.
    let mut waited = 0;
    while link(&sim, device_type::HEART_RATE) != Some(LinkState::Disconnected) {
        assert!(waited < seconds(15), "{:?}", sim.device.sim_mut().ant.log);
        sim.advance(100);
        waited += 100;
    }
    sim.nearby_page();
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.expect_text("HEART RATE");
    sim.checkpoint("heart rate offered as nearby");
    go_home(&mut sim);

    sim.power_loss();
    wait_connected(&mut sim, device_type::POWER, seconds(5));
    // Less than the dimming timeout, so the next press is not a wake press.
    sim.advance(seconds(20));
    assert!(connects(&sim, device_type::HEART_RATE).is_empty());
    assert_eq!(link(&sim, device_type::HEART_RATE), None);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.checkpoint("home without the removed sensor");
}

fn power_meter_on_channel(sim: &Sim) -> Option<u16> {
    let channel = shown_channel(sim, device_type::POWER)?;
    (channel.link == LinkState::Connected).then_some(channel.selected?.device_number)
}

fn shown_channel(sim: &Sim, kind: u8) -> Option<device_api::ant::Snapshot> {
    use device_api::ant::Ant;
    sim.device.sim().ant.channel(kind, sim.device.now)
}

#[test]
fn two_power_meters_stay_saved_and_the_one_in_range_takes_the_channel() {
    let mut sim = rider();
    const SPARE: u16 = 5151;
    sim.ant_sensor(device_type::POWER, SPARE);
    sim.home("SENSORS");
    sim.wait_for("SCAN DONE", seconds(20)).unwrap();
    sim.connect_sensor("POWER METER");
    sim.connect_sensor("POWER METER");
    sim.expect_text("2 ADDED");
    sim.checkpoint("two power meters added");
    let saved = sim.device.sdk.saved_sensors();
    assert_eq!(saved.count_of(device_type::POWER), 2);
    let in_use = saved.active(device_type::POWER).unwrap().1.device_number;
    let other = if in_use == POWER { SPARE } else { POWER };
    go_home(&mut sim);
    let mut waited = 0;
    while power_meter_on_channel(&sim) != Some(in_use) {
        assert!(
            waited < seconds(30),
            "{:?}",
            shown_channel(&sim, device_type::POWER)
        );
        sim.advance(100);
        waited += 100;
    }

    let scans = |sim: &Sim| {
        let log = &sim.device.sim().ant.log;
        log.iter()
            .filter(|(_, request)| matches!(request, Request::Scan { .. }))
            .count()
    };
    let searched = scans(&sim);
    sim.set_present(device_type::POWER, in_use, false);
    let mut waited = 0;
    while power_meter_on_channel(&sim) != Some(other) {
        assert!(
            waited < seconds(90),
            "{:?}",
            shown_channel(&sim, device_type::POWER)
        );
        sim.advance(100);
        waited += 100;
    }
    let saved = sim.device.sdk.saved_sensors();
    assert_eq!(saved.count_of(device_type::POWER), 2);
    assert_eq!(
        saved.active(device_type::POWER).unwrap().1.device_number,
        other
    );
    assert_eq!(
        scans(&sim),
        searched + 1,
        "one background discovery chose it"
    );
    sim.checkpoint("the other bike's power meter in use");

    sim.power_loss();
    wait_connected(&mut sim, device_type::POWER, seconds(15));
    assert_eq!(power_meter_on_channel(&sim), Some(other));
}

#[test]
fn saved_sensors_keep_the_training_profile() {
    let mut sim = rider();
    sim.set_profile(250, 175);
    sim.advance(seconds(5));
    sim.pair(&["POWER METER"]);
    sim.advance(seconds(1));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=250 lthr=175 weight=0 age=0 sex=- stored=current persistence=saved"
    );
    wait_connected(&mut sim, device_type::POWER, seconds(5));
}

#[test]
fn a_failed_ble_link_is_left_off_home() {
    let mut sim = rider();
    sim.ble_heart_strap(false);
    sim.select_ble_heart_strap();
    sim.advance(seconds(15));
    sim.device.sim_mut().ble.snapshot.link = Link::Failed;
    sim.advance(seconds(1));
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    // Home shows what a device with no sensor shows at the same moment.
    let mut bare = rider();
    bare.advance(sim.now() - bare.now());
    assert!(sim.screen().pixels == bare.screen().pixels);
}
