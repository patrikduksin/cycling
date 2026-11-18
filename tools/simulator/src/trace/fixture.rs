//! A fixture: a world slice from power-on, `slice.trace`, and what the
//! rider should have seen at its last pass, `expect.toml`. One generic test
//! replays every fixture as a changed build and asserts the expectation, so
//! a fixture fails while the bug is there and passes once it is fixed.
//!
//! Simulator fixtures are committed under [`PUBLIC`]. A device trace holds
//! where the rider rode, so its fixtures stay private, in the primary
//! checkout's [`PRIVATE`].
use crate::fuzz::check::{CHECKS, Moment, Observation, Verdict as Checked};
use crate::replay::{Kind, Mode, Options};
use crate::sim::Sim;
use crate::trace::find::{Query, Test};
use crate::trace::timeline::{At, field, session_clock};
use firmware_services::trace::{self, Event};
use serde::Serialize;
use std::fmt;
use std::io::Write;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Component, Path, PathBuf};

/// Committed fixtures, from the checkout root.
pub const PUBLIC: &str = "tools/simulator/tests/traces";
/// Private fixtures, from the primary checkout's root.
pub const PRIVATE: &str = ".local/fixtures";

const SLICE: &str = "slice.trace";
const EXPECT: &str = "expect.toml";
const SLOTS: [&str; 3] = ["page", "toast", "ride"];

/// Fuzz checks a fixture can name: each judges one observation of the
/// replayed internal flash, so a replay that ends at the slice's last pass
/// can fail it.
pub const CHECKED: [&str; 2] = ["rides decode", "no garbage sample"];

/// What the rider should have seen: `power!=--`, `page=ride/climb`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expect {
    pub name: String,
    pub test: Test,
    pub value: String,
}

impl Expect {
    /// `field=value`, `field!=value` or `field~text`, for a shown field or
    /// page, toast or ride.
    pub fn parse(text: &str) -> Result<Self, String> {
        match Query::parse(text) {
            Query::Field { name, test, value } => Ok(Self { name, test, value }),
            _ => Err(format!(
                "--expect takes field=value, field!=value or field~text, not {text:?}"
            )),
        }
    }

    fn holds(&self, got: Option<&str>) -> bool {
        match self.test {
            Test::Is => got == Some(self.value.as_str()),
            Test::IsNot => got != Some(self.value.as_str()),
            Test::Contains => got.is_some_and(|got| got.contains(self.value.as_str())),
        }
    }
}

impl fmt::Display for Expect {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let operator = match self.test {
            Test::Is => "=",
            Test::IsNot => "!=",
            Test::Contains => "~",
        };
        write!(f, "{}{operator}{}", self.name, self.value)
    }
}

/// What a fixture asserts at the slice's last pass.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Expectation {
    pub at_ms: u64,
    pub seen: Vec<Expect>,
    /// A fuzz check of [`CHECKED`].
    pub check: Option<&'static str>,
    /// What held at the moment and must go on holding: the rest of the
    /// moment, so a shrink cannot keep the failure by another cause.
    pub keep: Vec<Expect>,
}

impl Expectation {
    pub fn new(
        at_ms: u64,
        seen: Vec<Expect>,
        check: Option<&str>,
        keep: Vec<Expect>,
    ) -> Result<Self, String> {
        let check = check
            .map(|name| {
                CHECKED
                    .into_iter()
                    .find(|known| *known == name)
                    .ok_or_else(|| {
                        format!("--check names one of {CHECKED:?}, which judge the replayed flash")
                    })
            })
            .transpose()?;
        if seen.is_empty() && check.is_none() {
            return Err("a fixture needs --expect or --check".into());
        }
        Ok(Self {
            at_ms,
            seen,
            check,
            keep,
        })
    }

