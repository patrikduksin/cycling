use std::path::{Path, PathBuf};
use std::process::Command;

include!("src/build_sources.rs");

fn git(root: &Path, args: &[&str]) -> Option<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(args)
        .output()
        .ok()?;
    out.status
        .success()
        .then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}

fn main() {
    println!("cargo:rerun-if-env-changed=CYCLING_BUILD_COMMIT");
    if std::env::var_os("CYCLING_BUILD_COMMIT").is_some() {
        return;
    }
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo")).join("../..");
    let Some(commit) = git(&root, &["rev-parse", "HEAD"]).filter(|c| !c.is_empty()) else {
        return;
    };
    // HEAD's reflog moves on every commit, checkout and reset, also when the
    // branch ref lives in packed-refs; the index moves when a change is staged.
    for name in ["HEAD", "logs/HEAD", "packed-refs", "index"] {
        if let Some(path) = git(&root, &["rev-parse", "--git-path", name]) {
            let path = root.join(path);
            if path.exists() {
                println!("cargo:rerun-if-changed={}", path.display());
            }
        }
    }
    let tracked =
        git(&root, &[&["ls-files", "--"][..], BUILD_SOURCES].concat()).unwrap_or_default();
    for file in tracked.lines() {
        println!("cargo:rerun-if-changed={}", root.join(file).display());
    }
    let status = [
        &["status", "--porcelain", "--untracked-files=no", "--"][..],
        BUILD_SOURCES,
    ]
    .concat();
    let dirty = git(&root, &status).is_none_or(|changes| !changes.is_empty());
    let suffix = if dirty { "-dirty" } else { "" };
    println!("cargo:rustc-env=CYCLING_BUILD_COMMIT={commit}{suffix}");
}
