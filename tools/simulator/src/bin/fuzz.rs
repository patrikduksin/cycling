use shell_simulator::bundle::{Bundle, Expect, Run};
use shell_simulator::fuzz::plan::Rng;
use shell_simulator::fuzz::{self, Corpus, Coverage, FIXED, Finding, PAGES, Plan};
use std::collections::{BTreeSet, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ChildStdout, Command, ExitCode, Stdio};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Each boot leaks the device blocks VANA takes as `&'static mut`, about
/// 2.2 MB a session, so a child serves this many sessions and exits: 33 MB
/// at most, against 0.05 s to start the next.
const SESSIONS_PER_CHILD: u32 = 8;

/// The stderr lines a dead child's finding shows.
const STDERR_TAIL: usize = 12;

const USAGE: &str = "\
usage: cycling fuzz [--minutes N | --seed S | --seeds A-B | --fixed]
                    [--corpus PATH | --no-corpus] [--jobs N] [--out DIR]

Each session draws a plan from its seed: a course (synthetic, or a GPS track
from a world log under --corpus), sensors, a card, then button presses, rides,
sensor dropouts, GPS and barometer loss, radar vehicles, battery changes,
power cuts, torn flash writes and card refusals. After every step the checks
read the device; a panic is a finding too, and so is a crash that ends a
session's process. A finding prints the seed and the command that reproduces
it, and writes the plan, the world log of its boot and the corpus trace it
rode under --out DIR/<seed>, with finding.bundle: the finding as one file
`cycling bundle run` reproduces on another machine.

  --minutes N   fresh seeds for N minutes of wall time (the default, 1)
  --seed S      one session, printing each step and the page after it
  --seeds A-B   the seeds A to B
  --fixed       the fixed seed set tier 2 runs, with --no-corpus
  --corpus PATH courses from a .trace file, or every .trace under a
                directory; missing means none
  --no-corpus   no corpus, even with --corpus
  --jobs N      sessions side by side (default: half the cores)
  --out DIR     where findings are written

Seeds take decimal or 0x hex. Exit 1 on any finding, 2 on bad arguments.";

enum Seeds {
    One(u64),
    Range(u64, u64),
    Minutes(u64),
    /// A child of a run: the seeds its parent writes to stdin.
    Stdin,
}

struct Args {
    seeds: Seeds,
    no_corpus: bool,
    corpus: Option<PathBuf>,
    jobs: usize,
    out: Option<PathBuf>,
}

fn seed(text: &str) -> Result<u64, String> {
    match text.strip_prefix("0x") {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => text.parse(),
    }
    .map_err(|_| format!("not a seed: {text}"))
}

