pub mod check;
pub mod corpus;
pub mod plan;

pub use corpus::Corpus;
pub use plan::{FIXED, Plan, draw};

use crate::ant::device_type;
use crate::bulk::Presence;
use crate::route::{Point, Route};
use crate::sim::{Sim, seconds};
use crate::world::Vehicle;
use check::{CHECKS, Moment, Observation, Verdict, observe};
use device_api::input::Button;
use plan::{CENTERS, Card, Course, Sensor, Setup, Step};
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use vana::ride::log::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    Setup,
    Step(usize),
    End,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    pub seed: u64,
    /// A check's name, or "no panic".
    pub check: &'static str,
    pub at: At,
    pub at_ms: u64,
    pub detail: String,
    /// The corpus directory, when the session rode a course from it.
    pub corpus: Option<PathBuf>,
    pub world_log: WorldLog,
}

/// The trace of the boot the finding was made in.
#[derive(Clone, PartialEq, Eq)]
pub struct WorldLog(pub Vec<u8>);

impl WorldLog {
    /// A fingerprint of the trace of the boot the finding was made in.
    pub fn digest(&self) -> u64 {
        use std::hash::{DefaultHasher, Hash, Hasher};
        let mut hasher = DefaultHasher::new();
        self.0.hash(&mut hasher);
        hasher.finish()
    }
}

impl fmt::Debug for WorldLog {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "WorldLog({} bytes, {:016x})",
            self.0.len(),
            self.digest()
        )
    }
}

impl Finding {
    pub fn reproduce(&self) -> String {
        let corpus = self
            .corpus
            .as_ref()
            .map(|dir| format!(" --corpus {}", dir.display()))
            .unwrap_or_default();
        format!("cycling fuzz --seed {:#018x}{corpus}", self.seed)
    }
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let at = match self.at {
            At::Setup => "setup".to_string(),
            At::Step(index) => format!("step {index}"),
            At::End => "end".to_string(),
        };
        writeln!(
            f,
            "FAIL seed={:#018x} check={:?} at={at} t={}",
            self.seed,
            self.check,
            clock(self.at_ms)
        )?;
        writeln!(f, "  {}", self.detail.replace('\n', "\n  "))?;
        writeln!(f, "  world log {:016x}", self.world_log.digest())?;
        write!(f, "  reproduce: {}", self.reproduce())
    }
}

/// World time as h:mm:ss.
pub fn clock(ms: u64) -> String {
    let s = ms / 1000;
    format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
}

#[derive(Default)]
pub struct Coverage {
    pub sessions: u64,
    pub simulated_ms: u64,
    /// The wrapping sum of every session's world log digest; two runs of the
    /// same seeds on one build agree on it.
    pub world_log_digests: u64,
    /// Presses and holds by the page shown before them, and button.
    pub presses: BTreeMap<(&'static str, &'static str), u64>,
    /// Checks a guard kept from judging, by check and guard.
    pub skips: BTreeMap<(&'static str, &'static str), u64>,
}

impl Coverage {
    pub fn merge(&mut self, other: Coverage) {
        self.sessions += other.sessions;
        self.simulated_ms += other.simulated_ms;
        self.world_log_digests = self.world_log_digests.wrapping_add(other.world_log_digests);
        for (key, n) in other.presses {
            *self.presses.entry(key).or_default() += n;
        }
        for (key, n) in other.skips {
            *self.skips.entry(key).or_default() += n;
        }
    }
}

/// Every page a press can meet; "off" is a device that is off.
pub const PAGES: [&str; 17] = [
    "boot",
    "home",
    "sensors",
    "ready",
    "ride",
    "end_ride",
    "discard_ride",
    "summary",
    "settings",
    "bests",
    "form",
    "workouts",
    "phone",
    "history",
    "past_ride",
    "setup",
    "off",
];

/// Play `plan`, judging every step. With `verbose`, print each step and the
/// page after it.
pub fn run(plan: &Plan, corpus: &Corpus, verbose: bool) -> Result<Coverage, Finding> {
    crate::recording::capture_panics();
    let mut sim = Sim::named(&format!("fuzz-{:016x}", plan.seed));
    let mut session = Session {
        plan,
        verbose,
        at: At::Setup,
        coverage: Coverage::default(),
        course_trace: None,
    };
    let outcome = catch_unwind(AssertUnwindSafe(|| session.play(&mut sim, corpus)));
    let world_log = WorldLog(catch_unwind(AssertUnwindSafe(|| sim.trace())).unwrap_or_default());
    let (check, detail) = match outcome {
        Ok(Ok(())) => {
            session.coverage.sessions = 1;
            session.coverage.simulated_ms = sim.now();
            session.coverage.world_log_digests = world_log.digest();
            return Ok(session.coverage);
        }
        Ok(Err(failed)) => failed,
        Err(_) => (
            "no panic",
            crate::recording::take_panic().unwrap_or_default(),
        ),
    };
    Err(Finding {
        seed: plan.seed,
        check,
        at: session.at,
        at_ms: sim.now(),
        detail,
        corpus: session.course_trace.and(corpus.dir.clone()),
        world_log,
    })
}

