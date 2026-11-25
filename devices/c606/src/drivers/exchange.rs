//! The one exchange between the phone transports and the application. A
//! transport parses a request head, claims the exchange for it and then
//! serves it: the body moves from the peer into the inbox as the application
//! makes room by reading it, while the response the application writes into
//! the buffer moves out to the peer. One request is answered at a time; a
//! request that finds the exchange busy waits for it. Every wait is bounded,
//! and a peer that stops reading loses its request, which the application
//! sees as `request()` changing.
//!
//! Two transports feed it: TCP connections (`http`), one request each, and a
//! phone's Bluetooth channel (`serve_channel`), request after request, sealed
//! with the pairing's key (`seal`). The application says which one it listens
//! to, and a request is claimed only from that one, so a Bluetooth session
//! never answers the network.
use crate::drivers::seal::{Direction, HELLO, Link, TAG};
use core::cell::{Cell, RefCell};
use critical_section::Mutex;
use device_api::server::{Listen, Pairing, PhoneLink, Request};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use firmware_services::http::{Head, Parse, parse};

/// Response bytes the application can queue ahead of the peer.
pub const BUFFER: usize = 4096;
/// Request body bytes held for the application to read.
pub const INBOX: usize = 4096;
/// Requests waiting for the exchange give up after this long.
const QUEUE_MS: u64 = 15_000;
/// A request head must arrive within this time once it has begun.
pub const HEAD_MS: u64 = 5_000;
/// The peer and the application must move a byte within this time.
const STALL_MS: u64 = 10_000;
/// The most bytes a phone's sealed frame may carry, its tag included.
pub const FRAME_MAX: usize = 1024;

struct Exchange {
    listening: Listen,
    /// The saved pairing, which the device hands the Bluetooth driver.
    pairing: Option<Pairing>,
    /// Written by the Bluetooth driver, read by the application.
    link: PhoneLink,
    request: Option<Request>,
    next_id: u32,
    /// In external RAM, installed by the first transport.
    buffer: &'static mut [u8],
    queued: usize,
    /// Body bytes received and not yet read, in external RAM like `buffer`.
    inbox: &'static mut [u8],
    held: usize,
    finished: bool,
}

static EXCHANGE: Mutex<RefCell<Exchange>> = Mutex::new(RefCell::new(Exchange {
    listening: Listen::Off,
    pairing: None,
    link: PhoneLink::Off,
    request: None,
    next_id: 1,
    buffer: &mut [],
    queued: 0,
    inbox: &mut [],
    held: 0,
    finished: false,
}));

fn with<R>(f: impl FnOnce(&mut Exchange) -> R) -> R {
    critical_section::with(|cs| f(&mut EXCHANGE.borrow_ref_mut(cs)))
}

/// Give the exchange its buffers; the first transport to start does, and
/// later calls keep the first ones.
pub fn install(
    buffer: impl FnOnce() -> Option<&'static mut [u8]>,
    inbox: impl FnOnce() -> Option<&'static mut [u8]>,
) {
    with(|e| {
        if e.buffer.is_empty()
            && let Some(buffer) = buffer()
        {
            e.buffer = buffer;
        }
        if e.inbox.is_empty()
            && let Some(inbox) = inbox()
        {
            e.inbox = inbox;
        }
    });
}

/// Where the application listens.
pub fn listening() -> Listen {
    with(|e| e.listening)
}

/// The pairing the phone's channel is sealed with; none keeps the device
/// from advertising.
pub fn pairing() -> Option<Pairing> {
    with(|e| e.pairing)
}

/// The device hands over the saved pairing, once per pass.
#[inline(never)]
pub fn set_pairing(pairing: &Option<Pairing>) {
    with(|e| e.pairing = *pairing);
}

/// The Bluetooth driver reports the phone's link.
pub fn set_link(link: PhoneLink) {
    with(|e| e.link = link);
}

/// The application side of the exchange.
pub struct Server;