fn parse(mut argv: impl Iterator<Item = String>) -> Result<Args, String> {
    let mut args = Args {
        seeds: Seeds::Minutes(1),
        no_corpus: false,
        corpus: None,
        jobs: std::thread::available_parallelism().map_or(1, |n| (n.get() / 2).max(1)),
        out: None,
    };
    while let Some(flag) = argv.next() {
        let mut value = || argv.next().ok_or(format!("{flag} needs a value"));
        match flag.as_str() {
            "--minutes" => {
                let minutes = value()?;
                args.seeds = Seeds::Minutes(
                    minutes
                        .parse()
                        .map_err(|_| format!("not minutes: {minutes}"))?,
                );
            }
            "--seed" => args.seeds = Seeds::One(seed(&value()?)?),
            "--seeds" => {
                let range = value()?;
                let (a, b) = range
                    .split_once('-')
                    .ok_or(format!("not a range: {range}"))?;
                let (first, last) = (seed(a)?, seed(b)?);
                if first > last {
                    return Err(format!("an empty range: {range}"));
                }
                args.seeds = Seeds::Range(first, last);
            }
            "--fixed" => {
                args.seeds = Seeds::Range(*FIXED.start(), *FIXED.end());
                args.no_corpus = true;
            }
            "--corpus" => args.corpus = Some(value()?.into()),
            "--no-corpus" => args.no_corpus = true,
            "--jobs" => {
                let jobs = value()?;
                args.jobs = jobs
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or(format!("not a job count: {jobs}"))?;
            }
            "--out" => args.out = Some(value()?.into()),
            "--child" => args.seeds = Seeds::Stdin,
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    if argv == ["--build-hash"] {
        println!("{}", env!("CYCLING_BUILD_HASH"));
        return ExitCode::SUCCESS;
    }
    let args = match parse(argv.into_iter()) {
        Ok(args) => args,
        Err(why) => {
            if !why.is_empty() {
                eprintln!("{why}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let corpus = if args.no_corpus {
        Corpus::none()
    } else {
        Corpus::load(args.corpus.as_deref())
    };
    if let Seeds::Stdin = args.seeds {
        serve(&corpus, args.out.as_deref());
        return ExitCode::SUCCESS;
    }
    if let Some(path) = &corpus.path {
        println!("corpus: {} courses from {}", corpus.len(), path.display());
        let mut reasons = std::collections::BTreeMap::<&str, usize>::new();
        for (_, why) in &corpus.skipped {
            *reasons.entry(why).or_default() += 1;
        }
        for (why, n) in reasons {
            println!("corpus: {n} traces without a course: {why}");
        }
    }
    let started = Instant::now();
    let report = Mutex::new(Vec::new());
    let coverage = match args.seeds {
        Seeds::One(seed) => {
            let plan = fuzz::draw(seed);
            match on_session_thread(|| fuzz::run(&plan, &corpus, true)) {
                Ok(coverage) => {
                    println!("pass");
                    coverage
                }
                Err(finding) => {
                    let text = found(finding, &plan, args.out.as_deref());
                    reported(&report, plan.seed, &text);
                    Coverage::default()
                }
            }
        }
        Seeds::Range(first, last) => {
            let seeds = Mutex::new(first..=last);
            pool(&args, &corpus, &report, || {
                seeds.lock().unwrap_or_else(|e| e.into_inner()).next()
            })
        }
        Seeds::Minutes(minutes) => {
            let deadline = started + Duration::from_secs(minutes * 60);
            let base = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64);
            let next = AtomicU64::new(0);
            pool(&args, &corpus, &report, || {
                let index = next.fetch_add(1, Ordering::Relaxed);
                (Instant::now() < deadline).then(|| Rng::new(base.wrapping_add(index)).next_u64())
            })
        }
        Seeds::Stdin => unreachable!("a child serves before this"),
    };
    let findings = report.into_inner().unwrap_or_else(|e| e.into_inner());
    if !matches!(args.seeds, Seeds::One(_)) {
        summary(&coverage, findings.len(), started.elapsed(), args.jobs);
    }
    if findings.is_empty() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

/// Runs every seed `next` gives in child processes, `args.jobs` at a time,
/// so a run's memory stays that of a few children.
fn pool(
    args: &Args,
    corpus: &Corpus,
    report: &Mutex<Vec<u64>>,
    next: impl Fn() -> Option<u64> + Sync,
) -> Coverage {
    let exe = std::env::current_exe().expect("the fuzz binary finds itself");
    let spawn = || {
        let mut command = Command::new(&exe);
        command.arg("--child");
        if args.no_corpus {
            command.arg("--no-corpus");
        } else if let Some(path) = &args.corpus {
            command.arg("--corpus").arg(path);
        }
        if let Some(out) = &args.out {
            command.arg("--out").arg(out);
        }
        Child::spawn(command)
    };
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..args.jobs)
            .map(|_| {
                scope.spawn(|| {
                    let mut coverage = Coverage::default();
                    let mut child: Option<Child> = None;
                    while let Some(seed) = next() {
                        let mut serving = match child.take() {
                            Some(child) if child.sessions < SESSIONS_PER_CHILD => child,
                            spent => {
                                if let Some(spent) = spent {
                                    spent.finish();
                                }
                                spawn()
                            }
                        };
                        match serving.session(seed) {
                            Outcome::Pass(session) => coverage.merge(session),
                            Outcome::Found(text) => reported(report, seed, &text),
                            Outcome::Died(death) => {
                                let text = serving.died(seed, death, corpus, args.out.as_deref());
                                reported(report, seed, &text);
                                continue;
                            }
                        }
                        child = Some(serving);
                    }
                    if let Some(child) = child {
                        child.finish();
                    }
                    coverage
                })
            })
            .collect();
        let mut total = Coverage::default();
        for worker in workers {
            total.merge(
                worker
                    .join()
                    .expect("a worker never panics outside a session"),
            );
        }
        total
    })
}

/// What a child wrote for one seed.
enum Outcome {
    Pass(Coverage),
    /// The finding as the child printed it.
    Found(String),
    /// The child exited before the seed's record ended.
    Died(Death),
}

/// How far a dead child got with its seed.
enum Death {
    BeforeStart,
    /// The corpus trace the session rode, if any.
    Riding(Option<PathBuf>),
}

/// A `--child` process and the pipes to it.
///
/// A child reads one decimal seed per line until EOF and writes a record per
/// seed, one tab-separated line each:
///
///   start  SEED  COURSE          (COURSE: the corpus trace it rides, or -)
///   press  PAGE  BUTTON  N       (any number)
///   skip   CHECK  WHY  N         (any number)
///   pass   SIMULATED_MS  WORLD_LOG_DIGEST
///
/// or, for a finding, `start`, then `say  LINE` for each line it printed and
/// `found`.
struct Child {
    process: std::process::Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    stderr: JoinHandle<VecDeque<String>>,
    sessions: u32,
}

impl Child {
    fn spawn(mut command: Command) -> Self {
        let mut process = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("the fuzz binary starts a child of itself");
        let stderr = BufReader::new(process.stderr.take().expect("piped"));
        let stderr = std::thread::spawn(move || {
            let mut stderr = stderr;
            let mut tail = VecDeque::new();
            while let Some(line) = read_line(&mut stderr) {
                let _ = writeln!(std::io::stderr().lock(), "{line}");
                if line.trim().is_empty() {
                    continue;
                }
                if tail.len() == STDERR_TAIL {
                    tail.pop_front();
                }
                tail.push_back(line);
            }
            tail
        });
        Self {
            stdin: process.stdin.take().expect("piped"),
            stdout: BufReader::new(process.stdout.take().expect("piped")),
            process,
            stderr,
            sessions: 0,
        }
    }

    fn session(&mut self, seed: u64) -> Outcome {
        self.sessions += 1;
        if writeln!(self.stdin, "{seed}").is_err() {
            return Outcome::Died(Death::BeforeStart);
        }
        let mut death = Death::BeforeStart;
        let mut coverage = Coverage::default();
        let mut said = String::new();
        while let Some(line) = read_line(&mut self.stdout) {
            if let Some(text) = line.strip_prefix("say\t") {
                said.push_str(text);
                said.push('\n');
                continue;
            }
            if let Some(record) = line.strip_prefix("start\t") {
                let (started, course) = record
                    .split_once('\t')
                    .unwrap_or_else(|| panic!("a fuzz child wrote {line:?} for a start"));
                let started = number(started);
                assert_eq!(
                    started, seed,
                    "a fuzz child started seed {started:#x} while its parent sent {seed:#x}"
                );
                death = Death::Riding((course != "-").then(|| course.into()));
                continue;
            }
            match line.split('\t').collect::<Vec<_>>()[..] {
                ["press", page, button, n] => {
                    *coverage
                        .presses
                        .entry((name(page), name(button)))
                        .or_default() += number(n);
                }
                ["skip", check, why, n] => {
                    *coverage.skips.entry((name(check), name(why))).or_default() += number(n);
                }
                ["pass", simulated_ms, digest] => {
                    coverage.sessions = 1;
                    coverage.simulated_ms = number(simulated_ms);
                    coverage.world_log_digests = number(digest);
                    return Outcome::Pass(coverage);
                }
                ["found"] => return Outcome::Found(said.trim_end().to_string()),
                _ => println!("{line}"),
            }
        }
        Outcome::Died(death)
    }

    fn finish(mut self) {
        drop(self.stdin);
        let status = self.process.wait().expect("a child is waited on once");
        let _ = self.stderr.join();
        if !status.success() {
            eprintln!("fuzz: a child exited with {status} after its sessions");
        }
    }

    /// The finding for `seed`, whose record a dead child never ended.
    fn died(mut self, seed: u64, death: Death, corpus: &Corpus, out: Option<&Path>) -> String {
        drop(self.stdin);
        let status = self.process.wait().expect("a child is waited on once");
        let tail = self.stderr.join().unwrap_or_default();
        let (course, when) = match death {
            Death::Riding(course) => (course, ""),
            // The child never drew this seed's course; the parent's corpus is
            // the nearest guess at it.
            Death::BeforeStart => (
                fuzz::route(&fuzz::draw(seed).setup.course, corpus).1,
                ", before its session started",
            ),
        };
        let mut text =
            format!("FAIL seed={seed:#018x} check=\"the process died\" ({status}{when})\n");
        for line in tail {
            text.push_str(&format!("  {line}\n"));
        }
        let (course, wrote) = match (course, out) {
            (Some(trace), Some(out)) => {
                let dir = finding_dir(out, seed);
                match keep_course(&dir, &trace) {
                    Ok(copy) => (Some(copy), format!("\n  wrote {}", dir.display())),
                    Err(error) => (
                        Some(trace),
                        format!("\n  could not write {}: {error}", dir.display()),
                    ),
                }
            }
            (course, _) => (course, String::new()),
        };
        text.push_str(&format!(
            "  reproduce: {}{wrote}",
            fuzz::reproduce(seed, course.as_deref())
        ));
        text
    }
}

fn read_line(reader: &mut impl BufRead) -> Option<String> {
    let mut bytes = Vec::new();
    match reader.read_until(b'\n', &mut bytes) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(
            String::from_utf8_lossy(&bytes)
                .trim_end_matches(['\n', '\r'])
                .to_string(),
        ),
    }
}

fn number(text: &str) -> u64 {
    text.parse()
        .unwrap_or_else(|_| panic!("a fuzz child wrote {text:?} for a number"))
}

/// `text` as the `&'static str` a coverage key holds; a run meets a few dozen
/// distinct names, each kept once.
fn name(text: &str) -> &'static str {
    static NAMES: Mutex<BTreeSet<&'static str>> = Mutex::new(BTreeSet::new());
    let mut names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(known) = names.get(text) {
        return known;
    }
    let leaked: &'static str = Box::leak(text.into());
    names.insert(leaked);
    leaked
}

