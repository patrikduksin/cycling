//! Secrets a trace replaces as it records them. A trace leaves the device
//! for a host and may become a test fixture, so it keeps the fact and the
//! length of a secret and drops its value:
//!
//! - the password of a `WIFI CONFIG` command, and of saved Wi-Fi settings,
//!   becomes `x` repeated (hex `78`);
//! - an entropy word the phone access point gives, from which VANA makes
//!   the session's network name, password and token, is recorded as
//!   [`ENTROPY`];
//! - a phone request's session token, and the `t` value of its query,
//!   become [`TOKEN`] repeated when they are the open session's token and
//!   [`OTHER`] repeated when they are another.
//!
//! A strict replay of a phone session therefore runs on placeholders
//! consistently: VANA makes its network name, password and token from the
//! placeholder words, the replay hands VANA its own token wherever the
//! recording had the session's token, and another token stays another.
//! What the rider saw of the credentials, the phone page's codes and text,
//! differs from the recording; every request is answered as it was.

/// The entropy word a redacting trace records for the access point's.
pub const ENTROPY: u64 = 0;
/// Each byte of the open session's token, redacted.
pub const TOKEN: u8 = b'*';
/// Each byte of a token that is not the open session's, redacted.
pub const OTHER: u8 = b'-';

const PASSWORD: [u8; 2] = *b"78";

/// `line` as a redacting trace records it: a `WIFI CONFIG` command's
/// password, its seventh word, becomes `78` repeated to its length.
pub fn command<'a>(line: &'a str, out: &'a mut [u8; 256]) -> &'a str {
    let mut words = line
        .split_ascii_whitespace()
        .map(|word| (word.as_ptr() as usize - line.as_ptr() as usize, word));
    let configures = words
        .by_ref()
        .skip(2)
        .take(2)
        .map(|(_, word)| word)
        .eq(["WIFI", "CONFIG"]);
    let Some((at, password)) = words.nth(2).filter(|_| configures) else {
        return line;
    };
    let Some(copy) = out.get_mut(..line.len()) else {
        return line;
    };
    copy.copy_from_slice(line.as_bytes());
    for (index, byte) in copy[at..at + password.len()].iter_mut().enumerate() {
        *byte = PASSWORD[index % 2];
    }
    core::str::from_utf8(copy).unwrap_or(line)
}

/// A Wi-Fi password as a redacting trace records it: `x` repeated.
pub fn password(password: &[u8], out: &mut [u8; 63]) -> usize {
    let len = password.len().min(out.len());
    out[..len].fill(b'x');
    len
}

/// A request's session token as a redacting trace records it, given the
/// open session's `token`.
pub fn session<const N: usize>(session: &str, token: Option<&[u8]>) -> heapless::String<N> {
    let mark = if token == Some(session.as_bytes()) {
        TOKEN
    } else {
        OTHER
    };
    let mut out = heapless::String::new();
    for _ in 0..session.len() {
        let _ = out.push(char::from(mark));
    }
    out
}

/// A request's query as a redacting trace records it: its `t` value
/// redacted as a session token is.
pub fn query<const N: usize>(query: &str, token: Option<&[u8]>) -> heapless::String<N> {
    let mut out = heapless::String::new();
    for (index, pair) in query.split('&').enumerate() {
        if index > 0 {
            let _ = out.push('&');
        }
        match pair.strip_prefix("t=") {
            Some(value) => {
                let _ = out.push_str("t=");
                let _ = out.push_str(&session::<N>(value, token));
            }
            None => {
                let _ = out.push_str(pair);
            }
        }
    }
    out
}

/// Whether a recorded session token is the open session's, redacted.
pub fn is_token(session: &str) -> bool {
    !session.is_empty() && session.bytes().all(|byte| byte == TOKEN)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_wifi_config_keeps_its_network_and_the_length_of_its_password() {
        let mut out = [0u8; 256];
        let line = "CMD 7 WIFI CONFIG WPA2 486f6d65 7365637265743132";
        assert_eq!(
            command(line, &mut out),
            "CMD 7 WIFI CONFIG WPA2 486f6d65 7878787878787878"
        );
        let mut out = [0u8; 256];
        for kept in [
            "CMD 8 WIFI SCAN",
            "CMD 9 PHONE START",
            "CMD 1 WIFI CONFIG WPA2",
        ] {
            assert_eq!(command(kept, &mut out), kept);
        }
    }

    #[test]
    fn a_session_token_keeps_its_length_and_whether_it_was_the_sessions() {
        let token = Some(&b"0123456789abcdef"[..]);
        assert_eq!(
            session::<32>("0123456789abcdef", token).as_str(),
            "****************"
        );
        assert_eq!(
            session::<32>("ffffffffffffffff", token).as_str(),
            "----------------"
        );
        assert_eq!(session::<32>("", token).as_str(), "");
        assert_eq!(
            session::<32>("0123456789abcdef", None).as_str(),
            "----------------"
        );
        assert!(is_token("****************"));
        assert!(!is_token("--------") && !is_token(""));
        assert_eq!(
            query::<96>("t=0123456789abcdef&start=1700000000", token).as_str(),
            "t=****************&start=1700000000"
        );
        assert_eq!(query::<96>("start=1", token).as_str(), "start=1");
    }
}
