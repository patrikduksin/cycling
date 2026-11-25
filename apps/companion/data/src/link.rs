//! The encrypted link to the bike computer (protocol v2, issue #545). The
//! pairing link carries the device's id and the 32-byte key `K`. After the
//! L2CAP channel opens the phone sends `nonce_p`, the device answers
//! `nonce_d`, and HKDF-SHA256 splits `K` into one key per direction. Every
//! frame after that is a `u16` LE length and a ChaCha20-Poly1305 ciphertext
//! whose nonce is that direction's frame counter; an empty plaintext ends a
//! response.

use crate::aead::{hkdf, open, seal};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;

pub const NONCE: usize = 16;
const TAG: usize = 16;
const INFO: &[u8] = b"vana link 2";
/// The device takes one frame per 256-byte SDU, so the phone's frames are
/// no longer either.
const FRAME: usize = 256;
const PLAIN: usize = FRAME - 2 - TAG;

/// What `cycling pair` shows as a QR code: which bike computer, and its key.
#[derive(Clone, Debug, PartialEq)]
pub struct Pairing {
    pub id: [u8; 4],
    key: [u8; 32],
}

impl Pairing {
    /// `vanaride://pair?id=<hex8>&k=<base64url K>`, as scanned or pasted.
    pub fn parse(link: &str) -> Result<Pairing, String> {
        const NOT: &str = "That is not a VANA pairing link.";
        let query = link.trim().strip_prefix("vanaride://pair?").ok_or(NOT)?;
        let (mut id, mut key) = (None, None);
        for pair in query.split('&') {
            match pair.split_once('=') {
                Some(("id", v)) => id = Some(v),
                Some(("k", v)) => key = Some(v),
                _ => {}
            }
        }
        let id = id.and_then(unhex).ok_or(NOT)?;
        let key =
            key.and_then(|k| URL_SAFE_NO_PAD.decode(k.trim_end_matches('=')).ok()).and_then(|k| <[u8; 32]>::try_from(k).ok()).ok_or(NOT)?;
        Ok(Pairing { id, key })
    }

    pub fn link(&self) -> String {
        format!("vanaride://pair?id={}&k={}", self.id_hex(), URL_SAFE_NO_PAD.encode(self.key))
    }

    pub fn id_hex(&self) -> String {
        self.id.iter().map(|b| format!("{b:02x}")).collect()
    }
}

fn unhex(s: &str) -> Option<[u8; 4]> {
    if s.len() != 8 {
        return None;
    }
    let mut id = [0; 4];
    for (i, b) in id.iter_mut().enumerate() {
        *b = u8::from_str_radix(s.get(2 * i..2 * i + 2)?, 16).ok()?;
    }
    Some(id)
}

/// The phone's first frame: `nonce_p`, in the clear.
pub fn hello(nonce_p: &[u8; NONCE]) -> Vec<u8> {
    frame(nonce_p)
}

fn frame(body: &[u8]) -> Vec<u8> {
    let mut out = (body.len() as u16).to_le_bytes().to_vec();
    out.extend_from_slice(body);
    out
}

/// Whole frames' bodies, in order; bytes after the last whole frame are an error.
pub fn frames(mut bytes: &[u8]) -> Result<Vec<&[u8]>, String> {
    let mut out = Vec::new();
    while !bytes.is_empty() {
        let length = match bytes {
            [lo, hi, ..] => u16::from_le_bytes([*lo, *hi]) as usize,
            _ => return Err("the bike computer sent half a frame".into()),
        };
        let body = bytes.get(2..2 + length).ok_or("the bike computer sent half a frame")?;
        out.push(body);
        bytes = &bytes[2 + length..];
    }
    Ok(out)
}

/// The per-direction keys `(k_pd, k_dp)` for one session.
pub fn keys(key: &[u8; 32], nonce_p: &[u8; NONCE], nonce_d: &[u8; NONCE]) -> ([u8; 32], [u8; 32]) {
    let mut okm = [0; 64];
    hkdf(key, &[&nonce_p[..], &nonce_d[..]].concat(), INFO, &mut okm);
    let (pd, dp) = okm.split_at(32);
    (pd.try_into().expect("32"), dp.try_into().expect("32"))
}

