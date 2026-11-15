use shell_simulator::probe::{CASES, run};

fn main() {
    let mut args = std::env::args().skip(1);
    let seconds = args.next().and_then(|n| n.parse().ok()).unwrap_or(60);
    let filter = args.next().unwrap_or_default();
    println!(
        "| case | fps | changed strips med / p90 | changed px med / p90 | identical frames \
         | painted rows med / p90 | painted rows/s | painter work k/s |"
    );
    println!("|---|---:|---:|---:|---:|---:|---:|---:|");
    for case in CASES.iter().filter(|case| case.name.contains(&filter)) {
        let result = run(case, seconds);
        let (strips, pixels, rows) = (result.strips(), result.pixel_share(), result.rows());
        println!(
            "| {} | {:.1} | {} / {} (of {}) | {:.2}% / {:.2}% | {} of {} | {} / {} | {:.0} | {:.0} |",
            case.name,
            result.fps(),
            strips.median,
            strips.p90,
            result.strips,
            pixels.median * 100.0,
            pixels.p90 * 100.0,
            result.identical(),
            result.changes.len(),
            rows.median,
            rows.p90,
            result.rows_per_second(),
            result.work_per_second() / 1000.0,
        );
    }
}
