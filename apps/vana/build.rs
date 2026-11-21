use std::path::Path;
use std::process::Command;

const FEATURES_THAT_CHANGE_NO_PASS: [&str; 3] = ["default", "invariants", "testing"];

/// Script tiles VANA carries to test and benchmark its runtime: each
/// `guests/<name>.rs` but the shared `vana.rs` becomes `<name>.wasm`.
const GUESTS: [&str; 15] = [
    "bench",
    "countdown",
    "trend",
    "runaway",
    "spin",
    "outside",
    "wide",
    "ctor",
    "rightedge",
    "nullslice",
    "widths",
    "fill",
    "ringleft",
    "flat",
    "chatty",
];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let features: Vec<String> = std::env::vars()
        .filter_map(|(name, _)| name.strip_prefix("CARGO_FEATURE_").map(str::to_lowercase))
        .filter(|name| !FEATURES_THAT_CHANGE_NO_PASS.contains(&name.as_str()))
        .collect();
    println!("cargo::metadata=features={}", features.join(","));
    guests();
}

/// Compiles each of [`GUESTS`] to wasm32, sized to the one page of linear
/// memory an instance is given, with its stack inside.
fn guests() {
    println!("cargo::rerun-if-changed=guests");
    println!("cargo::rerun-if-changed=src/compose/script/workload.rs");
    println!("cargo::rerun-if-env-changed=CYCLING_WASM_RUSTC");
    // The firmware build puts the Xtensa rustc first on PATH: the dev shell
    // names the one with the wasm32 target.
    let rustc = std::env::var("CYCLING_WASM_RUSTC").unwrap_or_else(|_| "rustc".into());
    let out = std::env::var("OUT_DIR").unwrap();
    for guest in GUESTS {
        let source = Path::new("guests").join(format!("{guest}.rs"));
        let wasm = Path::new(&out).join(format!("{guest}.wasm"));
        let status = Command::new(&rustc)
            .args(["--edition", "2024", "--crate-type", "cdylib"])
            .args(["--target", "wasm32-unknown-unknown"])
            .args([
                "-C",
                "opt-level=z",
                "-C",
                "panic=abort",
                "-C",
                "lto",
                "-C",
                "strip=symbols",
            ])
            .args([
                "-C",
                "link-arg=--initial-memory=65536",
                "-C",
                "link-arg=--max-memory=65536",
            ])
            .args(["-C", "link-arg=-zstack-size=16384"])
            // The runtime refuses bulk memory and reference types: a fill
            // or copy is one metered instruction however long. LLVM emits
            // memory.fill and memory.copy under `bulk-memory-opt`, which
            // rustc passes on with a warning.
            .args([
                "-C",
                "target-feature=-bulk-memory,-bulk-memory-opt,-reference-types",
            ])
            .arg("-o")
            .arg(&wasm)
            .arg(&source)
            .env_remove("RUSTC_WRAPPER")
            .status()
            .unwrap_or_else(|error| panic!("{rustc} for the script guests: {error}"));
        assert!(
            status.success(),
            "{} did not compile to wasm32",
            source.display()
        );
    }
}
