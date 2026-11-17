const FEATURES_THAT_CHANGE_NO_PASS: [&str; 3] = ["default", "invariants", "testing"];

fn main() {
    println!("cargo::rerun-if-changed=build.rs");
    let features: Vec<String> = std::env::vars()
        .filter_map(|(name, _)| name.strip_prefix("CARGO_FEATURE_").map(str::to_lowercase))
        .filter(|name| !FEATURES_THAT_CHANGE_NO_PASS.contains(&name.as_str()))
        .collect();
    println!("cargo::metadata=features={}", features.join(","));
}
