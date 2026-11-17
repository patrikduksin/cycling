mod wipes;

use device_api::storage::Region;
use firmware_services::volume::Table;
use shell_simulator::bulk::Presence;
use shell_simulator::sim::{Sim, seconds};
use vana::app::Store;
use vana::screens::workout::Page;
use wipes::*;

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
    assert_eq!(
        sim.command("RIDE START").0,
        "BUSY",
        "no ride under the question"
    );
    let boot = sim.device.boot;
    sim.press(ACT);
    sim.expect_text("CHECKING STORAGE");
    sim.checkpoint("formatting");
    wait_for_restart(&mut sim, boot);
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
fn act_over_a_ride_log_of_another_version_keeps_the_card_rides_and_the_settings() {
    let mut sim = Sim::named("format-internal");
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    let mut sim = sim.boot();
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
    let committed = sim.bulk_rides();
    assert_eq!(committed.len(), 1);
    sim.make_foreign(&[Store::Internal]);
    sim.cut_power();
    let settings = flash(&mut sim).0;

    wait_for_offer(&mut sim);
    assert_eq!(field(&health(&mut sim), "internal"), "foreign");
    sim.expect_text("DEVICE RIDE LOG GOES");
    sim.checkpoint("the offer names the ride log");
    let boot = sim.device.boot;
    sim.press(ACT);
    assert_eq!(
        wait_for_the_wipe(&mut sim, boot),
        WipeEnded::JournalEmptied,
        "no restart"
    );
    assert_eq!(sim.device.sdk.page(), Page::Home, "the settings are kept");
    assert_eq!(flash(&mut sim).0, settings, "the settings are kept");
    let health = health(&mut sim);
    assert_eq!(field(&health, "state"), "idle", "{health}");
    assert_ne!(field(&health, "internal"), "foreign", "{health}");
    let rides = sim.bulk_rides();
    assert_eq!(rides.len(), 1, "the card's ride is kept");
    assert_eq!(rides[0].entries, committed[0].entries);

    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(30));
    sim.stop_ride();
    assert_eq!(sim.bulk_rides().len(), 2, "a new ride records");
}

fn used_device_with_foreign(name: &str, stores: &[Store]) -> (Sim, Table) {
    let (mut sim, table) = used_device(name, stores);
    wait_for_offer(&mut sim);
    (sim, table)
}

#[test]
fn act_writes_over_only_the_store_of_another_version() {
    for store in Store::ALL {
        let (mut sim, table) = used_device_with_foreign("format-one-store", &[store]);
        let before = stored(&mut sim, &table);
        let boot = sim.device.boot;
        sim.press(ACT);
        let ended = if store == Store::Internal {
            WipeEnded::JournalEmptied
        } else {
            WipeEnded::Restarted
        };
        assert_eq!(wait_for_the_wipe(&mut sim, boot), ended, "{}", store.name());
        assert_kept(&before, &stored(&mut sim, &table), &[store], store.name());
        let health = health(&mut sim);
        assert_eq!(field(&health, "state"), "idle", "{health}");
        let wiped = if store == Store::Settings {
            "blank"
        } else {
            "current"
        };
        assert_eq!(field(&health, store.name()), wiped, "{health}");
        let page = if store == Store::Settings {
            Page::Setup
        } else {
            Page::Home
        };
        assert_eq!(sim.device.sdk.page(), page, "{}", store.name());
    }
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
        let boot = sim.device.boot;
        sim.press(button);
        if button == ACT {
            wait_for_restart(&mut sim, boot);
        }
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
    let boot = sim.device.boot;
    assert_eq!(sim.command("FORMAT CONFIRM").0, "ACCEPTED");
    assert_eq!(sim.command("VOLUME INIT").0, "BUSY");
    wait_for_restart(&mut sim, boot);
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
    let boot = sim.device.boot;
    sim.press(ACT);
    wait_for_restart(&mut sim, boot);
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
    for _ in 0..2 {
        sim.press(ACT);
        sim.advance(seconds(1));
        assert_eq!(sim.device.sdk.page(), Page::Ready, "no ride opens");
        assert!(!sim.device.sdk.recording());
    }
    sim.expect_text("STORAGE NOT READY");
    assert_eq!(sim.command("RIDE START").0, "BUSY");
}

