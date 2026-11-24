use super::*;

/// The wheel size and mode from `OPTION`, such as "wheel=2136 wheel_mode=auto".
fn wheel_option(sim: &mut Sim) -> String {
    let (_, options) = sim.command("OPTION");
    options
        .split_whitespace()
        .filter(|field| field.starts_with("wheel"))
        .collect::<Vec<_>>()
        .join(" ")
}

/// `RIDE WHEEL` as fields.
fn wheel_fields(sim: &mut Sim) -> String {
    let (status, fields) = sim.command("RIDE WHEEL");
    assert_eq!(status, "OK", "{fields}");
    fields
}

/// The millimetres `name` holds in `RIDE WHEEL`'s `fields`.
#[track_caller]
fn wheel_field(fields: &str, name: &str) -> u16 {
    fields
        .split_whitespace()
        .find_map(|field| field.strip_prefix(name)?.strip_prefix('='))
        .and_then(|mm| mm.parse().ok())
        .unwrap_or_else(|| panic!("no {name}: {fields}"))
}

#[test]
fn a_wheel_three_percent_off_is_learned_from_gps_and_used_from_the_next_ride() {
    let mut sim = rider();
    // The bike's wheel is 2200 mm round; VANA assumes 2136 mm, 3 % short.
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED).wheel_mm = 2200.0;
    sim.set_gps_quality(true, 0.9, 1.5);
    sim.pair(&["SPEED/CAD"]);

    assert_eq!(wheel_option(&mut sim), "wheel=2136 wheel_mode=auto");

    // Ride 1 measures with the wrong size and learns the right one, but keeps
    // the wrong one until the ride ends, so its distance never jumps, and
    // writes nothing to the record until then.
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(4));
    let early_speed = reading(&sim, "SPD KM/H");
    assert!(
        (early_speed - 28.0 * 2136.0 / 2200.0).abs() < 0.2,
        "{early_speed}"
    );
    sim.checkpoint("ride 1: a wheel 3 % off reads slow");
    assert!(
        wheel_fields(&mut sim).contains("windows=1"),
        "{}",
        wheel_fields(&mut sim)
    );
    sim.advance(minutes(16));
    let late_speed = reading(&sim, "SPD KM/H");
    assert!(
        (late_speed - early_speed).abs() < 0.2,
        "a size learned mid-ride must not change speed: {early_speed} then {late_speed}"
    );
    let fields = wheel_fields(&mut sim);
    assert!(
        fields.contains("wheel_mm=2136 stored_mm=2136 mode=auto"),
        "{fields}"
    );
    // The learned size depends on where the ride's GPS windows fall, so a
    // millimetre of timing drift may move it.
    for field in ["estimate_mm", "pending_mm"] {
        let mm = wheel_field(&fields, field);
        assert!((2199..=2201).contains(&mm), "{fields}");
    }
    let (shown_before, true_before) = distances(&mut sim, start);
    let error_before = (shown_before - true_before) / true_before * 100.0;
    assert!((error_before + 2.9).abs() < 0.2, "{error_before:.2} %");
    sim.checkpoint("ride 1: still on the old size after learning");
    sim.stop_ride();
    // Stopped, so Ride done does not start the next ride.
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.wait_for("WHEEL SET 2", seconds(5)).unwrap();
    sim.checkpoint("ride 1 over: the rider is told once");
    let fields = wheel_fields(&mut sim);
    let learned = wheel_field(&fields, "stored_mm");
    assert!((2199..=2201).contains(&learned), "{fields}");
    assert!(
        fields.contains(&format!("wheel_mm={learned} stored_mm={learned} mode=auto")),
        "{fields}"
    );
    sim.advance(seconds(6));
    sim.expect_no_text("WHEEL SET");
    go_home(&mut sim);

    assert_eq!(
        wheel_option(&mut sim),
        format!("wheel={learned} wheel_mode=auto")
    );

    // Ride 2 measures with the learned size.
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(20));
    let speed = reading(&sim, "SPD KM/H");
    assert!((speed - 28.0).abs() < 0.2, "{speed}");
    let (shown_after, true_after) = distances(&mut sim, start);
    let error_after = (shown_after - true_after) / true_after * 100.0;
    assert!(error_after.abs() < 0.3, "{error_after:.2} %");
    sim.checkpoint("ride 2: the learned wheel reads right");
    sim.stop_ride();
    sim.advance(seconds(6));
    sim.expect_no_text("WHEEL SET");
    println!("| ride | wheel in force | VANA km | true km | distance error |");
    println!("| --- | --- | --- | --- | --- |");
    println!(
        "| 1, before | 2136 mm | {shown_before:.2} | {true_before:.2} | {error_before:+.2} % |"
    );
    println!(
        "| 2, after | {learned} mm | {shown_after:.2} | {true_after:.2} | {error_after:+.2} % |"
    );
}

#[test]
fn a_hilly_winding_loop_is_learned_just_as_well() {
    let mut sim = rider();
    // A 4 km loop bends 7.5 degrees per fix interval; it climbs 60 m.
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        4_000.0,
        60.0,
    ));
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED).wheel_mm = 2200.0;
    sim.set_gps_quality(true, 0.9, 1.5);
    sim.pair(&["SPEED/CAD"]);
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(20));
    let (shown_before, true_before) = distances(&mut sim, start);
    let error_before = (shown_before - true_before) / true_before * 100.0;
    sim.stop_ride();
    sim.wait_for("WHEEL SET ", seconds(5)).unwrap();
    let fields = wheel_fields(&mut sim);
    let learned: f64 = fields
        .split("stored_mm=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|mm| mm.parse().ok())
        .unwrap();
    assert!((learned - 2200.0).abs() / 2200.0 < 0.005, "{fields}");
    go_home(&mut sim);
    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    let start = sim.world().distance_m;
    sim.advance(minutes(20));
    let (shown_after, true_after) = distances(&mut sim, start);
    let error_after = (shown_after - true_after) / true_after * 100.0;
    assert!(error_after.abs() < 0.5, "{error_after:.2} %");
    sim.stop_ride();
    println!(
        "| hilly, winding 4 km loop, ride 1 before | 2136 mm | {shown_before:.2} | {true_before:.2} | {error_before:+.2} % |"
    );
    println!(
        "| same loop, ride 2 after (learned {learned} mm) | {learned} mm | {shown_after:.2} | {true_after:.2} | {error_after:+.2} % |"
    );
}

#[test]
fn a_manual_wheel_size_is_kept_across_rides_and_power_loss() {
    let mut sim = rider();
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED).wheel_mm = 2200.0;
    sim.set_gps_quality(true, 0.9, 1.5);
    sim.pair(&["SPEED/CAD"]);

    sim.option("WHEEL", "2155");
    assert_eq!(wheel_option(&mut sim), "wheel=2155 wheel_mode=manual");

    sim.ride_at(28.0, 0.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(15));
    let speed = reading(&sim, "SPD KM/H");
    assert!((speed - 28.0 * 2155.0 / 2200.0).abs() < 0.2, "{speed}");
    assert!(
        wheel_fields(&mut sim).contains("windows=0"),
        "no learning while manual"
    );
    // Stopped, so Ride done does not start the next ride.
    sim.ride_at(0.0, 0.0, 140.0, 0.0);
    sim.stop_ride();
    sim.advance(seconds(6));
    sim.expect_no_text("WHEEL SET");

    sim.power_loss();
    assert_eq!(wheel_option(&mut sim), "wheel=2155 wheel_mode=manual");
}
