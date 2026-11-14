//! Redraw probe (#369): how much of the screen each page changes from one
//! submitted frame to the next, under a named sensor feed.
//!
//! `cargo run -p shell-simulator --no-default-features --bin redraw-probe -- [seconds] [case filter]`
//! or `cycling redraw-probe [seconds] [case filter]`. The default is 60 s of
//! simulated time per case. Strips are the panel's 16-row strips; the median
//! and p90 run over every submitted frame, identical ones included.
use shell_simulator::probe::{CASES, run};

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds = args.next().and_then(|n| n.parse().ok()).unwrap_or(60);
    let filter = args.next().unwrap_or_default();
    println!("| case | fps | changed strips med / p90 | changed px med / p90 | identical frames |");
    println!("|---|---:|---:|---:|---:|");
    for case in CASES.iter().filter(|case| case.name.contains(&filter)) {
        let result = run(case, seconds);
        let (strips_median, strips_p90) = result.strips();
        let (pixels_median, pixels_p90) = result.pixels();
        println!(
            "| {} | {:.1} | {strips_median} / {strips_p90} (of {}) | {:.2}% / {:.2}% | {} of {} |",
            case.name,
            result.fps(),
            result.strips,
            pixels_median * 100.0,
            pixels_p90 * 100.0,
            result.identical(),
            result.changes.len(),
        );
    }
}
