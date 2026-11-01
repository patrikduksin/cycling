//! Stamps the checkout's commit into the simulator as `CYCLING_BUILD_COMMIT`,
//! as `devices/c606/build.sh` does for the device, so a trace it records
//! names the build that recorded it. A value from the environment wins.
use std::process::Command;

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
        .filter(|text| !text.is_empty())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CYCLING_BUILD_COMMIT");
    if std::env::var_os("CYCLING_BUILD_COMMIT").is_some() {
        return;
    }
    let Some(commit) = git(&["rev-parse", "HEAD"]) else {
        return;
    };
    // A new commit or checkout moves HEAD or the branch it names.
    for path in [
        git(&["rev-parse", "--git-path", "HEAD"]),
        git(&["rev-parse", "--symbolic-full-name", "HEAD"])
            .and_then(|branch| git(&["rev-parse", "--git-path", &branch])),
        git(&["rev-parse", "--git-path", "packed-refs"]),
    ]
    .into_iter()
    .flatten()
    .filter(|path| std::path::Path::new(path).exists())
    {
        println!("cargo:rerun-if-changed={path}");
    }
    println!("cargo:rustc-env=CYCLING_BUILD_COMMIT={commit}");
}
