use super::*;

#[test]
fn turning_off_from_home_asks_first_and_the_top_left_hold_turns_it_back_on() {
    let mut sim = Sim::new().boot();
    sim.press(Role::Back);
    sim.expect_text("TURN OFF");
    sim.checkpoint("turn off sheet");
    // BACK keeps it on.
    sim.press(Role::Back);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.press(Role::Back);
    sim.report(Role::Act);
    sim.advance(80);
    sim.expect_text("TURNING OFF");
    sim.checkpoint("turning off");
    sim.advance(seconds(1));
    assert!(sim.is_off(), "device still on");
    sim.checkpoint("off");
    // Presses other than the power-on hold do nothing while off.
    sim.press(Role::Act);
    sim.press(Role::Back);
    assert!(sim.is_off());
    sim.advance(minutes(1));
    let usable = sim.turn_on();
    assert!(usable <= seconds(5), "Home {usable} ms after turning on");
    sim.checkpoint("Home after turning on");

    // Separate top-left taps never add up to a hold.
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(sim.is_off());
    for _ in 0..shell_simulator::sim::POWER_ON_REPORTS * 2 {
        sim.press(Role::Back);
    }
    assert!(sim.is_off(), "taps turned the device on");

    // Holding the top-left button, as the live window does, also turns it on.
    sim.hold(Role::Back, seconds(2));
    assert!(!sim.is_off(), "a top-left hold did not turn the device on");
    sim.wait_usable();
}

/// Power on closes the VANA ring round the wordmark while Home gets ready;
/// turning off unwinds it to black before the device powers down.
#[test]
fn the_brand_ring_closes_on_power_on_and_unwinds_on_power_off() {
    use vana::screens::chrome::ACCENT;
    let accent = |sim: &Sim| {
        let screen = sim.screen();
        screen.pixels.iter().filter(|&&p| p == ACCENT).count()
    };
    let mut sim = Sim::new();
    let lit = wait_lit(&mut sim);
    sim.expect_text("VANA");
    sim.checkpoint("reveal: the first lit frame");
    let first = accent(&sim);
    sim.advance(200);
    let half = accent(&sim);
    sim.checkpoint("reveal: the ring half closed");
    assert!(half > first + 300, "the ring closes: {first} then {half}");
    let usable = sim.wait_usable();
    assert!(
        usable + 200 <= 500,
        "Home {} ms after the first frame",
        usable + 200
    );
    let _ = lit;
    assert!(sim.on_home(), "{}", sim.screen().text());

    sim.press(Role::Back);
    sim.report(Role::Act);
    sim.advance(80);
    sim.expect_text("TURNING OFF");
    let start = accent(&sim);
    sim.advance(200);
    sim.checkpoint("farewell: the ring unwinding");
    let unwinding = accent(&sim);
    assert!(
        unwinding + 300 < start,
        "the ring unwinds: {start} then {unwinding}"
    );
    assert!(
        !sim.is_off(),
        "the farewell plays before the device turns off"
    );
    sim.advance(seconds(1));
    assert!(sim.is_off());
}

/// Charging shows as a green bolt and a climbing fill; off on USB power,
/// the charge shows on a ring until the screen goes dark.
#[test]
fn charging_while_off_shows_the_charge_then_goes_dark() {
    use vana::app::STANDBY_LIT_MS;
    let mut sim = Sim::new().boot();
    sim.set_battery(64, false);
    sim.advance(seconds(2));
    sim.set_battery(64, true);
    sim.advance(seconds(1));
    sim.expect_text("CHARGING 64%");
    sim.checkpoint("plugged in: charging toast and bolt");
    sim.advance(seconds(4));
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "VANA stays on while charging");
    assert!(sim.device.standby_lit(), "charging standby is dark");
    sim.expect_text("64%");
    sim.expect_text("CHARGING");
    sim.checkpoint("off on USB: the charge");
    sim.set_battery(100, true);
    sim.advance(seconds(1));
    sim.expect_text("CHARGED");
    sim.checkpoint("off on USB: charged");
    sim.advance(STANDBY_LIT_MS);
    assert!(!sim.device.standby_lit(), "standby stays lit");
    assert!(sim.screen().pixels.iter().all(|&p| p == 0));
    sim.checkpoint("off on USB: dark");
    sim.turn_on();
    sim.advance(seconds(3));
    assert!(sim.on_home(), "{}", sim.screen().text());
}

