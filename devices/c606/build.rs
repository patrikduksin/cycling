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
    println!("cargo:rerun-if-env-changed=CYCLING_COMPANION_STOCK");
    println!("cargo:rerun-if-env-changed=CYCLING_COMPANION_OLD");
    let trial = std::env::var("CYCLING_COMPANION_TRIAL").ok();
    assert!(
        trial.is_none() || std::env::var_os("CYCLING_COMPANION_STOCK").is_some(),
        "trial requires the pinned stock recovery image"
    );
    let Ok(path) = std::env::var("CYCLING_COMPANION_STOCK") else {
        link();
        return;
    };
    // This experiment can only reinstall the independently identified 1.902
    // package. Vendor bytes stay in ignored local storage and build output.
    let image = decode(
        &path,
        "52437d4246744a6405aac58de575fa546170b860b541c96c9ab4a50e420f43bb",
        "e7e46db0b2f0f5be7a9fe4b10ef9996e15870f148fb4923df4833b9ee4dde6e5",
    );
    let output = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap());
    std::fs::write(output.join("companion-stock.bin"), &image).unwrap();
    // Each trial is one fixed candidate; no arbitrary input image is accepted.
    let candidate = match trial.as_deref() {
        None => None,
        // One Thumb MOVS immediate in the fixed identity reply: 2.19 -> 2.163.
        // Startup, power, vectors, descriptor and DFU stay stock.
        Some("1" | "identity") => Some(patched(&image, 0x15f12 - 0x12000, 0x13, 0xa3)),
        // One character of the descriptor's build date ("...14:38:24" -> 25):
        // data no code reads, so only a whole-image integrity check can object.
        Some("date") => Some(patched(&image, 0x47b, b'4', b'5')),
        // The date patch with the descriptor word re-solved so a reflected,
        // word-swapped CRC-32 (poly 0x6939cabd, init 0) over the whole image
        // stays 0x4921af60, the value both vendor images share. Tests the
        // one residue-scheme candidate the polynomial GCD search produced.
        Some("residue") => {
            let mut candidate = patched(&image, 0x47b, b'4', b'5');
            assert_eq!(&candidate[0x404..0x408], &[0x65, 0xc5, 0x2c, 0x62]);
            candidate[0x404..0x408].copy_from_slice(&[0x88, 0x54, 0xe7, 0x59]);
            Some(candidate)
        }
        // Unmodified older vendor release: does the loader take any intact
        // vendor image, including a downgrade?
        Some("old") => Some(decode(
            &std::env::var("CYCLING_COMPANION_OLD")
                .expect("CYCLING_COMPANION_OLD: the N22 1.638 container"),
            "756c7f25a0fbfe908a4ce52ed16d409807cd4dcb6915ee6b1be8e1e5ba313b6b",
            "",
        )),
        Some(other) => {
            panic!("unknown CYCLING_COMPANION_TRIAL={other}: identity, date, residue or old")
        }
    };
    if let Some(candidate) = candidate {
        std::fs::write(output.join("companion-trial.bin"), candidate).unwrap();
        println!("cargo:rustc-cfg=companion_trial");
    }
    println!("cargo:rustc-cfg=companion_update");
    link();
}

fn link() {
    if std::env::var("CARGO_CFG_TARGET_ARCH").as_deref() == Ok("xtensa") {
        println!("cargo:rustc-link-arg-bins=-Tlinkall.x");
    }
}

fn sha256(path: &std::path::Path) -> String {
    let hash = std::process::Command::new("sha256sum")
        .arg(path)
        .output()
        .expect("sha256sum");
    assert!(hash.status.success());
    String::from_utf8(hash.stdout)
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_owned()
}

/// Decode a pinned vendor N22 container into the image the loader receives.
/// `decoded` is empty when only the container hash is pinned.
fn decode(path: &str, container: &str, decoded: &str) -> Vec<u8> {
    let path = std::fs::canonicalize(path).expect("private companion container");
    println!("cargo:rerun-if-changed={}", path.display());
    assert_eq!(
        sha256(&path),
        container,
        "only pinned N22 containers are supported"
    );
    let raw = std::fs::read(&path).unwrap();
    let key = [0x9a, 0x92, 0x45, 0x42];
    let h6 = raw[6] ^ key[2];
    let h7 = raw[7] ^ key[3];
    let data_key = [h6 ^ key[0], h7 ^ key[1], h6 ^ key[2], h7 ^ key[3]];
    let image: Vec<u8> = raw[128..raw.len() - 2]
        .iter()
        .enumerate()
        .map(|(i, b)| b ^ data_key[i % 4])
        .collect();
    let out = std::path::PathBuf::from(std::env::var_os("OUT_DIR").unwrap()).join("decoded.bin");
    std::fs::write(&out, &image).unwrap();
    if !decoded.is_empty() {
        assert_eq!(
            sha256(&out),
            decoded,
            "decoded image must match the independently unpacked firmware"
        );
    }
    assert_eq!(
        &image[0x400..0x404],
        &[0xa5, 0x5a, 0x55, 0xaa],
        "N22 descriptor"
    );
    image
}

fn patched(image: &[u8], offset: usize, from: u8, to: u8) -> Vec<u8> {
    assert_eq!(image[offset], from);
    let mut candidate = image.to_vec();
    candidate[offset] = to;
    assert_eq!(
        candidate.iter().zip(image).filter(|(a, b)| a != b).count(),
        1
    );
    candidate
}
