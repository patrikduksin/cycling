//! Convert an exported raw ride prefix (`ride-slots.bin`) into FIT activity
//! files with the same encoder the device streams to phones.
//!
//! Usage: `ride-fit <ride-slots.bin> <output-dir> [--ride ID] [--start-utc-ms MS]`
//!
//! Writes `ride-<id>.fit` for every ride (or the one chosen) and prints one
//! JSON line per ride: its file and size, or why it has none. `--start-utc-ms`
//! dates rides recorded without a clock.

use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use vana::ride::fit::{Refusal, write_all};
use vana::ride::log::{Kind, SLOT_SIZE, Slot, decode};

struct Out(Vec<u8>);
impl firmware_services::fit::Sink for Out {
    fn put(&mut self, bytes: &[u8]) {
        self.0.extend_from_slice(bytes);
    }
}

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    let (Some(input), Some(output)) = (args.next(), args.next()) else {
        eprintln!("usage: ride-fit <ride-slots.bin> <output-dir> [--ride ID] [--start-utc-ms MS]");
        return ExitCode::from(2);
    };
    let mut only = None;
    let mut start_utc_ms = None;
    while let Some(flag) = args.next() {
        let value = args.next().and_then(|v| v.parse::<u64>().ok());
        match (flag.as_str(), value) {
            ("--ride", Some(id)) => only = u32::try_from(id).ok(),
            ("--start-utc-ms", Some(ms)) => start_utc_ms = Some(ms),
            _ => {
                eprintln!("unknown or malformed argument {flag}");
                return ExitCode::from(2);
            }
        }
    }
    let raw = match std::fs::read(&input) {
        Ok(raw) if raw.len() % SLOT_SIZE == 0 => raw,
        Ok(_) => {
            eprintln!("{input}: not a whole number of {SLOT_SIZE}-byte slots");
            return ExitCode::FAILURE;
        }
        Err(error) => {
            eprintln!("{input}: {error}");
            return ExitCode::FAILURE;
        }
    };
    let slots: Vec<Slot> = raw
        .as_chunks::<SLOT_SIZE>()
        .0
        .iter()
        .map(|chunk| Slot(*chunk))
        .collect();
    let output = PathBuf::from(output);
    if let Err(error) = std::fs::create_dir_all(&output) {
        eprintln!("{}: {error}", output.display());
        return ExitCode::FAILURE;
    }
    let mut failed = false;
    let stdout = std::io::stdout();
    let mut stdout = stdout.lock();
    for (index, slot) in slots.iter().enumerate() {
        let Some(entry) = decode(slot) else { continue };
        if entry.kind != Kind::Start || only.is_some_and(|id| id != entry.ride_id) {
            continue;
        }
        let mut out = Out(Vec::new());
        let line = match write_all(slots[index..].iter(), entry.ride_id, start_utc_ms, &mut out) {
            Ok(plan) => {
                let name = format!("ride-{}.fit", entry.ride_id);
                if let Err(error) = std::fs::write(output.join(&name), &out.0) {
                    eprintln!("{name}: {error}");
                    failed = true;
                    continue;
                }
                format!(
                    r#"{{"ride_id":{},"start_slot":{},"fit":"{}","bytes":{},"samples":{},"dated":{}}}"#,
                    entry.ride_id,
                    index,
                    name,
                    plan.size,
                    plan.samples,
                    plan.offset_ms.is_some()
                )
            }
            Err(refusal) => format!(
                r#"{{"ride_id":{},"start_slot":{},"fit":null,"fit_error":"{}"}}"#,
                entry.ride_id,
                index,
                match refusal {
                    Refusal::NoClock => "no UTC in ride; pass --start-utc-ms",
                    Refusal::Empty => "no samples",
                    Refusal::BadClock =>
                        "start time outside FIT dates; --start-utc-ms takes milliseconds",
                }
            ),
        };
        let _ = writeln!(stdout, "{line}");
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}
