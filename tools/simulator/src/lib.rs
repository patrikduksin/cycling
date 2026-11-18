pub mod ant;
pub mod artifacts;
pub mod bench;
pub mod bulk;
pub mod bundle;
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
pub mod twin;
pub mod world;

/// Runs `python -m cycling_devtools.<module> --root <checkout> <args>` with
/// the devtools of the checkout that holds the current directory.
pub fn devtools<S: AsRef<std::ffi::OsStr>>(
    module: &str,
    args: impl IntoIterator<Item = S>,
) -> Result<std::process::Output, String> {
    let top = std::process::Command::new("git")
        .args(["rev-parse", "--show-toplevel"])
        .output()
        .ok()
        .filter(|out| out.status.success())
        .ok_or_else(|| {
            format!("run it inside a checkout, whose devtools run cycling_devtools.{module}")
        })?;
    let root = std::path::PathBuf::from(String::from_utf8_lossy(&top.stdout).trim());
    std::process::Command::new("uv")
        .arg("run")
        .arg("--project")
        .arg(root.join("tools/devtools"))
        .args(["--locked", "python", "-m"])
        .arg(format!("cycling_devtools.{module}"))
        .arg("--root")
        .arg(&root)
        .args(args)
        .output()
        .map_err(|error| format!("uv did not run: {error}"))
}