impl device_api::server::Server for Server {
    fn listen(&mut self, on: Listen) {
        with(|e| e.listening = on);
    }
    fn request(&self) -> Option<Request> {
        with(|e| e.request.clone().filter(|_| !e.finished))
    }
    fn read(&mut self, id: u32, out: &mut [u8]) -> usize {
        with(|e| {
            if e.finished || e.request.as_ref().is_none_or(|r| r.id != id) {
                return 0;
            }
            let n = out.len().min(e.held);
            out[..n].copy_from_slice(&e.inbox[..n]);
            e.inbox.copy_within(n..e.held, 0);
            e.held -= n;
            n
        })
    }
    fn room(&self) -> usize {
        with(|e| {
            if e.request.is_some() && !e.finished {
                e.buffer.len() - e.queued
            } else {
                0
            }
        })
    }
    fn write(&mut self, id: u32, bytes: &[u8]) -> usize {
        with(|e| {
            if e.finished || e.request.as_ref().is_none_or(|r| r.id != id) {
                return 0;
            }
            let n = bytes.len().min(e.buffer.len() - e.queued);
            e.buffer[e.queued..e.queued + n].copy_from_slice(&bytes[..n]);
            e.queued += n;
            n
        })
    }
    fn finish(&mut self, id: u32) {
        with(|e| {
            if e.request.as_ref().is_some_and(|r| r.id == id) {
                e.finished = true;
            }
        });
    }
    fn link(&mut self) -> PhoneLink {
        with(|e| e.link)
    }
}

/// A peer's bytes in, as a transport receives them.
pub(crate) trait Rx {
    /// Some bytes into `out`; `None` once the peer closed or failed.
    async fn read(&mut self, out: &mut [u8]) -> Option<usize>;
}

/// Bytes out to a peer.
pub(crate) trait Tx {
    /// All of `bytes`, or false when the peer is gone.
    async fn write(&mut self, bytes: &[u8]) -> bool;
    /// The response is complete.
    async fn end(&mut self) -> bool {
        true
    }
}

/// Read until `head[..*used]` holds a whole request head, or a reason to
/// refuse it. `None` once the peer closed.
pub(crate) async fn read_head(
    rx: &mut impl Rx,
    head: &mut [u8],
    used: &mut usize,
) -> Option<Parse> {
    loop {
        match parse(&head[..*used]) {
            Parse::Incomplete if *used < head.len() => {}
            Parse::Incomplete => return Some(Parse::Refuse(431)),
            result => return Some(result),
        }
        *used += rx.read(&mut head[*used..]).await?;
    }
}

/// Answer a head the transport refuses, without the application.
pub(crate) async fn refuse(tx: &mut impl Tx, status: u16) {
    if let Some(head) = Head::new(status).length(0).finish() {
        let _ = tx.write(head.bytes()).await && tx.end().await;
    }
}

/// The exchange is this request's until dropped, so a transport that stops
/// early, or is cancelled, still releases it.
pub(crate) struct Claim {
    id: u32,
    via: Listen,
    /// Body bytes the peer still owes past those read with the head.
    owed: u32,
}

impl Drop for Claim {
    fn drop(&mut self) {
        let id = self.id;
        with(|e| {
            if e.request.as_ref().is_some_and(|r| r.id == id) {
                e.request = None;
                e.queued = 0;
                e.held = 0;
                e.finished = false;
            }
        });
    }
}

/// Claim the exchange for `request` from `via`, waiting for an earlier one to
/// finish. `early` holds the bytes read with the head; those past the
/// declared body are not this request's. `None` when the application does
/// not listen to `via` or the wait ran out.
pub(crate) async fn claim(mut request: Request, early: &[u8], via: Listen) -> Option<Claim> {
    let deadline = Instant::now() + Duration::from_millis(QUEUE_MS);
    loop {
        let claimed = with(|e| {
            if e.listening != via || request.body > 0 && e.inbox.is_empty() {
                return Some(None);
            }
            if e.request.is_some() {
                return None;
            }
            request.id = e.next_id;
            e.next_id = e.next_id.wrapping_add(1).max(1);
            e.queued = 0;
            e.finished = false;
            let early = &early[..early.len().min(request.body as usize).min(e.inbox.len())];
            e.inbox[..early.len()].copy_from_slice(early);
            e.held = early.len();
            let owed = request.body - early.len() as u32;
            e.request = Some(request.clone());
            Some(Some(Claim {
                id: request.id,
                via,
                owed,
            }))
        });
        match claimed {
            Some(result) => return result,
            None if Instant::now() >= deadline => return None,
            None => Timer::after_millis(5).await,
        }
    }
}

