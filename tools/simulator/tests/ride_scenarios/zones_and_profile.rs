use super::*;

const ZONE_GREY: u16 = ZONES[0];
const ZONE_BLUE: u16 = ZONES[1];
const ZONE_GREEN: u16 = ZONES[2];
const ZONE_YELLOW: u16 = ZONES[3];
const ZONE_ORANGE: u16 = ZONES[4];
const ZONE_RED: u16 = ZONES[5];
const ZONE_PURPLE: u16 = ZONES[6];

/// The main page's power (first) or heart rate (second) zone label, such as
/// "Z4", and the zone colour filling its field.
#[track_caller]
fn zone_chip(sim: &Sim, heart: bool) -> (String, u16) {
    let screen = sim.screen();
    let mut zones: Vec<_> = screen
        .frame()
        .read()
        .into_iter()
        .filter(|run| run.smooth && run.text.len() == 2 && run.text.starts_with('Z'))
        .collect();
    zones.sort_by_key(|run| run.x);
    let run = zones
        .get(usize::from(heart))
        .unwrap_or_else(|| panic!("no zone label; screen:\n{}", screen.text()));
    // The zone fills the field; its tag is written in the fill's ink.
    let fill = screen.pixels[run.y * screen.width + run.x - 2];
    (run.text.clone(), fill)
}

#[track_caller]
fn expect_chip(sim: &mut Sim, right: bool, zone: &str, colour: u16) {
    let (text, band) = zone_chip(sim, right);
    assert_eq!((text.as_str(), band), (zone, colour), "zone chip");
    sim.note(NoteKind::Check, format!("{zone} chip in {colour:#06x}"));
}

#[test]
fn zone_chips_take_the_colour_of_each_power_zone() {
    let mut sim = rider();
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    // Default profile: FTP 215 W, LTHR 185 bpm.
    for (watts, bpm, power, heart) in [
        (100.0, 120.0, ("Z1", ZONE_GREY), ("Z1", ZONE_GREY)),
        (150.0, 155.0, ("Z2", ZONE_BLUE), ("Z2", ZONE_BLUE)),
        (180.0, 170.0, ("Z3", ZONE_GREEN), ("Z3", ZONE_GREEN)),
        (210.0, 178.0, ("Z4", ZONE_YELLOW), ("Z4", ZONE_YELLOW)),
        (245.0, 188.0, ("Z5", ZONE_ORANGE), ("Z5", ZONE_ORANGE)),
        (300.0, 190.0, ("Z6", ZONE_RED), ("Z5", ZONE_ORANGE)),
        (450.0, 192.0, ("Z7", ZONE_PURPLE), ("Z5", ZONE_ORANGE)),
    ] {
        sim.ride_at(30.0, watts, bpm, 90.0);
        sim.advance(seconds(5));
        sim.expect_field("PWR 3S W", &format!("{watts}"));
        expect_chip(&mut sim, false, power.0, power.1);
        expect_chip(&mut sim, true, heart.0, heart.1);
        sim.checkpoint(&format!("{watts} W in {}", power.0));
    }
    // A lost reading takes its zone away.
    sim.set_present(device_type::POWER, POWER, false);
    sim.advance(seconds(5));
    sim.expect_field("PWR 3S W", "--");
    let (heart, _) = zone_chip(&sim, false);
    assert_eq!(heart, "Z5", "only heart rate's zone is left");
    sim.checkpoint("no power, no zone colour");
}

#[test]
fn raising_ftp_moves_the_same_watts_down_a_zone() {
    let mut sim = rider();
    sim.pair(&["POWER METER", "SPEED/CAD"]);
    sim.start_ride();
    sim.ride_at(30.0, 250.0, 0.0, 90.0);
    sim.advance(seconds(10));
    expect_chip(&mut sim, false, "Z5", ZONE_ORANGE);
    sim.checkpoint("250 W at FTP 215");
    sim.stop_ride();
    go_home(&mut sim);
    sim.set_profile(250, 185);

    sim.ride_at(30.0, 250.0, 0.0, 90.0);
    sim.start_ride();
    sim.advance(seconds(10));
    sim.expect_field("PWR 3S W", "250");
    expect_chip(&mut sim, false, "Z4", ZONE_YELLOW);
    sim.checkpoint("250 W at FTP 250");
}

