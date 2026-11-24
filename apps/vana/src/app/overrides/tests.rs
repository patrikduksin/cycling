use super::*;
use crate::compose::bench::MAIN_BYTES;
use crate::compose::builtin::ScreenId;
use crate::compose::data::Metric;
use crate::compose::parse::VERSION;
use crate::screens::C606;
use crate::testing::{Cut, JournalFlash, Rng, main_with};
use std::boxed::Box;
use std::vec::Vec;

const MAIN: u8 = ScreenId::RideMain.key();

fn flash() -> JournalFlash {
    JournalFlash::of(Region::Screens)
}

fn boot(flash: &mut JournalFlash) -> Box<Overrides> {
    let mut overrides = Box::new(Overrides::new());
    overrides.serve(&mut *flash, &C606);
    overrides
}

fn change(overrides: &mut Overrides, flash: &mut JournalFlash, change: Change, base: u32) -> Saved {
    overrides.propose(change, &C606).unwrap();
    overrides.save(base);
    overrides.serve(&mut *flash, &C606);
    overrides
        .saved()
        .expect("a save answers in the pass it was asked")
}

fn main_shown(overrides: &Overrides) -> Option<Vec<u8>> {
    let screen = overrides.shown().screen(MAIN)?;
    Some(
        screen
            .tree()
            .nodes()
            .iter()
            .flat_map(|node| node.bytes())
            .collect(),
    )
}

#[test]
fn a_saved_screen_shows_after_a_reboot_and_a_reset_brings_back_the_builtin() {
    let mut flash = flash();
    let mut overrides = boot(&mut flash);
    assert_eq!(overrides.revision(), Some(0));
    assert_eq!(main_shown(&overrides), None);

    let power = main_with(Metric::Power);
    let set = Change::Set {
        key: MAIN,
        nodes: &power,
    };
    let saved = change(&mut overrides, &mut flash, set, 0);
    assert_eq!(saved, Saved::Revision(1));
    assert_eq!(main_shown(&overrides).as_deref(), Some(&power[..]));

    let mut rebooted = boot(&mut flash);
    assert_eq!(rebooted.revision(), Some(1));
    assert!(!rebooted.reset());
    assert_eq!(main_shown(&rebooted).as_deref(), Some(&power[..]));

    let saved = change(&mut rebooted, &mut flash, Change::Reset(MAIN), 1);
    assert_eq!(saved, Saved::Revision(2));
    assert_eq!(main_shown(&boot(&mut flash)), None);
}

#[test]
fn a_save_against_an_older_revision_is_refused_and_changes_nothing() {
    let mut flash = flash();
    let mut overrides = boot(&mut flash);
    let power = main_with(Metric::Power);
    let set = Change::Set {
        key: MAIN,
        nodes: &power,
    };
    assert_eq!(
        change(&mut overrides, &mut flash, set, 0),
        Saved::Revision(1)
    );
    let writes = flash.mutations;

    let heart = main_with(Metric::Heart);
    let stale = Change::Set {
        key: MAIN,
        nodes: &heart,
    };
    assert_eq!(
        change(&mut overrides, &mut flash, stale, 0),
        Saved::Stale(1)
    );
    assert_eq!(flash.mutations, writes, "a stale save writes nothing");
    assert_eq!(main_shown(&overrides).as_deref(), Some(&power[..]));
    assert_eq!(main_shown(&boot(&mut flash)).as_deref(), Some(&power[..]));
}

#[test]
fn a_record_of_another_catalogue_version_shows_builtins_until_the_next_save() {
    let mut flash = flash();
    let mut older = std::vec![VERSION + 1, 1, MAIN, (MAIN_BYTES.len() / 4) as u8];
    older.extend_from_slice(&MAIN_BYTES);
    older.push(0);
    Journal::new(&mut flash, Region::Screens)
        .save(&older)
        .unwrap();

    let mut overrides = boot(&mut flash);
    assert!(overrides.reset());
    assert_eq!(overrides.revision(), Some(1));
    assert_eq!(main_shown(&overrides), None);
    assert!(boot(&mut flash).reset(), "a read alone never overwrites it");

    let power = main_with(Metric::Power);
    let set = Change::Set {
        key: MAIN,
        nodes: &power,
    };
    assert_eq!(
        change(&mut overrides, &mut flash, set, 1),
        Saved::Revision(2)
    );
    let rebooted = boot(&mut flash);
    assert!(!rebooted.reset());
    assert_eq!(main_shown(&rebooted).as_deref(), Some(&power[..]));
}

