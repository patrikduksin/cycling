//! What a replay starts from, in words: the media a capture's `.origin`
//! holds, read with the shell's and VANA's own decoders.
use crate::bulk::Presence;
use crate::replay::Capture;
use crate::trace::file::Media;
use crate::trace::timeline::{duration, sensor};
use device_api::storage::Region;
use std::fmt::Write as _;

pub fn describe(capture: &Capture, media: Media, origin: &str) -> String {
    let start = &capture.origin;
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}; the replay starts at power-on {}",
        match media {
            Media::Saved => format!("origin {origin}: the media as the session found it"),
            Media::Blank =>
                "no .origin beside the trace: blank media, erased flash and no bulk card, so rides and settings saved before this power-on are absent".into(),
        },
        start.boot
    );
    let mut flash = start.flash.clone();
    let mut store = firmware_shell::storage::Store::new(flash.clone());
    match store.load() {
        Ok(loaded) => {
            let settings = loaded.settings;
            let _ = writeln!(
                out,
                "shell settings: {}{}: brightness {}, dims after {} s to {}, timezone {:+} min, wifi {}, phone link {}",
                stored(&format!("{:?}", loaded.source)),
                loaded
                    .sequence
                    .map(|n| format!(" (journal record {n})"))
                    .unwrap_or_default(),
                settings.brightness,
                settings.dim_timeout_secs,
                settings.dim_brightness,
                settings.timezone_minutes,
                if settings.wifi.is_some() {
                    "saved"
                } else {
                    "none"
                },
                if settings.ble.is_some() {
                    "saved"
                } else {
                    "none"
                },
            );
            let (record, stored_as) = vana::app::decode_preferences(settings.app.bytes());
            let sensors: Vec<String> = record
                .sensors
                .peers()
                .map(|(_, peer)| sensor(peer))
                .collect();
            let _ = writeln!(
                out,
                "vana preferences: {}: ftp {} W, lthr {} bpm, sensors {}, auto-pause {}, sounds {}, setup {}",
                stored(stored_as.name()),
                record.profile.ftp_watts,
                record.profile.lthr_bpm,
                if sensors.is_empty() {
                    "none".into()
                } else {
                    sensors.join(", ")
                },
                on(record.options.auto_pause),
                on(record.options.sounds),
                match record.options.setup {
                    0 => "done".to_string(),
                    step => format!("at step {step}"),
                }
            );
        }
        Err(error) => {
            let _ = writeln!(out, "shell settings: unreadable ({error:?})");
        }
    }
    let rides = crate::rides::read(&flash.contents(Region::Data));
    let rides: Vec<String> = rides
        .iter()
        .map(|ride| {
            let active = ride.samples().last().map_or(0, |s| s.active_ms);
            format!("ride {} ({} of samples)", ride.id, duration(active))
        })
        .collect();
    let _ = writeln!(
        out,
        "rides on internal flash: {}",
        if rides.is_empty() {
            "none".into()
        } else {
            rides.join(", ")
        }
    );
    let bulk = start.bulk.image();
    let card = match bulk.presence {
        Presence::Absent => "none: this board has no bulk storage".to_string(),
        Presence::Missing => "missing: it did not start this boot".to_string(),
        Presence::Ready => {
            let mut volumes = String::new();
            let _ = firmware_console::commands::volumes::execute(
                firmware_console::commands::volumes::Command::Status,
                &mut start.bulk.copy(),
                &mut volumes,
            );
            format!(
                "ready, {:.1} GiB, {} sectors written; volumes {volumes}",
                bulk.sectors as f64 * device_api::bulk::SECTOR_SIZE as f64 / f64::from(1u32 << 30),
                bulk.written.len()
            )
        }
    };
    let _ = writeln!(out, "bulk card: {card}");
    out
}

fn stored(source: &str) -> String {
    match source.to_lowercase().as_str() {
        "missing" => "none saved, so defaults".into(),
        other => other.into(),
    }
}

fn on(value: bool) -> &'static str {
    if value { "on" } else { "off" }
}
