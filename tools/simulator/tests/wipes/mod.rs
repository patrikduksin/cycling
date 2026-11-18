#![allow(dead_code)]

use device_api::input::Role;
use device_api::storage::Region;
use firmware_services::volume::{self, Table};
use shell_simulator::bulk::Presence;
use shell_simulator::sim::{Sim, seconds};
use std::collections::BTreeMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use vana::app::{HOLD_GAP_MS, INTENT_IDLE_RECORD, Store};
use vana::screens::workout::Page;

pub const ACT: Role = Role::Act;
pub const BACK: Role = Role::Back;
pub const MOVE: Role = Role::Move;

pub fn field<'a>(status: &'a str, name: &str) -> &'a str {
    status
        .split_whitespace()
        .find_map(|f| f.strip_prefix(name).and_then(|f| f.strip_prefix('=')))
        .unwrap_or_else(|| panic!("{name} in {status}"))
}

pub fn flash(sim: &mut Sim) -> (Vec<u8>, Vec<u8>) {
    (
        sim.device.media.contents(Region::Configuration),
        sim.device.media.contents(Region::Data),
    )
}

pub struct Stored {
    digests: BTreeMap<&'static str, u64>,
    traces: BTreeMap<String, Vec<u8>>,
}

pub fn stored(sim: &mut Sim, table: &Table) -> Stored {
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

pub fn store_traces(sim: &mut Sim) -> Vec<(String, Vec<u8>)> {
    let names = store_names(sim).expect("STORE mounts");
    names
        .into_iter()
        .map(|name| {
            let bytes = sim.store_file(&name).expect("a trace reads whole");
            (name, bytes)
        })
        .collect()
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

pub fn held_in(store: Store) -> &'static str {
    match store {
        Store::Table => "table",
        Store::Rides => "rides",
        Store::Files | Store::Records => "files",
        Store::Traces => "traces",
        Store::Internal => "internal",
        Store::Settings => "settings",
    }
}

pub fn wiped_for(store: Store) -> &'static [&'static str] {
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
pub fn assert_kept(before: &Stored, after: &Stored, stores: &[Store], what: &str) {
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

pub fn wait_for_offer(sim: &mut Sim) {
    sim.wait_for("FORMAT STORAGE?", seconds(20))
        .expect("the offer");
    sim.advance(2 * HOLD_GAP_MS);
}

pub fn wait_for_restart(sim: &mut Sim, boot: u32) {
    for _ in 0..seconds(60) / 100 {
        if sim.device.boot != boot {
            sim.wait_usable();
            return;
        }
        sim.advance(100);
    }
    panic!("no restart; screen:\n{}", sim.screen().text());
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum WipeEnded {
    Restarted,
    JournalEmptied,
}

pub fn wait_for_the_wipe(sim: &mut Sim, boot: u32) -> WipeEnded {
    for _ in 0..seconds(60) / 100 {
        if sim.device.boot != boot {
            sim.wait_usable();
            return WipeEnded::Restarted;
        }
        let page = sim.device.sdk.page();
        if matches!(page, Page::Home | Page::Setup) && field(&health(sim), "state") != "running" {
            return WipeEnded::JournalEmptied;
        }
        sim.advance(100);
    }
    panic!("the wipe never ended; screen:\n{}", sim.screen().text());
}

pub fn health(sim: &mut Sim) -> String {
    let (status, health) = sim.command("FORMAT");
    assert_eq!(status, "OK");
    health
}

pub fn used_device(name: &str, stores: &[Store]) -> (Sim, Table) {
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
    (sim, table)
}

pub fn device_with_a_ride_on_internal_flash(name: &str) -> (Sim, String) {
    let mut sim = Sim::named(name);
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
    sim.start_ride();
    sim.advance(seconds(10));
    sim.fail_bulk(0, 0);
    sim.advance(seconds(20));
    sim.stop_ride();
    sim.done();
    sim.cut_power();
    sim.wait_usable();
    let (_, status) = sim.command("RIDE INTERNAL STATUS");
    let slots = field(&status, "slot").to_string();
    (sim, slots)
}

pub fn device_with_a_full_internal_journal(name: &str) -> (Sim, String) {
    use device_api::storage::OwnedFlash;
    let (mut sim, slots) = device_with_a_ride_on_internal_flash(name);
    let used = slots.parse::<usize>().unwrap() * vana::ride::log::SLOT_SIZE;
    let rest = shell_simulator::storage::DATA_BYTES - used;
    sim.device
        .media
        .untapped()
        .program(Region::Data, used, &vec![0; rest])
        .unwrap();
    sim.cut_power();
    sim.wait_usable();
    let slots = internal_slots(&mut sim);
    assert_eq!(
        slots,
        (shell_simulator::storage::DATA_BYTES / vana::ride::log::SLOT_SIZE).to_string(),
        "every sector holds data"
    );
    (sim, slots)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AfterCut {
    Offered,
    /// `resumed`: the boot finished a pending wipe.
    Usable {
        resumed: bool,
    },
}

impl AfterCut {
    #[track_caller]
    pub fn resumed(self) -> bool {
        match self {
            AfterCut::Usable { resumed } => resumed,
            AfterCut::Offered => panic!("no offer"),
        }
    }
}

#[track_caller]
pub fn boots_after_a_cut(sim: &mut Sim, what: &str) -> AfterCut {
    let due = intent_pending(sim) != 0;
    for _ in 0..seconds(60) / 100 {
        if sim.screen().contains("FORMAT STORAGE?") {
            assert!(!due, "{what}: the offer appeared");
            sim.advance(2 * HOLD_GAP_MS);
            return AfterCut::Offered;
        }
        let page = sim.device.sdk.page();
        if matches!(page, Page::Home | Page::Setup) && field(&health(sim), "state") == "idle" {
            return AfterCut::Usable { resumed: due };
        }
        sim.advance(100);
    }
    panic!("{what}: never usable; screen:\n{}", sim.screen().text());
}

#[track_caller]
pub fn assert_intent_settled(sim: &mut Sim, what: &str) {
    use firmware_services::storage::{Held, Journal};
    let mut journal = Journal::new(sim.device.media.untapped(), Region::Intent);
    for sector in 0..2 {
        let mut payload = [0; 2];
        let held = journal.held(sector, &mut payload);
        assert!(
            held == Ok(Held::Blank)
                || (matches!(held, Ok(Held::Committed(_))) && payload == INTENT_IDLE_RECORD),
            "{what}: intent sector {sector} {held:?} {payload:?}"
        );
    }
}

pub fn intent_pending(sim: &mut Sim) -> u8 {
    use firmware_services::storage::{Held, Journal};
    let (_, image) = sim.device.media.image().expect("memory media");
    let flash = shell_simulator::storage::FileFlash::from_image(0, image);
    let mut journal = Journal::new(flash, Region::Intent);
    let mut pending = 0;
    for sector in 0..2 {
        let mut payload = [0; 2];
        if matches!(journal.held(sector, &mut payload), Ok(Held::Committed(_)))
            && payload[0] == INTENT_IDLE_RECORD[0]
        {
            pending |= payload[1];
        }
    }
    pending
}

pub fn internal_slots(sim: &mut Sim) -> String {
    let (_, status) = sim.command("RIDE INTERNAL STATUS");
    field(&status, "slot").to_string()
}
