//! A failure as one file another machine replays: `cycling_devtools.bundle
//! write` packs the trace, the source tree of the build that recorded it and
//! what a run of it must show again.
use serde::{Serialize, Serializer};
use std::ffi::OsString;
use std::fmt;
use std::path::{Path, PathBuf};

/// What `cycling bundle run` runs.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Run {
    Replay {
        to_ms: u64,
    },
    Fuzz {
        #[serde(serialize_with = "hex")]
        seed: u64,
        course: bool,
        /// The fuzzer's profile checked `debug_assert!`s, which a rebuild
        /// must check too to meet the same finding.
        debug_assertions: bool,
    },
}

fn hex<S: Serializer>(seed: &u64, serializer: S) -> Result<S::Ok, S::Error> {
    serializer.serialize_str(&format!("{seed:#018x}"))
}

/// What the run must show to reproduce the failure.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Expect {
    /// The replay's first divergence, by kind name.
    Diverged {
        kind: &'static str,
        at_ms: u64,
        detail: String,
    },
    /// A fuzz finding, with the first line of its detail: a panic's message
    /// and location, or what the check saw.
    Found {
        check: &'static str,
        at: String,
        detail: String,
    },
}

pub struct Bundle<'a> {
    pub out: &'a Path,
    pub trace: &'a Path,
    pub build: &'a str,
    pub run: Run,
    pub expect: Expect,
    pub course: Option<&'a Path>,
    pub pack: Option<&'a Path>,
}

/// What became of a bundle a tool wrote on its own.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Bundled {
    Written(PathBuf),
    Failed(String),
}

impl fmt::Display for Bundled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Written(path) => write!(f, "bundle {}", path.display()),
            Self::Failed(why) => write!(f, "no bundle: {why}"),
        }
    }
}

impl Bundle<'_> {
    fn args(&self) -> Vec<OsString> {
        let path = |path: &Path| {
            std::path::absolute(path)
                .unwrap_or_else(|_| path.to_path_buf())
                .into_os_string()
        };
        fn json(value: &impl Serialize) -> OsString {
            serde_json::to_string(value)
                .expect("a bundle's run and expectation serialize")
                .into()
        }
        let mut args: Vec<OsString> = vec![
            "write".into(),
            "--out".into(),
            path(self.out),
            "--trace".into(),
            path(self.trace),
            "--build".into(),
            self.build.into(),
            "--run".into(),
            json(&self.run),
            "--expect".into(),
            json(&self.expect),
        ];
        for (flag, file) in [("--course", self.course), ("--pack", self.pack)] {
            if let Some(file) = file {
                args.extend([flag.into(), path(file)]);
            }
        }
        args
    }

    pub fn write(&self) -> Bundled {
        let out = match crate::devtools("bundle", self.args()) {
            Ok(out) => out,
            Err(why) => return Bundled::Failed(why),
        };
        if !out.status.success() {
            let said = String::from_utf8_lossy(&out.stderr);
            let said = said.trim();
            return Bundled::Failed(match said.is_empty() {
                true => format!("cycling_devtools.bundle write {}", out.status),
                false => said.to_string(),
            });
        }
        let printed = String::from_utf8_lossy(&out.stdout);
        Bundled::Written(PathBuf::from(printed.trim()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BUILD: &str = "0123456789abcdef0123456789abcdef01234567";

    fn args(bundle: &Bundle<'_>) -> Vec<String> {
        bundle
            .args()
            .into_iter()
            .map(|arg| arg.into_string().unwrap())
            .collect()
    }

    #[test]
    fn a_replay_bundle_names_its_trace_build_window_and_first_divergence() {
        let bundle = Bundle {
            out: Path::new("/r/failure.bundle"),
            trace: Path::new("/r/ride.trace"),
            build: BUILD,
            run: Run::Replay { to_ms: 123_456 },
            expect: Expect::Diverged {
                kind: "Seen",
                at_ms: 2_000,
                detail: "page ride/main, recorded ride/climb".into(),
            },
            course: None,
            pack: Some(Path::new("/packs/chile.terrain")),
        };
        assert_eq!(
            args(&bundle),
            [
                "write",
                "--out",
                "/r/failure.bundle",
                "--trace",
                "/r/ride.trace",
                "--build",
                BUILD,
                "--run",
                r#"{"kind":"replay","to_ms":123456}"#,
                "--expect",
                r#"{"diverged":{"kind":"Seen","at_ms":2000,"detail":"page ride/main, recorded ride/climb"}}"#,
                "--pack",
                "/packs/chile.terrain",
            ]
        );
    }

    #[test]
    fn a_fuzz_bundle_names_its_seed_course_and_check() {
        let bundle = Bundle {
            out: Path::new("/f/0x00000000000000ff/finding.bundle"),
            trace: Path::new("/f/0x00000000000000ff/world.trace"),
            build: BUILD,
            run: Run::Fuzz {
                seed: 0xff,
                course: true,
                debug_assertions: true,
            },
            expect: Expect::Found {
                check: "no panic",
                at: "step 12".into(),
                detail: "index out of bounds at apps/vana/src/ride/log.rs:88".into(),
            },
            course: Some(Path::new("/f/0x00000000000000ff/course.trace")),
            pack: None,
        };
        let args = args(&bundle);
        assert_eq!(
            args[7..],
            [
                "--run",
                r#"{"kind":"fuzz","seed":"0x00000000000000ff","course":true,"debug_assertions":true}"#,
                "--expect",
                r#"{"found":{"check":"no panic","at":"step 12","detail":"index out of bounds at apps/vana/src/ride/log.rs:88"}}"#,
                "--course",
                "/f/0x00000000000000ff/course.trace",
            ]
        );
    }

    #[test]
    fn a_relative_path_reaches_the_writer_absolute() {
        let bundle = Bundle {
            out: Path::new("out/failure.bundle"),
            trace: Path::new("ride.trace"),
            build: BUILD,
            run: Run::Replay { to_ms: 1 },
            expect: Expect::Found {
                check: "no panic",
                at: "end".into(),
                detail: String::new(),
            },
            course: None,
            pack: None,
        };
        let cwd = std::env::current_dir().unwrap();
        let args = args(&bundle);
        assert_eq!(
            [&args[2], &args[4]],
            [
                &cwd.join("out/failure.bundle").display().to_string(),
                &cwd.join("ride.trace").display().to_string()
            ]
        );
    }
}
