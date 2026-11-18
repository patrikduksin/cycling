//! The secrets a trace can hold. A trace that does not redact records the
//! password of a `WIFI CONFIG` command, of the request it makes of the radio
//! and of the settings the shell loads and saves, and the token a phone
//! sends with each request, so a replay hands VANA what it read. A
//! redacting trace holds none: see `firmware_services::trace::redact`.
use crate::trace::timeline::session_clock;
use crate::trace::world::World;
use crate::twin::script::declared;
use device_api::connectivity::Text;
use firmware_services::trace::wire::{Reader, Wire};
use firmware_services::trace::{self, Call, Event, redact};

/// Each secret in `trace`, with where it is.
pub fn secrets(trace: &[u8]) -> Vec<String> {
    let mut found = Vec::new();
    let events = match trace::decode(trace) {
        Ok((_, events)) => events,
        Err(error) => {
            found.push(format!(
                "the trace does not decode ({error}), so it may hold any"
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
        Event::Command(line) if command_password(line) => Some("a Wi-Fi password"),
        Event::Call(call) => {
            let method =
                declared(call.method).map(|(capability, method)| (capability.name, method.name));
            match method {
                Some(("Wifi", "request")) => {
                    requested_password(call).then_some("a Wi-Fi password in a radio request")
                }
                Some(("Settings", name @ ("load" | "save" | "save_connectivity"))) => {
                    settings_password(call, name == "load")
                        .then_some("a Wi-Fi password in the shell's settings")
                }
                _ => match crate::trace::world::read(call) {
                    Some(World::ServerRequest(Some(request)))
                        if !request.session.is_empty()
                            && !redact::is_redacted_session(request.session) =>
                    {
                        Some("a phone session token")
                    }
                    _ => None,
                },
            }
        }
        _ => None,
    }
}

fn kept(password: &[u8]) -> bool {
    !password.is_empty() && !redact::is_password(password)
}

/// Whether a `WIFI CONFIG` line holds its password: its seventh word, in
/// hex, which a redacting trace replaces with `x` repeated.
fn command_password(line: &str) -> bool {
    let words: Vec<&str> = line.split_whitespace().collect();
    wifi_config(&words)
        .and_then(|wifi| words.get(wifi + NETWORK_WORDS))
        .is_some_and(|password| !redact::is_command_password(password.as_bytes()))
}

fn requested_password(call: &Call<'_>) -> bool {
    let mut r = Reader::new(call.body, call.now_ms);
    if r.byte() != 1 {
        return false;
    }
    let config = <Option<(Text<32>, Text<63>, bool)> as Wire>::take(&mut r);
    r.read().is_some() && config.is_some_and(|(_, password, _)| kept(password.bytes()))
}

fn settings_password(call: &Call<'_>, load: bool) -> bool {
    let mut r = Reader::new(call.body, call.now_ms);
    if load && r.byte() != 0 {
        return false;
    }
    let bytes = <&[u8]>::take(&mut r);
    let (settings, _) = firmware_shell::preferences::decode(Some(bytes));
    r.read().is_some()
        && settings
            .wifi
            .is_some_and(|wifi| kept(wifi.password.bytes()))
}

/// `line` as a timeline shows it: a `WIFI CONFIG` keeps its network and
/// hides its password.
pub fn shown_command(line: &str) -> String {
    match before_wifi_password(line) {
        Some(network) => format!("{network} (password not shown)"),
        None => line.trim().to_string(),
    }
}

/// A request's query as a timeline shows it: the phone's session token in
/// `t=` is hidden, the rest kept.
pub fn shown_query(query: &str) -> String {
    query
        .split('&')
        .map(|pair| match pair.split_once('=') {
            Some(("t", _)) => "t=(not shown)",
            _ => pair,
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// `WIFI CONFIG <auth> <ssid>`: the words a `WIFI CONFIG` line keeps.
const NETWORK_WORDS: usize = 4;

/// Where `WIFI CONFIG` starts among `words`.
fn wifi_config(words: &[&str]) -> Option<usize> {
    words.windows(2).position(|pair| pair == ["WIFI", "CONFIG"])
}

fn before_wifi_password(line: &str) -> Option<String> {
    let words: Vec<&str> = line.split_whitespace().collect();
    let wifi = wifi_config(&words)?;
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
