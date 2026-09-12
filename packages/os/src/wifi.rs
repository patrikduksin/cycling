//! Wi-Fi station with bounded recovery and a public HTTP connectivity check.
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};
use embassy_executor::Spawner;
use embassy_futures::select::{Either, select};
use embassy_net::{
    Runner, Stack, StackResources,
    dns::DnsQueryType,
    tcp::TcpSocket,
    udp::{PacketMetadata, UdpSocket},
};
use embassy_time::{Duration, Instant, Timer, with_timeout};
use esp_hal::{peripherals::WIFI, rng::Rng};
use esp_radio::wifi::{
    AuthenticationMethod, Config, ControllerConfig, Interface, WifiController, scan::ScanConfig,
    sta::StationConfig,
};
use static_cell::StaticCell;

mod config {
    include!(env!("CYCLING_WIFI_CONFIG"));
}

const LINK_UNCONFIGURED: u8 = 0;
const LINK_CONNECTING: u8 = 1;
const LINK_ASSOCIATED: u8 = 2;
const LINK_RETRYING: u8 = 3;
const LINK_FAILED: u8 = 4;
const PROBE_WAITING: u8 = 0;
const PROBE_READY: u8 = 1;
const PROBE_OK: u8 = 2;
const PROBE_FAILED: u8 = 3;
const REQUEST_MANUAL: u8 = 1;
const REQUEST_RECOVERY: u8 = 2;

static LINK: AtomicU8 = AtomicU8::new(LINK_UNCONFIGURED);
static PROBE: AtomicU8 = AtomicU8::new(PROBE_WAITING);
static REQUEST: AtomicU8 = AtomicU8::new(0);
static RECOVERY_GENERATION: AtomicU32 = AtomicU32::new(0);
static FAULT: AtomicU8 = AtomicU8::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);
static ASSOCIATIONS: AtomicU32 = AtomicU32::new(0);
static PROBE_SUCCESSES: AtomicU32 = AtomicU32::new(0);
static PROBE_FAILURES: AtomicU32 = AtomicU32::new(0);
static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

fn public_state() -> u8 {
    match LINK.load(Ordering::Relaxed) {
        LINK_UNCONFIGURED => 0,
        LINK_CONNECTING => 1,
        LINK_ASSOCIATED => match PROBE.load(Ordering::Relaxed) {
            PROBE_WAITING => 2,
            PROBE_READY => 3,
            PROBE_OK => 4,
            _ => 5,
        },
        LINK_RETRYING => 5,
        _ => 6,
    }
}

pub fn state() -> u8 {
    public_state()
}
pub fn reconnect() {
    REQUEST.store(REQUEST_MANUAL, Ordering::Release);
}
#[cfg(feature = "debug-harness")]
pub fn set_fault(fault: u8) {
    FAULT.store(fault, Ordering::Relaxed);
}
pub fn stats() -> (u32, u32, u32, u8) {
    (
        ASSOCIATIONS.load(Ordering::Relaxed),
        PROBE_SUCCESSES.load(Ordering::Relaxed),
        PROBE_FAILURES.load(Ordering::Relaxed),
        FAULT.load(Ordering::Relaxed),
    )
}

pub fn online() -> bool {
    LINK.load(Ordering::Relaxed) == LINK_ASSOCIATED
}

#[embassy_executor::task]
pub async fn start(peripheral: WIFI<'static>, spawner: Spawner) {
    if config::SSID.is_empty() {
        log::info!(target: "wifi", "CYCLING_WIFI unconfigured");
        return;
    }
    LINK.store(LINK_CONNECTING, Ordering::Relaxed);
    let auth = if config::WPA3 {
        AuthenticationMethod::Wpa3Personal
    } else {
        AuthenticationMethod::Wpa2Personal
    };
    let station = StationConfig::default()
        .with_ssid(config::SSID)
        .with_password(config::PASSWORD.into())
        .with_auth_method(auth);
    let mut controller = match WifiController::new(
        peripheral,
        ControllerConfig::default().with_initial_config(Config::Station(station)),
    ) {
        Ok(controller) => controller,
        Err(_) => {
            LINK.store(LINK_FAILED, Ordering::Relaxed);
            log::warn!(target: "wifi", "CYCLING_WIFI init_failed");
            return;
        }
    };
    log::info!(target: "wifi",
        "CYCLING_WIFI radio_ready heap_free={}",
        esp_alloc::HEAP.free()
    );
    match with_timeout(
        Duration::from_secs(15),
        controller.scan_async(&ScanConfig::default().with_ssid(config::SSID).with_max(8)),
    )
    .await
    {
        Ok(Ok(aps)) => {
            log::info!(target: "wifi", "CYCLING_WIFI scan configured_network_matches={}", aps.len())
        }
        _ => log::warn!(target: "wifi", "CYCLING_WIFI scan_failed"),
    }
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    spawner.spawn(connection(controller, stack).unwrap());
    spawner.spawn(network(runner).unwrap());
    spawner.spawn(verify(stack).unwrap());
    spawner.spawn(time_sync(stack).unwrap());
}