    fn to_toml(&self) -> Result<String, String> {
        let quoted = |text: &str| {
            let plain = !text.contains(['"', '\\']) && !text.chars().any(char::is_control);
            plain.then(|| format!("\"{text}\"")).ok_or_else(|| {
                format!("{text:?}: an expectation holds no quote, backslash or control")
            })
        };
        let mut out = String::new();
        out += &format!("at = {}\n", quoted(&session_clock(self.at_ms).to_string())?);
        let seen: Vec<String> = (self.seen.iter())
            .map(|expect| quoted(&expect.to_string()))
            .collect::<Result<_, _>>()?;
        out += &format!("expect = [{}]\n", seen.join(", "));
        if !self.keep.is_empty() {
            let keep: Vec<String> = (self.keep.iter())
                .map(|expect| quoted(&expect.to_string()))
                .collect::<Result<_, _>>()?;
            out += &format!("keep = [{}]\n", keep.join(", "));
        }
        if let Some(check) = self.check {
            out += &format!("check = {}\n", quoted(check)?);
        }
        Ok(out)
    }

    fn from_toml(text: &str) -> Result<Self, String> {
        let string = |value: &str| {
            (value.strip_prefix('"').and_then(|v| v.strip_suffix('"')))
                .filter(|inner| !inner.contains('"'))
                .map(str::to_string)
                .ok_or_else(|| format!("{value} is no plain string"))
        };
        let (mut at, mut seen, mut check, mut keep) = (None, Vec::new(), None, Vec::new());
        for line in text.lines().map(str::trim) {
            if line.is_empty() || line.starts_with('#') {
                continue;
            }
            let (key, value) = line
                .split_once('=')
                .map(|(key, value)| (key.trim(), value.trim()))
                .ok_or_else(|| format!("{line:?} is no key = value"))?;
            match key {
                "at" => at = Some(string(value)?),
                "check" => check = Some(string(value)?),
                "expect" | "keep" => {
                    let inner = (value.strip_prefix('[').and_then(|v| v.strip_suffix(']')))
                        .ok_or_else(|| format!("{key} {value} is no list"))?;
                    let list = if key == "keep" { &mut keep } else { &mut seen };
                    for item in inner.split(',').map(str::trim).filter(|i| !i.is_empty()) {
                        list.push(Expect::parse(&string(item)?)?);
                    }
                }
                other => {
                    return Err(format!(
                        "no key {other:?}; a fixture has at, expect, check, keep"
                    ));
                }
            }
        }
        let at = at.ok_or("no at")?;
        let at_ms = match At::parse(&at)? {
            At::Session(ms) => ms,
            At::Ride(_) => {
                return Err(format!(
                    "at {at} is a ride time; a fixture names session time"
                ));
            }
        };
        Self::new(at_ms, seen, check.as_deref(), keep)
    }
}

/// One expectation as a replay judged it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Judged {
    pub expect: String,
    pub held: bool,
    pub got: String,
    /// A `keep`: it held at the cut and must go on holding.
    pub keep: bool,
}

/// A fixture's replay to its moment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// The replay did not reach the moment: it left the recorded world,
    /// panicked or the slice ended first. Nothing was judged.
    Invalid(String),
    Judged(Vec<Judged>),
}

/// How a candidate slice relates to a fixture's failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    Invalid,
    /// Every expectation fails, as it did on the fixture.
    Reproduced,
    Passing,
    /// It fails otherwise: some expectation holds, or a `keep` fails.
    Other,
}

impl Verdict {
    pub fn class(&self) -> Class {
        match self {
            Self::Invalid(_) => Class::Invalid,
            Self::Judged(judged) if judged.iter().all(|j| j.held) => Class::Passing,
            Self::Judged(judged) if judged.iter().all(|j| j.held == j.keep) => Class::Reproduced,
            Self::Judged(_) => Class::Other,
        }
    }
}

impl fmt::Display for Verdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(why) => write!(f, "invalid: {why}"),
            Self::Judged(judged) => {
                let lines: Vec<String> = (judged.iter())
                    .map(|j| {
                        let verdict = if j.held { "holds" } else { "fails" };
                        format!("{} {verdict}: got {}", j.expect, j.got)
                    })
                    .collect();
                f.write_str(&lines.join("; "))
            }
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Fixture {
    pub slice: Vec<u8>,
    pub expect: Expectation,
}

