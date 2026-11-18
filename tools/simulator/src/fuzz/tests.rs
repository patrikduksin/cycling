use super::*;

#[test]
fn a_panic_in_the_end_probes_power_cut_keeps_its_calls_and_prints_a_scenario_that_panics() {
    let plan = Plan {
        steps: Vec::new(),
        ..draw(1)
    };
    let corpus = Corpus::none();
    script::POWER_CUTS_PANIC.set(true);
    let Err(finding) = run(&plan, &corpus, false) else {
        panic!("the power cut panics");
    };
    let finding = *finding;
    assert_eq!(
        (finding.check, finding.at),
        ("no panic", At::End),
        "{finding}"
    );
    assert_eq!(
        finding.end_probe.last(),
        Some(&script::Call::CutPower),
        "the probe kept the call it panicked in: {:?}",
        finding.end_probe
    );
    let shrunk = shrink::Shrunk {
        plan,
        finding,
        minimal: true,
        tried: 0,
    };
    let printed = shrink::scenario(&shrunk, &corpus, "a_power_cut_panics");
    script::POWER_CUTS_PANIC.set(false);
    let printed = printed.expect("the printed scenario panics as the session did");
    assert!(printed.contains("sim.cut_power();"), "{printed}");
}
