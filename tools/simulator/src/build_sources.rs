/// The checkout paths whose tracked files build VANA in the simulator, from
/// the repository root. `build.rs` and `cycling replay` both include this
/// file: an edit under any of them makes the build `-dirty` and a replay
/// pick stimulus.
const BUILD_SOURCES: &[&str] = &[
    "apps",
    "packages",
    "tools/simulator",
    "vendor",
    "patches",
    ".cargo",
    "Cargo.toml",
    "Cargo.lock",
];
