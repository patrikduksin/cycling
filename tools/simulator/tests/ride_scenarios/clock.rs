use super::*;

/// Receiver UTC in ms at simulated time `t_ms`, for a receiver left on its
/// default clock: 10:00 UTC on 14 October 2026 at t=0.
fn receiver_utc_ms(t_ms: u64) -> u64 {
    unix_seconds(*b"141026", *b"100000").unwrap() as u64 * 1000 + t_ms
}

#[test]
fn a_ride_without_wifi_is_dated_by_the_gps_receiver() {
    let mut sim = rider();
    cyclist(&mut sim);
    let pressed = press_start(&mut sim);
    sim.advance(minutes(2));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    let utc: Vec<_> = ride.samples().map(|s| s.utc_ms).collect();
    assert!(utc.iter().all(Option::is_some), "every sample is dated");
    assert!(
        utc.windows(2).all(|w| w[0] < w[1]),
        "time only moves forward"
    );
    let start = fit_start_ms(&ride);
    let expected = receiver_utc_ms(pressed);
    assert!(
        start.abs_diff(expected) < 1_000,
        "the FIT starts at {start}, the rider pressed START at {expected}"
    );
}

#[test]
fn a_fix_found_after_the_start_dates_the_ride_from_its_start() {
    let mut sim = rider();
    sim.set_gps_fix(false);
    cyclist(&mut sim);
    let pressed = press_start(&mut sim);
    sim.advance(minutes(3));
    sim.set_gps_fix(true);
    sim.advance(minutes(2));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    let samples: Vec<_> = ride.samples().collect();
    let first_dated = samples.iter().position(|s| s.utc_ms.is_some()).unwrap();
    assert!(first_dated > 150, "undated for the first 3 minutes");
    assert!(
        samples[first_dated..].iter().all(|s| s.utc_ms.is_some()),
        "dated from the first fix on"
    );
    assert!(samples[..first_dated].iter().all(|s| s.utc_ms.is_none()));
    let start = fit_start_ms(&ride);
    let expected = receiver_utc_ms(pressed);
    assert!(
        start.abs_diff(expected) < 1_000,
        "the FIT starts at {start}, not at the fix: the rider pressed START at {expected}"
    );
}

#[test]
fn a_ride_through_utc_midnight_keeps_going_forward() {
    let mut sim = rider();
    sim.set_gps_clock(23 * 3600 + 59 * 60, (14, 10, 26));
    cyclist(&mut sim);
    sim.advance(seconds(5));
    press_start(&mut sim);
    sim.advance(minutes(3));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    let utc: Vec<_> = ride.samples().map(|s| s.utc_ms.unwrap()).collect();
    assert!(
        utc.windows(2).all(|w| w[0] < w[1]),
        "UTC steps back at midnight"
    );
    let day = |ms: u64| ms / 86_400_000;
    let (before, after) = (day(utc[0]), day(*utc.last().unwrap()));
    assert_eq!(after, before + 1, "the ride rolls into the next UTC day");
    assert_eq!(
        after,
        unix_seconds(*b"151026", *b"000000").unwrap() as u64 / 86_400
    );
}

#[test]
fn a_receiver_date_outside_the_plausible_window_leaves_the_ride_undated() {
    let mut sim = rider();
    // A GPS week rollover reports 2006 for today.
    sim.set_gps_clock(10 * 3600, (14, 10, 6));
    sim.advance(seconds(5));
    let gps = sim.device.sim().position.snapshot(sim.now()).unwrap().gps;
    assert_eq!(gps.state, FixState::Fresh);
    assert_eq!(gps.utc_ms, None, "the receiver never takes the date");
    cyclist(&mut sim);
    sim.advance(seconds(5));
    press_start(&mut sim);
    sim.advance(minutes(2));
    sim.stop_ride();
    let ride = sim.rides().pop().unwrap();
    assert!(ride.samples().all(|s| s.utc_ms.is_none()));
    assert_eq!(
        ride.fit_plan(&models::C606.product).err(),
        Some(Refusal::NoClock)
    );
}

fn clock_reads(date: (u8, u8, u8), local: &str, day: &str) -> Sim {
    let mut sim = rider();
    sim.set_gps_clock(16 * 3600, date);
    sim.advance(seconds(5));
    sim.expect_text(local);
    sim.checkpoint(&format!("home at 16:00 UTC, {day}"));
    sim
}

/// Santiago keeps UTC-4 in winter. A rider in Magallanes or Aysén picks
/// UTC-3, which holds all year.
#[test]
fn the_clock_reads_santiago_winter_time_in_june() {
    let mut sim = clock_reads((21, 6, 26), "12:00", "JUN 21");
    sim.option("ZONE", "UTC-3");
    sim.advance(seconds(1));
    assert!(
        sim.screen().contains("1:00") || sim.screen().contains("1:01"),
        "{}",
        sim.screen().text()
    );
    sim.checkpoint("time zone fixed at utc-3");
}

/// Santiago keeps UTC-3 in summer.
#[test]
fn the_clock_reads_santiago_summer_time_in_december() {
    clock_reads((21, 12, 26), "1:00", "DEC 21");
}

#[test]
fn a_clock_set_from_the_host_shows_on_home_and_outlives_a_crash() {
    let mut sim = hashed().boot();
    a_clock_set_from_the_host(&mut sim);
    sim.crash();
    sim.wait_usable();
    sim.expect_text("5:30");
    let (status, time) = sim.command("TIME");
    assert_eq!(status, "OK");
    assert!(
        time.contains("source=set"),
        "a crash keeps the clock: {time}"
    );
    let shown = time
        .split("unix_seconds: Some(")
        .nth(1)
        .and_then(|rest| rest.split(')').next());
    let shown: u64 = shown.and_then(|seconds| seconds.parse().ok()).expect(&time);
    assert!((SET_UTC_S..SET_UTC_S + 60).contains(&shown), "{time}");
    sim.power_loss();
    sim.expect_text("--:--");
    let (_, time) = sim.command("TIME");
    assert!(time.contains("source=none"), "power loss: {time}");
}

#[test]
fn the_receiver_overrules_a_set_clock() {
    let mut sim = rider();
    sim.advance(seconds(5));
    sim.expect_text("7:00");
    sim.command(&format!("TIME SET {SET_UTC_S}"));
    sim.advance(seconds(2));
    let (_, time) = sim.command("TIME");
    assert!(time.contains("source=gnss"), "{time}");
    sim.expect_text("7:00");
    sim.expect_no_text("5:30");
}
