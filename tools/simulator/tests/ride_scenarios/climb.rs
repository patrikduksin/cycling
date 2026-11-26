use super::*;

#[test]
fn ascent_counts_the_climbs_of_a_hilly_loop() {
    let mut sim = Sim::new().boot();
    sim.need(Need::Barometer);
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ant_sensor(device_type::POWER, POWER);
    // A 4 km loop that rises 50 m, drops 100 m and climbs back: 100 m of ascent.
    sim.ride_route(Route::loop_around(
        Point {
            ele: Some(600.0),
            ..Point::new(-33.45, -70.66)
        },
        4_000.0,
        50.0,
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["SPEED/CAD", "POWER METER"]);
    sim.start_ride();
    sim.ride_at(24.0, 230.0, 0.0, 80.0);
    sim.advance(minutes(5));
    sim.checkpoint("climbing on the main page");
    ride_page(&mut sim, "ride/main");
    // Two kilometres in: up 50 m, then down again.
    expect_near(&mut sim, "CLIMB M", 50.0, 4.0);
    sim.checkpoint("stats halfway round");

    sim.advance(minutes(5));
    // The final climb brought up the climb page; back to the totals.
    ride_page(&mut sim, "ride/main");
    expect_near(&mut sim, "DIST KM", 4.0, 0.05);
    // The 3 m band and smoothing may leave the last metres of the final climb.
    expect_near(&mut sim, "CLIMB M", 100.0, 5.0);
    sim.checkpoint("stats after one hilly lap");

    // The exports carry the barometric elevation: a 600 m start, up to 650 m,
    // down to 550 m, and the session's ascent and descent.
    sim.stop_ride();
    let climbed: f64 = done_stat(&sim, "climbed").unwrap().parse().unwrap();
    assert!((95.0..=105.0).contains(&climbed), "climbed {climbed} m");
    sim.checkpoint("ride done after the hilly lap");
    let rides = sim.rides();
    let ride = &rides[0];
    let metres: Vec<f64> = ride
        .samples()
        .filter_map(|s| s.altitude)
        .map(|a| f64::from(a.dm()) / 10.0)
        .collect();
    assert!(metres.len() > 500, "{} samples with altitude", metres.len());
    let (low, high) = metres
        .iter()
        .fold((f64::MAX, f64::MIN), |(l, h), m| (l.min(*m), h.max(*m)));
    assert!(
        low > 540.0 && high < 660.0 && high - low > 95.0,
        "{low} {high}"
    );
    let gpx = ride.gpx();
    assert!(gpx.matches("<ele>").count() > 500, "GPX elevation");
    let fit = ride
        .fit(&models::C606.product, Some(1_789_000_000_000))
        .expect("a FIT file");
    let records = fit_messages(&fit, 20);
    let altitudes: Vec<f64> = records
        .iter()
        .filter_map(|r| r.get(&2))
        .map(|&v| v as f64 / 5.0 - 500.0)
        .collect();
    assert_eq!(altitudes.len(), metres.len(), "FIT records with altitude");
    let session = &fit_messages(&fit, 18)[0];
    let (ascent, descent) = (session[&22], session[&23]);
    assert!((90..=110).contains(&ascent), "FIT ascent {ascent} m");
    assert!((90..=110).contains(&descent), "FIT descent {descent} m");
}

/// Flat for 1 km at 600 m, 1.5 km at 6% to 690 m, 1.5 km down, then flat.
fn hill_rider() -> Sim {
    let mut sim = hill(Sim::new());
    sim.need(Need::Barometer);
    sim
}

/// `sim`, booted, with the hill route and its sensors paired.
fn hill(sim: Sim) -> Sim {
    let mut sim = sim.boot();
    sim.ant_sensor(device_type::SPEED_CADENCE, SPEED);
    sim.ant_sensor(device_type::POWER, POWER);
    sim.ride_route(Route::profile(
        Point::new(-33.45, -70.66),
        &[
            (0.0, 600.0),
            (1000.0, 600.0),
            (2500.0, 690.0),
            (4000.0, 600.0),
            (6000.0, 600.0),
        ],
    ));
    sim.ride_at(0.0, 0.0, 0.0, 0.0);
    sim.pair(&["SPEED/CAD", "POWER METER"]);
    sim
}

/// The climb page's grade: its largest percentage.
#[track_caller]
fn expect_grade(sim: &mut Sim, expected: f64, tolerance: f64) {
    sim.need(Need::Barometer);
    let screen = sim.screen();
    let grade = screen
        .frame()
        .read()
        .into_iter()
        .filter(|run| run.text.ends_with('%'))
        .max_by_key(|run| run.scale)
        .unwrap_or_else(|| panic!("no grade; screen:\n{}", screen.text()));
    let number: f64 = grade.text.trim_end_matches('%').parse().unwrap();
    assert!(
        (number - expected).abs() <= tolerance,
        "grade {}, expected {expected}% ± {tolerance}",
        grade.text
    );
    sim.note(NoteKind::Check, format!("grade shows {}", grade.text));
}

#[test]
fn climb_page_measures_a_six_percent_climb_over_the_top() {
    let mut sim = hill_rider();
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_page(&mut sim, "ride/climb");
    ride_to(&mut sim, 800.0);
    sim.expect_text("ALTITUDE M");
    expect_near(&mut sim, "ALTITUDE M", 600.0, 2.0);
    sim.expect_field("ASCENT M", "0");
    expect_grade(&mut sim, 0.0, 0.5);
    sim.expect_text("180");
    sim.checkpoint("climb page on the flat");

    // Up at 14 km/h. 20 m of gain confirms the climb about 330 m in.
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_to(&mut sim, 1200.0);
    sim.expect_text("ALTITUDE M");
    let confirmed = ride_until_shown(&mut sim, "LENGTH KM", 1600.0);
    assert!(
        (1300.0..1450.0).contains(&confirmed),
        "climb confirmed at {confirmed:.0} m"
    );

    ride_to(&mut sim, 1750.0);
    expect_grade(&mut sim, 6.0, 0.5);
    expect_near(&mut sim, "HEIGHT M", 45.0, 4.0);
    expect_near(&mut sim, "LENGTH KM", 0.75, 0.1);
    sim.expect_text("260");
    sim.checkpoint("climb page mid-climb");

    ride_to(&mut sim, 2510.0);
    sim.expect_text("LENGTH KM");
    expect_near(&mut sim, "HEIGHT M", 90.0, 4.0);
    expect_near(&mut sim, "LENGTH KM", 1.5, 0.1);
    sim.checkpoint("climb page at the top");

    // Over the top at 40 km/h the climb ends within about 200 m.
    sim.ride_at(40.0, 60.0, 0.0, 0.0);
    let ended = ride_until_shown(&mut sim, "ALTITUDE M", 2800.0);
    assert!(ended > 2500.0, "climb ended at {ended:.0} m");
    ride_to(&mut sim, 4300.0);
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(20));
    sim.expect_text("ALTITUDE M");
    expect_near(&mut sim, "ALTITUDE M", 600.0, 2.0);
    expect_near(&mut sim, "ASCENT M", 90.0, 4.0);
    sim.checkpoint("climb page after the descent");
}