#[test]
fn a_declined_offer_leaves_bests_and_a_ride_log_of_another_version_alone() {
    let mut sim = device_with_foreign(
        "format-declined-records",
        &[Store::Records, Store::Internal],
    );
    let bests = sim.file("bests.dat");
    let (_, journal) = flash(&mut sim);
    wait_for_offer(&mut sim);
    sim.press(BACK);
    sim.skip_setup();
    assert_eq!(sim.command("RIDE INTERNAL INIT").0, "FOREIGN");
    assert_eq!(sim.command("RIDE CLEAR CONFIRM 1").0, "FOREIGN");
    sim.home("SETTINGS");
    sim.press(MOVE);
    sim.press(MOVE);
    sim.expect_field("BESTS", "NOT FORMATTED");
    sim.press(ACT);
    sim.expect_text("BESTS NOT FORMATTED");
    sim.checkpoint("bests of another version");
    sim.press(ACT);
    sim.press(ACT);
    sim.advance(seconds(1));
    sim.expect_no_text("CLEAR BESTS?");
    assert_eq!(sim.file("bests.dat"), bests, "the bests are kept");
    assert_eq!(flash(&mut sim).1, journal, "the ride journal is kept");
}

fn tear_internal_start_after_card_failure(sim: &mut Sim) {
    sim.fail_storage(0, 10);
    sim.fail_bulk(0, 0);
    sim.advance(seconds(5));
    let (_, status) = sim.command("RIDE INTERNAL STATUS");
    assert!(status.contains("state=error"), "{status}");
    sim.cut_power();
}

#[test]
fn starts_torn_on_internal_flash_raise_no_offer_and_keep_the_card_rides() {
    let mut sim = Sim::named("format-torn-internal");
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    let mut sim = sim.boot();
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
    let committed = sim.bulk_rides();
    assert_eq!(committed.len(), 1);

    for cut in 1..=2 {
        sim.start_ride();
        sim.advance(seconds(10));
        tear_internal_start_after_card_failure(&mut sim);
        let offer = sim.wait_for("FORMAT STORAGE?", seconds(20));
        assert!(offer.is_err(), "cut {cut}: the offer appeared");
        let (_, status) = sim.command("RIDE INTERNAL STATUS");
        assert_eq!(field(&status, "slot"), cut.to_string(), "{status}");
        let health = health(&mut sim);
        assert_eq!(field(&health, "state"), "idle", "cut {cut}: {health}");
        assert_eq!(field(&health, "internal"), "current", "cut {cut}: {health}");
    }

    let rides = sim.bulk_rides();
    assert_eq!(
        rides[0].entries, committed[0].entries,
        "the card's ride is kept"
    );
    sim.start_ride();
    sim.advance(seconds(30));
    sim.stop_ride();
    assert_eq!(
        sim.bulk_rides().len(),
        rides.len() + 1,
        "a new ride records"
    );
}

#[test]
fn a_resumed_clear_takes_the_brand_then_checking_storage_then_home() {
    let (mut sim, slots) = device_with_a_full_internal_journal("clear-resumed");
    sim.lose_power_at_flash_write(3, 128);
    assert_eq!(
        sim.command(&format!("RIDE CLEAR CONFIRM {slots}")).0,
        "ACCEPTED"
    );
    sim.advance(seconds(10));
    assert!(sim.power_lost());
    let boot = sim.device.boot;
    sim.cut_power();
    let cut = sim.now();
    sim.wait_for("CHECKING STORAGE", seconds(20))
        .expect("the resume says so");
    sim.checkpoint("a Clear cut by power finishes at boot");
    for _ in 0..2 {
        sim.press(BACK);
        sim.press(ACT);
    }
    assert!(!sim.device.sdk.recording(), "no ride starts");
    while sim.device.sdk.page() != Page::Home {
        assert!(sim.now() - cut < seconds(60), "the resume never ends");
        sim.expect_no_text("FORMAT STORAGE?");
        sim.advance(10);
    }
    assert_eq!(sim.device.boot, boot + 1, "no restart");
    assert_eq!(internal_slots(&mut sim), "0");
    assert_intent_settled(&mut sim, "after the resume");
}

