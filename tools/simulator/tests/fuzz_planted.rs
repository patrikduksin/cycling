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
    let found: Vec<Finding> = std::thread::scope(|scope| {
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
    for finding in &found {
        assert_eq!(finding.check, PLANT_CHECK, "{finding}");
    }
    let again = fuzz::run(&fuzz::draw(first.seed), &corpus, false).err();
    assert_eq!(
        again.as_ref(),
        Some(first),
        "the seed finds the plant again"
    );
}
