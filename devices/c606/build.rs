//! Links firmware binaries with the esp-hal linker script.
//!
//! This lives here rather than in `.cargo/config.toml` rustflags: Cargo merges every
//! config found up the directory tree, so a checkout nested in another (such as an
//! agent worktree) would pass `-Tlinkall.x` twice and fail to link.
fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rustc-check-cfg=cfg(companion_update)");
    println!("cargo:rustc-check-cfg=cfg(companion_trial)");
    println!("cargo:rerun-if-env-changed=CYCLING_COMPANION_TRIAL");
    let trial = std::env::var("CYCLING_COMPANION_TRIAL").is_ok_and(|v| v == "1");
    assert!(
        !trial || std::env::var_os("CYCLING_COMPANION_STOCK").is_some(),
        "trial requires the pinned stock recovery image"
    );
    println!("cargo:rerun-if-env-changed=CYCLING_COMPANION_STOCK");
    if let Ok(path) = std::env::var("CYCLING_COMPANION_STOCK") {
        // This experiment can only reinstall the independently identified 1.902
        // package. Vendor bytes stay in ignored local storage and build output.
        assert!(
            std::env::var_os("CARGO_FEATURE_DEBUG_HARNESS").is_some(),
            "companion update needs the harness"
        );
        let path = std::fs::canonicalize(path).expect("private stock container");
        println!("cargo:rerun-if-changed={}", path.display());
        let hash = std::process::Command::new("sha256sum")
            .arg(&path)
            .output()
            .expect("sha256sum");
        assert!(hash.status.success());
        assert_eq!(
            std::str::from_utf8(&hash.stdout)
                .unwrap()
                .split_whitespace()
                .next(),
            Some("52437d4246744a6405aac58de575fa546170b860b541c96c9ab4a50e420f43bb"),
            "only the verified N22 1.902 stock container is supported"
        );
        let raw = std::fs::read(path).unwrap();
        assert_eq!(raw.len(), 78634);
        let key = [0x9a, 0x92, 0x45, 0x42];
        let h6 = raw[6] ^ key[2];
        let h7 = raw[7] ^ key[3];
        let data_key = [h6 ^ key[0], h7 ^ key[1], h6 ^ key[2], h7 ^ key[3]];
        let image: Vec<u8> = raw[128..raw.len() - 2]
            .iter()
            .enumerate()
            .map(|(i, b)| b ^ data_key[i % 4])
            .collect();
        let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
        let decoded = output.join("companion-stock.bin");
        std::fs::write(&decoded, &image).unwrap();
        let hash = std::process::Command::new("sha256sum")
            .arg(decoded)
            .output()
            .unwrap();
        assert!(hash.status.success());
        assert_eq!(
            std::str::from_utf8(&hash.stdout)
                .unwrap()
                .split_whitespace()
                .next(),
            Some("e7e46db0b2f0f5be7a9fe4b10ef9996e15870f148fb4923df4833b9ee4dde6e5"),
            "decoded image must match the independently unpacked stock firmware"
        );
        if trial {
            // One Thumb MOVS immediate in the fixed identity reply: 2.19 ->
            // 2.163. Startup, power, vectors, descriptor and DFU stay stock.
            // No arbitrary input image is accepted by this experiment.
            let offset = 0x15f12 - 0x12000;
            assert_eq!(&image[offset..offset + 2], &[0x13, 0x20]);
            let mut candidate = image.clone();
            candidate[offset] = 0xa3;
            assert_eq!(
                candidate.iter().zip(&image).filter(|(a, b)| a != b).count(),
                1
            );
            std::fs::write(output.join("companion-trial.bin"), candidate).unwrap();
            println!("cargo:rustc-cfg=companion_trial");
        }
        println!("cargo:rustc-cfg=companion_update");
    }
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("xtensa") {
        println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    }
}
