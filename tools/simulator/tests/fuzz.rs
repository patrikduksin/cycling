use shell_simulator::fuzz::plan::Course;
use shell_simulator::fuzz::{self, At, Corpus, FIXED, Finding, WorldLog};
use std::path::Path;

fn short_fixed_seeds(count: usize) -> Vec<u64> {
    FIXED
        .filter(|seed| {
            let plan = fuzz::draw(*seed);
            plan.steps.iter().map(|s| s.nominal_ms()).sum::<u64>() < 12 * 60_000
        })
        .take(count)
        .collect()
}

#[test]
fn a_session_is_a_function_of_its_seed() {
    let seed = short_fixed_seeds(1)[0];
    let plan = fuzz::draw(seed);
    assert_eq!(fuzz::draw(seed), plan);
    assert_ne!(fuzz::draw(seed + 1), plan);
    let corpus = Corpus::none();
    let first = fuzz::run(&plan, &corpus, false).expect("a fixed seed passes");
    let second = fuzz::run(&plan, &corpus, false).expect("a fixed seed passes");
    assert_ne!(first.world_log_digests, 0);
    assert_eq!(
        first.world_log_digests, second.world_log_digests,
        "seed {seed:#x}"
    );
    assert_eq!(first.presses, second.presses, "seed {seed:#x}");
}

#[test]
fn short_fixed_seeds_pass() {
    std::thread::scope(|scope| {
        for seed in short_fixed_seeds(3) {
            scope.spawn(move || {
                if let Err(finding) = fuzz::run(&fuzz::draw(seed), &Corpus::none(), false) {
                    panic!("{finding}");
                }
            });
        }
    });
}

#[test]
fn seed_0xab_keeps_the_card_ride_through_torn_internal_starts() {
    if let Err(finding) = fuzz::run(&fuzz::draw(0xab), &Corpus::none(), false) {
        panic!("{finding}");
    }
}

#[test]
fn a_finding_names_the_course_it_rode() {
    let plan = FIXED
        .map(fuzz::draw)
        .find(|plan| matches!(plan.setup.course, Course::Corpus { .. }))
        .expect("a fixed seed draws a corpus course");
    let finding = |corpus: &Corpus| Finding {
        seed: plan.seed,
        check: "no panic",
        at: At::End,
        at_ms: 0,
        detail: String::new(),
        course_trace: fuzz::road(&plan.setup.course, corpus).1,
        world_log: WorldLog(Vec::new()),
        end_probe: Vec::new(),
    };
    let without = finding(&Corpus::none()).reproduce();
    assert!(without.ends_with(" --no-corpus"), "{without}");

    let trace = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces/laps-and-a-menu.trace");
    let one = Corpus::load(Some(&trace));
    assert_eq!(one.len(), 1, "{:?}", one.skipped);
    let with = finding(&one).reproduce();
    assert!(
        with.ends_with(&format!(" --corpus {}", trace.display())),
        "{with}"
    );
}

#[test]
fn a_reproduce_line_keeps_an_odd_course_path_one_shell_word() {
    let path = "/tmp/rider's rides/a lap.trace";
    let finding = Finding {
        seed: 1,
        check: "no panic",
        at: At::End,
        at_ms: 0,
        detail: String::new(),
        course_trace: Some(path.into()),
        world_log: WorldLog(Vec::new()),
        end_probe: Vec::new(),
    };
    let line = finding.reproduce();
    let word = line.split_once(" --corpus ").expect("names the course").1;
    assert_eq!(word, r"'/tmp/rider'\''s rides/a lap.trace'");
    let shell = std::process::Command::new("sh")
        .args(["-c", &format!("printf '%s|' {word}")])
        .output()
        .expect("sh runs");
    assert_eq!(String::from_utf8_lossy(&shell.stdout), format!("{path}|"));
}

#[test]
fn a_run_in_child_processes_adds_up_its_sessions() {
    let run = std::process::Command::new(env!("CARGO_BIN_EXE_fuzz"))
        .args(["--seeds", "1-2", "--no-corpus", "--jobs", "2"])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("the fuzz binary starts");
    let in_process = std::thread::scope(|scope| {
        let sessions: Vec<_> = (1..=2)
            .map(|seed| {
                scope.spawn(move || {
                    fuzz::run(&fuzz::draw(seed), &Corpus::none(), false)
                        .unwrap_or_else(|finding| panic!("{finding}"))
                        .world_log_digests
                })
            })
            .collect();
        sessions
            .into_iter()
            .map(|session| session.join().expect("a session returns"))
            .fold(0u64, u64::wrapping_add)
    });
    let output = run.wait_with_output().expect("the fuzz run ends");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "{stdout}{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(stdout.contains("fuzz: 2 sessions,"), "{stdout}");
    assert!(
        stdout.contains(&format!("world logs: {in_process:016x}\n")),
        "{stdout}"
    );
}

#[test]
fn a_child_that_dies_is_a_finding_with_a_reproduce_line() {
    let output = std::process::Command::new("sh")
        .args([
            "-c",
            "ulimit -c 0 && exec \"$0\" --seeds 1-1 --no-corpus --jobs 1",
        ])
        .arg(env!("CARGO_BIN_EXE_fuzz"))
        .env("RUST_MIN_STACK", "40000")
        .output()
        .expect("the fuzz run ends");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let all = format!("{stdout}{}", String::from_utf8_lossy(&output.stderr));
    assert_eq!(output.status.code(), Some(1), "{all}");
    assert!(stdout.contains("check=\"the process died\""), "{all}");
    assert!(
        stdout.contains("reproduce: cycling fuzz --seed 0x0000000000000001 --no-corpus"),
        "{all}"
    );
}

#[test]
fn a_scenario_that_would_not_fail_its_check_is_never_printed() {
    use shell_simulator::fuzz::shrink::{self, Shrunk};
    let mut plan = fuzz::draw(1);
    plan.steps.truncate(3);
    let corpus = Corpus::none();
    let shrunk = |check, at| Shrunk {
        plan: plan.clone(),
        finding: Finding {
            seed: 1,
            check,
            at,
            at_ms: 0,
            detail: String::new(),
            course_trace: None,
            world_log: WorldLog(Vec::new()),
            end_probe: Vec::new(),
        },
        minimal: true,
        tried: 0,
    };
    let passing = shrink::scenario(&shrunk("rides decode", At::End), &corpus, "t").unwrap_err();
    assert!(passing.contains("does not fail rides decode"), "{passing}");
    let no_panic = shrink::scenario(&shrunk("no panic", At::Step(2)), &corpus, "t").unwrap_err();
    assert!(no_panic.contains("runs without a panic"), "{no_panic}");
    let between = shrink::scenario(&shrunk("committed samples survive", At::End), &corpus, "t");
    assert!(between.unwrap_err().contains("judged between steps"));
}
