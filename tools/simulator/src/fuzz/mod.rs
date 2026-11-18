pub mod check;
pub mod corpus;
pub mod lift;
pub mod plan;
pub mod script;
pub mod shrink;

pub use corpus::Corpus;
pub use plan::{FIXED, Plan, draw};

use crate::flows::status_field;
use crate::sim::{Sim, seconds};
use check::{BOOT_PAGE_DEADLINE_MS, CHECKS, Moment, Observation, Verdict, observe};
use device_api::input::Role;
use plan::{Card, Course, Setup, Step};
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use vana::ride::log::Kind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum At {
    Setup,
    Step(usize),
    End,
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Setup => f.write_str("setup"),
            Self::Step(index) => write!(f, "step {index}"),
            Self::End => f.write_str("end"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Finding {
    pub seed: u64,
    /// A check's name, or "no panic".
    pub check: &'static str,
    pub at: At,
    pub at_ms: u64,
    pub detail: String,
    /// The corpus trace the session rode; without one, the replay takes no
    /// corpus.
    pub course_trace: Option<PathBuf>,
    pub world_log: WorldLog,
    /// What the end probe did, for a finding at the end: the calls it made
    /// after the last step, as it made them.
    pub end_probe: Vec<script::Call>,
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
        reproduce(self.seed, self.course_trace.as_deref())
    }
}

/// The command that replays `seed` on the course it rode.
pub fn reproduce(seed: u64, course_trace: Option<&Path>) -> String {
    let course = match course_trace {
        Some(trace) => format!("--corpus {}", shell_word(&trace.to_string_lossy())),
        None => "--no-corpus".to_string(),
    };
    format!("cycling fuzz --seed {seed:#018x} {course}")
}

impl fmt::Display for Finding {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(
            f,
            "FAIL seed={:#018x} check={:?} at={} t={}",
            self.seed,
            self.check,
            self.at,
            clock(self.at_ms)
        )?;
        writeln!(f, "  {}", self.detail.replace('\n', "\n  "))?;
        writeln!(f, "  world log {:016x}", self.world_log.digest())?;
        write!(f, "  reproduce: {}", self.reproduce())
    }
}

