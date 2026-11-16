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
        course_trace: fuzz::route(&plan.setup.course, corpus).1,
        world_log: WorldLog(Vec::new()),
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