#[test]
fn bytes_of_no_journal_reset_the_screens_and_the_next_save_replaces_them() {
    let mut rng = Rng::new(7);
    let mut flash = flash();
    for byte in flash.bytes.iter_mut() {
        *byte = rng.byte();
    }
    let mut overrides = boot(&mut flash);
    assert!(overrides.reset());
    assert_eq!(overrides.revision(), Some(0));

    let power = main_with(Metric::Power);
    let set = Change::Set {
        key: MAIN,
        nodes: &power,
    };
    assert!(matches!(
        change(&mut overrides, &mut flash, set, 0),
        Saved::Revision(_)
    ));
    assert_eq!(main_shown(&boot(&mut flash)).as_deref(), Some(&power[..]));
}

#[test]
fn a_refused_screen_leaves_the_proposal_empty_and_nothing_to_save() {
    let mut flash = flash();
    let mut overrides = boot(&mut flash);
    let mut unknown = MAIN_BYTES.to_vec();
    unknown[4] = 99;
    let refused = overrides.propose(
        Change::Set {
            key: MAIN,
            nodes: &unknown,
        },
        &C606,
    );
    assert_eq!(
        std::format!("{}", refused.unwrap_err()),
        "screen 0: node 1 is of unknown kind 99"
    );
    overrides.save(0);
    overrides.serve(&mut flash, &C606);
    assert_eq!(overrides.saved(), Some(Saved::Failed));
    assert_eq!(flash.mutations, 0);
}

/// Save `after` over `before`, cut at its mutation `cut`, then boot a few
/// times: every boot shows one of the two, and a save the device answered
/// stays saved.
fn cut_save(seed: u64, cut: Option<u64>, writes: &mut u64) {
    let what = std::format!("seed {seed} cut {cut:?}");
    let mut rng = Rng::new(seed);
    let (before, after) = (main_with(Metric::Power), main_with(Metric::Heart));
    let mut flash = flash();
    let set = |nodes| Change::Set { key: MAIN, nodes };
    assert_eq!(
        change(&mut boot(&mut flash), &mut flash, set(&before), 0),
        Saved::Revision(1)
    );
    let start = flash.mutations;
    flash.cut = cut.map(|cut| Cut {
        at: start + cut,
        seed: rng.next(),
    });
    let mut overrides = boot(&mut flash);
    let saved = change(&mut overrides, &mut flash, set(&after), 1);
    *writes = flash.mutations - start;
    for _ in 0..4 {
        flash.power_on(rng.next());
        let booted = boot(&mut flash);
        let shown = main_shown(&booted);
        assert!(
            shown.as_deref() == Some(&before[..]) || shown.as_deref() == Some(&after[..]),
            "{what}: a boot shows neither screen: {shown:?}"
        );
        if let Saved::Revision(revision) = saved {
            assert_eq!(
                shown.as_deref(),
                Some(&after[..]),
                "{what}: an answered save was lost"
            );
            assert_eq!(booted.revision(), Some(revision), "{what}");
        }
    }
    let mut overrides = boot(&mut flash);
    let revision = overrides.revision().expect("the journal reads after a cut");
    let reset = Change::Reset(MAIN);
    assert!(
        matches!(
            change(&mut overrides, &mut flash, reset, revision),
            Saved::Revision(_)
        ),
        "{what}: the journal takes the next save"
    );
    assert_eq!(main_shown(&boot(&mut flash)), None, "{what}");
}

#[test]
fn a_cut_at_any_write_of_a_save_boots_to_the_old_screens_or_the_new() {
    let mut writes = 0;
    cut_save(0, None, &mut writes);
    assert!(writes >= 3, "a save erases, programs and commits: {writes}");
    for seed in 0..32 {
        for cut in 0..writes {
            cut_save(seed, Some(cut), &mut 0);
        }
    }
}

