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
fn a_noisy_ride_changes_a_few_strips_a_frame() {
    let result = run(case("ride main, noisy, radar"), 10);
    let (median, p90) = result.strips();
    assert!(result.fps() > 5.0, "{}", result.fps());
    assert!((1..result.strips).contains(&median), "{median}");
    assert!(p90 < result.strips, "{p90}");
}
