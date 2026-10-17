//! Local HTTP transport for the phone companion: accepts connections on port
//! 80 of the access point and station stacks while the application listens,
//! parses one request head per connection and hands it over through one
//! shared exchange. The application writes the whole response into the
//! exchange's buffer; these tasks drain it into the socket and close.
//!
//! One request is answered at a time. Each stack has two listeners, so one
//! accepts the client's next request while the other is still closing; a
//! connection that finds the exchange busy waits for it. Every wait is
//! bounded, and a peer that stops reading loses its request, which the
//! application sees as `request()` changing.
use core::cell::RefCell;
use critical_section::Mutex;
use device_api::server::Request;
use embassy_net::Stack;
use embassy_net::tcp::TcpSocket;
use embassy_time::Duration;
use embassy_time::Instant;
use embassy_time::Timer;
use embassy_time::with_timeout;
use firmware_services::http::{Head, Parse, parse};

/// Response bytes the application can queue ahead of the socket.
const BUFFER: usize = 4096;
/// Connections waiting for the exchange give up after this long.
const QUEUE_MS: u64 = 15_000;
/// A request head must arrive within this time.
const HEAD_MS: u64 = 5_000;
/// The application must produce bytes within this time.
const STALL_MS: u64 = 10_000;

struct Exchange {
    listening: bool,
    request: Option<Request>,
    next_id: u32,
    /// In external RAM, installed by the first transport task.
    buffer: &'static mut [u8],
    queued: usize,
    finished: bool,
    served: u32,
    refused: u32,
    dropped: u32,
}

static EXCHANGE: Mutex<RefCell<Exchange>> = Mutex::new(RefCell::new(Exchange {
    listening: false,
    request: None,
    next_id: 1,
    buffer: &mut [],
    queued: 0,
    finished: false,
    served: 0,
    refused: 0,
    dropped: 0,
}));

fn with<R>(f: impl FnOnce(&mut Exchange) -> R) -> R {
    critical_section::with(|cs| f(&mut EXCHANGE.borrow_ref_mut(cs)))
}

/// Requests answered, refused by the transport, and dropped mid-response.
pub fn stats() -> (u32, u32, u32) {
    with(|e| (e.served, e.refused, e.dropped))
}

/// The application side of the exchange.
pub struct Server;

impl device_api::server::Server for Server {
    fn listen(&mut self, on: bool) {
        with(|e| e.listening = on);
    }
    fn request(&self) -> Option<Request> {
        with(|e| e.request.clone().filter(|_| !e.finished))
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
}

async fn write_all(socket: &mut TcpSocket<'_>, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        match socket.write(bytes).await {
            Ok(0) | Err(_) => return false,
            Ok(n) => bytes = &bytes[n..],
        }
    }
    true
}

/// Answer without the application, for heads the transport refuses.
async fn refuse(socket: &mut TcpSocket<'_>, status: u16) {
    with(|e| e.refused += 1);
    if let Some(head) = Head::new(status).length(0).finish() {
        let _ = write_all(socket, head.bytes()).await;
    }
}

/// Claim the exchange for `request`, waiting for an earlier one to finish.
async fn claim(mut request: Request) -> Option<u32> {
    let deadline = Instant::now() + Duration::from_millis(QUEUE_MS);
    loop {
        let claimed = with(|e| {
            if !e.listening {
                return Some(None);
            }
            if e.request.is_some() {
                return None;
            }
            request.id = e.next_id;
            e.next_id = e.next_id.wrapping_add(1).max(1);
            e.queued = 0;
            e.finished = false;
            e.request = Some(request.clone());
            Some(Some(request.id))
        });
        match claimed {
            Some(result) => return result,
            None if Instant::now() >= deadline => return None,
            None => Timer::after_millis(5).await,
        }
    }
}