/// A cut erase can leave cells that read erased yet hold weakly: here the
/// twin's tear lands every bit, so the cut sector reads erased.
#[test]
fn a_resumed_clear_erases_again_a_sector_its_cut_erase_left_reading_erased() {
    let (mut sim, slots) = device_with_a_full_internal_journal("clear-resumed-erases");
    const FIRST_SECTOR_ERASE: u32 = 3;
    sim.lose_power_at_flash_write(FIRST_SECTOR_ERASE, 4096);
    sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
    sim.advance(seconds(10));
    assert!(sim.power_lost());
    let data = sim.device.media.contents(Region::Data);
    assert!(
        data[..4096].iter().all(|&b| b == 0xff),
        "the cut sector reads erased"
    );
    assert!(
        data[4096..].iter().any(|&b| b != 0xff),
        "the rest is not wiped"
    );
    sim.cut_power();
    sim.wait_usable();
    assert_eq!(internal_slots(&mut sim), "0", "the wipe finished");
    let erased = sim.device.media.fault.borrow().erased.clone();
    for sector in 0..vana::ride::log::SECTORS {
        let offset = sector * vana::ride::log::SECTOR_SIZE;
        assert!(
            erased.contains(&(Region::Data, offset)),
            "sector {sector} was erased again"
        );
    }
}

/// A commit word the cut left marginal reads whole on some boots and torn
/// on others, so a saved capture keeps it to replay its boot as it ran.
#[test]
fn a_boot_over_a_marginal_intent_word_replays_the_same_after_a_save_and_load() {
    use shell_simulator::trace::file;
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("intent-marginal-saved");
    sim.discard_recording();
    for boot in 0..8 {
        let media = sim.media();
        sim.lose_power_at_flash_write(2, 2);
        sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
        sim.advance(seconds(10));
        assert!(sim.power_lost(), "boot {boot}: the commit word was cut");
        sim.cut_power();
        sim.advance(seconds(10));
        let capture = sim.capture();
        assert_eq!(Sim::replay(&capture).device.report().unwrap().first, None);
        let path = std::env::temp_dir()
            .join(format!("intent-marginal-{}", std::process::id()))
            .join(format!("boot-{boot}.trace"));
        file::save(&capture, &path).expect("the capture saves");
        let (loaded, _) = file::load(&path).expect("the capture loads");
        std::fs::remove_dir_all(path.parent().unwrap()).unwrap();
        let report = Sim::replay(&loaded).device.report().unwrap();
        assert_eq!(report.first, None, "boot {boot}: {:?}", report.counts);
        sim.restore_media(&media);
        sim.wait_usable();
    }
}

#[test]
fn a_resuming_wipe_answers_the_console_s_volume_commands_busy() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("clear-resumed-busy");
    let files = sim.device.bulk.image().written;
    sim.lose_power_at_flash_write(3, 128);
    sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
    sim.advance(seconds(10));
    assert!(sim.power_lost());
    // With no companion to wait for, the console answers from boot on.
    sim.device.sim_mut().startup = device_api::sensors::Startup::Unsupported;
    sim.cut_power();
    while field(&health(&mut sim), "state") != "resuming" {
        assert_eq!(sim.device.sdk.page(), Page::Boot, "the resume began");
        sim.advance(1);
    }
    assert_eq!(sim.command("FILES FORMAT CONFIRM").0, "BUSY");
    assert_eq!(sim.command("STORE FORMAT CONFIRM").0, "BUSY");
    assert!(
        sim.device.bulk.image().written == files,
        "the card is untouched"
    );
    sim.wait_usable();
    assert_eq!(internal_slots(&mut sim), "0", "the wipe finished");
}

