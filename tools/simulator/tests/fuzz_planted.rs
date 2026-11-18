#![cfg(feature = "planted-bug")]
use shell_simulator::fuzz::{self, Corpus, FIXED, Finding};
use shell_simulator::route::{Point, Route};
use shell_simulator::sim::{Sim, seconds};
use vana::ride::log::Kind;

const PROOF_SEEDS: u64 = 16;

const PLANT_CHECK: &str = "rides decode";

#[test]
fn the_plant_fires_by_hand() {
    let mut sim = Sim::new().boot();
    sim.ride_route(Route::loop_around(
        Point::new(-33.45, -70.66),
        5_000.0,
        20.0,
    ));
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.command("RIDE START");
    sim.advance(seconds(20));
    sim.command_after_reading_outcome("RIDE PAUSE");
    sim.advance(seconds(3));
    sim.power_loss();
    sim.advance(seconds(5));
    let (answer, out) = sim.command_after_reading_outcome("RIDE START");
    assert_eq!(answer, "ACCEPTED", "{out}");
    sim.advance(seconds(10));
    let rides = sim.rides();
    assert_eq!(rides.len(), 2);
    assert_eq!(
        rides[0].kinds().last(),
        Some(&Kind::Pause),
        "the plant no longer fires: the paused ride was closed"
    );
}

