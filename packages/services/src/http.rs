//! Minimal HTTP/1.1 framing for a small local server: request heads in,
//! response heads out. One request per connection (`Connection: close`), no
//! chunked encoding. A `PUT` or `POST` may carry a body of up to `MAX_BODY`
//! bytes, framed only by `Content-Length`; the transport streams it to the
//! application and never holds it whole. What a route means is the caller's.

use core::fmt::Write;
use device_api::server::{Method, Request};

/// Request heads larger than this are refused (`431`).
pub const MAX_HEAD: usize = 1024;
/// Request bodies larger than this are refused (`413`) before any is read.
pub const MAX_BODY: u32 = 4 * 1024 * 1024;

// Parsed once per connection and moved straight into the exchange.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Parse {
    /// Read more bytes.
    Incomplete,
    /// A whole head of `head` bytes; any bytes after it start the body.
    Request { request: Request, head: usize },
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
    if end + 4 > MAX_HEAD {
        return Parse::Refuse(431);
    }
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
    let mut length = None;
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
            let Some(value) = content_length(value) else {
                return Parse::Refuse(400);
            };
            // Two lengths that disagree frame the body two ways.
            if length.is_some_and(|known| known != value) {
                return Parse::Refuse(400);
            }
            length = Some(value);
        } else if name.eq_ignore_ascii_case("transfer-encoding") {
            return Parse::Refuse(501);
        }
    }
    // A token in the URL is the newest: it replaces an earlier session's
    // cookie the browser still sends.
    if let Some(token) = query_value(query, "t") {
        set_session(&mut request, token);
    }
    match length.unwrap_or(0) {
        0 => {}
        // Only an upload carries a body; refuse rather than leave one unread.
        _ if !matches!(method, Method::Put | Method::Post) => return Parse::Refuse(413),
        n if n > u64::from(MAX_BODY) => return Parse::Refuse(413),
        n => request.body = n as u32,
    }
    Parse::Request {
        request,
        head: end + 4,
    }
}

/// A `Content-Length` value: ASCII digits only, so `+1`, `-0`, `1 2` and a
/// list are refused. Values past `u64` are `u64::MAX`, which no cap allows.
fn content_length(value: &str) -> Option<u64> {
    if value.is_empty() || !value.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(value.bytes().fold(0u64, |n, b| {
        n.saturating_mul(10).saturating_add(u64::from(b - b'0'))
    }))
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
        507 => "Insufficient Storage",
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
    use std::format;
    use std::string::ToString;

    fn request(bytes: &[u8]) -> Request {
        match parse(bytes) {
            Parse::Request { request, .. } => request,
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
            parse(b"GET / HTTP/1.1\r\nContent-Length: 3\r\n\r\nabc"),
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
    fn an_upload_declares_its_body_and_where_it_starts() {
        let bytes = b"PUT /api/course HTTP/1.1\r\nContent-Length: 5\r\n\r\n<gpx>";
        let Parse::Request {
            request: upload,
            head,
        } = parse(bytes)
        else {
            panic!("{:?}", parse(bytes));
        };
        assert_eq!(upload.body, 5);
        assert_eq!(&bytes[head..], b"<gpx>");
        let same = b"POST / HTTP/1.1\r\nContent-Length: 7\r\ncontent-length: 007\r\n\r\n";
        assert_eq!(
            request(same).body,
            7,
            "a repeated equal length is one length"
        );
        assert_eq!(request(b"PUT / HTTP/1.1\r\n\r\n").body, 0);
        let most = format!("PUT / HTTP/1.1\r\nContent-Length: {MAX_BODY}\r\n\r\n");
        assert_eq!(request(most.as_bytes()).body, MAX_BODY);
    }

    #[test]
    fn a_body_is_refused_unless_one_plain_length_within_the_cap_frames_it() {
        let refused = |length: &str| {
            let head = format!("PUT / HTTP/1.1\r\nContent-Length: {length}\r\n\r\n");
            parse(head.as_bytes())
        };
        for bad in ["+5", "-0", "5 5", "5,5", "0x10", "", "1e3", "\u{0665}"] {
            assert_eq!(refused(bad), Parse::Refuse(400), "{bad:?}");
        }
        let over = (u64::from(MAX_BODY) + 1).to_string();
        for big in [over.as_str(), "4294967296", "99999999999999999999999999"] {
            assert_eq!(refused(big), Parse::Refuse(413), "{big}");
        }
        assert_eq!(
            parse(b"PUT / HTTP/1.1\r\nContent-Length: 5\r\nContent-Length: 6\r\n\r\n"),
            Parse::Refuse(400),
            "two lengths that disagree"
        );
        for method in ["GET", "HEAD", "DELETE", "OPTIONS"] {
            let head = format!("{method} / HTTP/1.1\r\nContent-Length: 1\r\n\r\nx");
            assert_eq!(parse(head.as_bytes()), Parse::Refuse(413), "{method}");
        }
    }

    /// Hostile heads: random bytes, damaged real requests and every split of
    /// a request into reads. Parsing never panics, a refusal is final, and a
    /// request parses the same however its bytes arrive.
    #[test]
    fn hostile_heads_never_panic_and_reads_split_anywhere_agree() {
        use crate::testing::Rng;
        let samples: [&[u8]; 4] = [
            b"PUT /api/course HTTP/1.1\r\nHost: 192.168.4.1\r\nX-Session: 0123456789abcdef\r\nContent-Length: 12\r\n\r\n<gpx></gpx>\n",
            b"GET /api/activities/b12-3.fit?t=abc&start=5 HTTP/1.1\r\nAccept-Encoding: gzip\r\n\r\n",
            b"POST /api/close HTTP/1.1\r\nCookie: a=1; vana=zz\r\nContent-Length: 0\r\n\r\n",
            b"DELETE /api/course HTTP/1.1\r\n\r\n",
        ];
        let (mut requests, mut refused) = (0, 0);
        for seed in 0..20_000u64 {
            let mut rng = Rng::new(seed);
            let bytes = if rng.one_in(5) {
                rng.bytes(MAX_HEAD + 64)
            } else {
                let sample = rng.pick(&samples).to_vec();
                rng.mutate(sample)
            };
            let whole = parse(&bytes);
            match &whole {
                Parse::Request { request, head } => {
                    requests += 1;
                    assert!(*head <= bytes.len() && *head <= MAX_HEAD + 4, "seed {seed}");
                    assert!(request.body <= MAX_BODY, "seed {seed}");
                    assert!(
                        request.body == 0 || matches!(request.method, Method::Put | Method::Post),
                        "seed {seed}"
                    );
                }
                Parse::Refuse(_) => refused += 1,
                Parse::Incomplete => {}
            }
            // The transport parses each longer prefix until one is final.
            let mut cut = 0;
            let first = loop {
                cut = (cut + 1 + rng.below(40) as usize).min(bytes.len());
                match parse(&bytes[..cut]) {
                    Parse::Incomplete if cut < bytes.len() => continue,
                    other => break other,
                }
            };
            assert_eq!(first, whole, "seed {seed}");
        }
        assert!(
            requests > 2_000 && refused > 2_000,
            "{requests} requests, {refused} refused"
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