/// A child's side: the seeds on stdin, a record each on stdout.
fn serve(corpus: &Corpus, out: Option<&Path>) {
    on_session_thread(|| {
        for line in std::io::stdin().lock().lines() {
            let line = line.expect("the parent writes text");
            let seed: u64 = line
                .parse()
                .unwrap_or_else(|_| panic!("the parent wrote {line:?} for a seed"));
            let plan = fuzz::draw(seed);
            let course = fuzz::route(&plan.setup.course, corpus).1;
            let course = course.map_or("-".into(), |trace| trace.display().to_string());
            println!("start\t{seed}\t{course}");
            match fuzz::run(&plan, corpus, false) {
                Ok(coverage) => {
                    for ((page, button), n) in &coverage.presses {
                        println!("press\t{page}\t{button}\t{n}");
                    }
                    for ((check, why), n) in &coverage.skips {
                        println!("skip\t{check}\t{why}\t{n}");
                    }
                    println!(
                        "pass\t{}\t{}",
                        coverage.simulated_ms, coverage.world_log_digests
                    );
                }
                Err(finding) => {
                    for line in found(finding, &plan, out).lines() {
                        println!("say\t{line}");
                    }
                    println!("found");
                }
            }
        }
    });
}

fn reported(report: &Mutex<Vec<u64>>, seed: u64, text: &str) {
    let mut report = report.lock().unwrap_or_else(|e| e.into_inner());
    report.push(seed);
    println!("{text}");
}