struct Session<'a> {
    plan: &'a Plan,
    verbose: bool,
    at: At,
    coverage: Coverage,
    course_trace: Option<PathBuf>,
}

type Failed = (&'static str, String);

impl Session<'_> {
    fn play(&mut self, sim: &mut Sim, corpus: &Corpus) -> Result<(), Failed> {
        let setup = &self.plan.setup;
        let card = setup.card == Card::Ready;
        self.course_trace = prepare(sim, setup, corpus);
        let mut before = observe(sim, card, None);
        if self.verbose {
            println!("seed {:#018x}", self.plan.seed);
            println!("setup {setup:?}");
            if let Some(path) = &self.course_trace {
                println!("course from {}", path.display());
            }
            println!("   0:00:00 setup -> {}", before.summary());
        }
        for (index, step) in self.plan.steps.iter().enumerate() {
            self.at = At::Step(index);
            if let Step::Press(button) | Step::Hold(button, _) = step {
                *self
                    .coverage
                    .presses
                    .entry((before.page, button_name(*button)))
                    .or_default() += 1;
            }
            apply(sim, step);
            let after = observe(sim, card, Some(&before));
            if self.verbose {
                println!(
                    "{:>4} {} {step:?} -> {}",
                    index,
                    clock(after.at_ms),
                    after.summary()
                );
            }
            self.judge(Some(step), &before, &after)?;
            before = after;
        }
        self.at = At::End;
        match still_a_bike_computer(sim, setup) {
            Verdict::Pass => {}
            Verdict::Skip(why) => self.skip("still a bike computer", why),
            Verdict::Fail(why) => return Err(("still a bike computer", why)),
        }
        let after = observe(sim, card, Some(&before));
        if self.verbose {
            println!("     {} end -> {}", clock(after.at_ms), after.summary());
        }
        self.judge(None, &before, &after)
    }

    fn judge(
        &mut self,
        step: Option<&Step>,
        before: &Observation,
        after: &Observation,
    ) -> Result<(), Failed> {
        let moment = Moment {
            step,
            before,
            after,
        };
        for check in CHECKS {
            match (check.run)(&moment) {
                Verdict::Pass => {}
                Verdict::Skip(why) => self.skip(check.name, why),
                Verdict::Fail(why) => return Err((check.name, why)),
            }
        }
        Ok(())
    }

    fn skip(&mut self, check: &'static str, why: &'static str) {
        *self.coverage.skips.entry((check, why)).or_default() += 1;
    }
}

fn button_name(button: Button) -> &'static str {
    match button {
        Button::TopLeft => "TopLeft",
        Button::BottomLeft => "BottomLeft",
        Button::BottomRight => "BottomRight",
        Button::Center => "Center",
    }
}

fn ant(sensor: Sensor) -> (u8, u16, &'static str) {
    match sensor {
        Sensor::Heart => (device_type::HEART_RATE, 4242, "HEART RATE"),
        Sensor::Power => (device_type::POWER, 1234, "POWER METER"),
        Sensor::SpeedCadence => (device_type::SPEED_CADENCE, 777, "SPEED/CAD"),
        Sensor::Radar => (device_type::RADAR, 31, "RADAR"),
    }
}

fn prepare(sim: &mut Sim, setup: &Setup, corpus: &Corpus) -> Option<PathBuf> {
    if setup.card != Card::None {
        sim.fit_bulk(if setup.card == Card::Ready {
            Presence::Ready
        } else {
            Presence::Missing
        });
    }
    sim.wait_usable();
    sim.skip_setup();
    for sensor in &setup.sensors {
        let (kind, number, _) = ant(*sensor);
        sim.ant_sensor(kind, number);
    }
    let (route, course) = route(&setup.course, corpus);
    sim.ride_route(route);
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    if !setup.sensors.is_empty() {
        let names: Vec<_> = setup.sensors.iter().map(|s| ant(*s).2).collect();
        sim.pair(&names);
    }
    if setup.card == Card::Ready {
        sim.init_bulk();
    }
    course
}

fn route(course: &Course, corpus: &Corpus) -> (Route, Option<PathBuf>) {
    let center = |index: u8| {
        let (lat, lon) = CENTERS[usize::from(index) % CENTERS.len()];
        Point::new(lat, lon)
    };
    match course {
        Course::Loop {
            center: at,
            length_m,
            climb_m,
        } => (
            Route::loop_around(center(*at), f64::from(*length_m), f64::from(*climb_m)),
            None,
        ),
        Course::Profile { center: at, knots } => {
            let knots: Vec<_> = knots
                .iter()
                .map(|&(d, e)| (f64::from(d), f64::from(e)))
                .collect();
            (Route::profile(center(*at), &knots), None)
        }
        Course::Corpus { pick } => match corpus.route(*pick) {
            Some((path, route)) => (route.clone(), Some(path.clone())),
            None => (
                Route::loop_around(
                    center((pick % 4) as u8),
                    (2_000 + pick % 30_000) as f64,
                    (pick % 200) as f64,
                ),
                None,
            ),
        },
    }
}