/// Whether request `id` from `via` still holds the exchange and its
/// transport is still listened to.
fn ours(id: u32, via: Listen) -> bool {
    with(|e| e.listening == via && e.request.as_ref().is_some_and(|r| r.id == id))
}

/// Serve a claimed request: its body in and its response out, side by side,
/// until the application finished. Returns body bytes the peer still owes, so
/// a transport that keeps its stream can skip them, or `None` when the peer,
/// the application or the session went away first.
pub(crate) async fn respond(
    rx: &mut impl Rx,
    tx: &mut impl Tx,
    claim: Claim,
    into: &mut [u8],
    out: &mut [u8],
) -> Option<u32> {
    let (id, via) = (claim.id, claim.via);
    let active = Cell::new(Instant::now());
    let stalled = || active.get().elapsed().as_millis() > STALL_MS;
    let feed = async {
        let mut owed = claim.owed;
        // Until the application finished: the rest is no longer its.
        while owed > 0 && ours(id, via) && !with(|e| e.finished) {
            let room = with(|e| e.inbox.len() - e.held);
            let wanted = (owed as usize).min(room).min(into.len());
            if wanted == 0 {
                if stalled() {
                    return None;
                }
                Timer::after_millis(2).await;
                continue;
            }
            let n = with_timeout(
                Duration::from_millis(STALL_MS),
                rx.read(&mut into[..wanted]),
            )
            .await
            .ok()??;
            owed -= n as u32;
            with(|e| {
                if e.request.as_ref().is_some_and(|r| r.id == id) && !e.finished {
                    e.inbox[e.held..e.held + n].copy_from_slice(&into[..n]);
                    e.held += n;
                }
            });
            active.set(Instant::now());
        }
        Some(owed)
    };
    let drain = async {
        loop {
            let (n, finished) = with(|e| {
                let n = e.queued.min(out.len());
                out[..n].copy_from_slice(&e.buffer[..n]);
                e.buffer.copy_within(n..e.queued, 0);
                e.queued -= n;
                (n, e.finished && e.queued == 0)
            });
            if n > 0 {
                if !tx.write(&out[..n]).await {
                    return false;
                }
                active.set(Instant::now());
                continue;
            }
            if finished {
                return tx.end().await;
            }
            if !ours(id, via) || stalled() {
                return false;
            }
            Timer::after_millis(2).await;
        }
    };
    let (owed, sent) = embassy_futures::join::join(feed, drain).await;
    drop(claim);
    owed.filter(|_| sent)
}

/// One SDU from a channel that carries them, such as an L2CAP channel.
pub(crate) trait ReceiveSdu {
    /// The next SDU into `sdu`, which holds the largest this side accepts;
    /// `None` once the channel closed.
    async fn receive(&mut self, sdu: &mut [u8]) -> Option<usize>;
}

/// One SDU out.
pub(crate) trait SendSdu {
    async fn send(&mut self, sdu: &[u8]) -> bool;
}

/// The channel's SDUs as one byte stream: their boundaries carry no meaning.
struct Bytes<'a, R> {
    rx: R,
    sdu: &'a mut [u8],
    at: usize,
    len: usize,
}

impl<R: ReceiveSdu> Bytes<'_, R> {
    async fn read(&mut self, out: &mut [u8]) -> Option<usize> {
        while self.at == self.len {
            self.len = self.rx.receive(self.sdu).await?.min(self.sdu.len());
            self.at = 0;
        }
        let n = out.len().min(self.len - self.at);
        out[..n].copy_from_slice(&self.sdu[self.at..self.at + n]);
        self.at += n;
        Some(n)
    }

    async fn exact(&mut self, out: &mut [u8]) -> Option<()> {
        let mut at = 0;
        while at < out.len() {
            at += self.read(&mut out[at..]).await?;
        }
        Some(())
    }

    /// The next frame, `u16` little-endian length and that many bytes, into
    /// the front of `frame`; `None` once the channel closed or for a frame
    /// longer than `frame`.
    async fn frame(&mut self, frame: &mut [u8]) -> Option<usize> {
        let mut length = [0; 2];
        self.exact(&mut length).await?;
        let length = usize::from(u16::from_le_bytes(length));
        self.exact(frame.get_mut(..length)?).await?;
        Some(length)
    }
}