impl Fixture {
    pub fn read(dir: &Path) -> Result<Self, String> {
        let at = |name: &str| dir.join(name);
        let read =
            |path: PathBuf| std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()));
        let text = String::from_utf8(read(at(EXPECT))?).map_err(|e| e.to_string())?;
        let expect =
            Expectation::from_toml(&text).map_err(|e| format!("{}: {e}", at(EXPECT).display()))?;
        Ok(Self {
            slice: read(at(SLICE))?,
            expect,
        })
    }

    /// Write both files, each whole or not at all, so a shrink cut short
    /// leaves its best fixture. A staging file is made new, never opened,
    /// so one a crash left, or a symlink out of `dir`, is replaced rather
    /// than written through.
    pub fn write(&self, dir: &Path) -> Result<(), String> {
        let failed = |e: std::io::Error| format!("{}: {e}", dir.display());
        std::fs::create_dir_all(dir).map_err(failed)?;
        for (name, bytes) in [
            (EXPECT, self.expect.to_toml()?.into_bytes()),
            (SLICE, self.slice.clone()),
        ] {
            let staged = dir.join(format!(".{name}.new"));
            if let Err(e) = std::fs::remove_file(&staged)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(failed(e));
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&staged)
                .map_err(failed)?;
            file.write_all(&bytes).map_err(failed)?;
            std::fs::rename(&staged, dir.join(name)).map_err(failed)?;
        }
        Ok(())
    }

    pub fn judge(&self) -> Verdict {
        judge(&self.slice, &self.expect)
    }
}

/// Replay `slice` as a changed build to the expectation's moment and judge
/// it there.
pub fn judge(slice: &[u8], expect: &Expectation) -> Verdict {
    crate::recording::capture_panics();
    let changed = Options {
        mode: Mode::Changed,
        model: false,
    };
    let replayed = catch_unwind(AssertUnwindSafe(|| {
        let mut sim = Sim::replaying(slice, changed, None).map_err(|e| e.to_string())?;
        let reached = sim.replay_until(expect.at_ms);
        Ok::<_, String>((sim, reached))
    }));
    let (mut sim, reached) = match replayed {
        Ok(Ok(replayed)) => replayed,
        Ok(Err(why)) => return Verdict::Invalid(why),
        Err(_) => {
            let why = crate::recording::take_panic().unwrap_or_default();
            return Verdict::Invalid(format!("the replay panicked: {why}"));
        }
    };
    let report = sim.device.report().expect("a replay reports");
    if let Some(stop) = &report.stopped {
        let kind = if stop.kind == Kind::Panicked {
            "panicked"
        } else {
            "stopped"
        };
        return Verdict::Invalid(format!("{kind}: {}", stop.detail));
    }
    if !reached {
        return Verdict::Invalid(format!(
            "the slice ends before {}",
            session_clock(expect.at_ms)
        ));
    }
    Verdict::Judged(judged_with_keeps(&mut sim, expect))
}

/// Every expectation of `expect`, then every keep, judged on `sim`.
pub fn judged_with_keeps(sim: &mut Sim, expect: &Expectation) -> Vec<Judged> {
    let mut judged = judged_at_end(sim, &expect.seen, expect.check);
    let kept = judged_at_end(sim, &expect.keep, None);
    judged.extend(kept.into_iter().map(|j| Judged { keep: true, ..j }));
    judged
}

/// `seen` and `check` judged on `sim` as it stands.
pub fn judged_at_end(sim: &mut Sim, seen: &[Expect], check: Option<&str>) -> Vec<Judged> {
    let screen = sim.seen();
    let text = |slot: &Option<(u64, String)>| slot.as_ref().map(|(_, text)| text.clone());
    let shown = text(&screen.shown);
    let mut judged = Vec::new();
    for expected in seen {
        let got = match expected.name.as_str() {
            "page" => text(&screen.page),
            "toast" => text(&screen.toast),
            "ride" => text(&screen.ride),
            name => shown
                .as_deref()
                .and_then(|s| field(s, name))
                .map(str::to_string),
        };
        judged.push(Judged {
            expect: expected.to_string(),
            held: expected.holds(got.as_deref()),
            got: got.unwrap_or_else(|| "nothing".into()),
            keep: false,
        });
    }
    if let Some(name) = check {
        let check = CHECKS
            .iter()
            .find(|c| c.name == name)
            .expect("a known check");
        let after = Observation {
            at_ms: sim.now(),
            boot: sim.device.boot,
            on_since_ms: 0,
            page: "",
            off: false,
            journal: sim.rides(),
            card: crate::fuzz::check::card_rides(sim),
            faulted: false,
        };
        let moment = Moment {
            step: None,
            before: &after,
            after: &after,
        };
        let (held, got) = match (check.run)(&moment) {
            Checked::Fail(why) => (false, why),
            Checked::Pass => (true, "pass".into()),
            Checked::Skip(why) => (true, format!("skipped: {why}")),
        };
        judged.push(Judged {
            expect: format!("check {name}"),
            held,
            got,
            keep: false,
        });
    }
    judged
}