fn apply(sim: &mut Sim, step: &Step) {
    match *step {
        Step::Wait { ms } => sim.advance(u64::from(ms)),
        Step::Press(button) => sim.press(button),
        Step::Hold(button, ms) => sim.hold(button, u64::from(ms)),
        Step::Ride {
            kmh,
            watts,
            bpm,
            rpm,
        } => sim.ride_at(
            f64::from(kmh),
            f64::from(watts),
            f64::from(bpm),
            f64::from(rpm),
        ),
        Step::Stop => sim.ride_at(0.0, 0.0, 90.0, 0.0),
        Step::Host(action) => {
            if !sim.is_off() {
                sim.host_command(action.command());
            }
        }
        Step::Dropout { sensor, present } => {
            let (kind, number, _) = ant(sensor);
            sim.set_present(kind, number, present);
        }
        Step::GpsFix(fix) => sim.set_gps_fix(fix),
        Step::Barometer(on) => sim.set_barometer(on),
        Step::Vehicle {
            range_m,
            closing_kmh,
            fast,
            side,
        } => sim.vehicle(Vehicle {
            range_m: f64::from(range_m),
            closing_kmh: f64::from(closing_kmh),
            fast,
            side,
        }),
        Step::Battery { percent, charging } => sim.set_battery(percent, charging),
        Step::PowerCut => sim.cut_power(),
        Step::TornWrite { after, partial } => {
            sim.fail_storage(u32::from(after), usize::from(partial));
        }
        Step::CardRefusal { after, partial } => {
            sim.fail_bulk(u32::from(after), usize::from(partial));
        }
    }
}

const STARTABLE: [&str; 3] = ["ready", "saved", "recovered"];

fn still_a_bike_computer(sim: &mut Sim, setup: &Setup) -> Verdict {
    for sensor in &setup.sensors {
        let (kind, number, _) = ant(*sensor);
        sim.set_present(kind, number, true);
    }
    sim.set_gps_fix(true);
    sim.set_barometer(true);
    sim.set_battery(80, false);
    sim.ride_at(0.0, 0.0, 72.0, 0.0);
    sim.device.media.fault.borrow_mut().after = None;
    sim.cut_power();
    let booted = sim.now();
    while sim.is_off() || !sim.device.lit() || sim.vana_page() == "boot" {
        if sim.now() - booted > seconds(15) {
            return Verdict::Fail(format!(
                "not past the boot page 15 s after power-on; screen:\n{}",
                sim.screen().text()
            ));
        }
        sim.advance(10);
    }
    sim.advance(seconds(5));
    let (_, status) = sim.command("RIDE STATUS");
    let state = field(&status, "state").unwrap_or_default();
    if !STARTABLE.contains(&state.as_str()) {
        return Verdict::Skip(match state.as_str() {
            "full" => "journal full",
            "error" => "recorder error",
            "needs_init" => "journal needs init",
            "corrupt" => "journal corrupt",
            _ => "recorder in another state",
        });
    }
    let known = (ids(&sim.rides()), ids(&card_rides_if(sim, setup)));
    sim.ride_at(25.0, 180.0, 130.0, 85.0);
    let (answer, out) = sim.command("RIDE START");
    if answer != "ACCEPTED" {
        return Verdict::Fail(format!("RIDE START answered {answer} {out} in {status}"));
    }
    sim.advance(seconds(30));
    let (_, recording) = sim.command("RIDE STATUS");
    let samples: u32 = field(&recording, "samples")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if field(&recording, "state").as_deref() != Some("recording") || samples < 20 {
        return Verdict::Fail(format!("30 s after RIDE START: {recording}"));
    }
    let (answer, out) = sim.command("RIDE FINISH");
    if answer != "ACCEPTED" {
        return Verdict::Fail(format!("RIDE FINISH answered {answer} {out}"));
    }
    sim.advance(seconds(5));
    let saved_on = |rides: &[crate::rides::Ride], known: &[u32]| {
        rides
            .iter()
            .any(|r| !known.contains(&r.id) && r.kinds().last() == Some(&Kind::Finish))
    };
    let saved = saved_on(&sim.rides(), &known.0) || saved_on(&card_rides_if(sim, setup), &known.1);
    if !saved {
        let (_, status) = sim.command("RIDE STATUS");
        return Verdict::Fail(format!("no new ride ends with Finish; {status}"));
    }
    Verdict::Pass
}

fn ids(rides: &[crate::rides::Ride]) -> Vec<u32> {
    rides.iter().map(|r| r.id).collect()
}

fn card_rides_if(sim: &Sim, setup: &Setup) -> Vec<crate::rides::Ride> {
    if setup.card != Card::Ready {
        return Vec::new();
    }
    check::card_rides(sim)
}

fn field(status: &str, name: &str) -> Option<String> {
    status
        .split_whitespace()
        .find_map(|part| part.strip_prefix(name)?.strip_prefix('='))
        .map(str::to_string)
}
