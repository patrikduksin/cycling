use shell_simulator::replay::{Kind, Options};
use shell_simulator::session::Step;
use shell_simulator::sim::{Sim, seconds};
use std::collections::BTreeMap;

const SCRIPT: [&str; 13] = [
    "POWER STATUS",
    "WIFI",
    "WIFI NETWORKS",
    "BLE",
    "BLE PEERS",
    "VOLUME STATUS",
    "STORE STATUS",
    "INFO",
    "HELP",
    "TIME",
    "INPUT",
    "BATTERY",
    "POWER SHUTDOWN",
];

const UNRECORDED: [&str; 5] = ["POWER STATUS", "TIME", "INPUT", "BATTERY", "POWER SHUTDOWN"];

fn recorded() -> (Vec<u8>, Vec<(&'static str, String)>) {
    let mut sim = Sim::new();
    sim.discard_recording();
    let mut sim = sim.boot();
    sim.sync_network_time(1_800_000_000_000);
    sim.advance(seconds(1));
    let replies = SCRIPT.iter().map(|line| sim.command(line)).collect();
    sim.advance(seconds(2));
    (sim.capture(), replies)
}

fn replayed(capture: &[u8]) -> (Sim, Vec<(String, &'static str, String)>) {
    let mut sim = Sim::replaying(capture, Options::default(), None).expect("a replayable capture");
    let mut replies = Vec::new();
    loop {
        match sim.device.replay_step() {
            Step::Pass => {}
            Step::Command {
                line,
                status,
                reply,
            } => replies.push((line, status, reply)),
            Step::End => return (sim, replies),
        }
    }
}

#[test]
fn a_replay_answers_each_console_line_as_the_recording_did() {
    let (capture, on_device) = recorded();
    assert_eq!(
        on_device[SCRIPT.len() - 1].0,
        "ACCEPTED",
        "the device shut down"
    );
    let time = SCRIPT.iter().position(|line| *line == "TIME").unwrap();
    assert!(on_device[time].1.contains("Fresh"), "{}", on_device[time].1);
    let (sim, on_replay) = replayed(&capture);
    let report = sim.device.report().expect("a replay");
    assert_eq!(on_replay.len(), SCRIPT.len(), "{report}");
    for ((line, recorded), (replayed_line, status, reply)) in
        SCRIPT.iter().zip(&on_device).zip(&on_replay)
    {
        assert!(replayed_line.ends_with(line), "{replayed_line}");
        if !UNRECORDED.contains(line) {
            assert_eq!(
                (*status, reply.clone()),
                recorded.clone(),
                "{line}; {report}"
            );
        }
    }
    assert_eq!(
        report.counts,
        BTreeMap::from([(Kind::Unrecorded, UNRECORDED.len() as u32)]),
        "{:?}",
        report.first
    );
    let first = report.first.expect("the power reads");
    assert!(first.detail.contains("power control"), "{}", first.detail);
}

#[test]
fn a_replay_refuses_a_store_format_while_its_own_trace_is_open() {
    let mut sim = Sim::new();
    sim.discard_recording();
    sim.fit_bulk(shell_simulator::bulk::Presence::Ready);
    let mut sim = sim.boot();
    sim.init_bulk();
    assert_eq!(sim.command("STORE FORMAT CONFIRM").0, "OK");
    sim.power_loss();
    let on_device = [
        sim.command("STORE STATUS"),
        sim.command("STORE FORMAT CONFIRM"),
    ];
    sim.advance(seconds(1));
    let (sim, on_replay) = replayed(&sim.capture());
    let stable_status = |reply: &str| {
        (reply.split(" trace=").next().unwrap_or(reply))
            .split_whitespace()
            .filter(|field| !field.starts_with("store="))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (status, store) = (&on_device[0], &on_replay[0]);
    assert!(status.1.contains(" trace=recording "), "{}", status.1);
    assert!(store.2.contains(" trace=recording "), "{}", store.2);
    assert_eq!(stable_status(&store.2), stable_status(&status.1));
    assert_eq!(on_device[1], ("BUSY", "trace open".into()));
    assert_eq!(
        (on_replay[1].1, on_replay[1].2.as_str()),
        ("BUSY", "trace open")
    );
    let report = sim.device.report().expect("a replay");
    assert_eq!(report.first, None, "{report}");
}
