//! The secrets a capture can hold: a trace records the password in a
//! `WIFI CONFIG` command and the token a phone sends with each request, so a
//! replay hands VANA what it read; the media keeps saved Wi-Fi credentials.
use crate::replay::Capture;
use crate::trace::timeline::session_clock;
use firmware_services::trace::{self, Event};

/// Each secret in `capture`, with where it is.
pub fn secrets(capture: &Capture) -> Vec<String> {
    let mut found = Vec::new();
    let mut store = firmware_shell::storage::Store::new(capture.origin.flash.clone());
    if let Ok(loaded) = store.load()
        && loaded.settings.wifi.is_some()
    {
        found.push("the origin's settings: saved Wi-Fi credentials".to_string());
    }
    let events = match trace::decode(&capture.trace) {
        Ok((_, events)) => events,
        Err(error) => {
            found.push(format!(
                "the trace does not decode ({error:?}), so it may hold any"
            ));
            return found;
        }
    };
    let mut now_ms = 0;
    for event in events {
        if let Event::Pass { now_ms: at } = event {
            now_ms = at;
        }
        if let Some(secret) = secret(&event) {
            found.push(format!("the pass at {}: {secret}", session_clock(now_ms)));
        }
    }
    found
}

fn secret(event: &Event<'_>) -> Option<&'static str> {
    match event {
        Event::Command(line) if before_wifi_password(line).is_some() => Some("a Wi-Fi password"),
        Event::Server {
            request: Some(request),
            ..
        } if !request.session.is_empty() => Some("a phone session token"),
        _ => None,
    }
}

/// `line` as a timeline shows it: a `WIFI CONFIG` keeps its network and
/// hides its password.
pub fn shown_command(line: &str) -> String {
    match before_wifi_password(line) {
        Some(network) => format!("{network} (password not shown)"),
        None => line.trim().to_string(),
    }
}

/// `WIFI CONFIG <auth> <ssid>`: the words a `WIFI CONFIG` line keeps.
const NETWORK_WORDS: usize = 4;

fn before_wifi_password(line: &str) -> Option<String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let wifi = words
        .windows(2)
        .position(|pair| pair == ["WIFI", "CONFIG"])?;
    Some(words[..(wifi + NETWORK_WORDS).min(words.len())].join(" "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wifi_config_shows_its_network_and_not_its_password() {
        let line = "CMD 1 WIFI CONFIG WPA2 54657374 70617373776f7264";
        assert_eq!(
            shown_command(line),
            "CMD 1 WIFI CONFIG WPA2 54657374 (password not shown)"
        );
        assert_eq!(secret(&Event::Command(line)), Some("a Wi-Fi password"));
        assert_eq!(shown_command("CMD 2 WIFI SCAN"), "CMD 2 WIFI SCAN");
        assert_eq!(secret(&Event::Command("CMD 2 WIFI SCAN")), None);
    }
}
