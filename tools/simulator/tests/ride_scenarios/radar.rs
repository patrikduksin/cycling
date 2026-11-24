use super::*;

/// Radar alerts sounded: the 300 ms pattern, which ride chimes never use.
fn alerts(sim: &Sim) -> usize {
    sim.device
        .sim()
        .speaker
        .played
        .iter()
        .filter(|(_, id)| *id == 22)
        .count()
}

#[test]
fn approaching_vehicle_is_drawn_and_sounds_one_alarm() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.advance(seconds(5));
    // Starting chimed once; no alert yet.
    assert_eq!(alerts(&sim), 0);
    assert!(
        !sim.device.sim_mut().speaker.played.is_empty(),
        "the start chime"
    );

    sim.vehicle(Vehicle {
        range_m: 120.0,
        closing_kmh: 40.0,
        fast: true,
        side: Side::Behind,
    });
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 1, "{radar:?}");
    assert_eq!(alerts(&sim), 1, "the alarm sounds as the vehicle appears");
    sim.advance(seconds(4));
    assert_eq!(alerts(&sim), 1, "closing in ticks, not another alarm");
    sim.checkpoint("vehicle close, alarm sounded once");
    sim.advance(seconds(15));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 0, "vehicle passed");
}

#[test]
fn radar_beeps_faster_and_higher_as_a_vehicle_closes_then_calls_all_clear() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(5));
    let before = sim.device.sim_mut().speaker.played.len();
    sim.vehicle(Vehicle {
        range_m: 140.0,
        closing_kmh: 30.0,
        fast: false,
        side: Side::Behind,
    });
    for _ in 0..80 {
        if sim.wait_for("ALL CLEAR", 500).is_ok() {
            break;
        }
        sim.expect_no_text("VEHICLE");
    }
    sim.expect_text("ALL CLEAR");
    sim.checkpoint("radar: all clear after the vehicle passed");
    sim.advance(seconds(1));
    let heard = sim.device.sim_mut().speaker.played[before..].to_vec();

    // The alarm, then low, middle and high ticks, then ALL CLEAR (low, high).
    let mut ids: Vec<_> = heard.iter().map(|&(_, id)| id).collect();
    ids.dedup();
    assert_eq!(ids, [22, 21, 0, 10, 21, 10], "{heard:?}");
    let gap = |id: u8| -> u64 {
        let at: Vec<_> = heard.iter().filter(|h| h.1 == id).map(|h| h.0).collect();
        at.windows(2).map(|pair| pair[1] - pair[0]).min().unwrap()
    };
    assert!(
        gap(21) > gap(0),
        "low ticks are slower than middle: {heard:?}"
    );
    assert!(
        gap(0) > gap(10),
        "middle ticks are slower than high: {heard:?}"
    );
}

#[test]
fn radar_tracks_a_group_of_six_vehicles_until_they_pass() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["RADAR"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    for (i, side) in [
        Side::Behind,
        Side::Behind,
        Side::Left,
        Side::Behind,
        Side::Right,
        Side::Behind,
    ]
    .into_iter()
    .enumerate()
    {
        sim.vehicle(Vehicle {
            range_m: 60.0 + 25.0 * i as f64,
            closing_kmh: 25.0,
            fast: false,
            side,
        });
    }
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 6, "{radar:?}");
    sim.checkpoint("six vehicles behind");
    // They pass one by one: the count never rises and reaches zero.
    let mut counts = Vec::new();
    for _ in 0..40 {
        sim.advance(seconds(1));
        let radar = sim.device.sdk.radar(sim.device.now);
        counts.push(radar.targets.iter().flatten().count());
    }
    assert!(counts.windows(2).all(|w| w[1] <= w[0]), "{counts:?}");
    assert!(counts.iter().any(|&n| (1..6).contains(&n)), "{counts:?}");
    assert_eq!(counts.last(), Some(&0), "{counts:?}");
    assert_eq!(alerts(&sim), 1, "one alarm for the whole group");
    sim.checkpoint("all vehicles passed");

    // The radar is still connected: a new vehicle is detected.
    sim.vehicle(Vehicle {
        range_m: 80.0,
        closing_kmh: 20.0,
        fast: false,
        side: Side::Behind,
    });
    sim.advance(seconds(2));
    let radar = sim.device.sdk.radar(sim.device.now);
    assert_eq!(radar.targets.iter().flatten().count(), 1, "{radar:?}");
    assert_eq!(alerts(&sim), 2, "the next vehicle sounds the alarm again");
}
