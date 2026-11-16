use shell_simulator::fuzz::plan::Rng;
use shell_simulator::fuzz::{self, Corpus, Coverage, FIXED, Finding, PAGES, Plan};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

const USAGE: &str = "\
usage: cycling fuzz [--minutes N | --seed S | --seeds A-B | --fixed]
                    [--corpus DIR] [--jobs N] [--out DIR]

Each session draws a plan from its seed: a course (synthetic, or a GPS track
from a world log under --corpus), sensors, a card, then button presses, rides,
sensor dropouts, GPS and barometer loss, radar vehicles, battery changes,
power cuts, torn flash writes and card refusals. After every step the checks
read the device; a panic is a finding too. A finding prints the seed and the
command that reproduces it, and writes the plan and the world log of its boot
under --out DIR/<seed>.

  --minutes N   fresh seeds for N minutes of wall time (the default, 1)
  --seed S      one session, printing each step and the page after it
  --seeds A-B   the seeds A to B
  --fixed       the fixed seed set tier 2 runs, on synthetic courses only
  --corpus DIR  courses from every .trace under DIR; missing means none
  --jobs N      sessions side by side (default: half the cores)
  --out DIR     where findings are written

Seeds take decimal or 0x hex. Exit 1 on any finding, 2 on bad arguments.";

enum Seeds {
    One(u64),
    Range(u64, u64),
    Minutes(u64),
}

struct Args {
    seeds: Seeds,
    fixed: bool,
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
        fixed: false,
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
                args.seeds = Seeds::Range(seed(a)?, seed(b)?);
            }
            "--fixed" => {
                args.seeds = Seeds::Range(*FIXED.start(), *FIXED.end());
                args.fixed = true;
            }
            "--corpus" => args.corpus = Some(value()?.into()),
            "--jobs" => {
                let jobs = value()?;
                args.jobs = jobs
                    .parse()
                    .ok()
                    .filter(|n| *n > 0)
                    .ok_or(format!("not a job count: {jobs}"))?;
            }
            "--out" => args.out = Some(value()?.into()),
            "-h" | "--help" => return Err(String::new()),
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(args)
}

fn main() -> ExitCode {
    let args = match parse(std::env::args().skip(1)) {
        Ok(args) => args,
        Err(why) => {
            if !why.is_empty() {
                eprintln!("{why}");
            }
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let corpus = if args.fixed {
        Corpus::none()
    } else {
        Corpus::load(args.corpus.as_deref())
    };
    if let Some(dir) = &corpus.dir {
        println!("corpus: {} courses from {}", corpus.len(), dir.display());
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
            match fuzz::run(&plan, &corpus, true) {
                Ok(coverage) => {
                    println!("pass");
                    coverage
                }
                Err(finding) => {
                    found(&finding, &plan, args.out.as_deref(), &report);
                    Coverage::default()
                }
            }
        }
        Seeds::Range(first, last) => {
            let next = AtomicU64::new(first);
            pool(args.jobs, &corpus, args.out.as_deref(), &report, || {
                let seed = next.fetch_add(1, Ordering::Relaxed);
                (seed <= last).then_some(seed)
            })
        }
        Seeds::Minutes(minutes) => {
            let deadline = started + Duration::from_secs(minutes * 60);
            let base = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_nanos() as u64);
            let next = AtomicU64::new(0);
            pool(args.jobs, &corpus, args.out.as_deref(), &report, || {
                let index = next.fetch_add(1, Ordering::Relaxed);
                (Instant::now() < deadline).then(|| Rng::new(base.wrapping_add(index)).next_u64())
            })
        }
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

fn pool(
    jobs: usize,
    corpus: &Corpus,
    out: Option<&Path>,
    report: &Mutex<Vec<u64>>,
    next: impl Fn() -> Option<u64> + Sync,
) -> Coverage {
    std::thread::scope(|scope| {
        let workers: Vec<_> = (0..jobs)
            .map(|_| {
                scope.spawn(|| {
                    let mut coverage = Coverage::default();
                    while let Some(seed) = next() {
                        let plan = fuzz::draw(seed);
                        match fuzz::run(&plan, corpus, false) {
                            Ok(session) => coverage.merge(session),
                            Err(finding) => found(&finding, &plan, out, report),
                        }
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

fn found(finding: &Finding, plan: &Plan, out: Option<&Path>, report: &Mutex<Vec<u64>>) {
    let mut report = report.lock().unwrap_or_else(|e| e.into_inner());
    report.push(finding.seed);
    println!("{finding}");
    let Some(out) = out else {
        return;
    };
    let dir = out.join(format!("{:#018x}", finding.seed));
    let written = std::fs::create_dir_all(&dir)
        .and_then(|()| std::fs::write(dir.join("plan.txt"), format!("{plan:#?}\n")))
        .and_then(|()| std::fs::write(dir.join("world.trace"), &finding.world_log.0));
    match written {
        Ok(()) => println!(
            "  wrote {} and {}",
            dir.join("plan.txt").display(),
            dir.join("world.trace").display()
        ),
        Err(error) => println!("  could not write {}: {error}", dir.display()),
    }
}

fn summary(coverage: &Coverage, findings: usize, wall: Duration, jobs: usize) {
    let seconds = wall.as_secs_f64();
    println!(
        "fuzz: {} sessions, {:.1} h simulated, {findings} findings, {seconds:.0} s on {jobs} threads ({:.1} sessions/s)",
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
    println!("broken(): not checked yet (#397 0b)");
}
