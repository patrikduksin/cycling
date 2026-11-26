use super::*;

fn run_color(sim: &Sim, text: &str) -> u16 {
    let screen = sim.screen();
    let run = screen
        .run(text)
        .or_else(|| screen.find(text).into_iter().next());
    run.unwrap_or_else(|| panic!("no {text:?}; screen:\n{}", screen.text()))
        .color
}

#[test]
fn a_low_battery_turns_sensors_amber_on_their_pages() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(2, 2_900),
    );
    battery(
        &mut sim,
        device_type::HEART_RATE,
        HEART,
        BatteryReport::new(4, 2_450),
    );
    sim.advance(seconds(12));
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());

    sim.home("SENSORS");
    sim.open_sensor_detail("HEART RATE");
    let theme = vana::screens::theme::DARK;
    assert!(shows(&sim, theme.warn), "the battery glyph fills amber");
    sim.checkpoint("heart rate detail with a low battery");
    sim.press(Role::Back);
    sim.open_sensor_detail("POWER METER");
    assert!(!shows(&sim, theme.warn), "a good battery fills in text");
    sim.checkpoint("power meter detail with a good battery");
}

fn shows(sim: &Sim, color: u16) -> bool {
    sim.screen().pixels.contains(&color)
}

/// Whether Home's status widget shows `color`, as its sensors' dots do.
fn widget_shows(sim: &Sim, color: u16) -> bool {
    let screen = sim.screen();
    let (left, right, top, bottom) = (8, 170, 113, 150);
    (top..bottom).any(|y| (left..right).any(|x| screen.pixels[y * screen.width + x] == color))
}

fn connected_strap(sim: &mut Sim, level: Option<u8>) {
    sim.ble_heart_strap(true);
    sim.ble_heart_strap_battery(level);
    sim.select_ble_heart_strap();
    sim.advance(seconds(15));
}

#[test]
fn a_low_ble_strap_battery_turns_its_dot_amber_on_home() {
    let mut sim = rider();
    connected_strap(&mut sim, Some(80));
    let theme = vana::screens::theme::DARK;
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    assert!(
        widget_shows(&sim, theme.good),
        "a live strap's dot is green"
    );
    assert!(!widget_shows(&sim, theme.warn));
    let mut sim = rider();
    connected_strap(&mut sim, Some(12));
    assert!(
        widget_shows(&sim, theme.warn),
        "a low battery's dot is amber"
    );
    assert!(!widget_shows(&sim, theme.bad));
    sim.checkpoint("home with a low ble heart strap battery");
}

#[test]
fn a_critical_ble_strap_battery_is_red_and_the_next_read_clears_it() {
    let mut sim = rider();
    connected_strap(&mut sim, Some(3));
    let theme = vana::screens::theme::DARK;
    assert!(
        widget_shows(&sim, theme.bad),
        "a critical battery's dot is red"
    );
    sim.checkpoint("home with a critical ble heart strap battery");
    // A fresh cell shows on the strap's next minute re-read.
    sim.ble_heart_strap_battery(Some(100));
    sim.advance(seconds(65));
    assert!(!widget_shows(&sim, theme.bad));
    assert!(widget_shows(&sim, theme.good));
}

#[test]
fn a_critical_ant_battery_is_red_on_home_and_a_fresh_one_clears_it() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(5, 2_100),
    );
    sim.advance(seconds(12));
    let theme = vana::screens::theme::DARK;
    assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    assert!(
        widget_shows(&sim, theme.bad),
        "a critical battery's dot is red"
    );
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(1, 3_000),
    );
    sim.advance(seconds(12));
    assert!(!widget_shows(&sim, theme.bad));
    assert!(widget_shows(&sim, theme.good));
}

#[test]
fn reserved_battery_values_and_battery_pages_never_disturb_readings() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER"]);
    for status in [0, 6, 7] {
        battery(
            &mut sim,
            device_type::HEART_RATE,
            HEART,
            BatteryReport::new(status, 2_450),
        );
        battery(
            &mut sim,
            device_type::POWER,
            POWER,
            BatteryReport::new(status, 2_450),
        );
        sim.advance(seconds(12));
        assert!(sim.on_home(), "screen:\n{}", sim.screen().text());
    }
    battery(
        &mut sim,
        device_type::HEART_RATE,
        HEART,
        BatteryReport::new(4, 2_450),
    );
    battery(
        &mut sim,
        device_type::POWER,
        POWER,
        BatteryReport::new(4, 2_450),
    );
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.expect_field("PWR 3S W", "205");
    sim.expect_field("HEART BPM", "138");
    sim.stop_ride();
    let ride = &sim.rides()[0];
    assert!(
        ride.samples()
            .skip(5)
            .all(|s| s.power_watts == Some(205) && s.heart_bpm == Some(138)),
        "battery pages leaked into the recording"
    );
}

#[test]
fn calibrating_a_power_meter_shows_the_offset_it_reports() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Offset(-37));
    sim.press(Role::Act);
    sim.wait_for("Zeroing", seconds(20)).unwrap();
    sim.checkpoint("power meter calibrating");
    sim.wait_for("Zeroed", seconds(5)).unwrap();
    sim.expect_text("offset -37");
    assert_eq!(run_color(&sim, "Zeroed, offset -37"), GREEN);
    sim.checkpoint("power meter calibrated with its offset");
    sim.expect_text("ANT+");
    sim.wait_gone("Zeroed", seconds(12)).unwrap();
}

#[test]
fn a_power_meter_that_refuses_to_zero_ends_in_calibration_failed() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Failure);
    sim.press(Role::Act);
    sim.wait_for("Keep cranks still", seconds(5)).unwrap();
    assert_eq!(run_color(&sim, "Keep cranks still"), AMBER);
    sim.checkpoint("power meter calibration failed");
    sim.press(Role::Act);
    sim.wait_for("Zeroing", seconds(1)).unwrap();
    sim.wait_for("Keep cranks still", seconds(5)).unwrap();
}

#[test]
fn a_silent_power_meter_times_out_in_calibration_failed() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Silence);
    sim.press(Role::Act);
    sim.wait_for("Zeroing", seconds(1)).unwrap();
    sim.advance(seconds(8));
    sim.expect_text("Zeroing");
    sim.checkpoint("power meter still waiting for its answer");
    sim.wait_for("Keep cranks still", seconds(4)).unwrap();
    sim.checkpoint("power meter calibration timed out");
}

#[test]
fn calibrating_a_power_meter_never_disturbs_a_later_ride() {
    let mut sim = rider();
    calibrate_power_meter(&mut sim, ZeroReply::Offset(512));
    sim.press(Role::Act);
    sim.wait_for("Zeroed", seconds(5)).unwrap();
    sim.press(Role::Back);
    sim.press(Role::Back);
    sim.ride_at(27.0, 205.0, 138.0, 88.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.expect_field("PWR 3S W", "205");
    sim.stop_ride();
    let ride = &sim.rides()[0];
    assert!(
        ride.samples().skip(5).all(|s| s.power_watts == Some(205)),
        "calibration pages leaked into the recording"
    );
}
