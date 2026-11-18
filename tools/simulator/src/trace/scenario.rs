//! `trace cut --scenario`: a fixture's slice lifted to a fuzz plan, run in
//! the simulator, and printed as the Rust test that runs it. The tool
//! writes the test only when its expectation fails in that run, so a
//! printed scenario is red on the bug by construction. With
//! `--synthetic-course` the recorded track gives way to a made-up course
//! of the same length, and the same check runs.
use super::fixture::{Class, Fixture, Judged, Verdict, judged_with_keeps};
use crate::fuzz::lift::{lift, synthetic};
use crate::fuzz::plan::Course;
use crate::fuzz::script::{self, Road};
use crate::sim::Sim;
use serde::Serialize;
use std::collections::BTreeMap;
use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;

#[derive(Debug, Serialize)]
pub struct Scenario {
    pub test: String,
    pub steps: usize,
    pub synthetic: bool,
    /// The test rides the recorded track, so it stays private.
    pub private: bool,
    pub left_out: BTreeMap<String, usize>,
    pub judged: Vec<Judged>,
}

impl fmt::Display for Scenario {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.test)
    }
}

/// The scenario of the fixture at `dir`, as a test named `name`.
pub fn scenario(dir: &Path, name: &str, synthetic_course: bool) -> Result<Scenario, String> {
    let fixture = Fixture::read(dir)?;
    let lifted = lift(&fixture.slice)?;
    let mut plan = lifted.plan;
    if synthetic_course {
        plan.setup.course = synthetic(&plan.setup.course);
    }
    let private = matches!(plan.setup.course, Course::Track { .. });
    let calls = script::calls(&plan, Road::of(&plan.setup.course, &mut |_| None));
    crate::recording::capture_panics();
    let expect = &fixture.expect;
    let ran = catch_unwind(AssertUnwindSafe(|| {
        let mut sim = Sim::new();
        for call in &calls {
            call.run(&mut sim);
        }
        judged_with_keeps(&mut sim, expect)
    }));
    let judged = match ran {
        Ok(judged) => judged,
        Err(_) => {
            let why = crate::recording::take_panic().unwrap_or_default();
            return Err(format!("the lifted scenario panicked: {why}"));
        }
    };
    let verdict = Verdict::Judged(judged.clone());
    let left: Vec<String> = (lifted.left_out.iter())
        .map(|(what, n)| format!("{what} ({n})"))
        .collect();
    if verdict.class() != Class::Reproduced {
        return Err(format!(
            "the lifted scenario does not fail as the fixture does, so the fixture stays one: {verdict}; the lift left out: {}",
            if left.is_empty() {
                "nothing".into()
            } else {
                left.join(", ")
            }
        ));
    }
    let seen: Vec<String> = expect
        .seen
        .iter()
        .map(|e| format!("{:?}", e.to_string()))
        .collect();
    let check = match expect.check {
        Some(check) => format!("Some({check:?})"),
        None => "None".into(),
    };
    let keep: Vec<String> = expect
        .keep
        .iter()
        .map(|e| format!("{:?}", e.to_string()))
        .collect();
    let then = format!(
        "    shell_simulator::trace::fixture::expect(&mut sim, &[{}], {check}, &[{}]);\n",
        seen.join(", "),
        keep.join(", ")
    );
    Ok(Scenario {
        test: script::print(name, &calls, &then),
        steps: plan.steps.len(),
        synthetic: synthetic_course,
        private,
        left_out: lifted.left_out,
        judged,
    })
}
