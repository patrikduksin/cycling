mod wipes;

use shell_simulator::sim::{Sim, seconds};
use vana::app::{HOLD_GAP_MS, Store};
use wipes::*;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Start {
    ActOnOffer,
    FormatConfirm,
}

impl Start {
    #[track_caller]
    fn wipe(self, sim: &mut Sim) {
        match self {
            Start::ActOnOffer => sim.press(ACT),
            Start::FormatConfirm => assert_eq!(sim.command("FORMAT CONFIRM").0, "ACCEPTED"),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum CutAt {
    Flash,
    Card,
}

const INTENT_WRITES: u32 = 3;

/// The trace goes on writing the card after a wipe of the boot flash alone.
fn cut_during_the_wipe(sim: &Sim, boot: u32) -> bool {
    sim.device.boot == boot && sim.device.sdk.owns_storage()
}

fn a_wipe_cut_at_every_write(foreign: &[Store], start: Start, at: CutAt) {
    let name = format!("format-cut-{start:?}");
    let (mut sim, table) = used_device(&name, foreign);
    let before = stored(&mut sim, &table);
    let media = sim.media();
    let wiped = match start {
        Start::ActOnOffer => foreign,
        Start::FormatConfirm => &Store::ALL,
    };
    let ready = |sim: &mut Sim| {
        if foreign.is_empty() {
            sim.wait_usable();
        } else {
            wait_for_offer(sim);
        }
    };
    let mut resumed = 0;
    let mut cuts = 0;
    for cut in 0.. {
        let what = format!("{foreign:?} {start:?} cut {cut} at {at:?}");
        sim.restore_media(&media);
        ready(&mut sim);
        match at {
            CutAt::Flash => sim.lose_power_at_flash_write(cut, 2),
            CutAt::Card => sim.lose_power_at_card_write(cut, 100),
        }
        let boot = sim.device.boot;
        start.wipe(&mut sim);
        sim.advance(seconds(10));
        let cut_short = sim.power_lost();
        if cut_short {
            cuts += u32::from(cut_during_the_wipe(&sim, boot));
            sim.cut_power();
            match boots_after_a_cut(&mut sim, &what) {
                AfterCut::Usable { resumed: wiped } => resumed += u32::from(wiped),
                AfterCut::Offered => {
                    assert_kept(&before, &stored(&mut sim, &table), &[], &what);
                    let boot = sim.device.boot;
                    sim.advance(2 * HOLD_GAP_MS);
                    start.wipe(&mut sim);
                    wait_for_the_wipe(&mut sim, boot);
                }
            }
        } else {
            wait_for_the_wipe(&mut sim, boot);
        }
        assert_kept(&before, &stored(&mut sim, &table), wiped, &what);
        let health = health(&mut sim);
        assert_eq!(field(&health, "state"), "idle", "{what}: {health}");
        for store in Store::ALL {
            let held = field(&health, store.name());
            let wiped = wiped
                .iter()
                .any(|&wiped| wiped_for(wiped).contains(&held_in(store)));
            if wiped {
                assert!(matches!(held, "current" | "blank"), "{what}: {health}");
            } else {
                assert_eq!(held, "current", "{what}: {health}");
            }
        }
        let (_, bulk) = sim.command("RIDE BULK STATUS");
        assert!(
            matches!(field(&bulk, "state"), "ready" | "needs_init"),
            "{what}: {bulk}"
        );
        assert_intent_settled(&mut sim, &what);
        if foreign.contains(&Store::Table) {
            let (_, terrain) = sim.command("TERRAIN STATUS");
            assert_eq!(field(&terrain, "state"), "empty", "{what}");
        }
        if !cut_short {
            break;
        }
    }
    // Only the cuts in the intent's own three writes find nothing to finish.
    assert!(
        cuts <= INTENT_WRITES || resumed > 0,
        "{foreign:?} {start:?} at {at:?}: no boot resumed"
    );
}

fn a_wipe_of_one_store_cut_at_any_write_finishes_at_the_next_boot(stores: &[Store]) {
    for &store in stores {
        for at in [CutAt::Card, CutAt::Flash] {
            a_wipe_cut_at_every_write(&[store], Start::ActOnOffer, at);
        }
    }
}

#[test]
fn a_wipe_of_the_table_or_the_ride_log_cut_at_any_write_finishes_at_the_next_boot() {
    a_wipe_of_one_store_cut_at_any_write_finishes_at_the_next_boot(&[Store::Table, Store::Rides]);
}

#[test]
fn a_wipe_of_files_records_or_traces_cut_at_any_write_finishes_at_the_next_boot() {
    a_wipe_of_one_store_cut_at_any_write_finishes_at_the_next_boot(&[
        Store::Files,
        Store::Records,
        Store::Traces,
    ]);
}

#[test]
fn a_wipe_of_the_internal_journal_cut_at_any_write_finishes_at_the_next_boot() {
    a_wipe_of_one_store_cut_at_any_write_finishes_at_the_next_boot(&[Store::Internal]);
}

#[test]
fn a_wipe_of_the_settings_cut_at_any_write_finishes_at_the_next_boot() {
    a_wipe_of_one_store_cut_at_any_write_finishes_at_the_next_boot(&[Store::Settings]);
}

#[test]
fn a_wipe_of_every_store_cut_at_any_write_finishes_at_the_next_boot() {
    for at in [CutAt::Card, CutAt::Flash] {
        a_wipe_cut_at_every_write(&Store::ALL, Start::ActOnOffer, at);
    }
}

#[test]
fn format_confirm_cut_at_any_write_finishes_at_the_next_boot() {
    for foreign in [&[][..], &[Store::Settings]] {
        for at in [CutAt::Card, CutAt::Flash] {
            a_wipe_cut_at_every_write(foreign, Start::FormatConfirm, at);
        }
    }
}

fn a_clear_cut_at_every_write(shard: u32) -> u32 {
    let (mut sim, slots) = device_with_a_full_internal_journal("clear-cut");
    let media = sim.media();
    let card: Vec<_> = sim.bulk_rides().into_iter().map(|r| r.entries).collect();
    let mut resumed = 0;
    for cut in (shard..).step_by(SHARDS as usize) {
        let what = format!("cut {cut}");
        sim.restore_media(&media);
        sim.wait_usable();
        sim.lose_power_at_flash_write(cut, 128);
        let (reply, out) = sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
        assert_eq!(reply, "ACCEPTED", "{what}: {out}");
        sim.advance(seconds(10));
        if !sim.power_lost() {
            sim.clear_storage_fault();
            let (_, status) = sim.command("RIDE INTERNAL STATUS");
            assert_eq!(field(&status, "slot"), "0", "{what}: {status}");
            break;
        }
        sim.cut_power();
        resumed += u32::from(boots_after_a_cut(&mut sim, &what).resumed());
        let health = health(&mut sim);
        assert!(
            matches!(field(&health, "internal"), "current" | "blank"),
            "{what}: {health}"
        );
        let (_, status) = sim.command("RIDE INTERNAL STATUS");
        let left = field(&status, "slot");
        let intent_whole = cut >= INTENT_WRITES;
        assert!(
            left == "0" || (!intent_whole && left == slots),
            "{what}: {status}"
        );
        let kept: Vec<_> = sim.bulk_rides().into_iter().map(|r| r.entries).collect();
        assert!(kept == card, "{what}: the card's rides are kept");
        assert_intent_settled(&mut sim, &what);
    }
    resumed
}

const SHARDS: u32 = 2;

#[test]
fn a_clear_cut_at_any_even_write_resumes_at_boot_and_never_reads_foreign() {
    let resumed = a_clear_cut_at_every_write(0);
    assert!(resumed > 250 / SHARDS, "{resumed} boots resumed the Clear");
}

#[test]
fn a_clear_cut_at_any_odd_write_resumes_at_boot_and_never_reads_foreign() {
    let resumed = a_clear_cut_at_every_write(1);
    assert!(resumed > 250 / SHARDS, "{resumed} boots resumed the Clear");
}

#[test]
fn a_clear_cut_again_while_it_resumes_still_finishes() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("clear-cut-twice");
    let media = sim.media();
    let mut resumed = 0;
    'first: for first in INTENT_WRITES.. {
        for second in 0.. {
            let what = format!("cut {first} then {second}");
            sim.restore_media(&media);
            sim.wait_usable();
            sim.lose_power_at_flash_write(first, 128);
            sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
            sim.advance(seconds(10));
            if !sim.power_lost() {
                break 'first;
            }
            sim.cut_power();
            sim.lose_power_at_flash_write(second, 128);
            sim.advance(seconds(10));
            let cut_again = sim.power_lost();
            if cut_again {
                sim.cut_power();
            }
            resumed += u32::from(boots_after_a_cut(&mut sim, &what).resumed());
            assert_eq!(internal_slots(&mut sim), "0", "{what}");
            assert_intent_settled(&mut sim, &what);
            if !cut_again {
                break;
            }
        }
    }
    assert!(resumed >= 6, "{resumed} boots resumed");
}

/// Stages never run behind a commit a later boot can discard, so no boot
/// leaves the journal half wiped.
#[test]
fn a_clear_whose_intent_commits_are_cut_twice_never_drops_a_half_run_wipe() {
    const IN_THE_COMMIT_WORD: (u32, usize) = (2, 2);
    const HALF_A_SECTOR: usize = 2048;
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("clear-commit-cuts");
    let media = sim.media();
    let (_, journal) = flash(&mut sim);
    let (mut wiped, mut kept) = (0, 0);
    for run in 0..24 {
        for third in 0..4 {
            let what = format!("run {run} cuts 2, 2, {third}");
            sim.restore_media(&media);
            sim.wait_usable();
            let cuts = [
                IN_THE_COMMIT_WORD,
                IN_THE_COMMIT_WORD,
                (third, HALF_A_SECTOR),
            ];
            for (boot, (cut, partial)) in cuts.into_iter().enumerate() {
                sim.lose_power_at_flash_write(cut, partial);
                if boot == 0 {
                    sim.command(&format!("RIDE CLEAR CONFIRM {slots}"));
                }
                sim.advance(seconds(10));
                if !sim.power_lost() {
                    sim.clear_storage_fault();
                    break;
                }
                sim.cut_power();
            }
            boots_after_a_cut(&mut sim, &what);
            let (_, left) = flash(&mut sim);
            if left == journal {
                kept += 1;
            } else {
                assert!(left.iter().all(|byte| *byte == 0xff), "{what}: half wiped");
                assert_eq!(internal_slots(&mut sim), "0", "{what}");
                wiped += 1;
            }
            assert_intent_settled(&mut sim, &what);
        }
    }
    assert!(wiped > 0 && kept > 0, "{wiped} wiped, {kept} kept");
}

#[test]
fn format_confirm_cut_on_the_card_finishes_the_boot_flash_stores_with_the_card_out() {
    use shell_simulator::bulk::Presence;
    let (mut sim, _) = used_device("format-card-out", &[]);
    sim.wait_usable();
    sim.lose_power_at_card_write(0, 100);
    assert_eq!(sim.command("FORMAT CONFIRM").0, "ACCEPTED");
    sim.advance(seconds(10));
    assert!(sim.power_lost(), "the wipe was cut on the card");
    sim.fit_bulk(Presence::Missing);
    sim.cut_power();
    let boot = sim.device.boot;
    assert_eq!(wait_for_the_wipe(&mut sim, boot), WipeEnded::Restarted);
    assert_intent_settled(&mut sim, "card out");
    let (_, journal) = flash(&mut sim);
    assert!(
        journal.iter().all(|byte| *byte == 0xff),
        "the internal journal went"
    );
    let health = health(&mut sim);
    assert_eq!(field(&health, "state"), "idle", "{health}");
    for store in ["internal", "settings"] {
        assert!(
            matches!(field(&health, store), "blank" | "current"),
            "{health}"
        );
    }
}

/// A Clear leaves STORE alone, and a resumed boot must not leave a trace
/// without its header.
#[test]
fn a_live_or_resumed_clear_keeps_every_trace_decodable_and_one_running() {
    let (mut sim, slots) = device_with_a_ride_on_internal_flash("clear-traces");
    let media = sim.media();
    for resumed in [false, true] {
        let what = if resumed { "resumed" } else { "live" };
        sim.restore_media(&media);
        sim.wait_usable();
        sim.advance(seconds(5));
        let before = sim.device.trace_file();
        assert!(before.is_some(), "{what}: a trace runs before the Clear");
        if resumed {
            sim.lose_power_at_flash_write(INTENT_WRITES + 1, 2048);
        }
        let boot = sim.device.boot;
        assert_eq!(
            sim.command(&format!("RIDE CLEAR CONFIRM {slots}")).0,
            "ACCEPTED"
        );
        if resumed {
            sim.advance(seconds(10));
            assert!(sim.power_lost(), "{what}: cut mid-wipe");
            sim.cut_power();
            assert!(boots_after_a_cut(&mut sim, what).resumed(), "{what}");
        } else {
            assert_eq!(wait_for_the_wipe(&mut sim, boot), WipeEnded::JournalEmptied);
            assert_eq!(sim.device.trace_file(), before, "{what}: the same file");
        }
        assert_eq!(internal_slots(&mut sim), "0", "{what}");
        sim.advance(seconds(5));
        assert!(
            sim.device.sync_trace(),
            "{what}: a trace runs after the wipe"
        );
        let traces = store_traces(&mut sim);
        assert!(traces.len() >= 2, "{what}: {} traces", traces.len());
        for (name, bytes) in traces {
            let decoded = firmware_services::trace::decode(&bytes).err();
            assert_eq!(decoded, None, "{what}: {name} of {} bytes", bytes.len());
        }
    }
}