async fn reconnect_requested() -> u8 {
    loop {
        let request = REQUEST.swap(0, Ordering::AcqRel);
        if request == REQUEST_MANUAL
            || (request == REQUEST_RECOVERY
                && RECOVERY_GENERATION.load(Ordering::Acquire)
                    == GENERATION.load(Ordering::Acquire))
        {
            return request;
        }
        Timer::after_millis(250).await;
    }
}

fn request_recovery(generation: u32) {
    if generation == GENERATION.load(Ordering::Acquire)
        && LINK.load(Ordering::Relaxed) == LINK_ASSOCIATED
    {
        RECOVERY_GENERATION.store(generation, Ordering::Release);
        REQUEST.store(REQUEST_RECOVERY, Ordering::Release);
    }
}

async fn retry_or_late_connection(stack: Stack<'_>, seconds: u64) -> bool {
    for _ in 0..seconds * 4 {
        if stack.is_link_up() {
            log::info!(target: "wifi", "CYCLING_WIFI associated_late");
            return true;
        }
        Timer::after_millis(250).await;
    }
    stack.is_link_up()
}

#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>, stack: Stack<'static>) {
    let mut failures = 0u8;
    loop {
        LINK.store(LINK_CONNECTING, Ordering::Relaxed);
        PROBE.store(PROBE_WAITING, Ordering::Relaxed);
        log::info!(target: "wifi",
            "CYCLING_WIFI connecting attempt={}",
            u16::from(failures) + 1
        );
        let connected = if stack.is_link_up() {
            true
        } else {
            match with_timeout(Duration::from_secs(20), controller.connect_async()).await {
                Ok(Ok(_)) => true,
                Ok(Err(_)) => {
                    log::warn!(target: "wifi", "CYCLING_WIFI connect_failed kind=driver");
                    false
                }
                Err(_) => {
                    log::warn!(target: "wifi", "CYCLING_WIFI connect_failed kind=timeout");
                    false
                }
            }
        };
        let connected = if connected {
            true
        } else {
            LINK.store(LINK_RETRYING, Ordering::Relaxed);
            let delay = cycling_os::network::retry_delay_secs(failures);
            failures = failures.saturating_add(1);
            log::info!(target: "wifi", "CYCLING_WIFI retry_in_s={}", delay);
            retry_or_late_connection(stack, delay).await
        };
        if !connected {
            continue;
        }

        failures = 0;
        GENERATION.fetch_add(1, Ordering::Relaxed);
        ASSOCIATIONS.fetch_add(1, Ordering::Relaxed);
        LINK.store(LINK_ASSOCIATED, Ordering::Relaxed);
        PROBE.store(PROBE_WAITING, Ordering::Relaxed);
        log::info!(target: "wifi", "CYCLING_WIFI associated");
        match select(
            controller.wait_for_disconnect_async(),
            reconnect_requested(),
        )
        .await
        {
            Either::First(_) => log::info!(target: "wifi", "CYCLING_WIFI disconnected"),
            Either::Second(reason) => {
                GENERATION.fetch_add(1, Ordering::Relaxed);
                LINK.store(LINK_RETRYING, Ordering::Relaxed);
                PROBE.store(PROBE_WAITING, Ordering::Relaxed);
                log::info!(target: "wifi",
                    "CYCLING_WIFI disconnecting reason={}",
                    if reason == REQUEST_MANUAL {
                        "test"
                    } else {
                        "recovery"
                    }
                );
                match with_timeout(Duration::from_secs(5), controller.disconnect_async()).await {
                    Ok(Ok(_)) => {}
                    Ok(Err(_)) => {
                        log::warn!(target: "wifi", "CYCLING_WIFI disconnect_failed kind=driver")
                    }
                    Err(_) => {
                        log::warn!(target: "wifi", "CYCLING_WIFI disconnect_failed kind=timeout")
                    }
                }
            }
        }
        if LINK.load(Ordering::Relaxed) != LINK_RETRYING {
            GENERATION.fetch_add(1, Ordering::Relaxed);
            LINK.store(LINK_RETRYING, Ordering::Relaxed);
            PROBE.store(PROBE_WAITING, Ordering::Relaxed);
        }
        Timer::after_secs(1).await;
    }
}