/// For a scenario `cut --scenario` printed: every expectation holds on
/// `sim` as it stands, or the test fails with what it got.
#[track_caller]
pub fn expect(sim: &mut Sim, seen: &[&str], check: Option<&str>, keep: &[&str]) {
    let parse = |list: &[&str]| list.iter().map(|e| Expect::parse(e).unwrap()).collect();
    let expect = Expectation::new(0, parse(seen), check, parse(keep)).unwrap();
    let judged = judged_with_keeps(sim, &expect);
    let failed: Vec<String> = (judged.iter())
        .filter(|j| !j.held)
        .map(|j| format!("{} fails: got {}", j.expect, j.got))
        .collect();
    assert!(failed.is_empty(), "{}", failed.join("; "));
}

/// `trace` from power-on through its last pass at or before `to_ms`, and
/// that pass's time. Records the device made after it, commands included,
/// are left out.
pub fn slice(trace: &[u8], to_ms: u64) -> Result<(Vec<u8>, u64), String> {
    let (header, events) = trace::decode(trace).map_err(|e| e.to_string())?;
    let mut kept: Vec<Event<'_>> = Vec::new();
    let mut last = None;
    for event in events {
        if let Event::Pass { now_ms } = event {
            if now_ms > to_ms {
                break;
            }
            last = Some((kept.len(), now_ms));
        }
        kept.push(event);
    }
    let (pass, at_ms) =
        last.ok_or_else(|| format!("no pass at or before {}", session_clock(to_ms)))?;
    if let Some(command) = kept[pass..]
        .iter()
        .position(|e| matches!(e, Event::Command(_)))
    {
        kept.truncate(pass + command);
    }
    Ok((super::surgery::encode(&header, kept), at_ms))
}

/// Where a fixture of `trace` named `name` belongs, and why: committed for
/// a simulator trace that holds no secret, private otherwise. `name` is one
/// plain path component. `out`, when given, replaces the default place; a
/// private fixture goes only where git ignores it, with `..` and symlinks
/// resolved first.
pub fn home(
    trace: &[u8],
    name: &str,
    out: Option<&Path>,
) -> Result<(PathBuf, &'static str), String> {
    let mut parts = Path::new(name).components();
    let plain = matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none();
    if !plain || name.contains(['/', '\\']) {
        return Err(format!(
            "--name {name:?}: a fixture's name is one plain name"
        ));
    }
    let device = crate::trace::world::opened(trace).redacted;
    let secrets = crate::trace::secret::secrets(trace);
    let (root, under, why) = match (device, secrets.is_empty()) {
        (true, _) => (
            primary_checkout()?,
            PRIVATE,
            "private: a device trace records where the rider rode",
        ),
        (false, false) => (
            primary_checkout()?,
            PRIVATE,
            "private: the trace holds a secret",
        ),
        (false, true) => (checkout()?, PUBLIC, "a simulator trace with no secret"),
    };
    let dir = resolved(&out.map_or_else(|| root.join(under).join(name), Path::to_path_buf))?;
    if under == PRIVATE && !ignored(&dir)? {
        return Err(format!(
            "{}: {why}, and git does not ignore it there, so it could be committed",
            dir.display()
        ));
    }
    Ok((dir, why))
}

/// `path` with every symlink and `..` resolved, as far as it exists, and
/// lexically beyond, which is where creating its directories puts it.
fn resolved(path: &Path) -> Result<PathBuf, String> {
    let absolute = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut at = PathBuf::new();
    for part in absolute.components() {
        match part {
            Component::ParentDir => {
                at.pop();
            }
            Component::CurDir => {}
            part => at.push(part),
        }
        if let Ok(real) = at.canonicalize() {
            at = real;
        }
    }
    Ok(at)
}