/// `text` as one POSIX shell word.
fn shell_word(text: &str) -> String {
    let plain = |c: char| c.is_ascii_alphanumeric() || "/._-".contains(c);
    if !text.is_empty() && text.chars().all(plain) {
        return text.to_string();
    }
    format!("'{}'", text.replace('\'', r"'\''"))
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
pub fn run(plan: &Plan, corpus: &Corpus, verbose: bool) -> Result<Coverage, Box<Finding>> {
    crate::recording::capture_panics();
    let mut sim = Sim::named(&format!("fuzz-{:016x}", plan.seed));
    let mut session = Session {
        plan,
        verbose,
        at: At::Setup,
        coverage: Coverage::default(),
        course_trace: None,
        end_probe: Vec::new(),
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
    Err(Box::new(Finding {
        seed: plan.seed,
        check,
        at: session.at,
        at_ms: sim.now(),
        detail,
        course_trace: session.course_trace,
        world_log,
        end_probe: session.end_probe,
    }))
}

struct Session<'a> {
    plan: &'a Plan,
    verbose: bool,
    at: At,
    coverage: Coverage,
    course_trace: Option<PathBuf>,
    end_probe: Vec<script::Call>,
}

type Failed = (&'static str, String);

impl Session<'_> {
    fn play(&mut self, sim: &mut Sim, corpus: &Corpus) -> Result<(), Failed> {
        let setup = &self.plan.setup;
        let card = setup.card == Card::Ready;
        let (road, course_trace) = road(&setup.course, corpus);
        self.course_trace = course_trace;
        for call in script::prepare(setup, road) {
            call.run(sim);
        }
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
            script::step(step).run(sim);
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
        let mut probe = Probe {
            sim,
            ran: &mut self.end_probe,
        };
        match still_a_bike_computer(&mut probe, setup) {
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

fn button_name(button: Role) -> &'static str {
    match button {
        Role::Back => "Back",
        Role::Move => "Move",
        Role::Act => "Act",
    }
}

/// The road `course` rides, and the corpus trace it came from.
pub fn road(course: &Course, corpus: &Corpus) -> (script::Road, Option<PathBuf>) {
    let mut from = None;
    let road = script::Road::of(course, &mut |pick| {
        let (path, route) = corpus.route(pick)?;
        from = Some(path.clone());
        Some(route.clone())
    });
    (road, from)
}

const STARTABLE: [&str; 3] = ["ready", "saved", "recovered"];

/// The end probe's hold on the simulator: each call it makes is kept in
/// the session before it runs, so a scenario prints what it did, even
/// when a call panics.
struct Probe<'s> {
    sim: &'s mut Sim,
    ran: &'s mut Vec<script::Call>,
}

impl Probe<'_> {
    fn run(&mut self, call: script::Call) {
        self.ran.push(call.clone());
        call.run(self.sim);
    }

    fn command(&mut self, line: &'static str) -> String {
        self.ran.push(script::Call::Command(line));
        self.sim.command(line).1
    }

    fn ride(&mut self, action: plan::RideAction) -> (&'static str, String) {
        self.ran.push(script::Call::Host(action));
        self.sim.command_after_reading_outcome(action.command())
    }
}

fn still_a_bike_computer(probe: &mut Probe, setup: &Setup) -> Verdict {
    use plan::RideAction;
    use script::Call;
    for sensor in &setup.sensors {
        probe.run(Call::Present(*sensor, true));
    }
    probe.run(Call::GpsFix(true));
    probe.run(Call::Barometer(true));
    probe.run(Call::Battery {
        percent: 80,
        charging: false,
    });
    probe.run(Call::RideAt {
        kmh_tenths: 0,
        watts: 0,
        bpm: 72,
        rpm: 0,
    });
    probe.run(Call::ClearStorageFault);
    probe.run(Call::CutPower);
    probe.run(Call::LeaveBootPage(BOOT_PAGE_DEADLINE_MS));
    let sim = &mut *probe.sim;
    if sim.is_off() || !sim.device.lit() || sim.vana_page() == "boot" {
        return Verdict::Fail(format!(
            "not past the boot page {} s after power-on; screen:\n{}",
            BOOT_PAGE_DEADLINE_MS / 1000,
            sim.screen().text()
        ));
    }
    probe.run(Call::Advance(seconds(5)));
    let status = probe.command("RIDE STATUS");
    let state = status_field(&status, "state").unwrap_or_default();
    if !STARTABLE.contains(&state) {
        return Verdict::Skip(match state {
            "full" => "journal full",
            "error" => "recorder error",
            "needs_init" => "journal needs init",
            "corrupt" => "journal corrupt",
            _ => "recorder in another state",
        });
    }
    let known = (
        ids(&probe.sim.rides()),
        ids(&card_rides_if(probe.sim, setup)),
    );
    probe.run(Call::RideAt {
        kmh_tenths: 250,
        watts: 180,
        bpm: 130,
        rpm: 85,
    });
    let (answer, out) = probe.ride(RideAction::Start);
    if answer != "ACCEPTED" {
        return Verdict::Fail(format!("RIDE START answered {answer} {out} in {status}"));
    }
    probe.run(Call::Advance(seconds(30)));
    let recording = probe.command("RIDE STATUS");
    let samples: u32 = status_field(&recording, "samples")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    if status_field(&recording, "state") != Some("recording") || samples < 20 {
        return Verdict::Fail(format!("30 s after RIDE START: {recording}"));
    }
    let (answer, out) = probe.ride(RideAction::Finish);
    if answer != "ACCEPTED" {
        return Verdict::Fail(format!("RIDE FINISH answered {answer} {out}"));
    }
    probe.run(Call::Advance(seconds(5)));
    let saved_on = |rides: &[crate::rides::Ride], known: &[u32]| {
        rides
            .iter()
            .any(|r| !known.contains(&r.id) && r.kinds().last() == Some(&Kind::Finish))
    };
    let sim = &mut *probe.sim;
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

#[cfg(test)]
mod tests;
