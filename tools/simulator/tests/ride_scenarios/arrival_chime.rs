use super::*;

fn following_a_short_course_with_a_radar() -> Sim {
    let course = gpx_through("short", &[(0.0, 0.0), (0.0, 800.0)]);
    let mut sim = with_a_course(&course);
    let ride = gpx_through("past its end", &[(0.0, 0.0), (0.0, 1_200.0)]);
    sim.ride_route(Route::from_gpx(&ride).unwrap());
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["POWER METER", "SPEED/CAD", "RADAR"]);
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.start_ride();
    ride_until_shown(&mut sim, "FOLLOWING COURSE", 150.0);
    ride_to(&mut sim, 600.0);
    sim
}

fn arrival_chime_ms() -> u64 {
    let mut sim = following_a_short_course_with_a_radar();
    ride_until_cue(&mut sim, ARRIVAL_CUE, 1_000.0);
    let second = sim.now();
    let mut fine = following_a_short_course_with_a_radar();
    fine.advance(second - seconds(1) - fine.now());
    let heard = fine.device.sim().speaker.played.len();
    while fine.device.sim().speaker.played.len() == heard {
        assert!(fine.now() < second, "no note by {second} ms");
        fine.advance(10);
    }
    fine.now()
}

const CHIME_WITHIN_MS: i64 = 10_000;

fn cues_after_acting_before_chime(
    chime_ms: u64,
    lead_ms: u64,
    for_ms: u64,
    act: impl Fn(&mut Sim),
) -> Vec<(i64, String, Vec<u8>)> {
    let mut sim = following_a_short_course_with_a_radar();
    let from = sim.device.sim().speaker.played.len();
    sim.advance(chime_ms - lead_ms - sim.now());
    act(&mut sim);
    sim.advance(for_ms);
    timed_cues_since(&sim, from)
        .into_iter()
        .map(|(at, name, notes)| (at as i64 - chime_ms as i64, name, notes))
        .collect()
}

fn whole_chime_at(heard: &[(i64, String, Vec<u8>)]) -> Option<i64> {
    heard
        .iter()
        .find(|(_, name, notes)| name == ARRIVAL_CUE && notes[..] == ARRIVED)
        .map(|&(at, ..)| at)
}

fn first_at(heard: &[(i64, String, Vec<u8>)], cue: &str) -> Option<i64> {
    heard
        .iter()
        .find(|(_, name, _)| name == cue)
        .map(|&(at, ..)| at)
}

fn closing_car() -> Vehicle {
    Vehicle {
        range_m: 60.0,
        closing_kmh: 30.0,
        fast: true,
        side: Side::Behind,
    }
}

#[test]
fn a_radar_alarm_that_cuts_the_arrival_chime_short_lets_it_sound_whole_after() {
    let chime_ms = arrival_chime_ms();
    let heard =
        cues_after_acting_before_chime(chime_ms, 0, seconds(15), |sim| sim.vehicle(closing_car()));
    let first: Vec<_> = heard
        .iter()
        .take(2)
        .map(|(_, name, notes)| (name, notes))
        .collect();
    assert_eq!(
        first,
        [
            (&ARRIVAL_CUE.to_string(), &vec![0]),
            (&"radar_approach".to_string(), &vec![22])
        ],
        "{heard:?}"
    );
    let whole = whole_chime_at(&heard);
    assert!(whole.is_some_and(|at| at < CHIME_WITHIN_MS), "{heard:?}");
}

#[test]
fn a_radar_alarm_at_arrival_still_gives_the_whole_chime_within_10_s() {
    let chime_ms = arrival_chime_ms();
    let heard = cues_after_acting_before_chime(chime_ms, seconds(3), seconds(15), |sim| {
        sim.vehicle(closing_car())
    });
    let whole = whole_chime_at(&heard).expect("the whole chime");
    assert!((0..CHIME_WITHIN_MS).contains(&whole), "{heard:?}");
    assert!(
        heard
            .iter()
            .any(|(at, name, _)| name.starts_with("radar_") && (0..whole).contains(at)),
        "the chime waited behind the alarm: {heard:?}"
    );
    assert!(first_at(&heard, "radar_passing").is_some(), "{heard:?}");
}

#[test]
fn an_alarm_sustained_past_10_s_after_arrival_drops_the_arrival_chime() {
    let chime_ms = arrival_chime_ms();
    let heard = cues_after_acting_before_chime(chime_ms, 0, seconds(30), |sim| {
        for range_m in [12.0, 22.0, 32.0] {
            sim.vehicle(Vehicle {
                range_m,
                closing_kmh: 6.0,
                fast: false,
                side: Side::Behind,
            });
        }
    });
    let radar_after = |ms: i64| {
        heard
            .iter()
            .any(|(at, name, _)| name.starts_with("radar_") && *at >= ms)
    };
    assert!(radar_after(CHIME_WITHIN_MS), "sustained: {heard:?}");
    assert!(first_at(&heard, "radar_clear").is_some(), "{heard:?}");
    assert_eq!(whole_chime_at(&heard), None, "{heard:?}");
    assert!(
        heard
            .iter()
            .all(|(at, name, _)| name != ARRIVAL_CUE || *at < CHIME_WITHIN_MS),
        "{heard:?}"
    );
}

#[track_caller]
fn pressed_as_the_rider_arrives_the_chime_still_sounds(button: Role, cue: &str) {
    let chime_ms = arrival_chime_ms();
    let by_lead: Vec<_> = (0..=60)
        .step_by(10)
        .map(|lead_ms| {
            let press = |sim: &mut Sim| sim.report(button);
            let heard = cues_after_acting_before_chime(chime_ms, lead_ms, seconds(15), press);
            (lead_ms, heard)
        })
        .collect();
    for (lead_ms, heard) in &by_lead {
        let whole = whole_chime_at(heard);
        assert!(
            first_at(heard, cue).is_some() && whole.is_some_and(|at| at < CHIME_WITHIN_MS),
            "{lead_ms} ms before: {heard:?}"
        );
    }
    let chime_first =
        |heard: &[(i64, String, Vec<u8>)]| first_at(heard, ARRIVAL_CUE) < first_at(heard, cue);
    assert!(
        by_lead.iter().any(|(_, heard)| chime_first(heard))
            && by_lead.iter().any(|(_, heard)| !chime_first(heard)),
        "the presses straddle the pass that arrives: {by_lead:?}"
    );
}

#[test]
fn a_lap_in_the_pass_that_arrives_and_the_arrival_chime_both_sound() {
    pressed_as_the_rider_arrives_the_chime_still_sounds(Role::Back, "lap");
}

#[test]
fn pausing_in_the_pass_that_arrives_keeps_the_arrival_chime() {
    pressed_as_the_rider_arrives_the_chime_still_sounds(Role::Act, "pause");
}
