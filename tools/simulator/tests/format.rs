use device_api::input::Button;
use device_api::storage::Region;
use shell_simulator::bulk::Presence;
use shell_simulator::sim::{Sim, seconds};
use vana::app::{HOLD_GAP_MS, Store};
use vana::screens::workout::Page;

const ACT: Button = Button::BottomRight;
const BACK: Button = Button::TopLeft;

fn field<'a>(status: &'a str, name: &str) -> &'a str {
    status
        .split_whitespace()
        .find_map(|f| f.strip_prefix(name).and_then(|f| f.strip_prefix('=')))
        .unwrap_or_else(|| panic!("{name} in {status}"))
}

fn device_with_foreign(name: &str, stores: &[Store]) -> Sim {
    let mut sim = Sim::named(name);
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    sim.make_foreign(stores);
    sim.cut_power();
    sim
}

fn flash(sim: &mut Sim) -> (Vec<u8>, Vec<u8>) {
    (
        sim.device.media.contents(Region::Configuration),
        sim.device.media.contents(Region::Data),
    )
}

fn wait_for_offer(sim: &mut Sim) {
    sim.wait_for("FORMAT STORAGE?", seconds(20))
        .expect("the offer");
    sim.advance(2 * HOLD_GAP_MS);
}

fn wait_for_restart(sim: &mut Sim) {
    let boot = sim.device.boot;
    for _ in 0..seconds(60) / 100 {
        sim.advance(100);
        if sim.device.boot != boot {
            sim.wait_usable();
            return;
        }
    }
    panic!("no restart; screen:\n{}", sim.screen().text());
}

fn health(sim: &mut Sim) -> String {
    let (status, health) = sim.command("FORMAT");
    assert_eq!(status, "OK");
    health
}

#[test]
fn each_store_of_another_version_alone_raises_the_offer() {
    for store in Store::ALL {
        let mut sim = device_with_foreign(&format!("format-offer-{}", store.name()), &[store]);
        sim.discard_recording();
        wait_for_offer(&mut sim);
        let health = health(&mut sim);
        assert_eq!(field(&health, store.name()), "foreign", "{health}");
        assert_eq!(field(&health, "state"), "offered");
    }
}

#[test]
fn act_formats_everything_and_the_device_then_records_and_keeps_a_ride() {
    let mut sim = device_with_foreign("format-offer", &Store::ALL);
    wait_for_offer(&mut sim);
    sim.expect_text("MADE BY ANOTHER VERSION");
    sim.expect_text("RIDES AND SETTINGS GO");
    sim.checkpoint("the offer");
    sim.press(ACT);
    sim.expect_text("CHECKING STORAGE");
    sim.checkpoint("formatting");
    wait_for_restart(&mut sim);
    assert_eq!(sim.device.sdk.page(), Page::Setup, "a fresh device sets up");
    sim.skip_setup();
    sim.advance(seconds(2));
    let health = health(&mut sim);
    assert_eq!(field(&health, "state"), "idle", "{health}");
    for store in Store::ALL {
        assert_eq!(field(&health, store.name()), "current", "{health}");
    }

    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(30));
    sim.stop_ride();
    assert_eq!(sim.bulk_rides().len(), 1);
    sim.power_loss();
    assert_eq!(sim.device.sdk.page(), Page::Home, "setup stayed done");
    assert_eq!(sim.bulk_rides().len(), 1);
    assert_eq!(field(&health, "state"), "idle");
}

#[test]
fn back_declines_and_nothing_is_written_until_the_next_boot_asks_again() {
    let mut sim = device_with_foreign("format-declined", &[Store::Settings, Store::Rides]);
    let before = flash(&mut sim);
    wait_for_offer(&mut sim);
    sim.press(BACK);
    assert_ne!(sim.device.sdk.page(), Page::Format);
    sim.skip_setup();
    sim.advance(seconds(5));
    assert_eq!(field(&health(&mut sim), "state"), "declined");

    sim.home("SETTINGS");
    sim.expect_text("NOT SAVED: DATA KEPT");
    sim.checkpoint("settings are kept");
    sim.press(BACK);
    sim.home("RIDE");
    sim.advance(seconds(1));
    assert_eq!(flash(&mut sim), before, "nothing written");

    sim.cut_power();
    wait_for_offer(&mut sim);
    assert_eq!(field(&health(&mut sim), "rides"), "foreign");
}

#[test]
fn a_ride_log_of_another_version_says_so_on_the_ready_page() {
    let mut sim = device_with_foreign("format-declined-rides", &[Store::Rides, Store::Internal]);
    wait_for_offer(&mut sim);
    sim.press(BACK);
    sim.skip_setup();
    sim.home("RIDE");
    sim.expect_text("STORAGE NOT FORMATTED");
    sim.expect_text("OFF AND ON TO FORMAT");
    sim.checkpoint("not formatted");
}

