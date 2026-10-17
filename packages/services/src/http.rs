//! Minimal HTTP/1.1 framing for a small local server: request heads in,
//! response heads out. One request per connection (`Connection: close`), no
//! request bodies, no chunked encoding. What a route means is the caller's.

use core::fmt::Write;
use device_api::server::{Method, Request};

/// Request heads larger than this are refused (`431`).
pub const MAX_HEAD: usize = 1024;

// Parsed once per connection and moved straight into the exchange.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parse {
    /// Read more bytes.
    Incomplete,
    Request(Request),
    /// Answer with this status and close.
    Refuse(u16),
}

/// Parse a request head from the bytes received so far.
pub fn parse(bytes: &[u8]) -> Parse {
    let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") else {
        return if bytes.len() >= MAX_HEAD {
            Parse::Refuse(431)
        } else {
            Parse::Incomplete
        };
    };
    let Ok(head) = core::str::from_utf8(&bytes[..end]) else {
        return Parse::Refuse(400);
    };
    let mut lines = head.split("\r\n");
    let mut start = lines.next().unwrap_or("").split(' ');
    let (Some(method), Some(target), Some(version), None) =
        (start.next(), start.next(), start.next(), start.next())
    else {
        return Parse::Refuse(400);
    };
    if !version.starts_with("HTTP/1.") {
        return Parse::Refuse(505);
    }
    let method = match method {
        "GET" => Method::Get,
        "HEAD" => Method::Head,
        "DELETE" => Method::Delete,
        "PUT" => Method::Put,
        "POST" => Method::Post,
        "OPTIONS" => Method::Options,
        _ => return Parse::Refuse(405),
    };
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut request = Request::new(method);
    if !path.starts_with('/') || request.path.push_str(path).is_err() {
        return Parse::Refuse(414);
    }
    // Refused rather than cut: a cut could split a character.
    if request.query.push_str(query).is_err() {
        return Parse::Refuse(414);
    }
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            return Parse::Refuse(400);
        };
        let value = value.trim();
        if name.eq_ignore_ascii_case("x-session") {
            set_session(&mut request, value);
        } else if name.eq_ignore_ascii_case("cookie") {
            for cookie in value.split(';') {
                if let Some(token) = cookie.trim().strip_prefix("vana=") {
                    set_session(&mut request, token);
                }
            }
        } else if name.eq_ignore_ascii_case("accept-encoding") {
            request.gzip = value.split(',').any(|v| v.trim().starts_with("gzip"));
        } else if name.eq_ignore_ascii_case("content-length") {
            match value.parse::<u32>() {
                Ok(0) => {}
                // No route takes a body; refuse rather than leave it unread.
                Ok(_) => return Parse::Refuse(413),
                Err(_) => return Parse::Refuse(400),
            }
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Parse::Refuse(501);
        }
    }
    // A token in the URL is the newest: it replaces an earlier session's
    // cookie the browser still sends.
    if let Some(token) = query_value(query, "t") {
        set_session(&mut request, token);
    }
    Parse::Request(request)
}

fn set_session(request: &mut Request, token: &str) {
    request.session.clear();
    if token.len() <= request.session.capacity() {
        let _ = request.session.push_str(token);
    }
}

/// The value of `key` in a query string, undecoded.
pub fn query_value<'a>(query: &'a str, key: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
        (name == key).then_some(value)
    })
}

pub const fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        413 => "Content Too Large",
        414 => "URI Too Long",
        422 => "Unprocessable Content",
        431 => "Request Header Fields Too Large",
        501 => "Not Implemented",
        503 => "Service Unavailable",
        505 => "HTTP Version Not Supported",
        _ => "Error",
    }
}

/// A response head being assembled. Call [`Head::finish`] once; the bytes
/// are then [`Head::bytes`].
pub struct Head {
    text: heapless::String<512>,
    overflow: bool,
}

impl Head {
    pub fn new(status: u16) -> Self {
        let mut head = Self {
            text: heapless::String::new(),
            overflow: false,
        };
        head.overflow |= write!(head.text, "HTTP/1.1 {status} {}\r\n", reason(status)).is_err();
        head.header("Connection", "close")
    }