#[test]
fn training_profile_survives_power_loss() {
    let mut sim = rider();
    let (_, before) = sim.command("PROFILE");
    assert_eq!(
        before, "ftp=215 lthr=185 weight=0 age=0 sex=- stored=missing persistence=saved",
        "a fresh device has the defaults and stores nothing until the first edit"
    );
    sim.set_profile(280, 170);
    sim.advance(seconds(5));

    sim.power_loss();
    let (status, after) = sim.command("PROFILE");
    assert_eq!(status, "OK");
    assert_eq!(
        after,
        "ftp=280 lthr=170 weight=0 age=0 sex=- stored=current persistence=saved"
    );

    // The terminal edits the same profile; an edit that never settled before
    // power was cut is not saved.
    let (status, reply) = sim.command("PROFILE LTHR 175");
    assert_eq!(status, "OK");
    assert_eq!(
        reply,
        "ftp=280 lthr=175 weight=0 age=0 sex=- stored=current persistence=pending"
    );
    assert_eq!(sim.command("PROFILE FTP 9000").0, "INVALID");
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=170 weight=0 age=0 sex=- stored=current persistence=saved"
    );
    sim.command("PROFILE LTHR 175");
    sim.advance(seconds(3));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 weight=0 age=0 sex=- stored=current persistence=saved"
    );
    sim.command("PROFILE WEIGHT 72");
    sim.command("PROFILE AGE 41");
    sim.command("PROFILE SEX M");
    assert_eq!(sim.command("PROFILE WEIGHT 12").0, "INVALID");
    assert_eq!(sim.command("PROFILE SEX X").0, "INVALID");
    sim.advance(seconds(3));
    sim.power_loss();
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 weight=72 age=41 sex=m stored=current persistence=saved"
    );
    // Shell preferences saved later keep the profile.
    assert_eq!(sim.command("BRIGHTNESS 70").0, "OK");
    assert_eq!(sim.command("SAVE").0, "OK");
    sim.power_loss();
    assert!(sim.command("SETTINGS").1.contains("brightness=70"));
    assert_eq!(
        sim.command("PROFILE").1,
        "ftp=280 lthr=175 weight=72 age=41 sex=m stored=current persistence=saved"
    );
}

#[test]
fn options_set_from_the_terminal_apply_at_once_and_survive_power_loss() {
    let mut sim = rider();
    let defaults = "auto_pause=on sounds=on auto_lap=5 wheel=2136 wheel_mode=auto hr_low=0 hr_high=0 power_low=0 power_high=0 cadence_low=0 fuel=0 radar=on theme=night display=normal brightness=100 zone=chile";
    let (status, before) = sim.command("OPTION");
    assert_eq!(status, "OK");
    assert!(before.starts_with(defaults), "{before}");
    for (name, value) in [
        ("AUTO_PAUSE", "OFF"),
        ("SOUNDS", "OFF"),
        ("AUTO_LAP", "START"),
        ("WHEEL", "2100"),
        ("HR_LOW", "100"),
        ("HR_HIGH", "170"),
        ("POWER_LOW", "150"),
        ("POWER_HIGH", "300"),
        ("CADENCE_LOW", "70"),
        ("FUEL", "20"),
        ("RADAR", "OFF"),
        ("THEME", "DAY"),
        ("DISPLAY", "SAVER"),
        ("BRIGHTNESS", "60"),
        ("ZONE", "UTC-4"),
    ] {
        let (status, reply) = sim.command(&format!("OPTION {name} {value}"));
        assert_eq!(status, "OK", "OPTION {name} {value}: {reply}");
    }
    let set = "auto_pause=off sounds=off auto_lap=start wheel=2100 wheel_mode=manual hr_low=100 hr_high=170 power_low=150 power_high=300 cadence_low=70 fuel=20 radar=off theme=day display=saver brightness=60 zone=utc-4";
    assert_eq!(
        sim.command("OPTION").1,
        format!("{set} stored=missing persistence=pending"),
        "a fresh device stores nothing until the first edit settles"
    );
    sim.advance(100);
    let shell = sim.command("SETTINGS").1;
    assert!(shell.starts_with("brightness=60 "), "{shell}");
    for refused in [
        "OPTION AUTO_LAP 101",
        "OPTION WHEEL 90",
        "OPTION HR_LOW 180",
        "OPTION FUEL 25",
        "OPTION BRIGHTNESS 2",
        "OPTION BRIGHTNESS 65",
        "OPTION BRIGHTNESS AUTO",
        "OPTION ZONE UTC+20",
        "OPTION SOUNDS",
        "OPTION DECK MAP",
    ] {
        assert_eq!(sim.command(refused).0, "INVALID", "{refused}");
    }
    sim.advance(seconds(3));
    sim.power_loss();
    assert_eq!(
        sim.command("OPTION").1,
        format!("{set} stored=current persistence=saved")
    );
    assert_eq!(sim.command("OPTION WHEEL_MODE AUTO").0, "OK");
    let reply = sim.command("OPTION").1;
    assert!(reply.contains("wheel=2100 wheel_mode=auto"), "{reply}");
}
