use shell_simulator::replay::first_difference;
use shell_simulator::sim::{Sim, seconds};
use shell_simulator::trace::file;
use shell_simulator::trace::timeline::{Kind, read};
use std::path::Path;

fn light_lines(trace: &[u8]) -> Vec<String> {
    read(trace)
        .expect("the trace decodes")
        .entries
        .iter()
        .filter(|event| event.kind == Kind::Sensors && event.text.starts_with("ambient light"))
        .map(|event| event.text.clone())
        .collect()
}

#[test]
fn the_light_a_ride_saw_is_in_its_trace_and_replays_exactly() {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.set_ambient_light(Some(68));
    sim.advance(seconds(2));
    sim.set_ambient_light(Some(1_557));
    sim.advance(seconds(2));
    sim.set_ambient_light(None);
    sim.advance(seconds(1));
    let capture = sim.capture();

    let recorded = light_lines(&capture.trace);
    assert!(
        recorded.contains(&"ambient light 68".to_string()),
        "{recorded:?}"
    );
    assert!(
        recorded.contains(&"ambient light 1557".to_string()),
        "{recorded:?}"
    );
    assert_eq!(recorded.last().unwrap(), "ambient light unavailable");
    assert!(
        recorded.len() >= 8,
        "about three readings a second are kept: {recorded:?}"
    );

    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
    assert_eq!(light_lines(&replayed.trace()), recorded);
}

#[test]
fn a_trace_recorded_before_ambient_light_was_traced_still_replays_exactly() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces/laps-and-a-menu.trace");
    let (capture, _) = file::load(&path).expect("the fixture loads");
    assert_eq!(light_lines(&capture.trace), Vec::<String>::new());
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture.trace, &replayed.trace()), None);
}