#[embassy_executor::task]
async fn network(mut runner: Runner<'static, Interface>) {
    runner.run().await
}

#[embassy_executor::task]
async fn verify(stack: Stack<'static>) {
    let mut successes = 0u32;
    let mut probe_failures = 0u8;
    let mut dhcp_failures = 0u8;
    loop {
        if with_timeout(Duration::from_secs(20), stack.wait_config_up())
            .await
            .is_err()
        {
            log::warn!(target: "wifi", "CYCLING_WIFI dhcp_failed kind=timeout");
            PROBE.store(PROBE_FAILED, Ordering::Relaxed);
            if LINK.load(Ordering::Relaxed) == LINK_ASSOCIATED {
                let generation = GENERATION.load(Ordering::Relaxed);
                let delay = cycling_os::network::retry_delay_secs(dhcp_failures);
                dhcp_failures = dhcp_failures.saturating_add(1);
                log::info!(target: "wifi", "CYCLING_WIFI dhcp_retry_in_s={}", delay);
                Timer::after_secs(delay).await;
                if generation == GENERATION.load(Ordering::Relaxed)
                    && LINK.load(Ordering::Relaxed) == LINK_ASSOCIATED
                    && !stack.is_config_up()
                {
                    request_recovery(generation);
                }
            }
            continue;
        }
        dhcp_failures = 0;
        let generation = GENERATION.load(Ordering::Relaxed);
        PROBE.store(PROBE_READY, Ordering::Relaxed);
        log::info!(target: "wifi", "CYCLING_WIFI dhcp_ready");
        let result = with_timeout(Duration::from_secs(15), probe(stack)).await;
        if generation != GENERATION.load(Ordering::Relaxed)
            || LINK.load(Ordering::Relaxed) != LINK_ASSOCIATED
            || !stack.is_config_up()
        {
            log::info!(target: "wifi", "CYCLING_WIFI request_stale");
            PROBE.store(PROBE_WAITING, Ordering::Relaxed);
            continue;
        }
        match result {
            Ok(Ok(())) => {
                successes = successes.saturating_add(1);
                PROBE_SUCCESSES.fetch_add(1, Ordering::Relaxed);
                probe_failures = 0;
                PROBE.store(PROBE_OK, Ordering::Relaxed);
                log::info!(target: "wifi",
                    "CYCLING_WIFI http_verified count={} heap_free={}",
                    successes,
                    esp_alloc::HEAP.free()
                );
                stack.wait_config_down().await;
                continue;
            }
            Ok(Err(kind)) => {
                PROBE_FAILURES.fetch_add(1, Ordering::Relaxed);
                PROBE.store(PROBE_FAILED, Ordering::Relaxed);
                log::warn!(target: "wifi", "CYCLING_WIFI request_failed kind={}", kind);
            }
            Err(_) => {
                PROBE_FAILURES.fetch_add(1, Ordering::Relaxed);
                PROBE.store(PROBE_FAILED, Ordering::Relaxed);
                log::warn!(target: "wifi", "CYCLING_WIFI request_failed kind=timeout");
            }
        }
        let delay = cycling_os::network::retry_delay_secs(probe_failures);
        probe_failures = probe_failures.saturating_add(1);
        log::info!(target: "wifi", "CYCLING_WIFI request_retry_in_s={}", delay);
        let _ = with_timeout(Duration::from_secs(delay), stack.wait_config_down()).await;
    }
}

