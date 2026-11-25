//! Local HTTP over TCP for the phone companion: accepts connections on port
//! 80 of the access point and station stacks while the application listens
//! on the network, parses one request head per connection, serves it through
//! the shared exchange and closes.
//!
//! Each stack has two listeners, so one accepts the client's next request
//! while the other is still closing; a connection that finds the exchange
//! busy waits for it.
use crate::drivers::exchange::{self, Rx, Tx};
use device_api::server::Listen;
use embassy_net::Stack;
use embassy_net::tcp::{TcpReader, TcpSocket, TcpWriter};
use embassy_time::Duration;
use embassy_time::Timer;
use embassy_time::with_timeout;
use firmware_services::http::Parse;

impl Rx for TcpReader<'_> {
    async fn read(&mut self, out: &mut [u8]) -> Option<usize> {
        match TcpReader::read(self, out).await {
            Ok(0) | Err(_) => None,
            Ok(n) => Some(n),
        }
    }
}

impl Tx for TcpWriter<'_> {
    async fn write(&mut self, mut bytes: &[u8]) -> bool {
        while !bytes.is_empty() {
            match TcpWriter::write(self, bytes).await {
                Ok(0) | Err(_) => return false,
                Ok(n) => bytes = &bytes[n..],
            }
        }
        true
    }
}

/// Socket receive bytes: a head, or a stretch of an upload's body.
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
    let Some(block) = crate::drivers::psram::zeroed(RX + TX + 2 * CHUNK + HEAD) else {
        log::warn!(target: "http", "CYCLING_HTTP no external RAM for buffers");
        return;
    };
    exchange::install(
        || crate::drivers::psram::zeroed(exchange::BUFFER),
        || crate::drivers::psram::zeroed(exchange::INBOX),
    );
    let (rx, rest) = block.split_at_mut(RX);
    let (tx, rest) = rest.split_at_mut(TX);
    let (into, rest) = rest.split_at_mut(CHUNK);
    let (out, head) = rest.split_at_mut(CHUNK);
    let network = || exchange::listening() == Listen::Network;
    loop {
        if !network() || !stack.is_config_up() {
            Timer::after_millis(200).await;
            continue;
        }
        let mut socket = TcpSocket::new(stack, &mut *rx, &mut *tx);
        socket.set_timeout(Some(Duration::from_secs(10)));
        // Re-check listening while parked in accept, so a closed session
        // stops answering promptly.
        let accepted = embassy_futures::select::select(socket.accept(80), async {
            while network() {
                Timer::after_millis(250).await;
            }
        })
        .await;
        if !matches!(accepted, embassy_futures::select::Either::First(Ok(()))) {
            socket.abort();
            let _ = with_timeout(Duration::from_millis(100), socket.flush()).await;
            continue;
        }
        {
            let (mut reader, mut writer) = socket.split();
            let mut used = 0;
            let parsed = with_timeout(
                Duration::from_millis(exchange::HEAD_MS),
                exchange::read_head(&mut reader, head, &mut used),
            )
            .await;
            match parsed {
                Ok(Some(Parse::Request {
                    request,
                    head: length,
                })) => match exchange::claim(request, &head[length..used], Listen::Network).await {
                    // The connection closes, so owed body bytes need no skipping.
                    Some(claim) => {
                        let _ = exchange::respond(&mut reader, &mut writer, claim, into, out).await;
                    }
                    None => exchange::refuse(&mut writer, 503).await,
                },
                Ok(Some(Parse::Refuse(status))) => exchange::refuse(&mut writer, status).await,
                _ => {}
            }
        }
        socket.close();
        let _ = with_timeout(Duration::from_secs(2), socket.flush()).await;
        socket.abort();
    }
}
