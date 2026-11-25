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
//! phone's Bluetooth channel (`serve_channel`), request after request. The
//! application says which one it listens to, and a request is claimed only
//! from that one, so a Bluetooth session never answers the network.
use core::cell::{Cell, RefCell};
use critical_section::Mutex;
use device_api::server::{Listen, PhoneLink, Request};
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

struct Exchange {
    listening: Listen,
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

impl<R: ReceiveSdu> Rx for Bytes<'_, R> {
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
}

impl<R: ReceiveSdu> Bytes<'_, R> {
    /// Pass over `count` bytes, such as a refused request's body.
    async fn skip(&mut self, mut count: u32, scratch: &mut [u8]) -> Option<()> {
        while count > 0 {
            let wanted = (count as usize).min(scratch.len());
            let n = with_timeout(
                Duration::from_millis(STALL_MS),
                self.read(&mut scratch[..wanted]),
            )
            .await
            .ok()??;
            count -= n as u32;
        }
        Some(())
    }
}

/// Responses as frames: `u16` little-endian length, then that many bytes; a
/// frame of length zero ends a response. Each SDU carries one frame.
struct Frames<'a, S> {
    tx: S,
    frame: &'a mut [u8],
}

impl<S: SendSdu> Tx for Frames<'_, S> {
    async fn write(&mut self, bytes: &[u8]) -> bool {
        for part in bytes.chunks(self.frame.len() - 2) {
            self.frame[..2].copy_from_slice(&(part.len() as u16).to_le_bytes());
            self.frame[2..2 + part.len()].copy_from_slice(part);
            if !self.tx.send(&self.frame[..2 + part.len()]).await {
                return false;
            }
        }
        true
    }
    async fn end(&mut self) -> bool {
        self.tx.send(&[0, 0]).await
    }
}

/// Working memory for one channel, in external RAM on the device.
pub(crate) struct ChannelBuffers<'a> {
    /// At least the SDU size this side accepts.
    pub sdu: &'a mut [u8],
    pub head: &'a mut [u8],
    pub into: &'a mut [u8],
    pub out: &'a mut [u8],
}

/// Serve a phone's channel, request after request, until it closes, the
/// session stops listening on Bluetooth, or a response cannot complete.
/// `piece` is the largest SDU both sides take.
pub(crate) async fn serve_channel(
    rx: impl ReceiveSdu,
    tx: impl SendSdu,
    piece: usize,
    buffers: ChannelBuffers<'_>,
) {
    let ChannelBuffers {
        sdu,
        head,
        into,
        out,
    } = buffers;
    let (frame, out) = out.split_at_mut(piece.clamp(3, out.len() / 2));
    let mut rx = Bytes {
        rx,
        sdu,
        at: 0,
        len: 0,
    };
    let mut tx = Frames { tx, frame };
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
            read_head(&mut rx, head, &mut used),
        )
        .await;
        let (request, length) = match parsed {
            Ok(Some(Parse::Request { request, head })) => (request, head),
            Ok(Some(Parse::Refuse(status))) => {
                // Where this request ends is unknown: close the channel.
                refuse(&mut tx, status).await;
                return;
            }
            _ => return,
        };
        let declared = request.body;
        let body = (used - length).min(declared as usize);
        let owed = match claim(request, &head[length..length + body], Listen::Bluetooth).await {
            Some(claim) => match respond(&mut rx, &mut tx, claim, into, out).await {
                Some(owed) => owed,
                None => return,
            },
            None => {
                refuse(&mut tx, 503).await;
                declared - body as u32
            }
        };
        if rx.skip(owed, into).await.is_none() {
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

    /// Both ends of a fake L2CAP channel: SDUs the phone sent that are not
    /// read yet, and SDUs the device sent.
    #[derive(Default)]
    struct Wire {
        incoming: VecDeque<Vec<u8>>,
        sent: Vec<Vec<u8>>,
        closed: bool,
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
        /// The phone writes `bytes` in SDUs of at most `piece`.
        fn write(&self, bytes: &[u8], piece: usize) {
            let mut wire = self.0.borrow_mut();
            wire.incoming
                .extend(bytes.chunks(piece).map(<[u8]>::to_vec));
        }

        /// Wait for the next whole response; panics on a malformed frame or
        /// an SDU longer than `piece`.
        async fn response(&self, piece: usize) -> String {
            loop {
                if let Some(response) = self.take_response(piece) {
                    return response;
                }
                Timer::after_millis(1).await;
            }
        }

        fn take_response(&self, piece: usize) -> Option<String> {
            let sent = &mut self.0.borrow_mut().sent;
            let end = sent.iter().position(|sdu| sdu == &[0, 0])?;
            let mut bytes = Vec::new();
            for sdu in sent.drain(..=end).take(end) {
                assert!(sdu.len() <= piece, "an SDU of {} bytes", sdu.len());
                let length = u16::from_le_bytes([sdu[0], sdu[1]]) as usize;
                assert_eq!(sdu.len(), 2 + length, "one frame per SDU");
                bytes.extend_from_slice(&sdu[2..]);
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
            head: leak(firmware_services::http::MAX_HEAD),
            into: leak(512),
            out: leak(512 + 256),
        }
    }

    /// Serve `channel` with an SDU limit of `piece` while `phone` runs.
    fn run(channel: &Channel, piece: usize, phone: impl Future<Output = ()>) {
        install(|| Some(leak(BUFFER)), || Some(leak(INBOX)));
        Server.listen(Listen::Bluetooth);
        let device = serve_channel(channel.clone(), channel.clone(), piece, buffers());
        let phone = async {
            phone.await;
            channel.0.borrow_mut().closed = true;
        };
        embassy_futures::block_on(embassy_futures::select::select(
            embassy_futures::join::join(device, phone),
            application(),
        ));
        assert!(Server.request().is_none(), "the exchange is free again");
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
        run(&channel, 23, async {
            channel.write(&get("/api/status"), 7);
            let first = channel.response(23).await;
            assert!(first.starts_with("HTTP/1.1 200 OK\r\n"), "{first}");
            let body = body_of(&first);
            assert!(body.starts_with("Get /api/status "), "{body}");
            assert_eq!(body.len(), 300 + "Get /api/status  ".len());
            channel.write(&get("/api/workouts"), 61);
            let second = channel.response(23).await;
            assert!(
                body_of(&second).starts_with("Get /api/workouts "),
                "{second}"
            );
        });
    }

    #[test]
    fn a_body_split_across_sdus_reaches_the_application_and_the_next_request_follows_it() {
        let _serial = SERIAL.lock();
        let channel = Channel::default();
        run(&channel, 122, async {
            let mut bytes =
                b"PUT /api/workouts HTTP/1.1\r\nContent-Length: 10\r\n\r\n0123456789".to_vec();
            bytes.extend_from_slice(&get("/api/status"));
            channel.write(&bytes, 9);
            let put = channel.response(122).await;
            assert!(
                body_of(&put).starts_with("Put /api/workouts 0123456789 "),
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

    #[test]
    fn a_channel_is_not_served_while_the_session_listens_elsewhere() {
        let _serial = SERIAL.lock();
        install(|| Some(leak(BUFFER)), || Some(leak(INBOX)));
        Server.listen(Listen::Network);
        let channel = Channel::default();
        channel.write(&get("/api/status"), 64);
        embassy_futures::block_on(serve_channel(
            channel.clone(),
            channel.clone(),
            64,
            buffers(),
        ));
        assert!(channel.0.borrow().sent.is_empty());
    }
}
