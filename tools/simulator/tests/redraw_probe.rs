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
    let result = run(case("settings (still)"), 30);
    assert!(result.changes.len() <= 1, "{}", result.changes.len());
}

#[test]
fn home_on_usb_draws_only_while_its_battery_fill_visibly_climbs() {
    let climbing = run(case("home, charging at 50%"), 30);
    assert!(climbing.fps() >= 1.9, "{}", climbing.fps());
    for full in ["home, charging at 99%", "home, charging at 100%"] {
        let result = run(case(full), 30);
        assert!(
            result.changes.len() <= 2,
            "{full}: {}",
            result.changes.len()
        );
    }
}

#[test]
fn a_noisy_ride_changes_a_few_strips_a_frame() {
    let result = run(case("ride main, noisy, radar"), 10);
    let (median, p90) = result.strips();
    assert!(result.fps() > 5.0, "{}", result.fps());
    assert!((1..result.strips).contains(&median), "{median}");
    assert!(p90 < result.strips, "{p90}");
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