async fn probe(stack: Stack<'static>) -> Result<(), &'static str> {
    if FAULT.load(Ordering::Relaxed) == 1 {
        return Err("injected_dns");
    }
    let addresses = stack
        .dns_query("example.com", DnsQueryType::A)
        .await
        .map_err(|_| "dns")?;
    if FAULT.load(Ordering::Relaxed) == 2 {
        return Err("injected_request");
    }
    let host = *addresses.first().ok_or("dns_empty")?;
    let mut rx = [0; 1024];
    let mut tx = [0; 512];
    let mut socket = TcpSocket::new(stack, &mut rx, &mut tx);
    socket.set_timeout(Some(Duration::from_secs(10)));
    socket.connect((host, 80)).await.map_err(|_| "connect")?;
    let mut request =
        b"GET / HTTP/1.0\r\nHost: example.com\r\nConnection: close\r\n\r\n".as_slice();
    while !request.is_empty() {
        let n = socket.write(request).await.map_err(|_| "write")?;
        if n == 0 {
            return Err("write_zero");
        }
        request = &request[n..];
    }
    let mut response = [0u8; 4096];
    let mut used = 0;
    loop {
        if used == response.len() {
            return Err("response_large");
        }
        let n = socket
            .read(&mut response[used..])
            .await
            .map_err(|_| "read")?;
        if n == 0 {
            break;
        }
        used += n;
    }
    cycling_os::network::verified_response(&response[..used])
        .then_some(())
        .ok_or("response")
}

#[embassy_executor::task]
async fn time_sync(stack: Stack<'static>) {
    let mut retry_not_before = 0u64;
    loop {
        stack.wait_config_up().await;
        let mut failures = 0u8;
        while stack.is_config_up() {
            let now = Instant::now().as_millis();
            if now < retry_not_before {
                if matches!(
                    select(
                        stack.wait_config_down(),
                        Timer::after_millis(retry_not_before - now),
                    )
                    .await,
                    Either::First(_)
                ) {
                    break;
                }
                continue;
            }
            cycling_os::network_time::set_syncing(true);
            let generation = GENERATION.load(Ordering::Acquire);
            let result = with_timeout(Duration::from_secs(10), sync_time(stack)).await;
            let current = generation == GENERATION.load(Ordering::Acquire) && stack.is_config_up();
            match result {
                Ok(Ok((timestamp, rtt_ms))) if current => {
                    let now = Instant::now().as_millis();
                    cycling_os::network_time::update(timestamp, now);
                    failures = 0;
                    log::info!(target: "wifi",
                        "CYCLING_TIME synced unix={} stratum={} rtt_ms={}",
                        timestamp.unix_seconds, timestamp.stratum, rtt_ms
                    );
                    match select(stack.wait_config_down(), Timer::after_secs(60 * 60)).await {
                        Either::First(_) => cycling_os::network_time::set_syncing(false),
                        Either::Second(_) => {}
                    }
                }
                result => {
                    cycling_os::network_time::set_syncing(false);
                    if current {
                        if matches!(result, Ok(Err("denied"))) {
                            log::info!(target: "wifi", "CYCLING_TIME sync_disabled kind=denied until=restart");
                            return;
                        }
                        let (kind, delay) = if matches!(result, Ok(Err("rate_limited"))) {
                            retry_not_before = Instant::now().as_millis() + 15 * 60 * 1_000;
                            ("rate_limited", 15 * 60)
                        } else {
                            let delay = cycling_os::network::retry_delay_secs(failures);
                            failures = failures.saturating_add(1);
                            ("request", delay)
                        };
                        log::info!(target: "wifi",
                            "CYCLING_TIME sync_failed kind={} retry_in_s={}",
                            kind, delay
                        );
                        if matches!(
                            select(stack.wait_config_down(), Timer::after_secs(delay)).await,
                            Either::First(_)
                        ) {
                            break;
                        }
                    }
                }
            }
        }
    }
}

async fn sync_time(
    stack: Stack<'static>,
) -> Result<(cycling_os::network_time::Timestamp, u64), &'static str> {
    let addresses = with_timeout(
        Duration::from_secs(5),
        stack.dns_query("time.cloudflare.com", DnsQueryType::A),
    )
    .await
    .map_err(|_| "dns_timeout")?
    .map_err(|_| "dns")?;
    let host = *addresses.first().ok_or("dns_empty")?;
    let mut rx_meta = [PacketMetadata::EMPTY];
    let mut tx_meta = [PacketMetadata::EMPTY];
    let mut socket_rx = [0; 512];
    let mut tx = [0; 48];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut socket_rx, &mut tx_meta, &mut tx);
    socket.bind(0).map_err(|_| "bind")?;
    let mut request = [0u8; 48];
    request[0] = 0x23;
    let sent_ms = Instant::now().as_millis();
    request[40..48].copy_from_slice(&sent_ms.to_be_bytes());
    socket
        .send_to(&request, (host, 123))
        .await
        .map_err(|_| "send")?;
    let mut response = [0; 512];
    let (length, source) = with_timeout(Duration::from_secs(5), socket.recv_from(&mut response))
        .await
        .map_err(|_| "response_timeout")?
        .map_err(|_| "response")?;
    let timestamp = cycling_os::network_time::parse_response(
        &response[..length],
        request[40..48].try_into().unwrap(),
        source.endpoint == (host, 123).into(),
    )
    .map_err(|error| match error {
        cycling_os::network_time::ParseError::RateLimited => "rate_limited",
        cycling_os::network_time::ParseError::Denied => "denied",
        _ => "invalid_response",
    })?;
    Ok((
        timestamp,
        Instant::now().as_millis().saturating_sub(sent_ms),
    ))
}