/// Git ignores `path`, or no checkout holds it.
fn ignored(path: &Path) -> Result<bool, String> {
    let existing = path.ancestors().find(|a| a.is_dir()).unwrap_or(path);
    let git = |args: &[&std::ffi::OsStr]| {
        std::process::Command::new("git")
            .arg("-C")
            .arg(existing)
            .args(args)
            .output()
            .map_err(|e| format!("git: {e}"))
    };
    if !git(&["rev-parse".as_ref(), "--is-inside-work-tree".as_ref()])?
        .status
        .success()
    {
        return Ok(true);
    }
    let checked = git(&[
        "check-ignore".as_ref(),
        "-q".as_ref(),
        "--".as_ref(),
        path.as_os_str(),
    ])?;
    match checked.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(format!(
            "git check-ignore {}: {}",
            path.display(),
            String::from_utf8_lossy(&checked.stderr).trim()
        )),
    }
}

fn git(args: &[&str]) -> Result<PathBuf, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .output()
        .map_err(|e| format!("git: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "git {}: {}",
            args.join(" "),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(PathBuf::from(String::from_utf8_lossy(&out.stdout).trim()))
}

fn checkout() -> Result<PathBuf, String> {
    git(&["rev-parse", "--show-toplevel"])
}

/// The checkout that owns `.local/`, which every worktree shares.
pub fn primary_checkout() -> Result<PathBuf, String> {
    let common = git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?;
    common
        .parent()
        .map(Path::to_path_buf)
        .ok_or_else(|| "no primary checkout".into())
}

/// Every fixture directory under `root`, by name.
pub fn under(root: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(root) else {
        return Vec::new();
    };
    let mut dirs: Vec<PathBuf> = entries
        .filter_map(|entry| Some(entry.ok()?.path()))
        .filter(|path| path.join(EXPECT).is_file())
        .collect();
    dirs.sort();
    dirs
}

/// What `cut` wrote.
#[derive(Debug, Serialize)]
pub struct Cut {
    pub fixture: PathBuf,
    pub at_ms: u64,
    pub bytes: usize,
    pub why_here: &'static str,
    pub judged: Vec<Judged>,
}

impl fmt::Display for Cut {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "{}", self.fixture.display())?;
        writeln!(
            f,
            "  {} bytes, from power-on to {} ({})",
            self.bytes,
            session_clock(self.at_ms),
            self.why_here
        )?;
        for judged in &self.judged {
            let verdict = if judged.keep { "holds" } else { "fails" };
            let (expect, got) = (&judged.expect, &judged.got);
            writeln!(f, "  {expect} {verdict} on the slice: got {got}")?;
        }
        Ok(())
    }
}

/// Cut `trace` at `to_ms` into a fixture at `dir`. Refused unless every
/// expectation fails and every keep holds on the slice.
pub fn cut(
    trace: &[u8],
    to_ms: u64,
    seen: Vec<Expect>,
    check: Option<&str>,
    keep: Vec<Expect>,
    dir: &Path,
    why_here: &'static str,
) -> Result<Cut, String> {
    let (slice, at_ms) = slice(trace, to_ms)?;
    let fixture = Fixture {
        slice,
        expect: Expectation::new(at_ms, seen, check, keep)?,
    };
    let verdict = fixture.judge();
    let Verdict::Judged(judged) = &verdict else {
        return Err(format!(
            "the slice does not replay to {}: {verdict}",
            session_clock(at_ms)
        ));
    };
    let named = fixture.expect.seen.iter().chain(&fixture.expect.keep);
    let unseen = (named.zip(judged.iter().filter(|j| !j.expect.starts_with("check "))))
        .find(|(expect, judged)| !SLOTS.contains(&expect.name.as_str()) && judged.got == "nothing");
    if let Some((expect, _)) = unseen {
        return Err(format!(
            "--expect {expect}: the rider saw no {} then; name page, toast, ride or a shown field",
            expect.name
        ));
    }
    if verdict.class() != Class::Reproduced {
        return Err(format!(
            "the test would pass on the slice, so it would not catch the bug: {verdict}"
        ));
    }
    fixture.write(dir)?;
    Ok(Cut {
        fixture: dir.to_path_buf(),
        at_ms,
        bytes: fixture.slice.len(),
        why_here,
        judged: judged.clone(),
    })
}

#[cfg(test)]
mod tests;
