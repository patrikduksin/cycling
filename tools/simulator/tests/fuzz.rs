//! The fuzzer's promise: a session is a function of its seed.
use shell_simulator::fuzz;

#[test]
fn a_plan_is_a_function_of_its_seed() {
    let plan = fuzz::draw(7);
    assert_eq!(fuzz::draw(7), plan);
    assert_ne!(fuzz::draw(8), plan);
}
