mod power_shows_at_45_kmh;
mod test_step_power_shows_at_45_kmh;

#[cfg(feature = "planted-reading-bug")]
#[test]
#[should_panic(expected = "power!=-- fails: got --")]
fn the_planted_reading_bug_makes_power_shows_at_45_kmh_fail() {
    power_shows_at_45_kmh::power_shows_at_45_kmh();
}

#[cfg(feature = "planted-reading-bug")]
#[test]
#[should_panic(expected = "power!=-- fails: got --")]
fn the_planted_reading_bug_makes_test_step_power_shows_at_45_kmh_fail() {
    test_step_power_shows_at_45_kmh::test_step_power_shows_at_45_kmh();
}