/// The 96-bit little-endian frame counter as a ChaCha20-Poly1305 nonce.
fn nonce(counter: u64) -> [u8; 12] {
    let mut n = [0; 12];
    n[..8].copy_from_slice(&counter.to_le_bytes());
    n
}

/// One direction: its key and the counter of its next frame.
#[derive(Debug)]
pub struct Direction {
    key: [u8; 32],
    pub counter: u64,
}

impl Direction {
    pub fn new(key: [u8; 32]) -> Direction {
        Direction { key, counter: 0 }
    }

    /// `plain` as sealed frames, each at most one SDU; nothing for nothing.
    pub fn seal(&mut self, plain: &[u8]) -> Vec<u8> {
        plain.chunks(PLAIN).flat_map(|part| self.seal_one(part)).collect()
    }

    /// One sealed frame; an empty `plain` is the end of a response.
    pub fn seal_one(&mut self, plain: &[u8]) -> Vec<u8> {
        let sealed = seal(&self.key, &nonce(self.counter), &[], plain);
        self.counter += 1;
        frame(&sealed)
    }

    /// The frame at `counter`, opened, or `None` when it does not authenticate.
    pub fn open(&self, counter: u64, sealed: &[u8]) -> Option<Vec<u8>> {
        open(&self.key, &nonce(counter), &[], sealed)
    }
}

/// An open session: the phone's sending and receiving directions.
#[derive(Debug)]
pub struct Session {
    pub tx: Direction,
    pub rx: Direction,
}

impl Session {
    pub fn open(pairing: &Pairing, nonce_p: &[u8; NONCE], nonce_d: &[u8; NONCE]) -> Session {
        let (pd, dp) = keys(&pairing.key, nonce_p, nonce_d);
        Session { tx: Direction::new(pd), rx: Direction::new(dp) }
    }

    /// A request's frames, ready for the channel.
    pub fn request(&mut self, bytes: &[u8]) -> Vec<u8> {
        self.tx.seal(bytes)
    }