/// Writes `finding`'s plan, world log and course under `out`, and returns
/// the finding as printed.
fn found(mut finding: Finding, plan: &Plan, out: Option<&Path>) -> String {
    let Some(out) = out else {
        return finding.to_string();
    };
    let dir = finding_dir(out, finding.seed);
    let written = (|| -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("plan.txt"), format!("{plan:#?}\n"))?;
        std::fs::write(dir.join("world.trace"), &finding.world_log.0)?;
        if let Some(trace) = &finding.course_trace {
            finding.course_trace = Some(keep_course(&dir, trace)?);
        }
        Ok(())
    })();
    if let Err(error) = written {
        return format!("{finding}\n  could not write {}: {error}", dir.display());
    }
    let bundled = Bundle {
        out: &dir.join("finding.bundle"),
        trace: &dir.join("world.trace"),
        build: env!("CYCLING_BUILD_HASH"),
        run: Run::Fuzz {
            seed: finding.seed,
            course: finding.course_trace.is_some(),
            debug_assertions: cfg!(debug_assertions),
        },
        expect: Expect::Found {
            check: finding.check,
            at: finding.at.to_string(),
            detail: finding.detail.split('\n').next().unwrap_or_default().into(),
        },
        course: finding.course_trace.as_deref(),
        pack: None,
    }
    .write();
    format!("{finding}\n  wrote {}\n  {bundled}", dir.display())
}

