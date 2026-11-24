use super::*;

struct Drawn {
    at: u64,
    drawn_ms: u64,
    numbers: [Option<String>; 4],
    clock: Option<String>,
    texts: Vec<String>,
}

fn is_ride_clock(text: &str) -> bool {
    let parts: Vec<&str> = text.split(':').collect();
    parts.len() == 3
        && parts[0].chars().all(|c| c.is_ascii_digit())
        && parts[1..]
            .iter()
            .all(|part| part.len() == 2 && part.chars().all(|c| c.is_ascii_digit()))
}

const TICK_SLACK_MS: u64 = 60;
const WALL_AND_RIDE_SECONDS_APART_MS: u64 = 370;
const RADAR_REPORTS_PER_S: usize = 4;
const CAR_SECONDS: u64 = 6;

fn tick_phase(frames: &[Drawn]) -> u64 {
    frames
        .windows(2)
        .find(|pair| pair[0].clock != pair[1].clock)
        .map(|pair| pair[1].drawn_ms % 1000)
        .expect("the ride clock turned over")
}

fn off_tick_in(frames: &[Drawn], phase: u64) -> Vec<u64> {
    frames
        .windows(2)
        .filter(|pair| pair[0].texts != pair[1].texts)
        .filter(|pair| (pair[1].drawn_ms + 1000 - phase) % 1000 > TICK_SLACK_MS)
        .map(|pair| pair[1].at)
        .collect()
}

fn frames_drawn(sim: &mut Sim, secs: u64, mut feed: impl FnMut(&mut Sim, u64)) -> Vec<Drawn> {
    use std::hash::{DefaultHasher, Hash, Hasher};
    let (mut frames, mut last, mut shown) = (Vec::new(), 0, 0);
    let start = sim.now();
    while sim.now() - start < seconds(secs) {
        if (sim.now() - start).is_multiple_of(250) {
            feed(sim, sim.now() - start);
        }
        sim.advance(10);
        let submissions = sim.device.shell.display_submissions;
        if submissions == shown {
            continue;
        }
        shown = submissions;
        let screen = sim.screen();
        let mut hasher = DefaultHasher::new();
        screen.pixels.hash(&mut hasher);
        let pixels = hasher.finish();
        if pixels == last {
            continue;
        }
        last = pixels;
        let texts: Vec<String> = screen
            .frame()
            .read()
            .into_iter()
            .map(|run| run.text)
            .collect();
        let clock = texts.iter().find(|text| is_ride_clock(text)).cloned();
        frames.push(Drawn {
            at: sim.now() - start,
            drawn_ms: sim.now(),
            numbers: ["SPD KM/H", "PWR 3S W", "HEART BPM", "CAD RPM"]
                .map(|label| screen.field(label)),
            clock,
            texts,
        });
    }
    frames
}

fn wobbling() -> impl FnMut(&mut Sim, u64) {
    let mut seed = 0x9e37_79b9_7f4a_7c15u64;
    move |sim, _| {
        let mut wobble = || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            (seed % 2001) as f64 / 1000.0 - 1.0
        };
        sim.ride_at(
            28.0 + wobble(),
            230.0 + 35.0 * wobble(),
            142.0 + 2.0 * wobble(),
            88.0 + 3.0 * wobble(),
        );
    }
}

struct Rides {
    number_changes: Vec<u64>,
    longest_burst: usize,
    car_frames: usize,
    off_tick: Vec<u64>,
}

fn measure(sim: &mut Sim) -> Rides {
    sim.advance(WALL_AND_RIDE_SECONDS_APART_MS);
    sim.start_ride();
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(10));
    let noisy = frames_drawn(sim, 10, wobbling());
    let number_changes = noisy
        .windows(2)
        .filter(|pair| pair[0].numbers != pair[1].numbers)
        .map(|pair| pair[1].at)
        .collect();
    let phase = tick_phase(&noisy);
    let mut off_tick = off_tick_in(&noisy, phase);
    for _ in 0..8 {
        sim.press(Role::Move);
        sim.advance(seconds(1));
        if sim.seen().page.is_some_and(|(_, page)| page == "ride/main") {
            break;
        }
        off_tick.extend(off_tick_in(&frames_drawn(sim, 5, wobbling()), phase));
    }
    sim.ride_at(28.0, 230.0, 142.0, 88.0);
    sim.advance(seconds(5));
    let step = frames_drawn(sim, 6, |sim, at| {
        if at == 1_000 {
            sim.ride_at(28.0, 330.0, 142.0, 88.0);
        }
    });
    let (mut longest_burst, mut run) = (0, 0);
    for pair in step.windows(2) {
        run = if pair[1].at - pair[0].at <= 50 {
            run + 1
        } else {
            0
        };
        longest_burst = longest_burst.max(run + 1);
    }
    sim.vehicle(Vehicle {
        range_m: 150.0,
        closing_kmh: 25.0,
        fast: false,
        side: Side::Behind,
    });
    let car_frames = frames_drawn(sim, CAR_SECONDS, |_, _| {}).len();
    Rides {
        number_changes,
        longest_burst,
        car_frames,
        off_tick,
    }
}

#[test]
fn saver_mode_is_kept_over_a_restart_and_draws_only_what_arrives() {
    let mut sim = rider();
    sim.ant_sensor(device_type::RADAR, RADAR);
    sim.pair(&["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"]);

    let mut normal = rider();
    normal.ant_sensor(device_type::RADAR, RADAR);
    normal.pair(&["HEART RATE", "POWER METER", "SPEED/CAD", "RADAR"]);
    let smooth = measure(&mut normal);
    assert!(
        smooth
            .number_changes
            .windows(2)
            .any(|pair| pair[1] - pair[0] < 300),
        "{:?}",
        smooth.number_changes
    );

    assert!(sim.command("OPTION").1.contains("display=normal"));
    sim.option("DISPLAY", "SAVER");
    sim.advance(seconds(3));

    sim.power_loss();
    let options = sim.command("OPTION").1;
    assert!(
        options.contains("display=saver") && options.contains("stored=current"),
        "{options}"
    );

    let saver = measure(&mut sim);
    sim.checkpoint("main page in SAVER");
    assert!(
        saver.number_changes.len() >= 8
            && saver
                .number_changes
                .windows(2)
                .all(|pair| pair[1] - pair[0] >= 900),
        "numbers change once a second at most: {:?}",
        saver.number_changes
    );
    assert!(
        saver.off_tick.is_empty(),
        "text changed between ride-clock seconds at {:?} ms",
        saver.off_tick
    );
    assert!(
        saver.longest_burst <= 2,
        "no glide frames: {}",
        saver.longest_burst
    );
    let radar_and_numbers = CAR_SECONDS as usize * (RADAR_REPORTS_PER_S + 1);
    assert!(
        saver.car_frames <= radar_and_numbers && saver.car_frames * 2 < smooth.car_frames,
        "{} frames in SAVER, {} in NORMAL",
        saver.car_frames,
        smooth.car_frames
    );
}