/// The phone's requests: its sealed frames, opened in order.
struct Opened<'a, R> {
    bytes: Bytes<'a, R>,
    open: Direction,
    /// One sealed frame, then its plaintext.
    frame: &'a mut [u8],
    at: usize,
    len: usize,
    /// A frame failed to authenticate.
    refused: bool,
}

impl<R: ReceiveSdu> Rx for Opened<'_, R> {
    async fn read(&mut self, out: &mut [u8]) -> Option<usize> {
        while self.at == self.len {
            let sealed = self.bytes.frame(self.frame).await?;
            let Some(len) = self.open.open(&mut self.frame[..sealed]) else {
                self.refused = true;
                return None;
            };
            (self.at, self.len) = (0, len);
        }
        let n = out.len().min(self.len - self.at);
        out[..n].copy_from_slice(&self.frame[self.at..self.at + n]);
        self.at += n;
        Some(n)
    }
}

/// Pass over `count` bytes, such as a refused request's body.
async fn skip(rx: &mut impl Rx, mut count: u32, scratch: &mut [u8]) -> Option<()> {
    while count > 0 {
        let wanted = (count as usize).min(scratch.len());
        let n = with_timeout(
            Duration::from_millis(STALL_MS),
            rx.read(&mut scratch[..wanted]),
        )
        .await
        .ok()??;
        count -= n as u32;
    }
    Some(())
}

/// Responses as sealed frames, one per SDU; a frame with no plaintext, its
/// tag alone, ends a response.
struct Sealed<'a, S> {
    tx: S,
    seal: Direction,
    frame: &'a mut [u8],
}

impl<S: SendSdu> Sealed<'_, S> {
    async fn send(&mut self, plaintext: usize) -> bool {
        let length = self.seal.seal(&mut self.frame[2..], plaintext);
        self.frame[..2].copy_from_slice(&(length as u16).to_le_bytes());
        self.tx.send(&self.frame[..2 + length]).await
    }
}

impl<S: SendSdu> Tx for Sealed<'_, S> {
    async fn write(&mut self, bytes: &[u8]) -> bool {
        for part in bytes.chunks(self.frame.len() - 2 - TAG) {
            self.frame[2..2 + part.len()].copy_from_slice(part);
            if !self.send(part.len()).await {
                return false;
            }
        }
        true
    }
    async fn end(&mut self) -> bool {
        self.send(0).await
    }
}

/// Working memory for one channel, in external RAM on the device.
pub(crate) struct ChannelBuffers<'a> {
    /// At least the SDU size this side accepts.
    pub sdu: &'a mut [u8],
    /// `FRAME_MAX` bytes: one of the phone's sealed frames.
    pub frame: &'a mut [u8],
    pub head: &'a mut [u8],
    pub into: &'a mut [u8],
    pub out: &'a mut [u8],
}

/// How a channel ended, for the log.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Ended {
    /// The phone closed it, or the session or a response ended it.
    Closed,
    /// No hello in time, or not one.
    Hello,
    /// A frame failed to authenticate: another key, or tampered.
    Refused,
}