    pub fn header(mut self, name: &str, value: impl core::fmt::Display) -> Self {
        self.overflow |= write!(self.text, "{name}: {value}\r\n").is_err();
        self
    }

    pub fn length(self, bytes: usize) -> Self {
        self.header("Content-Length", bytes)
    }

    /// End the head. `None` when it did not fit, which is a caller bug.
    pub fn finish(mut self) -> Option<Self> {
        self.overflow |= self.text.push_str("\r\n").is_err();
        (!self.overflow).then_some(self)
    }

    pub fn bytes(&self) -> &[u8] {
        self.text.as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(bytes: &[u8]) -> Request {
        match parse(bytes) {
            Parse::Request(request) => request,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn parses_target_session_and_encoding() {
        let r = request(
            b"GET /api/activities/b12-3.fit?t=abc&start=5 HTTP/1.1\r\nHost: x\r\nAccept-Encoding: br, gzip\r\n\r\n",
        );
        assert_eq!(r.method, Method::Get);
        assert_eq!(r.path.as_str(), "/api/activities/b12-3.fit");
        assert_eq!(query_value(&r.query, "start"), Some("5"));
        assert_eq!(r.session.as_str(), "abc");
        assert!(r.gzip);
        let r = request(b"DELETE /a HTTP/1.1\r\nCookie: x=1; vana=zz\r\nX-Session: hh\r\n\r\n");
        assert_eq!(r.method, Method::Delete);
        assert_eq!(r.session.as_str(), "hh", "the last session header wins");
        assert!(!r.gzip);
        let r = request(b"GET /?t=new HTTP/1.1\r\nCookie: vana=old\r\n\r\n");
        assert_eq!(
            r.session.as_str(),
            "new",
            "a URL token replaces an old cookie"
        );
    }

    #[test]
    fn refuses_what_it_cannot_serve() {
        assert_eq!(parse(b"GET / HTTP/1.1\r\nHost: x\r\n"), Parse::Incomplete);
        assert_eq!(parse(&[b'a'; MAX_HEAD]), Parse::Refuse(431));
        assert_eq!(parse(b"BREW / HTTP/1.1\r\n\r\n"), Parse::Refuse(405));
        assert_eq!(parse(b"GET / SPDY/3\r\n\r\n"), Parse::Refuse(505));
        assert_eq!(parse(b"GET /\r\n\r\n"), Parse::Refuse(400));
        assert_eq!(parse(b"GET x HTTP/1.1\r\n\r\n"), Parse::Refuse(414));
        let long = [b"GET /".as_slice(), &[b'a'; 200], b" HTTP/1.1\r\n\r\n"].concat();
        assert_eq!(parse(&long), Parse::Refuse(414));
        // A long query is refused whole, never cut inside a character.
        let query = [
            b"GET /?".as_slice(),
            &[b'a'; 95],
            "\u{e9}".as_bytes(),
            b" HTTP/1.1\r\n\r\n",
        ]
        .concat();
        assert_eq!(parse(&query), Parse::Refuse(414));
        assert_eq!(
            parse(b"PUT / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc"),
            Parse::Refuse(413)
        );
        assert_eq!(
            parse(b"POST / HTTP/1.1\r\nTransfer-Encoding: chunked\r\n\r\n"),
            Parse::Refuse(501)
        );
        assert_eq!(
            parse(b"GET / HTTP/1.1\r\nbroken\r\n\r\n"),
            Parse::Refuse(400)
        );
    }

    #[test]
    fn heads_are_complete_and_bounded() {
        let head = Head::new(200)
            .header("Content-Type", "application/json")
            .length(12)
            .finish()
            .unwrap();
        assert_eq!(
            head.bytes(),
            b"HTTP/1.1 200 OK\r\nConnection: close\r\nContent-Type: application/json\r\nContent-Length: 12\r\n\r\n"
        );
        let mut big = Head::new(404);
        for _ in 0..40 {
            big = big.header("X-Filler", "0123456789");
        }
        assert!(big.finish().is_none());
    }
}
