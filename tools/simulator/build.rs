use std::path::PathBuf;
use std::process::Command;

const BUILD_SOURCES: &str = include_str!("build-sources");

fn features() -> String {
    let names: Vec<String> = std::env::vars()
        .filter_map(|(name, _)| name.strip_prefix("CARGO_FEATURE_").map(str::to_owned))
        .collect();
    names.join(",")
}

/// Cargo reruns this script for a new rustc or feature set and otherwise only
/// on the rerun-if lines it prints, so the hash may take no other input.
fn main() {
    let root =
        PathBuf::from(std::env::var_os("CARGO_MANIFEST_DIR").expect("set by cargo")).join("../..");
    for source in BUILD_SOURCES.lines() {
        println!("cargo::rerun-if-changed={}", root.join(source).display());
    }
    println!(
        "cargo::rerun-if-changed={}",
        root.join(".gitignore").display()
    );
    for script in ["build_hash.py", "tree.py"] {
        println!(
            "cargo::rerun-if-changed={}",
            root.join("tools/devtools/src/cycling_devtools")
                .join(script)
                .display()
        );
    }
    let exclude = Command::new("git")
        .arg("-C")
        .arg(&root)
        .args([
            "rev-parse",
            "--path-format=absolute",
            "--git-path",
            "info/exclude",
        ])
        .output()
        .expect("git runs");
    let exclude = PathBuf::from(String::from_utf8_lossy(&exclude.stdout).trim());
    // A path that does not exist reruns the script on every build.
    if exclude.is_file() {
        println!("cargo::rerun-if-changed={}", exclude.display());
    }
    let vana = std::env::var("DEP_VANA_FEATURES").expect("vana's build script names its features");
    let out = Command::new("uv")
        .arg("run")
        .arg("--project")
        .arg(root.join("tools/devtools"))
        .args(["--locked", "python", "-m", "cycling_devtools.build_hash"])
        .arg("--root")
        .arg(&root)
        .args(["hash", "--scope", "sources", "--features", &features()])
        .args(["--vana-features", &vana])
        .output()
        .expect("uv runs: build in the dev shell (docs/dev-setup.md)");
    assert!(
        out.status.success(),
        "cycling_devtools.build_hash failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    let printed = String::from_utf8(out.stdout).expect("build_hash prints hex");
    let hash = printed
        .split_whitespace()
        .next()
        .expect("build_hash prints the hash first");
    println!("cargo::rustc-env=CYCLING_BUILD_HASH={hash}");
    println!("cargo::rustc-env=CYCLING_VANA_FEATURES={vana}");
}
