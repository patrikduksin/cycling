use super::*;

fn paired_from_the_code(sim: &mut Sim) -> String {
    sim.wait_usable();
    sim.open_phone_page();
    sim.expect_text("iPhone Camera");
    let (status, phone) = sim.command("PHONE");
    assert_eq!(status, "OK", "{phone}");
    let code = status_field(&phone, "code");
    assert_eq!(code.len(), 8, "the code's id: {phone}");
    assert_eq!(status_field(&phone, "paired"), "none", "nothing saved yet");
    sim.advance(seconds(3));
    sim.iphone_on_link(true);
    sim.wait_for("iPhone paired", seconds(5)).unwrap();
    let (_, phone) = sim.command("PHONE");
    assert_eq!(status_field(&phone, "paired"), code, "{phone}");
    code
}

#[test]
fn the_rider_pairs_the_iphone_from_the_sheet_and_the_tick_closes_itself() {
    let mut sim = rider();
    paired_from_the_code(&mut sim);
    sim.checkpoint("iphone paired");
    sim.advance(seconds(4));
    assert!(sim.on_home(), "{}", sim.screen().text());

    sim.open_phone_page();
    sim.expect_text("Connected");
    sim.checkpoint("the paired page");
    sim.iphone_on_link(false);
    sim.advance(seconds(1));
    sim.press(Role::Move);
    sim.expect_text("Forget iPhone?");
    sim.press(Role::Act);
    sim.advance(seconds(1));
    let (_, phone) = sim.command("PHONE");
    assert_eq!(status_field(&phone, "paired"), "none", "{phone}");
    assert!(sim.on_home(), "{}", sim.screen().text());
}

#[test]
fn back_on_the_code_keeps_the_device_unpaired() {
    let mut sim = rider();
    sim.wait_usable();
    sim.open_phone_page();
    sim.expect_text("iPhone Camera");
    sim.press(Role::Back);
    sim.advance(seconds(1));
    assert!(sim.on_home(), "{}", sim.screen().text());
    sim.iphone_on_link(true);
    sim.advance(seconds(3));
    let (_, phone) = sim.command("PHONE");
    assert_eq!(status_field(&phone, "paired"), "none", "{phone}");
    assert_eq!(
        status_field(&phone, "state"),
        "closed",
        "nothing advertises"
    );
}

fn a_phone_paired_from_the_code(redacting: bool) -> Vec<u8> {
    let mut sim = hashed();
    if redacting {
        sim.device.redact_secrets();
    }
    let mut sim = sim.boot();
    paired_from_the_code(&mut sim);
    sim.capture()
}

#[test]
fn a_redacted_pairing_from_the_code_replays_faithfully_and_holds_no_key() {
    use shell_simulator::trace::secret::secrets;
    let capture = a_phone_paired_from_the_code(true);
    assert_eq!(secrets(&capture), [] as [&str; 0]);
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{report}");
    let faithful = report.faithful.clone().expect("a replay that ended");
    assert_eq!(faithful.ended, None, "{report}");
}
