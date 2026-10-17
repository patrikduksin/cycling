//! Links firmware binaries with the esp-hal linker script.
//!
//! This lives here rather than in `.cargo/config.toml` rustflags: Cargo merges every
//! config found up the directory tree, so a checkout nested in another (such as an
//! agent worktree) would pass `-Tlinkall.x` twice and fail to link.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("xtensa") {
        println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    }
}