#[test]
fn turned_off_on_usb_power_the_device_starts_again_in_charging_standby() {
    let mut sim = hashed().boot();
    sim.set_battery(64, true);
    sim.advance(seconds(2));
    let ridden = sim.device.boot;
    sim.press(Role::Back);
    sim.press(Role::Act);
    let pressed = sim.now();
    sim.advance(seconds(2));
    assert_eq!(sim.now(), pressed + 2_000, "the world went on for 2 s");
    assert!(sim.is_off(), "turned off");
    let standby = sim.device.boot;
    assert_ne!(standby, ridden, "the companion started the device again");
    assert!(sim.device.standby_lit(), "the charge shows");
    sim.expect_text("CHARGING");
    sim.advance(seconds(3));
    let capture = sim.capture();
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    let frames = sim.frame_hashes();
    assert!(!frames.is_empty() && replayed.frame_hashes() == frames);
    sim.turn_on();
    sim.advance(seconds(3));
    assert!(sim.on_home(), "{}", sim.screen().text());
    assert_eq!(sim.device.boot, standby, "the wake keeps the session");
}

#[test]
fn a_ride_after_a_long_charging_standby_is_in_its_file_whole() {
    use firmware_services::trace::{Event, decode};
    use shell_simulator::replay::first_difference;
    let mut sim = hashed();
    sim.fit_bulk(Presence::Ready);
    sim.format_store();
    let mut sim = equip(sim.boot());
    sim.set_battery(64, true);
    sim.advance(seconds(2));
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(2));
    assert!(sim.is_off(), "in charging standby");
    sim.advance(minutes(40));
    sim.turn_on();
    sim.advance(seconds(3));
    sim.pair(&["HEART RATE", "POWER METER"]);
    sim.ride_at(28.0, 200.0, 140.0, 85.0);
    sim.start_ride();
    sim.advance(minutes(1));
    sim.stop_ride();
    assert!(sim.device.sync_trace(), "the wake opened the file");
    let pulled = sim.store_file(&sim.device.trace_file().unwrap()).unwrap();
    let (_, events) = decode(&pulled).unwrap();
    let lost = events
        .filter(|event| matches!(event, Event::Lost { .. }))
        .count();
    assert_eq!(lost, 0, "the standby fit the spool");
    println!("{}", trace_size(&pulled, sim.device.now));
    let capture = pulled;
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
    assert!(
        replayed.frame_hashes() == sim.frame_hashes(),
        "the same frames"
    );
    let faithful = report.faithful.expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{faithful}");
    let samples: Vec<usize> = (sim.rides().iter())
        .map(|ride| ride.samples().count())
        .collect();
    assert_eq!(samples, [61], "the ride was saved whole");
}

/// Falling charge is announced once at 20%, then urgently at 10% and 5%,
/// on whatever page shows, ride pages included.
#[test]
fn a_falling_battery_warns_once_per_level_during_a_ride() {
    let mut sim = rider();
    sim.set_battery(40, false);
    sim.pair(&["POWER METER"]);
    sim.start_ride();
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    sim.advance(seconds(10));
    sim.set_battery(20, false);
    sim.wait_for("LOW BATTERY 20%", seconds(2)).unwrap();
    sim.checkpoint("low battery during a ride");
    sim.advance(seconds(6));
    sim.expect_no_text("LOW BATTERY");
    sim.set_battery(19, false);
    sim.advance(seconds(6));
    sim.expect_no_text("BATTERY");
    sim.set_battery(10, false);
    sim.wait_for("CHARGE SOON 10%", seconds(2)).unwrap();
    sim.checkpoint("critical battery during a ride");
    sim.stop_ride();
}

#[test]
fn a_fast_drain_during_a_ride_warns_once_that_the_battery_may_not_last() {
    let mut sim = rider();
    sim.pair(&["POWER METER"]);
    sim.set_battery(82, false);
    // A ride drains at 40 %/h, a point every 90 seconds.
    sim.start_ride();
    sim.ride_at(25.0, 180.0, 0.0, 85.0);
    let mut percent = 82;
    let mut shown = false;
    while percent > 12 && !shown {
        // The warning comes on its own tick, so wait for it between points.
        shown = sim.wait_for(" LEFT", seconds(90)).is_ok();
        if !shown {
            percent -= 1;
            sim.set_battery(percent, false);
        }
    }
    assert!(shown, "warned before the battery ran out, at {percent}%");
    sim.checkpoint("ride: the battery may not last another hour");
    // Once: not again while the ride goes on.
    sim.advance(seconds(10));
    sim.expect_no_text(" LEFT");
    sim.set_battery(percent - 1, false);
    sim.advance(seconds(150));
    sim.expect_no_text(" LEFT");
    sim.stop_ride();
}