/// Serve a phone's channel: the handshake, then request after request until
/// it closes, a frame fails to authenticate, the session stops listening on
/// Bluetooth, or a response cannot complete. `piece` is the largest SDU both
/// sides take, `key` the pairing's and `hello` this side's random nonce.
pub(crate) async fn serve_channel(
    rx: impl ReceiveSdu,
    mut tx: impl SendSdu,
    piece: usize,
    buffers: ChannelBuffers<'_>,
    key: &[u8; 32],
    hello: [u8; HELLO],
) -> Ended {
    let ChannelBuffers {
        sdu,
        frame,
        head,
        into,
        out,
    } = buffers;
    if listening() != Listen::Bluetooth {
        return Ended::Closed;
    }
    let mut bytes = Bytes {
        rx,
        sdu,
        at: 0,
        len: 0,
    };
    let phone = with_timeout(Duration::from_millis(HEAD_MS), bytes.frame(frame)).await;
    let Ok(Some(HELLO)) = phone else {
        return Ended::Hello;
    };
    let phone: [u8; HELLO] = frame[..HELLO].try_into().expect("a hello's length");
    let mut reply = [0; 2 + HELLO];
    reply[..2].copy_from_slice(&(HELLO as u16).to_le_bytes());
    reply[2..].copy_from_slice(&hello);
    if !tx.send(&reply).await {
        return Ended::Closed;
    }
    let Link { incoming, outgoing } = Link::new(key, &phone, &hello);
    let (sealing, out) = out.split_at_mut(piece.clamp(3 + TAG, out.len() / 2));
    let mut rx = Opened {
        bytes,
        open: incoming,
        frame,
        at: 0,
        len: 0,
        refused: false,
    };
    let mut tx = Sealed {
        tx,
        seal: outgoing,
        frame: sealing,
    };
    serve_requests(&mut rx, &mut tx, head, into, out).await;
    if rx.refused {
        Ended::Refused
    } else {
        Ended::Closed
    }
}

