use super::*;

/// A course loaded from the phone page is kept on the memory card beside
/// the files already there, read back after a restart, and cleared from the
/// page.
#[test]
fn a_course_loaded_from_the_phone_is_kept_across_a_restart() {
    use device_api::server::Method;
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.put_file("workouts/keep.vw", b"kept as it was");
    sim.connect_phone();
    let gpx = bend_loop_gpx();
    let (status, body) = sim.phone_request(Method::Put, "/api/course", gpx.as_bytes());
    assert_eq!(status, 200, "{body}");
    assert!(body.contains(r#""name":"BEND LOOP""#), "{body}");
    let stored = sim.file("course.dat").expect("course.dat on FILES");
    assert_eq!(&stored[..5], b"VCRS\x02");
    let turns = sim.file("course-turns.dat").expect("its turns beside it");
    assert_eq!(&turns[..5], b"VTRN\x02");
    assert_eq!(
        &turns[8..12],
        &stored[stored.len() - 4..],
        "named by the course's CRC"
    );
    assert_eq!(
        sim.file("workouts/keep.vw").as_deref(),
        Some(&b"kept as it was"[..])
    );

    sim.power_loss();
    sim.connect_phone();
    let (_, body) = sim.phone_request(Method::Get, "/api/course", b"");
    assert!(
        body.contains(r#""loaded":true"#) && body.contains("BEND LOOP"),
        "{body}"
    );
    let (status, _) = sim.phone_request(Method::Delete, "/api/course", b"");
    assert_eq!(status, 204);
    assert!(sim.file("course.dat").is_none());
    assert!(sim.file("course-turns.dat").is_none());
    let (_, body) = sim.phone_request(Method::Get, "/api/course", b"");
    assert!(body.contains(r#""loaded":false"#), "{body}");
}

/// A workout added from the phone page is stored beside the files already
/// on the card as today's workout, ridden from Home's WORKOUT, and deleted
/// from the page. A damaged upload changes nothing.
#[test]
fn a_workout_added_from_the_phone_is_todays_and_ridden() {
    use device_api::server::Method;
    use vana::workout::codec::encode;
    use vana::workout::model::{Block, Intensity, Length, Step, Target, Workout};
    const STEPS: &[Step] = &[
        Step::new(
            Intensity::Warmup,
            Length::Time(600),
            Target::Ftp { low: 50, high: 65 },
        ),
        Step::new(
            Intensity::Work,
            Length::Time(600),
            Target::Ftp { low: 88, high: 94 },
        ),
        Step::new(
            Intensity::Recovery,
            Length::Time(300),
            Target::Ftp { low: 45, high: 55 },
        ),
        Step::new(
            Intensity::Cooldown,
            Length::Time(600),
            Target::Ftp { low: 40, high: 55 },
        ),
    ];
    const BLOCKS: &[Block] = &[Block {
        first: 1,
        len: 2,
        count: 3,
    }];
    let bytes = encode(&Workout::new("FROM THE PHONE", STEPS, BLOCKS, None));
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    let mut sim = equip(sim.boot());
    sim.init_bulk();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.put_file("workouts/keep.vw", b"not a workout, left as it was");
    sim.connect_phone();
    let (status, body) = sim.phone_request(Method::Get, "/api/workouts", b"");
    assert_eq!(
        (status, body.as_str()),
        (200, r#"{"store":"ok","workout":null}"#)
    );

    // A damaged upload changes nothing.
    let mut damaged = bytes.to_vec();
    damaged[9] ^= 1;
    let (status, body) = sim.phone_request(Method::Put, "/api/workouts", &damaged);
    assert_eq!(status, 422, "{body}");
    let (status, body) = sim.phone_request(Method::Put, "/api/workouts", &bytes);
    assert_eq!(status, 200, "{body}");
    assert!(
        body.contains(r#""name":"FROM THE PHONE","minutes":65,"steps":8"#),
        "{body}"
    );
    let stored = sim
        .file(vana::workout::today::FILE)
        .expect("the workout file on FILES");
    assert_eq!(stored, &bytes[..]);
    assert_eq!(
        sim.file("workouts/keep.vw").as_deref(),
        Some(&b"not a workout, left as it was"[..])
    );
    go_home(&mut sim);

    sim.press_home_start();
    sim.wait_for("RECORDING", seconds(2)).unwrap();
    sim.ride_at(30.0, 120.0, 0.0, 90.0);
    assert_eq!(sim.workout()["workout"], "FROM_THE_PHONE");
    assert_eq!(sim.workout()["step"], "1/8");
    sim.checkpoint("phone workout riding");
    sim.stop_ride();
    sim.done();

    sim.connect_phone();
    let (_, body) = sim.phone_request(Method::Get, "/api/workouts", b"");
    assert!(body.contains(r#""name":"FROM THE PHONE""#), "{body}");
    let (status, _) = sim.phone_request(Method::Delete, "/api/workouts", b"");
    assert_eq!(status, 204);
    assert_eq!(sim.file(vana::workout::today::FILE), None);
    assert!(sim.file("workouts/keep.vw").is_some());
    let (_, today) = sim.command("WORKOUT STATUS");
    assert_eq!(today, "workout=none");
}