#[test]
fn an_unreadable_journal_shows_builtins_and_a_save_made_over_them_is_never_written() {
    let mut flash = flash();
    let power = main_with(Metric::Power);
    let heart = main_with(Metric::Heart);
    let set = |nodes| Change::Set { key: MAIN, nodes };
    assert_eq!(
        change(&mut boot(&mut flash), &mut flash, set(&power), 0),
        Saved::Revision(1)
    );
    flash.off = true;
    let mut overrides = boot(&mut flash);
    assert_eq!(overrides.revision(), None);
    assert_eq!(main_shown(&overrides), None);
    assert_eq!(
        change(&mut overrides, &mut flash, Change::Reset(MAIN), 1),
        Saved::Failed
    );

    flash.power_on(0);
    let writes = flash.mutations;
    assert_eq!(
        change(&mut overrides, &mut flash, set(&heart), 1),
        Saved::Stale(1),
        "made against screens the journal had not given, the phone reads them again"
    );
    assert_eq!(flash.mutations, writes, "and nothing is written");
    assert_eq!(main_shown(&overrides).as_deref(), Some(&power[..]));
    assert_eq!(main_shown(&boot(&mut flash)).as_deref(), Some(&power[..]));

    assert_eq!(
        change(&mut overrides, &mut flash, set(&heart), 1),
        Saved::Revision(2)
    );
    let rebooted = boot(&mut flash);
    assert_eq!(rebooted.revision(), Some(2));
    assert_eq!(main_shown(&rebooted).as_deref(), Some(&heart[..]));
}

/// Save `torn` over `older` with its commit word cut, boot on reads drawn
/// from `read_seed`, then save `third` cut at its mutation `cut`. The torn
/// commit word may read either way on every boot until a save programs it
/// again, so a boot may show either of the first two; never neither.
fn torn_then_cut(tear_seed: u64, read_seed: u64, cut: Option<u64>) {
    let what = std::format!("tear {tear_seed} read {read_seed} cut {cut:?}");
    let (older, torn, third) = (
        main_with(Metric::Power),
        main_with(Metric::Heart),
        main_with(Metric::Cadence),
    );
    let set = |nodes| Change::Set { key: MAIN, nodes };
    let mut flash = flash();
    let mut first = boot(&mut flash);
    assert_eq!(
        change(&mut first, &mut flash, set(&older), 0),
        Saved::Revision(1)
    );
    flash.cut = Some(Cut {
        at: flash.mutations + 2,
        seed: tear_seed,
    });
    assert_eq!(change(&mut first, &mut flash, set(&torn), 1), Saved::Failed);
    flash.power_on(read_seed);
    let mut second = boot(&mut flash);
    let revision = second.revision().expect("{what}: the journal reads");
    flash.cut = cut.map(|at| Cut {
        at: flash.mutations + at,
        seed: tear_seed ^ read_seed,
    });
    let saved = change(&mut second, &mut flash, set(&third), revision);
    if let Saved::Revision(next) = saved {
        assert!(
            newer(next, revision),
            "{what}: revision {next} after {revision}"
        );
    }
    for boot_seed in 0..4 {
        flash.power_on(boot_seed);
        let booted = boot(&mut flash);
        let now = main_shown(&booted);
        match saved {
            Saved::Revision(next) => {
                assert_eq!(
                    now.as_deref(),
                    Some(&third[..]),
                    "{what}: an answered save was lost"
                );
                assert_eq!(booted.revision(), Some(next), "{what}");
            }
            _ => assert!(
                [&older, &torn, &third]
                    .iter()
                    .any(|saved| now.as_deref() == Some(&saved[..])),
                "{what}: a boot shows none of the saves: {now:?}"
            ),
        }
    }
}

fn newer(candidate: u32, current: u32) -> bool {
    candidate != current && candidate.wrapping_sub(current) < (1 << 31)
}

#[test]
fn a_save_after_a_torn_commit_never_reuses_a_revision_and_a_second_cut_keeps_one_record() {
    for tear_seed in 0..48 {
        for read_seed in 0..16 {
            for cut in [None, Some(0), Some(1), Some(2), Some(3), Some(4)] {
                torn_then_cut(tear_seed, read_seed, cut);
            }
        }
    }
}