#[test]
fn an_intent_region_this_version_cannot_read_raises_nothing_and_settles() {
    use device_api::storage::OwnedFlash;
    let foreign: [&[u8]; 3] = [&[0x5a; 8192], b"C606\x02\x00\x18\x00", &[0; 4]];
    for (case, bytes) in foreign.into_iter().enumerate() {
        let what = format!("case {case}");
        let (mut sim, slots) = device_with_a_ride_on_internal_flash("intent-unreadable");
        let card: Vec<_> = sim.bulk_rides().into_iter().map(|r| r.entries).collect();
        let mut media = sim.device.media.untapped();
        media.erase(Region::Intent, 0, 8192).unwrap();
        media.program(Region::Intent, 0, bytes).unwrap();
        sim.cut_power();
        assert_eq!(
            boots_after_a_cut(&mut sim, &what),
            AfterCut::Usable { resumed: false },
            "{what}"
        );
        assert_intent_settled(&mut sim, &what);
        assert_eq!(internal_slots(&mut sim), slots, "{what}");
        let kept: Vec<_> = sim.bulk_rides().into_iter().map(|r| r.entries).collect();
        assert!(kept == card, "{what}: the card's rides are kept");
    }
}

#[test]
fn settings_and_rides_written_before_the_intent_region_survive_with_no_offer() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("intent-first-boot");
    assert_eq!(sim.command("PROFILE FTP 225").0, "OK");
    sim.advance(seconds(5));
    let card: Vec<_> = sim.bulk_rides().into_iter().map(|r| r.entries).collect();
    let (settings, internal) = flash(&mut sim);
    {
        use device_api::storage::OwnedFlash;
        let mut media = sim.device.media.untapped();
        media.erase(Region::Intent, 0, 8192).unwrap();
    }
    sim.cut_power();
    assert_eq!(
        boots_after_a_cut(&mut sim, "first boot"),
        AfterCut::Usable { resumed: false }
    );
    assert_eq!(health(&mut sim).matches("current").count(), 7);
    assert_eq!(flash(&mut sim), (settings, internal), "nothing rewritten");
    assert_eq!(internal_slots(&mut sim), slots);
    let kept: Vec<_> = sim.bulk_rides().into_iter().map(|r| r.entries).collect();
    assert!(kept == card, "the card's rides are kept");
    assert!(
        sim.device
            .media
            .contents(Region::Intent)
            .iter()
            .all(|&b| b == 0xff),
        "a blank intent region needs no write"
    );
}

#[test]
fn format_confirm_over_a_card_that_failed_wipes_the_boot_flash_and_leaves_the_card() {
    let (mut sim, _) = device_with_a_ride_on_internal_flash("format-confirm-failed-card");
    let card = sim.bulk_rides()[0].entries.clone();
    sim.start_ride();
    sim.advance(seconds(10));
    sim.fail_bulk(0, 0);
    sim.advance(seconds(20));
    sim.stop_ride();
    sim.done();
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    assert_eq!(field(&bulk, "state"), "error", "{bulk}");
    let boot = sim.device.boot;
    assert_eq!(sim.command("FORMAT CONFIRM").0, "ACCEPTED");
    wait_for_restart(&mut sim, boot);
    assert_eq!(sim.device.sdk.page(), Page::Setup, "the settings went");
    assert_eq!(internal_slots(&mut sim), "0");
    assert!(
        sim.bulk_rides()[0].entries == card,
        "the card's ride is kept"
    );
    assert_intent_settled(&mut sim, "after the wipe");
}

#[test]
fn an_intent_a_boot_cannot_read_holds_off_rides_and_the_next_boot_finishes_its_wipe() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("intent-unreadable-boot");
    sim.lose_power_at_flash_write(2, 4);
    sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
    sim.advance(seconds(10));
    assert!(
        sim.power_lost(),
        "the Clear was cut at its intent's commit word"
    );
    assert_eq!(intent_pending(&mut sim), 1 << Store::Internal as u8);
    sim.fit_bulk(Presence::Missing);
    sim.cut_power();
    sim.fail_flash_reads(Region::Intent);
    sim.advance(seconds(10));
    assert_eq!(
        sim.command("RIDE START").0,
        "BUSY",
        "no ride on a journal a wipe is due on"
    );
    assert!(!sim.device.sdk.recording());
    assert_eq!(field(&health(&mut sim), "state"), "failed");
    sim.expect_text("OFF AND ON TO FORMAT");
    sim.checkpoint("the intent cannot be read");
    assert_eq!(internal_slots(&mut sim), slots, "nothing wiped yet");
    assert_eq!(
        intent_pending(&mut sim),
        1 << Store::Internal as u8,
        "the wipe is still due"
    );

    sim.cut_power();
    sim.wait_usable();
    assert_eq!(internal_slots(&mut sim), "0", "the wipe finished");
    assert_intent_settled(&mut sim, "after the resume");
    sim.start_ride();
    sim.advance(seconds(20));
    sim.stop_ride();
    sim.done();
    let recorded = internal_slots(&mut sim);
    assert_ne!(recorded, "0");
    sim.power_loss();
    assert_eq!(internal_slots(&mut sim), recorded, "the new ride is kept");
}