#[test]
fn the_fuzzer_finds_the_planted_bug_and_its_seed_reproduces_it() {
    assert!(FIXED.contains(&1) && FIXED.contains(&PROOF_SEEDS));
    let corpus = Corpus::none();
    let found: Vec<Box<Finding>> = std::thread::scope(|scope| {
        let searches: Vec<_> = (1..=PROOF_SEEDS)
            .map(|seed| {
                let corpus = &corpus;
                scope.spawn(move || fuzz::run(&fuzz::draw(seed), corpus, false).err())
            })
            .collect();
        searches
            .into_iter()
            .filter_map(|search| search.join().expect("a session never panics out"))
            .collect()
    });
    let first = found
        .first()
        .unwrap_or_else(|| panic!("no seed in 1..={PROOF_SEEDS} found the planted bug"));
    println!(
        "{} of {PROOF_SEEDS} seeds found the plant; the first:\n{first}",
        found.len()
    );
    println!(
        "reproduce with the plant: CARGO_TARGET_DIR=target/planted cargo run -p shell-simulator \
         --no-default-features --features planted-bug --bin fuzz -- --seed {:#018x} --no-corpus",
        first.seed
    );
    for finding in &found {
        assert_eq!(finding.check, PLANT_CHECK, "{finding}");
    }
    let again = fuzz::run(&fuzz::draw(first.seed), &corpus, false).err();
    assert_eq!(
        again.as_ref(),
        Some(first),
        "the seed finds the plant again"
    );

    let out = std::env::temp_dir().join(format!("fuzz-planted-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&out);
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_fuzz"))
        .env("PATH", "")
        .arg("--seed")
        .arg(format!("{:#018x}", first.seed))
        .arg("--no-corpus")
        .arg("--out")
        .arg(&out)
        .output()
        .expect("the fuzz binary runs");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(run.status.code(), Some(1), "{stdout}");
    let report = &stdout[stdout.find("FAIL seed=").expect("a finding")..];
    assert!(
        report.contains("\n  no bundle: run it inside a checkout"),
        "a bundle it could not write, with no git on PATH, is reported: {report}"
    );
    let dir = out.join(format!("{:#018x}", first.seed));
    assert!(dir.join("world.trace").is_file(), "{report}");
    let shrunk = &report[report.find("shrunk seed=").expect("the finding shrinks")..];
    println!("{shrunk}");
    let steps = |line: &str| -> Vec<usize> {
        let sizes = line.split_once(" steps").unwrap().0;
        let (before, after) = sizes.rsplit_once(" -> ").unwrap();
        [before.rsplit(' ').next().unwrap(), after]
            .map(|n| n.parse().unwrap())
            .to_vec()
    };
    let plan = shrunk
        .lines()
        .find(|l| l.contains("  plan "))
        .expect("its plan's sizes");
    assert!(steps(plan)[1] < steps(plan)[0], "{shrunk}");
    assert!(shrunk.contains("\n  world slice "), "{shrunk}");
    assert!(
        shrunk.contains("\n  scenario.rs fails the check"),
        "{shrunk}"
    );
    for file in [
        "plan.txt",
        "fixture/slice.trace",
        "fixture/expect.toml",
        "scenario.rs",
    ] {
        assert!(dir.join("shrunk").join(file).is_file(), "{file}: {shrunk}");
    }
    std::fs::remove_dir_all(&out).unwrap();

    let unshrunk = std::process::Command::new(env!("CARGO_BIN_EXE_fuzz"))
        .args([
            "--seed",
            &format!("{:#018x}", first.seed),
            "--no-corpus",
            "--no-shrink",
        ])
        .arg("--out")
        .arg(&out)
        .output()
        .expect("the fuzz binary runs");
    assert_eq!(unshrunk.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&unshrunk.stdout).contains("shrunk seed="));
    assert!(!dir.join("shrunk").exists(), "--no-shrink shrinks nothing");
    std::fs::remove_dir_all(&out).unwrap();
}

struct Shrunk {
    seed: u64,
    line: String,
    slice_fails_alone: Result<(), String>,
}

fn shrink_finding(seed: u64, corpus: &Corpus, out: &std::path::Path) -> Option<Shrunk> {
    use shell_simulator::fuzz::shrink;
    use shell_simulator::trace::fixture::{Class, Fixture};
    use std::time::Duration;
    let plan = fuzz::draw(seed);
    fuzz::run(&plan, corpus, false).err()?;
    let shrunk =
        shrink::plan(&plan, corpus, PLANT_CHECK, Duration::from_secs(240)).expect("it fails again");
    assert!(shrunk.minimal, "seed {seed}: the budget ran out");
    for skipped in 0..shrunk.plan.steps.len() {
        let mut fewer = shrunk.plan.clone();
        fewer.steps.remove(skipped);
        let again = fuzz::run(&fewer, corpus, false).err();
        assert!(
            again.is_none_or(|f| f.check != PLANT_CHECK),
            "seed {seed}: step {skipped} could go too"
        );
    }
    let dir = out.join(format!("{seed}"));
    let mut line = format!(
        "seed {seed:>2}: plan {} -> {} steps ({} tries), found at {}",
        plan.steps.len(),
        shrunk.plan.steps.len(),
        shrunk.tried,
        shrunk.finding.at
    );
    let source = shrink::scenario(&shrunk, corpus, &format!("plant_seed_{seed}"))
        .unwrap_or_else(|why| panic!("seed {seed}: {why}"));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("scenario.rs"), source).unwrap();
    let sliced = shrink::world_slice(&shrunk.finding, &dir, Duration::from_secs(60))
        .expect("rides decode is judged on a replay");
    let slice_fails_alone = sliced.map(|sliced| {
        let fixture = Fixture::read(&dir).unwrap();
        assert_eq!(fixture.judge().class(), Class::Reproduced, "seed {seed}");
        line += &format!(
            "; world slice {} -> {} bytes, {} -> {} passes, {} -> {} records",
            sliced.before.bytes,
            sliced.after.bytes,
            sliced.before.passes,
            sliced.after.passes,
            sliced.before.records,
            sliced.after.records,
        );
    });
    if let Err(why) = &slice_fails_alone {
        line += &format!("; {why}");
    }
    Some(Shrunk {
        seed,
        line,
        slice_fails_alone,
    })
}

#[test]
fn every_finding_of_the_plant_shrinks_to_a_one_minimal_plan_and_a_world_slice() {
    let corpus = Corpus::none();
    let out = std::env::temp_dir().join(format!("fuzz-shrunk-{}", std::process::id()));
    let shrunk: Vec<Shrunk> = std::thread::scope(|scope| {
        let shrinks: Vec<_> = (1..=PROOF_SEEDS)
            .map(|seed| {
                let (corpus, out) = (&corpus, &out);
                scope.spawn(move || shrink_finding(seed, corpus, out))
            })
            .collect();
        shrinks
            .into_iter()
            .filter_map(|shrink| shrink.join().expect("each plan shrinks 1-minimal"))
            .collect()
    });
    let _ = std::fs::remove_dir_all(&out);
    for found in &shrunk {
        println!("{}", found.line);
    }
    let alone: Vec<u64> = (shrunk.iter())
        .filter(|found| found.slice_fails_alone.is_ok())
        .map(|found| found.seed)
        .collect();
    println!(
        "{} of {} world slices fail alone: seeds {alone:?}",
        alone.len(),
        shrunk.len()
    );
    assert_eq!(alone.len(), shrunk.len(), "every world slice fails alone");
}