fn finding_dir(out: &Path, seed: u64) -> PathBuf {
    out.join(format!("{seed:#018x}"))
}

/// Copies `trace` into `dir` and returns the copy, the corpus the finding's
/// reproduce line names.
fn keep_course(dir: &Path, trace: &Path) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let course = std::path::absolute(dir.join("course.trace"))?;
    // A replay's --corpus names this very file, and a copy onto itself would
    // truncate it before reading.
    let staged = dir.join("course.trace.new");
    std::fs::copy(trace, &staged)?;
    std::fs::rename(&staged, &course)?;
    Ok(course)
}

/// Runs `session` on a spawned thread with the default settings, the thread
/// a child serves on, so a deep recursion overflows its stack at the same
/// depth under --seed as in a run.
fn on_session_thread<T: Send>(session: impl FnOnce() -> T + Send) -> T {
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .spawn_scoped(scope, session)
            .expect("a session thread starts")
            .join()
            .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
    })
}

fn summary(coverage: &Coverage, findings: usize, wall: Duration, jobs: usize) {
    let seconds = wall.as_secs_f64();
    println!(
        "fuzz: {} sessions, {:.1} h simulated, {findings} findings, {seconds:.0} s on {jobs} processes ({:.1} sessions/s)",
        coverage.sessions,
        coverage.simulated_ms as f64 / 3_600_000.0,
        coverage.sessions as f64 / seconds.max(0.001),
    );
    println!("world logs: {:016x}", coverage.world_log_digests);
    let pressed: Vec<_> = PAGES
        .iter()
        .filter(|page| coverage.presses.keys().any(|(p, _)| p == *page))
        .collect();
    let never: Vec<_> = PAGES
        .iter()
        .filter(|p| !pressed.contains(p))
        .copied()
        .collect();
    println!(
        "coverage: presses on {}/{} pages; never pressed on: {}",
        pressed.len(),
        PAGES.len(),
        if never.is_empty() {
            "none".to_string()
        } else {
            never.join(", ")
        }
    );
    for page in pressed {
        let count = |button| coverage.presses.get(&(*page, button)).copied().unwrap_or(0);
        println!(
            "  {page:<13} TopLeft {:>6}  BottomLeft {:>6}  BottomRight {:>6}",
            count("TopLeft"),
            count("BottomLeft"),
            count("BottomRight")
        );
    }
    if coverage.skips.is_empty() {
        println!("skipped: none");
    }
    for ((check, why), n) in &coverage.skips {
        println!("skipped: {check}: {why} x{n}");
    }
}
