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

/// Seconds each charging case runs, and the glyph's fills a second.
const SECONDS: u64 = 30;
const FILLS_A_SECOND: usize = 2;

#[test]
fn home_on_usb_draws_once_a_step_while_its_battery_glyph_fills() {
    for (charging, most) in [
        (
            "home, charging at 50%",
            FILLS_A_SECOND * SECONDS as usize + 2,
        ),
        (
            "home, charging at 99%",
            FILLS_A_SECOND * SECONDS as usize + 2,
        ),
        ("home, charging at 100%", 2),
    ] {
        let result = run(case(charging), SECONDS);
        assert!(
            result.changes.len() <= most,
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