#[test]
fn a_change_whose_record_would_not_fit_is_refused_with_its_length() {
    use crate::compose::node::{MAX_NODES, Node, Size};
    use crate::compose::parse::RECORD;
    use crate::compose::registry::SPACE;
    use crate::compose::solve::MAX_LEAVES;
    // The most nodes a tree holds and the most tiles that show, small
    // enough to fit between any screen's corner cues.
    let space = Node::leaf(SPACE, 0, Size::units(1));
    let mut nodes = std::vec![Node::col(10, Size::grow(1))];
    for _ in 0..7 {
        nodes.extend([Node::row(3, Size::CONTENT), space, space, space]);
    }
    nodes.extend([space; 3]);
    assert_eq!(nodes.len(), MAX_NODES);
    assert_eq!(
        nodes.iter().filter(|node| **node == space).count(),
        MAX_LEAVES
    );
    let mut record = std::vec![VERSION, 31];
    for key in (0..32).filter(|key| *key != MAIN) {
        record.extend([key, nodes.len() as u8]);
        record.extend(nodes.iter().flat_map(|node| node.bytes()));
    }
    record.push(0);
    let mut flash = flash();
    Journal::new(&mut flash, Region::Screens)
        .save(&record)
        .unwrap();
    let mut overrides = boot(&mut flash);
    assert_eq!(overrides.shown().screens().len(), 31);

    let refused = overrides.propose(
        Change::Set {
            key: MAIN,
            nodes: &MAIN_BYTES,
        },
        &C606,
    );
    let long = record.len() + 2 + MAIN_BYTES.len();
    assert!(long > RECORD);
    assert_eq!(
        refused.map_err(|reject| std::format!("{reject}")),
        Err(std::format!("{long} bytes is longer than a record"))
    );
    overrides.save(1);
    overrides.serve(&mut flash, &C606);
    assert_eq!(overrides.saved(), Some(Saved::Failed));
}

#[test]
fn a_record_from_the_old_catalogue_wipes_to_the_builtins() {
    use crate::compose::node::{Node, Size};
    use crate::compose::registry::VALUE;

    let mut flash = flash();
    // The previous catalogue encoded a speed reading as value tile 1, arg 0.
    let old_node = Node::leaf(VALUE, 0, Size::CONTENT).bytes();
    let old_record = [&[1, 1, 2, 1][..], &old_node, &[0]].concat();
    Journal::new(&mut flash, Region::Screens)
        .save(&old_record)
        .unwrap();

    let overrides = boot(&mut flash);
    assert!(
        overrides.reset(),
        "old catalogue was accepted at revision {:?} as keys {:?}",
        overrides.revision(),
        overrides
            .shown()
            .screens()
            .iter()
            .map(Screen::key)
            .collect::<Vec<_>>()
    );
    assert!(overrides.shown().screens().is_empty());
}

#[test]
fn reset_after_saving_the_builtin_shows_the_builtin_again() {
    use crate::compose::Flags;
    use crate::compose::builtin::{Laids, main_tree};

    let mut flash = flash();
    let mut overrides = boot(&mut flash);
    let mut laids = Laids::new(&C606, Flags::NONE, overrides.shown());
    let identical: Vec<_> = main_tree()
        .into_iter()
        .flat_map(|node| node.bytes())
        .collect();
    let set = Change::Set {
        key: MAIN,
        nodes: &identical,
    };
    assert_eq!(
        change(&mut overrides, &mut flash, set, 0),
        Saved::Revision(1)
    );
    assert_eq!(
        laids.main().screen(),
        overrides.shown().screen(MAIN).unwrap()
    );
    laids.update(Flags::NONE, overrides.shown());
    assert_eq!(
        laids.main().screen(),
        overrides.shown().screen(MAIN).unwrap()
    );

    assert_eq!(
        change(&mut overrides, &mut flash, Change::Reset(MAIN), 1),
        Saved::Revision(2)
    );
    assert!(overrides.shown().screen(MAIN).is_none());
    laids.update(Flags::NONE, overrides.shown());
    assert_eq!(laids.main().screen().tree().nodes(), main_tree().as_slice());
}