async fn serve_requests(
    rx: &mut impl Rx,
    tx: &mut impl Tx,
    head: &mut [u8],
    into: &mut [u8],
    out: &mut [u8],
) {
    let mut used = 0;
    while listening() == Listen::Bluetooth {
        if used == 0 {
            // Between requests the phone may stay quiet for as long as the
            // session lasts.
            let Some(n) = rx.read(head).await else {
                return;
            };
            used = n;
        }
        let parsed = with_timeout(
            Duration::from_millis(HEAD_MS),
            read_head(rx, head, &mut used),
        )
        .await;
        let (request, length) = match parsed {
            Ok(Some(Parse::Request { request, head })) => (request, head),
            Ok(Some(Parse::Refuse(status))) => {
                // Where this request ends is unknown: close the channel.
                refuse(tx, status).await;
                return;
            }
            _ => return,
        };
        let declared = request.body;
        let body = (used - length).min(declared as usize);
        let owed = match claim(request, &head[length..length + body], Listen::Bluetooth).await {
            Some(claim) => match respond(rx, tx, claim, into, out).await {
                Some(owed) => owed,
                None => return,
            },
            None => {
                refuse(tx, 503).await;
                declared - body as u32
            }
        };
        if skip(rx, owed, into).await.is_none() {
            return;
        }
        // Bytes after this request start the next one.
        head.copy_within(length + body..used, 0);
        used -= length + body;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use device_api::server::Server as _;
    use std::boxed::Box;
    use std::collections::VecDeque;
    use std::format;
    use std::rc::Rc;
    use std::string::String;
    use std::vec::Vec;

    /// Tests share the one exchange.
    static SERIAL: std::sync::Mutex<()> = std::sync::Mutex::new(());

    const KEY: [u8; 32] = [0x4b; 32];
    const PHONE_HELLO: [u8; HELLO] = [0x70; HELLO];
    const DEVICE_HELLO: [u8; HELLO] = [0xd0; HELLO];
    /// Plaintext bytes in each of the phone's frames.
    const PHONE_FRAME: usize = 100;

    /// The phone's half of the link: what it seals and opens, and how many
    /// frames each way.
    struct Phone {
        seal: Direction,
        open: Direction,
        sealed: u64,
        opened: u64,
    }

    /// Both ends of a fake L2CAP channel: SDUs the phone sent that are not
    /// read yet, SDUs the device sent, and the phone once it shook hands.
    #[derive(Default)]
    struct Wire {
        incoming: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        closed: bool,
        phone: Option<Phone>,
    }

    #[derive(Clone, Default)]
    struct Channel(Rc<RefCell<Wire>>);

    impl ReceiveSdu for Channel {
        async fn receive(&mut self, sdu: &mut [u8]) -> Option<usize> {
            loop {
                {
                    let mut wire = self.0.borrow_mut();
                    if let Some(next) = wire.incoming.pop_front() {
                        sdu[..next.len()].copy_from_slice(&next);
                        return Some(next.len());
                    }
                    if wire.closed {
                        return None;
                    }
                }
                Timer::after_millis(1).await;
            }
        }
    }

    impl SendSdu for Channel {
        async fn send(&mut self, sdu: &[u8]) -> bool {
            self.0.borrow_mut().sent.push(sdu.to_vec());
            true
        }
    }

    impl Channel {
        /// The phone's side of the handshake, with `key`.
        async fn shake_hands(&self, key: &[u8; 32]) {
            let mut hello = std::vec![HELLO as u8, 0];
            hello.extend_from_slice(&PHONE_HELLO);
            self.0.borrow_mut().incoming.push_back(hello);
            let reply = loop {
                if let Some(reply) = self.0.borrow_mut().sent.pop() {
                    break reply;
                }
                Timer::after_millis(1).await;
            };
            assert_eq!(reply[..2], [HELLO as u8, 0], "a plain hello");
            assert_eq!(reply[2..], DEVICE_HELLO);
            let Link { incoming, outgoing } = Link::new(key, &PHONE_HELLO, &DEVICE_HELLO);
            self.0.borrow_mut().phone = Some(Phone {
                seal: incoming,
                open: outgoing,
                sealed: 0,
                opened: 0,
            });
        }

        fn phone<R>(&self, f: impl FnOnce(&mut Phone) -> R) -> R {
            f(self.0.borrow_mut().phone.as_mut().expect("the handshake"))
        }

        /// One sealed frame of `plaintext`, framed.
        fn seal(&self, plaintext: &[u8]) -> Vec<u8> {
            self.phone(|phone| {
                let mut frame = plaintext.to_vec();
                frame.resize(plaintext.len() + TAG, 0);
                let length = phone.seal.seal(&mut frame, plaintext.len());
                phone.sealed += 1;
                let mut framed = (length as u16).to_le_bytes().to_vec();
                framed.extend_from_slice(&frame);
                framed
            })
        }

        /// The phone seals `bytes` in frames and writes them in SDUs of at
        /// most `piece`, so a frame may span SDUs and an SDU hold frames.
        fn write(&self, bytes: &[u8], piece: usize) {
            let stream: Vec<u8> = bytes
                .chunks(PHONE_FRAME)
                .flat_map(|part| self.seal(part))
                .collect();
            self.write_raw(&stream, piece);
        }

        fn write_raw(&self, stream: &[u8], piece: usize) {
            let mut wire = self.0.borrow_mut();
            wire.incoming
                .extend(stream.chunks(piece).map(<[u8]>::to_vec));
        }

        /// Wait for the next whole response; panics on a frame that does not
        /// open, or one that is not an SDU of its own of at most `piece`.
        async fn response(&self, piece: usize) -> String {
            loop {
                if let Some(response) = self.take_response(piece) {
                    return response;
                }
                Timer::after_millis(1).await;
            }
        }

        fn take_response(&self, piece: usize) -> Option<String> {
            let wire = &mut *self.0.borrow_mut();
            // The end frame carries its tag alone.
            let end = wire
                .sent
                .iter()
                .position(|sdu| sdu[..2] == (TAG as u16).to_le_bytes())?;
            let phone = wire.phone.as_mut().expect("the handshake");
            let mut bytes = Vec::new();
            for mut sdu in wire.sent.drain(..=end) {
                assert!(sdu.len() <= piece, "an SDU of {} bytes", sdu.len());
                let length = u16::from_le_bytes([sdu[0], sdu[1]]) as usize;
                assert_eq!(sdu.len(), 2 + length, "one frame per SDU");
                let plain = phone.open.open(&mut sdu[2..]).expect("a frame that opens");
                phone.opened += 1;
                bytes.extend_from_slice(&sdu[2..2 + plain]);
            }
            Some(String::from_utf8(bytes).unwrap())
        }
    }

    /// The application: answers each request with its method, path, the body
    /// it read and filler, except `/refuse`, answered without reading.
    async fn application() {
        let mut server = Server;
        let mut answered = 0;
        loop {
            Timer::after_millis(1).await;
            let Some(request) = server.request().filter(|r| r.id != answered) else {
                continue;
            };
            answered = request.id;
            let mut body = Vec::new();
            while request.path != "/refuse" && body.len() < request.body as usize {
                let mut piece = [0; 64];
                let n = server.read(request.id, &mut piece);
                body.extend_from_slice(&piece[..n]);
                Timer::after_millis(1).await;
            }
            let text = format!(
                "{:?} {} {} {}",
                request.method,
                request.path,
                String::from_utf8(body).unwrap(),
                "x".repeat(300)
            );
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{text}",
                text.len()
            );
            let mut at = 0;
            while at < response.len() {
                at += server.write(request.id, &response.as_bytes()[at..]);
                Timer::after_millis(1).await;
            }
            server.finish(request.id);
        }
    }

    fn leak(size: usize) -> &'static mut [u8] {
        Box::leak(std::vec![0; size].into_boxed_slice())
    }

    fn buffers() -> ChannelBuffers<'static> {
        ChannelBuffers {
            sdu: leak(512),
            frame: leak(FRAME_MAX),
            head: leak(firmware_services::http::MAX_HEAD),
            into: leak(512),
            out: leak(512 + 256),
        }
    }

    /// Serve `channel` with an SDU limit of `piece` while `phone` runs, after
    /// it shook hands with `key`; how the channel ended.
    fn run_with(
        channel: &Channel,
        piece: usize,
        key: [u8; 32],
        phone: impl Future<Output = ()>,
    ) -> Ended {
        install(|| Some(leak(BUFFER)), || Some(leak(INBOX)));
        Server.listen(Listen::Bluetooth);
        let device = serve_channel(
            channel.clone(),
            channel.clone(),
            piece,
            buffers(),
            &KEY,
            DEVICE_HELLO,
        );
        let phone = async {
            channel.shake_hands(&key).await;
            phone.await;
            channel.0.borrow_mut().closed = true;
        };
        let embassy_futures::select::Either::First((ended, ())) =
            embassy_futures::block_on(embassy_futures::select::select(
                embassy_futures::join::join(device, phone),
                application(),
            ))
        else {
            unreachable!("the application serves until the channel ends");
        };
        assert!(Server.request().is_none(), "the exchange is free again");
        ended
    }

    fn run(channel: &Channel, piece: usize, phone: impl Future<Output = ()>) -> Ended {
        run_with(channel, piece, KEY, phone)
    }

    fn get(path: &str) -> Vec<u8> {
        format!("GET {path} HTTP/1.1\r\nHost: vana\r\n\r\n").into_bytes()
    }

    fn body_of(response: &str) -> &str {
        response.split_once("\r\n\r\n").unwrap().1
    }

    #[test]
    fn a_request_split_across_sdus_is_answered_in_frames_and_the_channel_takes_the_next() {
        let _serial = SERIAL.lock();
        let channel = Channel::default();
        let ended = run(&channel, 40, async {
            channel.write(&get("/api/status"), 7);
            let first = channel.response(40).await;
            assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
            let body = body_of(&first);
            assert!(body.starts_with("Get /api/status "), "{body}");
            assert_eq!(body.len(), 300 + "Get /api/status  ".len());
            channel.write(&get("/api/workouts"), 61);
            let second = channel.response(40).await;
            assert!(
                body_of(&second).starts_with("Get /api/workouts "),
                "{second}"
            );
        });
        assert_eq!(ended, Ended::Closed);
    }

    #[test]
    fn a_body_split_across_sdus_reaches_the_application_and_the_next_request_follows_it() {
        let _serial = SERIAL.lock();
        let channel = Channel::default();
        run(&channel, 122, async {
            let mut bytes =
                b"PUT /api/wifi HTTP/1.1\r\nContent-Length: 35\r\n\r\n{\"ssid\":\"home\",\"password\":\"secret\"}"
                    .to_vec();
            bytes.extend_from_slice(&get("/api/status"));
            channel.write(&bytes, 9);
            let put = channel.response(122).await;
            assert!(
                body_of(&put).starts_with(r#"Put /api/wifi {"ssid":"home","password":"secret"} "#),
                "{put}"
            );
            let next = channel.response(122).await;
            assert!(body_of(&next).starts_with("Get /api/status "), "{next}");
        });
    }

    #[test]
    fn a_body_the_application_never_reads_is_passed_over() {
        let _serial = SERIAL.lock();
        let channel = Channel::default();
        run(&channel, 64, async {
            let body = "b".repeat(5000);
            let put = format!("PUT /refuse HTTP/1.1\r\nContent-Length: 5000\r\n\r\n{body}");
            channel.write(put.as_bytes(), 100);
            let refused = channel.response(64).await;
            assert!(body_of(&refused).starts_with("Put /refuse  "), "{refused}");
            channel.write(&get("/api/activities"), 100);
            let next = channel.response(64).await;
            assert!(body_of(&next).starts_with("Get /api/activities "), "{next}");
        });
    }

    /// Each side counts its own frames from 0: the phone's request takes
    /// several frames, and the device's first answer still opens at 0.
    #[test]
    fn each_direction_counts_its_own_frames() {
        let _serial = SERIAL.lock();
        let channel = Channel::default();
        run(&channel, 64, async {
            let long = format!("/api/{}", "s".repeat(80));
            channel.write(&get(&long), 30);
            let (sealed, opened) = channel.phone(|phone| (phone.sealed, phone.opened));
            assert_eq!((sealed, opened), (2, 0));
            channel.response(64).await;
            let opened = channel.phone(|phone| phone.opened);
            assert!(opened > 2, "{opened} frames from the device");
            channel.write(&get("/api/status"), 30);
            channel.response(64).await;
            assert_eq!(channel.phone(|phone| phone.sealed), 3);
        });
    }

    #[test]
    fn a_tampered_frame_closes_the_channel_unanswered() {
        let _serial = SERIAL.lock();
        let channel = Channel::default();
        let ended = run(&channel, 64, async {
            channel.write(&get("/api/status"), 64);
            channel.response(64).await;
            let mut frame = channel.seal(&get("/api/wifi"));
            frame[5] ^= 0x20;
            channel.write_raw(&frame, 64);
            while !channel.0.borrow().incoming.is_empty() {
                Timer::after_millis(1).await;
            }
            Timer::after_millis(50).await;
            assert!(channel.0.borrow().sent.is_empty(), "nothing answered");
        });
        assert_eq!(ended, Ended::Refused);
    }

    #[test]
    fn a_phone_with_another_key_is_refused_at_its_first_request() {
        let _serial = SERIAL.lock();
        let channel = Channel::default();
        let ended = run_with(&channel, 64, [0x4c; 32], async {
            channel.write(&get("/api/status"), 64);
            Timer::after_millis(50).await;
            assert!(channel.0.borrow().sent.is_empty(), "nothing answered");
        });
        assert_eq!(ended, Ended::Refused);
    }

    #[test]
    fn a_channel_without_a_hello_is_closed() {
        let _serial = SERIAL.lock();
        install(|| Some(leak(BUFFER)), || Some(leak(INBOX)));
        Server.listen(Listen::Bluetooth);
        let channel = Channel::default();
        channel.write_raw(&get("/api/status"), 64);
        channel.0.borrow_mut().closed = true;
        let ended = embassy_futures::block_on(serve_channel(
            channel.clone(),
            channel.clone(),
            64,
            buffers(),
            &KEY,
            DEVICE_HELLO,
        ));
        assert_eq!(ended, Ended::Hello);
        assert!(channel.0.borrow().sent.is_empty());
    }

    #[test]
    fn a_channel_is_not_served_while_the_session_listens_elsewhere() {
        let _serial = SERIAL.lock();
        install(|| Some(leak(BUFFER)), || Some(leak(INBOX)));
        Server.listen(Listen::Network);
        let channel = Channel::default();
        channel.write_raw(&get("/api/status"), 64);
        embassy_futures::block_on(serve_channel(
            channel.clone(),
            channel.clone(),
            64,
            buffers(),
            &KEY,
            DEVICE_HELLO,
        ));
        assert!(channel.0.borrow().sent.is_empty());
    }
}
