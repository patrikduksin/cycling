//! A finding made small: [`ddmin`] over the plan's steps, with "the same
//! check fails" as the oracle; then the world log of the shrunk plan's
//! failing boot, cut as a fixture and shrunk by `trace shrink`; then the
//! shrunk plan printed as a scenario test that fails the same check.
use super::{At, Corpus, Finding, Plan, run, script};
use crate::ddmin::ddmin;
use crate::sim::Sim;
use crate::trace::fixture::{CHECKED, Class, Expectation, Fixture, Verdict, judged_at_end, slice};
use crate::trace::shrink::{Shrunk as Sliced, shrink as shrink_fixture};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::time::{Duration, Instant};

#[derive(Debug)]
pub struct Shrunk {
    pub plan: Plan,
    /// The shrunk plan's own finding: the same check, failed again.
    pub finding: Finding,
    /// No single step more can go.
    pub minimal: bool,
    pub tried: usize,
}

/// `plan`'s finding of `check` with as few steps as still fail it.
pub fn plan(plan: &Plan, corpus: &Corpus, check: &str, budget: Duration) -> Option<Shrunk> {
    let fails = |steps: &[super::plan::Step]| {
        let candidate = Plan {
            steps: steps.to_vec(),
            ..plan.clone()
        };
        run(&candidate, corpus, false)
            .err()
            .filter(|finding| finding.check == check)
    };
    let shrunk = ddmin(plan.steps.clone(), Some(Instant::now() + budget), |steps| {
        fails(steps).is_some()
    });
    let finding = fails(&shrunk.kept)?;
    Some(Shrunk {
        plan: Plan {
            steps: shrunk.kept,
            ..plan.clone()
        },
        finding: *finding,
        minimal: shrunk.minimal,
        tried: shrunk.tried,
    })
}

/// The finding's world log from its boot's power-on to its last pass, as a
/// fixture of its check, written to `dir` and shrunk there. `None` for a
/// check no replay judges.
pub fn world_slice(
    finding: &Finding,
    dir: &Path,
    budget: Duration,
) -> Option<Result<Sliced, String>> {
    let check = CHECKED.into_iter().find(|c| *c == finding.check)?;
    let made = (|| {
        let (slice, at_ms) = slice(&finding.world_log.0, u64::MAX)?;
        let fixture = Fixture {
            slice,
            expect: Expectation::new(at_ms, Vec::new(), Some(check), Vec::new())?,
        };
        let verdict = fixture.judge();
        if verdict.class() != Class::Reproduced {
            return Err(format!(
                "the world log alone does not fail {check}: {verdict}"
            ));
        }
        fixture.write(dir)?;
        shrink_fixture(dir, budget)
    })();
    Some(made)
}

/// The shrunk plan as a test named `name`: the calls its session made up to
/// the finding, the end probe's among them for a finding at the end, then
/// the check. Refused unless running those calls fails the check, so a
/// printed scenario is red on the bug by construction.
pub fn scenario(shrunk: &Shrunk, corpus: &Corpus, name: &str) -> Result<String, String> {
    let finding = &shrunk.finding;
    let check = CHECKED.into_iter().find(|c| *c == finding.check);
    if check.is_none() && finding.check != "no panic" {
        return Err(format!(
            "{} is judged between steps, which a scenario's end cannot judge",
            finding.check
        ));
    }
    let (road, _) = super::road(&shrunk.plan.setup.course, corpus);
    let mut calls = script::prepare(&shrunk.plan.setup, road);
    let steps = match finding.at {
        At::Setup => &[][..],
        At::Step(index) => &shrunk.plan.steps[..=index],
        At::End => &shrunk.plan.steps[..],
    };
    calls.extend(steps.iter().map(script::step));
    calls.extend(finding.end_probe.iter().cloned());
    crate::recording::capture_panics();
    let ran = catch_unwind(AssertUnwindSafe(|| {
        let mut sim = Sim::new();
        for call in &calls {
            call.run(&mut sim);
        }
        check.map(|check| judged_at_end(&mut sim, &[], Some(check)))
    }));
    let failed = match (ran, check) {
        (Err(_), None) => Ok(()),
        (Err(_), Some(_)) => Err(format!(
            "it panics: {}",
            crate::recording::take_panic().unwrap_or_default()
        )),
        (Ok(_), None) => Err("it runs without a panic".into()),
        (Ok(judged), Some(_)) => {
            let judged = judged.unwrap_or_default();
            match judged.iter().find(|j| !j.held) {
                Some(_) => Ok(()),
                None => Err(format!("{}", Verdict::Judged(judged))),
            }
        }
    };
    failed.map_err(|why| {
        format!(
            "the printed scenario does not fail {} as the finding did: {why}",
            finding.check
        )
    })?;
    let then = match check {
        Some(check) => format!(
            "    shell_simulator::trace::fixture::expect(&mut sim, &[], Some({check:?}), &[]);\n"
        ),
        None => String::new(),
    };
    Ok(script::print(name, &calls, &then))
}
