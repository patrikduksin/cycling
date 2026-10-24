//! Host render benchmark (#167): paint representative VANA screens many
//! times and print host CPU microseconds per frame with the painters' work.
//!
//! `cargo run -p shell-simulator --release --features cycling --bin sim-bench -- [frames] [scene filter]`
//!
//! `... -- bus [seconds] [scene filter]` instead runs each screen and prints
//! the display bus traffic of the frames it submits, sending only changed
//! strips, against the 153,600 pixel bytes and 22 commands of a whole frame.
//!
//! Host time is a proxy only; on the device the frame time is STATUS
//! `display_frame_us`.
use shell_simulator::bench::{SCENES, bus, measure};

fn main() {
    let mut args = std::env::args().skip(1).peekable();
    if args.peek().is_some_and(|mode| mode == "bus") {
        args.next();
        let seconds = args.next().and_then(|n| n.parse().ok()).unwrap_or(10);
        let filter = args.next().unwrap_or_default();
        return traffic(seconds, &filter);
    }
    let frames = args.next().and_then(|n| n.parse().ok()).unwrap_or(300);
    let filter = args.next().unwrap_or_default();
    println!(
        "| screen | min µs | median µs | spans | span px | evaluated px | label rows | glyph columns | glyph px | mixes | hash |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---|");
    for scene in SCENES.iter().filter(|scene| scene.name.contains(&filter)) {
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

fn traffic(seconds: u64, filter: &str) {
    println!(
        "| screen | frames | mean pixel bytes | median | max | strips sent per frame | commands per frame | host hash µs per frame |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for scene in SCENES.iter().filter(|scene| scene.name.contains(filter)) {
        let mut sim = (scene.reach)();
        let bus = bus(&mut sim, seconds * 1000);
        let frames = bus.bytes.len();
        assert!(frames > 0, "{}: no frames in {seconds} s", scene.name);
        let mut sorted = bus.bytes.clone();
        sorted.sort_unstable();
        let total: usize = sorted.iter().sum();
        let mean = total as f64 / frames as f64;
        println!(
            "| {} | {frames} | {mean:.0} | {} | {} | {:.2} | {:.2} | {:.1} |",
            scene.name,
            sorted[frames / 2],
            sorted[frames - 1],
            mean / 7680.0,
            bus.commands as f64 / frames as f64,
            bus.hash_ns as f64 / frames as f64 / 1000.0,
        );
    }
}