#[test]
fn a_button_held_through_boot_neither_formats_nor_declines() {
    for button in [ACT, BACK] {
        let mut sim = device_with_foreign("format-held", &[Store::Settings]);
        sim.discard_recording();
        let before = flash(&mut sim);
        sim.report(button);
        for _ in 0..150 {
            sim.advance(100);
            if !sim.is_off() {
                sim.repeat(button);
            }
        }
        sim.expect_text("FORMAT STORAGE?");
        assert_eq!(field(&health(&mut sim), "state"), "offered", "{button:?}");
        assert_eq!(flash(&mut sim), before, "{button:?}");

        sim.advance(300);
        sim.press(button);
        assert_ne!(field(&health(&mut sim), "state"), "offered", "{button:?}");
    }
}

#[test]
fn format_confirm_wipes_a_current_device_and_keeps_its_terrain() {
    let mut sim = Sim::named("format-confirm");
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    sim.install_terrain(b"CHILE-TERRAIN-V1", &[7; 600]);
    sim.put_file("workouts/a.vw", b"host file");
    let mut sim = sim.boot();
    sim.start_ride();
    sim.advance(seconds(20));
    sim.stop_ride();
    assert_eq!(sim.bulk_rides().len(), 1);
    assert_eq!(sim.command("FORMAT CONFIRM").0, "ACCEPTED");
    assert_eq!(sim.command("VOLUME INIT").0, "BUSY");
    wait_for_restart(&mut sim);
    assert_eq!(sim.device.sdk.page(), Page::Setup);
    assert_eq!(sim.bulk_rides().len(), 0);
    assert_eq!(sim.file("workouts/a.vw"), None);
    let (_, terrain) = sim.command("TERRAIN STATUS");
    assert_eq!(field(&terrain, "state"), "ready", "{terrain}");
}

#[test]
fn a_table_of_another_version_takes_the_terrain_pack_with_it() {
    let mut sim = Sim::named("format-table");
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    sim.make_terrain_foreign();
    sim.make_foreign(&[Store::Table]);
    sim.cut_power();
    wait_for_offer(&mut sim);
    sim.press(ACT);
    wait_for_restart(&mut sim);
    let (_, terrain) = sim.command("TERRAIN STATUS");
    assert_eq!(field(&terrain, "state"), "empty", "{terrain}");
    sim.install_terrain(b"CHILE-TERRAIN-V1", &[7; 600]);
}

#[test]
fn a_terrain_pack_of_another_version_alone_raises_no_offer() {
    let mut sim = Sim::named("format-terrain");
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    sim.make_terrain_foreign();
    let mut sim = sim.boot();
    sim.advance(seconds(5));
    assert_eq!(field(&health(&mut sim), "state"), "idle");
}

#[test]
fn a_wipe_cut_at_any_write_is_offered_again_and_then_completes() {
    for (flash, cut) in (0..40)
        .map(|cut| (false, cut))
        .chain([(true, 0), (true, 1), (true, 2)])
    {
        let mut sim = Sim::named("format-cut");
        sim.discard_recording();
        sim.fit_bulk(Presence::Ready);
        sim.format_rides();
        sim.format_files();
        sim.format_store();
        sim.install_terrain(b"CHILE-TERRAIN-V1", &[7; 600]);
        sim.make_foreign(&Store::ALL);
        sim.cut_power();
        wait_for_offer(&mut sim);
        if flash {
            sim.fail_storage(cut, 2);
        } else {
            sim.fail_bulk(cut, 100);
        }
        sim.press(ACT);
        sim.advance(seconds(10));
        sim.cut_power();
        wait_for_offer(&mut sim);
        sim.press(ACT);
        wait_for_restart(&mut sim);
        let health = health(&mut sim);
        assert_eq!(field(&health, "settings"), "blank", "cut {cut} {flash}");
        for store in Store::ALL.iter().filter(|store| **store != Store::Settings) {
            assert_eq!(
                field(&health, store.name()),
                "current",
                "cut {cut} {flash}: {health}"
            );
        }
        let (_, terrain) = sim.command("TERRAIN STATUS");
        assert_eq!(field(&terrain, "state"), "empty", "cut {cut} {flash}");
    }
}

#[test]
fn a_wipe_that_fails_says_so_on_the_ready_page() {
    let mut sim = device_with_foreign("format-failed", &Store::ALL);
    wait_for_offer(&mut sim);
    sim.fail_bulk(3, 100);
    sim.press(ACT);
    sim.advance(seconds(5));
    assert_eq!(field(&health(&mut sim), "state"), "failed");
    sim.expect_no_text("CHECKING STORAGE");
    sim.expect_text("STORAGE NOT FORMATTED");
    sim.expect_text("OFF AND ON TO FORMAT");
    sim.checkpoint("the wipe failed");
}