#[derive(Clone, Copy, Debug)]
enum Wipe {
    Clear,
    ClearCut,
    Offer,
}

fn device_with_a_deleted_internal_ride(name: &str) -> Sim {
    let mut sim = Sim::named(name);
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    sim.format_files();
    let mut sim = sim.boot();
    sim.start_ride();
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
    assert_eq!(internal_rides(&mut sim), 1);
    sim.home("RIDES");
    sim.advance(seconds(5));
    sim.expect_text("1/1");
    sim.press(ACT);
    sim.wait_gone("READING", seconds(5)).unwrap();
    for _ in 0..6 {
        if sim.screen().contains("DELETE RIDE?") {
            break;
        }
        sim.press(MOVE);
    }
    sim.press(ACT);
    sim.wait_for("RIDE DELETED", seconds(5)).unwrap();
    assert_eq!(hidden_internal_rides(&mut sim), 1);
    while !sim.on_home() {
        sim.press(BACK);
    }
    sim
}

fn internal_rides(sim: &mut Sim) -> usize {
    let (_, history) = sim.command("RIDE INTERNAL HISTORY");
    field(&history, "count").parse().unwrap()
}

fn hidden_internal_rides(sim: &mut Sim) -> usize {
    use vana::kept::deleted;
    let table = sim.file(deleted::TABLE).unwrap_or_default();
    deleted::decode_table(&table)
        .filter(|record| record.ride.journal == b'i')
        .count()
}

fn a_wipe_of_the_ride_log_alone_ends_on_the_first_page_and_its_next_ride_shows(wipe: Wipe) {
    {
        let what = format!("{wipe:?}");
        let mut sim = device_with_a_deleted_internal_ride("ride-log-wipe");
        let slots = internal_slots(&mut sim);
        let boot = match wipe {
            Wipe::Clear => {
                let boot = sim.device.boot;
                let (reply, out) = sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
                assert_eq!(reply, "ACCEPTED", "{out}");
                boot
            }
            Wipe::ClearCut => {
                sim.lose_power_at_flash_write(3, 128);
                sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
                sim.advance(seconds(10));
                assert!(sim.power_lost(), "{what}");
                sim.cut_power();
                sim.device.boot
            }
            Wipe::Offer => {
                use device_api::storage::OwnedFlash;
                let mut media = sim.device.media.untapped();
                media.erase(Region::Data, 0, 4096).unwrap();
                sim.make_foreign(&[Store::Internal]);
                sim.cut_power();
                wait_for_offer(&mut sim);
                sim.expect_text("DEVICE RIDE LOG GOES");
                let boot = sim.device.boot;
                sim.press(ACT);
                boot
            }
        };
        sim.wait_for("CHECKING STORAGE", seconds(20))
            .unwrap_or_else(|error| panic!("{what}: {error}"));
        sim.wait_usable();
        assert_eq!(sim.device.boot, boot, "{what}: no restart");
        assert_eq!(sim.device.sdk.page(), Page::Home, "{what}");
        assert_eq!(field(&health(&mut sim), "state"), "idle", "{what}");
        assert_eq!(internal_rides(&mut sim), 0, "{what}");
        assert_eq!(hidden_internal_rides(&mut sim), 0, "{what}: forgotten");
        assert_intent_settled(&mut sim, &what);

        sim.start_ride();
        sim.advance(seconds(30));
        sim.stop_ride();
        sim.done();
        sim.home("RIDES");
        sim.advance(seconds(5));
        sim.expect_no_text("NO RIDES YET");
        sim.expect_text("1/1");
    }
}