    /// One response: the frames the Swift module read, the first of them
    /// the `at`-th sealed frame the device sent this session, opened and
    /// joined up to the empty frame that ends it.
    pub fn response(&mut self, at: u64, bytes: &[u8]) -> Result<Vec<u8>, String> {
        if at < self.rx.counter {
            return Err("the bike computer replayed a frame".into());
        }
        let mut plain = Vec::new();
        for (i, sealed) in frames(bytes)?.into_iter().enumerate() {
            let counter = at + i as u64;
            let part = self.rx.open(counter, sealed).ok_or("the bike computer's reply did not authenticate")?;
            self.rx.counter = counter + 1;
            if part.is_empty() {
                return Ok(plain);
            }
            plain.extend_from_slice(&part);
        }
        Err("the bike computer's reply ended early".into())
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[test]
    fn the_nonce_is_the_counter_little_endian_in_96_bits() {
        assert_eq!(nonce(0), [0; 12]);
        assert_eq!(nonce(0x0102), [2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn a_pairing_link_reads_back_as_written() {
        let key: Vec<u8> = (0..32).collect();
        let link = format!("vanaride://pair?id=0a1B2c3D&k={}", URL_SAFE_NO_PAD.encode(&key));
        let p = Pairing::parse(&format!("  {link}\n")).unwrap();
        assert_eq!(p.id, [0x0a, 0x1b, 0x2c, 0x3d]);
        assert_eq!(p.key.to_vec(), key);
        assert_eq!(p.link(), link.replace("0a1B2c3D", "0a1b2c3d"));
        assert_eq!(Pairing::parse(&format!("{link}=")).unwrap(), p);
    }

    #[test]
    fn a_link_with_a_short_key_or_id_is_refused() {
        for bad in [
            "https://cycling.computer",
            "vanaride://pair?id=0a1b2c&k=AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
            "vanaride://pair?id=0a1b2c3d&k=AAAA",
            "vanaride://pair?id=0a1b2c3d",
        ] {
            assert!(Pairing::parse(bad).is_err(), "{bad}");
        }
    }

    /// The device's side of the channel, as the firmware runs it.
    pub struct Device {
        pairing: Pairing,
        pub nonce_d: [u8; NONCE],
        session: Option<(Direction, Direction)>,
        pub sent: u64,
    }

    impl Device {
        pub fn new(pairing: Pairing) -> Device {
            Device { pairing, nonce_d: [0xd0; NONCE], session: None, sent: 0 }
        }

        /// One exchange: the phone's bytes in (a hello, or one request's
        /// frames), the device's frames out.
        pub fn receive(&mut self, bytes: &[u8], mut serve: impl FnMut(&[u8]) -> Vec<u8>) -> Vec<u8> {
            let bodies = frames(bytes).expect("whole frames from the phone");
            let Some((from_phone, to_phone)) = &mut self.session else {
                let nonce_p: [u8; NONCE] = bodies[0].try_into().expect("a 16-byte hello");
                let (pd, dp) = keys(&self.pairing.key, &nonce_p, &self.nonce_d);
                self.session = Some((Direction::new(pd), Direction::new(dp)));
                return frame(&self.nonce_d);
            };
            let mut plain = Vec::new();
            for body in bodies {
                plain.extend(from_phone.open(from_phone.counter, body).expect("the phone's frame authenticates"));
                from_phone.counter += 1;
            }
            let mut out = Vec::new();
            for part in serve(&plain).chunks(PLAIN) {
                out.extend(to_phone.seal_one(part));
            }
            out.extend(to_phone.seal_one(&[]));
            self.sent = to_phone.counter;
            out
        }
    }

    pub fn pairing() -> Pairing {
        Pairing { id: [0xc6, 0x06, 0x00, 0x01], key: [7; 32] }
    }

    #[test]
    fn a_session_carries_requests_and_responses_both_ways() {
        let pairing = pairing();
        let mut device = Device::new(pairing.clone());
        let nonce_p = [0xa0; NONCE];
        let answer = device.receive(&hello(&nonce_p), |_| unreachable!());
        let nonce_d: [u8; NONCE] = frames(&answer).unwrap()[0].try_into().unwrap();
        let mut session = Session::open(&pairing, &nonce_p, &nonce_d);
        let echo = |req: &[u8]| [b"HTTP/1.1 200 OK\r\n\r\n", req].concat();

        let long = vec![b'x'; 1000];
        for (n, body) in [&b"GET /api/status"[..], &long].into_iter().enumerate() {
            let at = device.sent;
            let reply = device.receive(&session.request(body), echo);
            assert_eq!(session.response(at, &reply).unwrap(), echo(body), "request {n}");
        }
        assert!(frames(&session.request(&long)).unwrap().iter().all(|f| f.len() + 2 <= FRAME));
    }

    #[test]
    fn a_tampered_or_replayed_frame_does_not_authenticate() {
        let pairing = pairing();
        let mut device = Device::new(pairing.clone());
        let nonce_p = [0xa0; NONCE];
        device.receive(&hello(&nonce_p), |_| unreachable!());
        let mut session = Session::open(&pairing, &nonce_p, &device.nonce_d);
        let reply = device.receive(&session.request(b"GET /"), |_| b"ok".to_vec());
        let mut tampered = reply.clone();
        tampered[3] ^= 1;
        assert!(session.response(0, &tampered).is_err());
        assert_eq!(session.response(0, &reply).unwrap(), b"ok");
        assert_eq!(session.response(0, &reply), Err("the bike computer replayed a frame".into()));
    }

    #[test]
    fn another_key_does_not_open_the_session() {
        let mut device = Device::new(pairing());
        let nonce_p = [0xa0; NONCE];
        device.receive(&hello(&nonce_p), |_| unreachable!());
        let other = Pairing { key: [8; 32], ..pairing() };
        let mut session = Session::open(&other, &nonce_p, &device.nonce_d);
        let mut phone = Session::open(&pairing(), &nonce_p, &device.nonce_d);
        let reply = device.receive(&phone.request(b"GET /"), |_| b"ok".to_vec());
        assert!(session.response(0, &reply).is_err());
    }
}
