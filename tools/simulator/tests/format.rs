use device_api::input::Button;
use device_api::storage::Region;
use firmware_services::volume::{self, Table};
use shell_simulator::bulk::Presence;
use shell_simulator::sim::{Sim, seconds};
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use vana::app::{HOLD_GAP_MS, Store};
use vana::screens::workout::Page;

const ACT: Button = Button::BottomRight;
const BACK: Button = Button::TopLeft;
const MOVE: Button = Button::BottomLeft;

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

struct Stored {
    digests: BTreeMap<&'static str, u64>,
    traces: BTreeMap<String, Vec<u8>>,
}

fn stored(sim: &mut Sim, table: &Table) -> Stored {
    let mut traces = BTreeMap::new();
    if let Some(names) = store_names(sim) {
        for name in names {
            let bytes = sim.store_file(&name).expect("a trace reads whole");
            traces.insert(name, bytes);
        }
    }
    let mut held: BTreeMap<_, DefaultHasher> = BTreeMap::new();
    for (sector, bytes) in sim.device.bulk.image().written {
        let name = match table
            .volumes()
            .find(|v| (v.start..v.start + v.sectors).contains(&sector))
            .map(|v| v.name)
        {
            Some(volume::RIDES) => "rides",
            Some(volume::FILES) => "files",
            Some(volume::STORE) => "traces",
            Some(volume::TERRAIN) => "terrain",
            _ => "table",
        };
        (sector, bytes).hash(held.entry(name).or_default());
    }
    let (settings, internal) = flash(sim);
    settings.hash(held.entry("settings").or_default());
    internal.hash(held.entry("internal").or_default());
    Stored {
        digests: held
            .into_iter()
            .map(|(name, h)| (name, h.finish()))
            .collect(),
        traces,
    }
}

fn store_names(sim: &mut Sim) -> Option<Vec<String>> {
    use firmware_services::store;
    let mut sector = [0u8; 512];
    let mut media = sim.device.bulk.host();
    let mut traces = volume::open(&mut media, &volume::STORE, &mut sector).ok()?;
    let mounted = store::mount(&mut traces, &mut sector).ok()?;
    let mut names = Vec::new();
    mounted
        .list(&mut traces, &store::Dir::ROOT, &mut sector, |entry| {
            names.push(entry.name.as_str().to_string())
        })
        .ok()?;
    Some(names)
}

fn held_in(store: Store) -> &'static str {
    match store {
        Store::Table => "table",
        Store::Rides => "rides",
        Store::Files | Store::Records => "files",
        Store::Traces => "traces",
        Store::Internal => "internal",
        Store::Settings => "settings",
    }
}

fn wiped_for(store: Store) -> &'static [&'static str] {
    match store {
        Store::Table => &["table", "terrain", "rides", "files", "traces"],
        Store::Rides => &["rides"],
        Store::Files | Store::Records => &["files"],
        Store::Traces => &["traces"],
        Store::Internal => &["internal"],
        Store::Settings => &["settings"],
    }
}

#[track_caller]
fn assert_kept(before: &Stored, after: &Stored, stores: &[Store], what: &str) {
    let wiped: Vec<&str> = stores
        .iter()
        .flat_map(|&store| wiped_for(store))
        .copied()
        .collect();
    for (name, held) in &before.digests {
        // The device's own trace keeps appending to STORE; its files are compared below.
        if !wiped.contains(name) && *name != "traces" {
            assert_eq!(
                after.digests.get(name),
                Some(held),
                "{what}: {name} changed"
            );
        }
    }
    if !wiped.contains(&"traces") {
        for (name, bytes) in &before.traces {
            let kept = after.traces.get(name).is_some_and(|a| a.starts_with(bytes));
            assert!(kept, "{what}: trace {name} changed");
        }
    }
}

fn wait_for_offer(sim: &mut Sim) {
    sim.wait_for("FORMAT STORAGE?", seconds(20))
        .expect("the offer");
    sim.advance(2 * HOLD_GAP_MS);
}

fn wait_for_restart(sim: &mut Sim, boot: u32) {
    for _ in 0..seconds(60) / 100 {
        if sim.device.boot != boot {
            sim.wait_usable();
            return;
        }
        sim.advance(100);
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
    wait_for_restart(&mut sim, boot);
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
    let mut sim = Sim::named(name);
    sim.discard_recording();
    sim.fit_bulk(Presence::Ready);
    sim.format_rides();
    sim.format_files();
    sim.format_store();
    sim.install_terrain(b"CHILE-TERRAIN-V1", &[7; 600]);
    sim.put_file("workouts/a.vw", b"host file");
    let table = sim.init_volumes();
    let mut sim = sim.boot();
    sim.start_ride();
    sim.ride_at(30.0, 200.0, 140.0, 90.0);
    sim.advance(seconds(30));
    sim.stop_ride();
    sim.done();
    sim.make_foreign(stores);
    sim.cut_power();
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
        wait_for_restart(&mut sim, boot);
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

fn wipe_cut_short(stores: &[Store], flash: bool, cut: u32) -> bool {
    let what = format!("{stores:?} cut {cut} flash {flash}");
    let (mut sim, table) = used_device_with_foreign("format-cut", stores);
    let before = stored(&mut sim, &table);
    if flash {
        sim.fail_storage(cut, 2);
    } else {
        sim.fail_bulk(cut, 100);
    }
    let boot = sim.device.boot;
    sim.press(ACT);
    sim.advance(seconds(10));
    let cut_short = sim.device.boot == boot;
    if cut_short {
        sim.cut_power();
        if sim.wait_for("FORMAT STORAGE?", seconds(20)).is_ok() {
            sim.advance(2 * HOLD_GAP_MS);
            let boot = sim.device.boot;
            sim.press(ACT);
            wait_for_restart(&mut sim, boot);
        }
    }
    assert_kept(&before, &stored(&mut sim, &table), stores, &what);
    let health = health(&mut sim);
    assert_eq!(field(&health, "state"), "idle", "{what}: {health}");
    for store in Store::ALL {
        let held = field(&health, store.name());
        let wiped = stores
            .iter()
            .any(|&foreign| wiped_for(foreign).contains(&held_in(store)));
        if wiped {
            assert!(matches!(held, "current" | "blank"), "{what}: {health}");
        } else {
            assert_eq!(held, "current", "{what}: {health}");
        }
    }
    let (_, bulk) = sim.command("RIDE BULK STATUS");
    let rides_wiped = stores
        .iter()
        .any(|&foreign| wiped_for(foreign).contains(&"rides"));
    let rides_log_cut_corrupt_427 = cut_short && rides_wiped && field(&bulk, "state") == "corrupt";
    assert!(
        matches!(field(&bulk, "state"), "ready" | "needs_init") || rides_log_cut_corrupt_427,
        "{what}: {bulk}"
    );
    if stores.contains(&Store::Table) {
        let (_, terrain) = sim.command("TERRAIN STATUS");
        assert_eq!(field(&terrain, "state"), "empty", "{what}");
    }
    cut_short
}

#[test]
fn a_wipe_cut_at_any_write_keeps_the_other_stores_and_leaves_none_foreign_or_unusable() {
    let scopes = Store::ALL.map(|store| vec![store]).into_iter();
    for stores in scopes.chain([Store::ALL.to_vec()]) {
        for flash in [false, true] {
            let mut cut = 0;
            while wipe_cut_short(&stores, flash, cut) {
                cut += 1;
            }
        }
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