#[test]
fn a_silent_barometer_mid_climb_is_a_gap_not_a_flat_road() {
    let mut sim = hill_rider();
    sim.need(Need::Barometer);
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_page(&mut sim, "ride/climb");
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_until_shown(&mut sim, "LENGTH KM", 1600.0);
    ride_to(&mut sim, 1600.0);

    // 400 m of climbing without pressure readings: the climb page stays.
    sim.set_barometer(false);
    while sim.world().distance_m < 2000.0 {
        sim.advance(seconds(1));
        assert_eq!(sim.ride_page_name().as_deref(), Some("ride/climb"));
        sim.expect_text("LENGTH KM");
    }
    sim.checkpoint("climbing through a barometer gap");
    sim.set_barometer(true);
    ride_to(&mut sim, 2400.0);
    sim.expect_text("LENGTH KM");
    expect_near(&mut sim, "HEIGHT M", 84.0, 5.0);
    sim.checkpoint("climbing after the barometer returns");
}

#[test]
fn bare_shows_no_grade_without_a_barometer() {
    let mut sim = hill(Sim::on(shell_simulator::models::BARE));
    sim.start_ride();
    sim.ride_at(30.0, 180.0, 0.0, 85.0);
    ride_page(&mut sim, "ride/climb");
    ride_to(&mut sim, 1000.0);
    sim.ride_at(14.0, 260.0, 0.0, 75.0);
    ride_to(&mut sim, 1750.0);
    let screen = sim.screen();
    let grades: Vec<_> = screen
        .frame()
        .read()
        .into_iter()
        .filter(|run| run.text.ends_with('%'))
        .map(|run| run.text)
        .collect();
    assert!(
        grades.contains(&"--%".to_string())
            && grades
                .iter()
                .all(|grade| ["%", "--%"].contains(&grade.as_str())),
        "screen:\n{}",
        screen.text()
    );
    sim.expect_field("ALTITUDE M", "--");
    sim.checkpoint("climb page without a barometer");
}