#[test]
fn a_ride_must_end_before_the_device_can_turn_off() {
    let mut sim = rider();
    sim.pair(&["HEART RATE"]);
    sim.start_ride();
    sim.ride_at(25.0, 0.0, 140.0, 0.0);
    sim.advance(seconds(30));
    // TopLeft marks a lap while recording; it never leaves the ride.
    sim.press(Role::Back);
    sim.expect_no_text("TURN OFF");
    let (_, status) = sim.command("RIDE STATUS");
    assert!(status.contains("state=recording"), "{status}");
    // Paused, TopLeft only asks to end the ride; BACK answers no.
    sim.pause_ride();
    sim.press(Role::Back);
    sim.expect_no_text("TURN OFF");
    sim.expect_text("End ride?");
    sim.press(Role::Back);
    sim.resume_ride();
    sim.stop_ride();
    sim.press(Role::Back);
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(2));
    assert!(sim.is_off());
    let rides = sim.rides();
    assert_eq!(rides.len(), 1);
    assert_eq!(rides[0].kinds().last(), Some(&Kind::Finish));
    sim.turn_on();
    let (_, status) = sim.command("RIDE STATUS");
    assert!(!status.contains("state=recovered"), "{status}");
}

#[test]
fn an_idle_device_warns_then_turns_itself_off_unless_on_usb() {
    use vana::app::{AUTO_OFF_MS, AUTO_OFF_WARNING_MS};
    let mut sim = Sim::new().boot();
    sim.set_battery(60, true);
    sim.advance(AUTO_OFF_MS + minutes(5));
    assert!(!sim.is_off(), "turned off on USB power");
    assert!(sim.on_home(), "{}", sim.screen().text());

    sim.set_battery(60, false);
    sim.press(Role::Move);
    sim.advance(AUTO_OFF_MS - AUTO_OFF_WARNING_MS + seconds(2));
    sim.expect_text("PRESS ANY BUTTON");
    sim.checkpoint("idle off warning");
    // A press keeps it on; the first press may only wake a dimmed screen.
    sim.press(Role::Move);
    sim.expect_no_text("PRESS ANY BUTTON");
    sim.advance(AUTO_OFF_MS + seconds(5));
    assert!(sim.is_off(), "idle device stayed on");
}

#[test]
fn a_ble_heart_strap_on_home_keeps_vana_on() {
    use vana::app::AUTO_OFF_MS;
    let mut sim = Sim::new().boot();
    sim.select_ble_heart_strap();
    sim.ble_heart_strap(true);
    sim.advance(AUTO_OFF_MS + minutes(5));
    assert!(!sim.is_off(), "turned off with a notifying BLE strap");
    sim.expect_no_text("PRESS ANY BUTTON");
    // Once the strap goes away, the idle period runs out.
    sim.ble_heart_strap(false);
    sim.advance(AUTO_OFF_MS + minutes(1));
    assert!(sim.is_off(), "idle device stayed on after the strap left");
}

#[test]
fn turning_off_right_after_a_profile_edit_keeps_the_edit() {
    let mut sim = rider();
    let (_, reply) = sim.command("PROFILE FTP 225");
    assert!(reply.contains("persistence=pending"), "{reply}");
    // Off before the edit settles.
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(sim.is_off());
    sim.turn_on();
    let (_, after) = sim.command("PROFILE");
    assert!(after.starts_with("ftp=225 "), "{after}");
    assert!(after.contains("persistence=saved"), "{after}");
}

#[test]
fn each_unsaved_profile_edit_is_reported_before_turning_off() {
    let mut sim = rider();
    sim.command("PROFILE FTP 225");
    sim.fail_storage(0, 0);
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost FTP 225");
    sim.expect_text("CHANGES NOT");
    // The background retry saves it.
    sim.advance(seconds(10));
    assert!(sim.command("PROFILE").1.contains("persistence=saved"));

    // A new edit whose turn-off save fails is reported again, not dropped.
    sim.command("PROFILE FTP 250");
    sim.fail_storage(0, 0);
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost FTP 250");
    sim.expect_text("CHANGES NOT");
    // Turning off again now saves it if it can.
    sim.advance(seconds(5));
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(sim.is_off());
    sim.turn_on();
    assert!(sim.command("PROFILE").1.starts_with("ftp=250 "));
}

#[test]
fn an_unsaved_brightness_change_is_reported_before_turning_off() {
    let mut sim = rider();
    sim.option("BRIGHTNESS", "30");
    sim.advance(seconds(5));
    sim.fail_storage(0, 0);
    sim.option("BRIGHTNESS", "70");
    sim.press(Role::Back);
    sim.press(Role::Act);
    sim.advance(seconds(1));
    assert!(!sim.is_off(), "turned off and lost the brightness");
    sim.expect_text("CHANGES NOT");
    sim.checkpoint("brightness not saved, still on");
    // The background retry stores it, and it survives a power loss.
    sim.advance(seconds(10));
    sim.power_loss();
    assert_eq!(sim.device.shell.settings.brightness, 70);
}
