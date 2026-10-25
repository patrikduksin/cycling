//! Host render benchmark (#167): paint representative VANA screens many
//! times and print host CPU microseconds per frame with the painters' work.
//!
//! `cargo run -p shell-simulator --release --features cycling --bin sim-bench -- [frames] [scene filter]`
//! A filter that is a scene's whole name picks only that scene.
//!
//! Host time is a proxy only; on the device the frame time is STATUS
//! `display_frame_us`.
use shell_simulator::bench::{SCENES, measure};

fn main() {
    let mut args = std::env::args().skip(1);
    let frames = args.next().and_then(|n| n.parse().ok()).unwrap_or(300);
    let filter = args.next().unwrap_or_default();
    println!(
        "| screen | min µs | median µs | spans | span px | evaluated px | label rows | glyph columns | glyph px | mixes | hash |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|");
    let exact = SCENES.iter().any(|scene| scene.name == filter);
    let picked = |name: &str| {
        if exact {
            name == filter
        } else {
            name.contains(&filter)
        }
    };
    for scene in SCENES.iter().filter(|scene| picked(scene.name)) {
        let mut sim = (scene.reach)();
        let request = measure(&mut sim, frames);
        assert!(
            request.stable,
            "{}: passes painted different pixels",
            scene.name
        );
        let (min, median) = request.micros();
        let work = request.work;
        println!(
            "| {} | {min:.0} | {median:.0} | {} | {} | {} | {} | {} | {} | {} | {:016x} |",
            scene.name,
            work.spans,
            work.span_pixels,
            work.evaluations,
            work.label_rows,
            work.glyph_columns,
            work.glyph_pixels,
            work.mixes,
            request.hash
        );
    }
}
