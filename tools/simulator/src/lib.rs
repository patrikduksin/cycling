pub mod ant;
pub mod artifacts;
pub mod bench;
pub mod bulk;
pub mod compare;
pub mod devices;
mod flows;
pub mod fuzz;
pub mod gps;
pub mod live;
pub mod logs;
pub mod models;
pub mod phone;
pub mod probe;
pub mod recording;
pub mod replay;
pub mod rides;
pub mod route;
pub mod screen;
pub mod session;
pub mod sim;
pub mod storage;
pub mod trace;
pub mod world;

/// Runs `cycling_devtools.build_hash` with `args` in the checkout that holds
/// the current directory.
pub fn build_hash(args: &[&str]) -> Result<std::process::Output, String> {
    let top = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .ok_or("run it inside a checkout, whose tools relate builds")?;
    let root = std::path::PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    std::process::Command::new("uv")
        .arg("run")
        .arg("--project")
        .arg(root.join("tools/devtools"))
        .args(["--locked", "python", "-m", "cycling_devtools.build_hash"])
        .arg("--root")
        .arg(&root)
        .args(args)
        .output()
        .map_err(|error| format!("uv did not run: {error}"))
}