#[test]
fn a_clear_ends_on_the_first_page_and_its_next_ride_shows() {
    a_wipe_of_the_ride_log_alone_ends_on_the_first_page_and_its_next_ride_shows(Wipe::Clear);
}

#[test]
fn a_clear_cut_by_power_ends_on_the_first_page_and_its_next_ride_shows() {
    a_wipe_of_the_ride_log_alone_ends_on_the_first_page_and_its_next_ride_shows(Wipe::ClearCut);
}

#[test]
fn an_accepted_ride_log_offer_ends_on_the_first_page_and_its_next_ride_shows() {
    a_wipe_of_the_ride_log_alone_ends_on_the_first_page_and_its_next_ride_shows(Wipe::Offer);
}

#[test]
fn a_clear_holds_storage_while_it_wipes_and_reports_to_the_host() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("clear-host");
    let boot = sim.device.boot;
    let (reply, out) = sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
    assert_eq!(reply, "ACCEPTED", "{out}");
    let token = field(&out, "token").to_string();
    sim.advance(100);
    sim.expect_text("CHECKING STORAGE");
    assert_eq!(field(&health(&mut sim), "state"), "running");
    assert_eq!(sim.command("RIDE START").0, "BUSY");
    assert_eq!(sim.command("RIDE BULK INIT").0, "BUSY");
    let (status, during) = sim.command("RIDE INTERNAL STATUS");
    assert_eq!(status, "OK", "{during}");
    assert_eq!(field(&during, "pending"), token, "{during}");
    sim.wait_usable();
    assert_eq!(sim.device.boot, boot, "no restart");
    sim.advance(100);
    let (_, after) = sim.command("RIDE INTERNAL STATUS");
    assert_eq!(field(&after, "completed"), token, "{after}");
    assert_eq!(field(&after, "result"), "OK", "{after}");
    assert_eq!(field(&after, "state"), "ready", "{after}");
    assert_eq!(field(&after, "slot"), "0", "{after}");
}

#[test]
fn a_clear_whose_write_fails_holds_storage_until_the_next_boot() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("clear-fails");
    let media = sim.media();
    const INTENT_ERASE: u32 = 0;
    const FIRST_SECTOR_ERASE: u32 = 3;
    const SECOND_SECTOR_ERASE: u32 = 4;
    for fails in [INTENT_ERASE, FIRST_SECTOR_ERASE, SECOND_SECTOR_ERASE] {
        let what = format!("write {fails} fails");
        sim.restore_media(&media);
        sim.wait_usable();
        sim.fail_storage(fails, 0);
        let (reply, out) = sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
        assert_eq!(reply, "ACCEPTED", "{what}: {out}");
        let token = field(&out, "token").to_string();
        sim.advance(seconds(5));
        assert_eq!(field(&health(&mut sim), "state"), "failed", "{what}");
        assert_eq!(sim.command("RIDE START").0, "BUSY", "{what}");
        let (_, status) = sim.command("RIDE INTERNAL STATUS");
        assert_eq!(field(&status, "pending"), "0", "{what}: {status}");
        assert_eq!(field(&status, "completed"), token, "{what}: {status}");
        assert_eq!(field(&status, "result"), "FAILED", "{what}: {status}");
        assert_intent_settled(&mut sim, &what);
        sim.power_loss();
        assert_eq!(field(&health(&mut sim), "state"), "idle", "{what}");
        if fails < SECOND_SECTOR_ERASE {
            assert_eq!(
                internal_slots(&mut sim),
                slots,
                "{what}: nothing was erased"
            );
        }
    }
}

#[test]
fn a_clear_waits_for_a_ride_on_the_card_to_end() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("clear-under-a-ride");
    sim.start_ride();
    sim.advance(seconds(5));
    assert_eq!(
        sim.command(&format!("RIDE CLEAR CONFIRM {slots}")).0,
        "STATE"
    );
    sim.stop_ride();
    sim.done();
    assert_eq!(
        sim.command(&format!("RIDE CLEAR CONFIRM {slots}")).0,
        "ACCEPTED"
    );
}
