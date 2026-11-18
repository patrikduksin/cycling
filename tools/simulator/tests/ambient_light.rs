use shell_simulator::replay::{Options, first_difference};
use shell_simulator::sim::{Sim, seconds};
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

    let recorded = light_lines(&capture);
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
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
    assert_eq!(light_lines(&replayed.trace()), recorded);
}

#[test]
fn a_trace_of_a_ride_with_no_light_feed_replays_exactly() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/traces/laps-and-a-menu.trace");
    let capture = std::fs::read(&path).expect("the fixture reads");
    assert_eq!(light_lines(&capture), ["ambient light unavailable"]);
    let replayed = Sim::replay(&capture);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
    assert_eq!(first_difference(&capture, &replayed.trace()), None);
}

fn levels(mut samples: Vec<u8>) -> Vec<u8> {
    samples.dedup();
    samples
}

#[test]
fn a_ride_with_a_light_feed_replays_to_the_same_backlight() {
    let mut sim = Sim::new().boot();
    sim.discard_recording();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.set_ambient_light(Some(68));
    sim.advance(seconds(5));
    let start = sim.now();
    let mut seen = vec![sim.device.shell.effective()];
    for (light, seconds) in [
        (Some(68), 10),
        (Some(1_557), 10),
        (Some(300), 30),
        (None, 10),
    ] {
        sim.set_ambient_light(light);
        for _ in 0..seconds * 100 {
            sim.advance(10);
            seen.push(sim.device.shell.effective());
        }
    }
    let seen = levels(seen);
    assert!(seen.len() > 20, "the backlight moved through {seen:?}");
    let capture = sim.capture();

    let mut replayed = Sim::replaying(&capture, Options::default(), None).unwrap();
    replayed.replay_until(start);
    let mut again = vec![replayed.device.shell.effective()];
    while replayed.replay_until(replayed.device.now + 1) {
        again.push(replayed.device.shell.effective());
    }
    assert_eq!(levels(again), seen);
    let report = replayed.device.report().unwrap();
    assert_eq!(report.first, None, "{:?}", report.counts);
}
