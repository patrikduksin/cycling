//! Links firmware binaries with the esp-hal linker script.
//!
//! This lives here rather than in `.cargo/config.toml` rustflags: Cargo merges every
//! config found up the directory tree, so a checkout nested in another (such as an
//! agent worktree) would pass `-Tlinkall.x` twice and fail to link.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-env-changed=CYCLING_VANA_FEATURES");
    if let Ok(hashed) = std::env::var("CYCLING_VANA_FEATURES") {
        let built =
            std::env::var("DEP_VANA_FEATURES").expect("vana's build script names its features");
        let names = |list: &str| {
            let mut names: Vec<String> = list
                .split(',')
                .map(|name| name.trim().to_lowercase().replace('-', "_"))
                .filter(|name| !name.is_empty())
                .collect();
            names.sort();
            names
        };
        assert_eq!(
            names(&hashed),
            names(&built),
            "build.sh hashed VANA features {hashed:?}, but cargo built VANA with {built:?}: name them in build.sh"
        );
    }
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("xtensa") {
        println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    }
}
