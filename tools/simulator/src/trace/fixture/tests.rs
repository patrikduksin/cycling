use super::*;
use firmware_services::trace::Header;

fn expectation() -> Expectation {
    let seen = vec![
        Expect::parse("power!=--").unwrap(),
        Expect::parse("page=ride/climb").unwrap(),
    ];
    Expectation::new(
        190_500,
        seen,
        Some("rides decode"),
        vec![Expect::parse("speed=45.0").unwrap()],
    )
    .unwrap()
}

#[test]
fn an_expectation_reads_back_as_written() {
    let written = expectation().to_toml().unwrap();
    assert_eq!(
        Expectation::from_toml(&written),
        Ok(expectation()),
        "{written}"
    );
}

#[test]
fn an_expectation_file_names_session_time_and_known_keys_only() {
    let ride_time = "at = \"r1:00\"\nexpect = [\"power!=--\"]\n";
    assert!(
        Expectation::from_toml(ride_time)
            .unwrap_err()
            .contains("ride time")
    );
    let other = "at = \"01:00\"\nexpect = [\"power!=--\"]\nowner = \"follower\"\n";
    assert!(Expectation::from_toml(other).unwrap_err().contains("owner"));
    let empty = "at = \"01:00\"\nexpect = []\n";
    assert!(
        Expectation::from_toml(empty)
            .unwrap_err()
            .contains("--expect or --check")
    );
    let unknown_check = Expectation::new(
        0,
        Vec::new(),
        Some("keeps recording while riding"),
        Vec::new(),
    );
    assert!(unknown_check.is_err());
}

fn passes(trace: &[u8]) -> Vec<String> {
    let (_, events) = trace::decode(trace).unwrap();
    events
        .filter_map(|event| match event {
            Event::Pass { now_ms } => Some(format!("pass {now_ms}")),
            Event::Command(line) => Some(format!("command {line}")),
            _ => None,
        })
        .collect()
}

#[test]
fn a_slice_ends_with_the_last_pass_at_or_before_its_time_and_runs_no_command_after_it() {
    let header = Header {
        build: [0; 20],
        device: "c606",
        boot: 1,
        terrain: firmware_services::trace::origin::Found::Absent,
    };
    let events = [
        Event::Pass { now_ms: 10 },
        Event::Command("CMD 1 RIDE STATUS"),
        Event::Pass { now_ms: 20 },
        Event::Command("CMD 2 RIDE STATUS"),
        Event::Pass { now_ms: 30 },
    ];
    let trace = super::super::surgery::encode(&header, events);
    let (slice, at_ms) = slice(&trace, 25).unwrap();
    assert_eq!(at_ms, 20);
    assert_eq!(
        passes(&slice),
        ["pass 10", "command CMD 1 RIDE STATUS", "pass 20"]
    );
    assert!(
        super::slice(&trace, 5).is_err(),
        "no pass at or before 5 ms"
    );
}

/// A device trace: its fixtures are private.
fn device_trace() -> Vec<u8> {
    let header = Header {
        build: [0; 20],
        device: "c606",
        boot: 1,
        terrain: firmware_services::trace::origin::Found::Absent,
    };
    let session = Event::Session {
        memory: firmware_services::trace::origin::Memory::ALL,
        redacted: true,
    };
    super::super::surgery::encode(&header, [session, Event::Pass { now_ms: 10 }])
}

#[test]
fn a_private_fixture_goes_only_where_git_ignores_it() {
    let trace = device_trace();
    let tree = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let refused = |name: &str, out: Option<&Path>| match home(&trace, name, out) {
        Ok((dir, _)) => panic!("{name} {out:?} is written to {}", dir.display()),
        Err(why) => why,
    };
    for name in ["../../tools/simulator/tests/traces/x", "a/b", "..", ".", ""] {
        let why = refused(name, None);
        assert!(why.contains("one plain name"), "{name}: {why}");
    }
    let escaping = tree.join(".local/../tools/simulator/tests/traces/x");
    let why = refused("x", Some(&escaping));
    assert!(why.contains("git does not ignore"), "{why}");

    let local = tree.join(format!(".local/fixture-home-{}", std::process::id()));
    std::fs::create_dir_all(&local).unwrap();
    let link = local.join("traces");
    std::os::unix::fs::symlink(tree.join("tools/simulator/tests/traces"), &link).unwrap();
    let why = refused("x", Some(&link.join("x")));
    assert!(why.contains("git does not ignore"), "{why}");

    let (kept, _) = home(&trace, "x", Some(&local.join("x"))).unwrap();
    assert_eq!(kept, local.canonicalize().unwrap().join("x"));
    let outside = std::env::temp_dir().join("x");
    assert!(
        home(&trace, "x", Some(&outside)).is_ok(),
        "outside any checkout"
    );
    let (default, _) = home(&trace, "x", None).unwrap();
    assert!(
        default.ends_with(".local/fixtures/x"),
        "{}",
        default.display()
    );
    std::fs::remove_dir_all(&local).unwrap();
}

#[test]
fn a_replay_reads_the_card_the_recording_wrote() {
    use crate::bulk::Presence;
    use crate::fuzz::check::card_rides;
    use crate::sim::{Sim, seconds};
    let mut sim = Sim::new();
    sim.fit_bulk(Presence::Ready);
    sim.wait_usable();
    sim.init_bulk();
    sim.ride_at(25.0, 0.0, 0.0, 0.0);
    sim.start_ride();
    sim.advance(seconds(60));
    let recorded = card_rides(&sim);
    let ride = recorded.last().expect("a ride on the card");
    assert!(ride.samples().count() > 0);
    let (slice, at_ms) = slice(&sim.capture(), u64::MAX).unwrap();
    let mut replay = Sim::replaying(
        &slice,
        Options {
            mode: Mode::Changed,
            model: false,
        },
        None,
    )
    .unwrap();
    assert!(replay.replay_until(at_ms));
    let replayed = card_rides(&replay);
    let ids = |rides: &[crate::rides::Ride]| rides.iter().map(|r| r.id).collect::<Vec<_>>();
    assert_eq!(ids(&replayed), ids(&recorded));
}
