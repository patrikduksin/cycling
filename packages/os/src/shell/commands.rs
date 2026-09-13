//! Ordinary shell operations shared by firmware terminal and virtual stream.
use super::Shell;
use crate::{
    capabilities::{Display, InputSource, Power},
    storage::OwnedFlash,
    terminal_protocol::Command,
};
use core::fmt::Write;
pub fn execute<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
    command: Command,
    system: &mut Shell<D, I, P, B>,
    now: u64,
    out: &mut impl Write,
) -> Option<&'static str> {
    let mut status = "OK";
    match command {
        Command::Harness(command) => status = system.harness_command(command, now, out),
        Command::Settings => {
            let s = system.settings;
            let _ = write!(
                out,
                "brightness={} dim_timeout={} dim_brightness={} timezone={} effective={} dimmed={} source={} persistence_error={}",
                s.brightness,
                s.dim_timeout_secs,
                s.dim_brightness,
                s.timezone_minutes,
                system.effective(),
                system.dimmed(),
                system.settings_source,
                system.settings_error
            );
        }
        Command::SetBrightness(v) => {
            if let Some(s) = system.settings.with_brightness(v) {
                system.settings = s;
                system.activity(now);
            } else {
                status = "INVALID";
            }
        }
        Command::SetTimezone(v) => {
            if let Some(s) = system.settings.with_timezone(v) {
                system.settings = s;
            } else {
                status = "INVALID";
            }
        }
        Command::SetIdle(t, v) => {
            if let Some(s) = system.settings.with_idle_preferences(t, v) {
                system.settings = s;
                system.activity(now);
            } else {
                status = "INVALID";
            }
        }
        Command::Save => {
            if !system.save() {
                status = "FAILED";
            }
        }
        Command::Activity => system.activity(now),
        Command::Display(color) => {
            system.fill(color);
            if system.display_error {
                status = "FAILED";
            }
        }
        Command::Position => {
            if let Some(p) = system.position {
                let g = p.gps;
                let _ = write!(
                    out,
                    "transport={} fix={} sequence={} bytes={} valid={} checksum_errors={} parse_errors={} dma_losses={} line_overflows={} uart_errors={} fix_age_ms={:?} satellites={:?}",
                    p.transport.name(),
                    g.state.name(),
                    p.sequence,
                    g.bytes,
                    g.valid_sentences,
                    g.checksum_errors,
                    g.parse_errors,
                    g.overflows,
                    g.line_overflows,
                    g.uart_errors,
                    g.age_ms,
                    g.satellites
                );
            } else {
                status = "UNAVAILABLE";
            }
        }
        _ => return None,
    }
    Some(status)
}
pub fn status<D: Display, I: InputSource, P: Power, B: OwnedFlash>(
    system: &Shell<D, I, P, B>,
    now: u64,
    out: &mut impl Write,
) {
    let _ = write!(
        out,
        "uptime_ms={} input_events={} synthetic_events={} routed_events={} display_submissions={} display_max_ms={} storage_ops={} storage_max_ms={} foreground={:?} ",
        now,
        system.input_events,
        system.synthetic_events,
        system.routed_events,
        system.display_submissions,
        system.display_max_ms,
        system.operations,
        system.storage_max_ms,
        system.foreground
    );
}
