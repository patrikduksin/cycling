use shell_simulator::probe::{CASES, run};

fn case(name: &str) -> &'static shell_simulator::probe::Case {
    CASES.iter().find(|case| case.name == name).unwrap()
}

#[test]
fn no_page_submits_a_frame_the_panel_already_shows() {
    for case in &CASES {
        let result = run(case, 10);
        assert_eq!(result.identical(), 0, "{}", case.name);
    }
}

#[test]
fn a_still_page_submits_no_frame() {
    let result = run(case("home"), 30);
    assert!(result.changes.len() <= 1, "{}", result.changes.len());
}

#[test]
fn home_on_usb_draws_only_when_its_percentage_changes() {
    for charging in [
        "home, charging at 50%",
        "home, charging at 99%",
        "home, charging at 100%",
    ] {
        let result = run(case(charging), 30);
        assert!(
            result.changes.len() <= 2,
            "{charging}: {}",
            result.changes.len()
        );
    }
}

#[test]
fn a_noisy_ride_changes_a_few_strips_a_frame() {
    let result = run(case("ride main, noisy, radar"), 10);
    let strips = result.strips();
    assert!(result.fps() > 5.0, "{}", result.fps());
    assert!((1..result.strips).contains(&strips.median), "{strips:?}");
    assert!(strips.p90 < result.strips, "{strips:?}");
}

#[test]
fn saver_submits_a_fraction_of_the_frames_a_noisy_ride_does_in_normal() {
    let normal = run(case("ride main, noisy, radar"), 20);
    let saver = run(case("ride main, noisy, radar, saver"), 20);
    assert!(
        saver.fps() * 3.0 < normal.fps(),
        "{} fps in saver, {} in normal",
        saver.fps(),
        normal.fps()
    );
}