fn release(id: u32, complete: bool) {
    with(|e| {
        if e.request.as_ref().is_some_and(|r| r.id == id) {
            e.request = None;
            e.queued = 0;
            e.finished = false;
            if complete {
                e.served += 1;
            } else {
                e.dropped += 1;
            }
        }
    });
}

/// Move queued response bytes to the socket until the application finishes.
async fn respond(socket: &mut TcpSocket<'_>, id: u32, chunk: &mut [u8]) -> bool {
    let mut idle_since = Instant::now();
    loop {
        let (n, finished) = with(|e| {
            let n = e.queued.min(chunk.len());
            chunk[..n].copy_from_slice(&e.buffer[..n]);
            e.buffer.copy_within(n..e.queued, 0);
            e.queued -= n;
            (n, e.finished && e.queued == 0)
        });
        if n > 0 {
            if !write_all(socket, &chunk[..n]).await {
                return false;
            }
            idle_since = Instant::now();
            continue;
        }
        if finished {
            return true;
        }
        let ours = with(|e| e.listening && e.request.as_ref().is_some_and(|r| r.id == id));
        if !ours || idle_since.elapsed().as_millis() > STALL_MS {
            return false;
        }
        Timer::after_millis(2).await;
    }
}

/// Socket receive bytes: requests are one small head.
const RX: usize = 1024;
/// Socket send bytes: two segments, so the phone acknowledges promptly.
const TX: usize = 2920;
const CHUNK: usize = 512;
const HEAD: usize = firmware_services::http::MAX_HEAD;

/// One of the listeners on port 80 of `stack`.
#[embassy_executor::task(pool_size = 4)]
pub async fn serve(stack: Stack<'static>) {
    // Buffers live in external RAM, so idle tasks cost internal RAM only
    // their state. Only tasks touch them; flash writes block the executor.
    let Some(block) = crate::drivers::psram::zeroed(RX + TX + CHUNK + HEAD) else {
        log::warn!(target: "http", "CYCLING_HTTP no external RAM for buffers");
        return;
    };
    with(|e| {
        if e.buffer.is_empty()
            && let Some(buffer) = crate::drivers::psram::zeroed(BUFFER)
        {
            e.buffer = buffer;
        }
    });
    let (rx, rest) = block.split_at_mut(RX);
    let (tx, rest) = rest.split_at_mut(TX);
    let (chunk, head) = rest.split_at_mut(CHUNK);
    loop {
        if !with(|e| e.listening) || !stack.is_config_up() {
            Timer::after_millis(200).await;
            continue;
        }
        let mut socket = TcpSocket::new(stack, &mut *rx, &mut *tx);
        socket.set_timeout(Some(Duration::from_secs(10)));
        // Re-check listening while parked in accept, so a closed session
        // stops answering promptly.
        let accepted = embassy_futures::select::select(socket.accept(80), async {
            while with(|e| e.listening) {
                Timer::after_millis(250).await;
            }
        })
        .await;
        if !matches!(accepted, embassy_futures::select::Either::First(Ok(()))) {
            socket.abort();
            let _ = with_timeout(Duration::from_millis(100), socket.flush()).await;
            continue;
        }
        let mut used = 0;
        let parsed = with_timeout(Duration::from_millis(HEAD_MS), async {
            loop {
                match socket.read(&mut head[used..]).await {
                    Ok(0) | Err(_) => return None,
                    Ok(n) => used += n,
                }
                match parse(&head[..used]) {
                    Parse::Incomplete => continue,
                    result => return Some(result),
                }
            }
        })
        .await;
        match parsed {
            Ok(Some(Parse::Request(request))) => match claim(request).await {
                Some(id) => {
                    let complete = respond(&mut socket, id, chunk).await;
                    release(id, complete);
                }
                None => refuse(&mut socket, 503).await,
            },
            Ok(Some(Parse::Refuse(status))) => refuse(&mut socket, status).await,
            _ => {}
        }
        socket.close();
        let _ = with_timeout(Duration::from_secs(2), socket.flush()).await;
        socket.abort();
    }
}
