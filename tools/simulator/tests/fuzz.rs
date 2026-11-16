use shell_simulator::fuzz::{self, Corpus, FIXED};

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
