//! Wi-Fi station with bounded recovery and a public HTTP connectivity check.
use core::sync::atomic::AtomicBool;
use core::sync::atomic::AtomicU8;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;
use embassy_executor::Spawner;
use embassy_futures::select::Either;
use embassy_futures::select::select;
use embassy_net::Runner;
use embassy_net::Stack;
use embassy_net::StackResources;
use embassy_net::dns::DnsQueryType;
use embassy_net::tcp::TcpSocket;
use embassy_net::udp::PacketMetadata;
use embassy_net::udp::UdpSocket;
use embassy_time::Duration;
use embassy_time::Instant;
use embassy_time::Timer;
use embassy_time::with_timeout;
use esp_hal::peripherals::WIFI;
use esp_hal::rng::Rng;
use esp_radio::wifi::AuthenticationMethod;
use esp_radio::wifi::Config;
use esp_radio::wifi::ControllerConfig;
use esp_radio::wifi::Interface;
use esp_radio::wifi::WifiController;
use esp_radio::wifi::scan::ScanConfig;
use esp_radio::wifi::sta::StationConfig;
use firmware_services::connectivity::Reconnect;
use firmware_services::network_time::ParseError;
use firmware_services::network_time::Timestamp;
use static_cell::StaticCell;

use crate::drivers::radio_control::DISCOVERIES;
use crate::drivers::radio_control::Shared;
use core::cell::RefCell;
use critical_section::Mutex;
use device_api::connectivity::ControlError;
use device_api::connectivity::ControlStatus;
use device_api::connectivity::NetworkDiscovery;
use device_api::connectivity::Text;
use device_api::connectivity::WifiConfig;
use device_api::connectivity::WifiOperation;
use device_api::connectivity::WifiState;
use device_api::connectivity::WifiStats;
use device_api::observation::Error;
use device_api::power::PeripheralState;

/// Power, command admission and discoveries. Public driver APIs cannot cancel
/// an in-flight scan or association after a timeout, so those mark it
/// uncertain for the rest of the boot; see `radio_control`.
static CONTROL: Shared<WifiOperation, NetworkDiscovery> = Shared::new();

pub fn power_status() -> PeripheralState {
    CONTROL.power()
}
fn suspended() -> bool {
    CONTROL.suspended()
}
pub fn power_request(suspend: bool) -> Result<(), Error> {
    CONTROL.power_request(suspend, || {
        RECOVERY_REQUESTED.store(false, Ordering::Release);
        GENERATION.fetch_add(1, Ordering::AcqRel);
    })
}
pub fn control() -> ControlStatus {
    CONTROL.status()
}
pub fn discoveries() -> [Option<NetworkDiscovery>; DISCOVERIES] {
    CONTROL.discoveries()
}
pub fn request(operation: WifiOperation) -> Result<(), Error> {
    CONTROL.request(operation)
}

static CONFIGURATION: Mutex<RefCell<Option<WifiConfig>>> = Mutex::new(RefCell::new(None));

/// Station association as seen by the connection owner.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Link {
    Unconfigured,
    Connecting,
    Associated,
    Retrying,
    Failed,
}
impl Link {
    fn load() -> Self {
        match LINK.load(Ordering::Relaxed) {
            0 => Self::Unconfigured,
            1 => Self::Connecting,
            2 => Self::Associated,
            3 => Self::Retrying,
            _ => Self::Failed,
        }
    }
    fn store(self) {
        LINK.store(self as u8, Ordering::Relaxed);
    }
}

/// DHCP and connectivity-check progress on the current association.
#[derive(Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Probe {
    Waiting,
    Ready,
    Verified,
    Failed,
}
impl Probe {
    fn load() -> Self {
        match PROBE.load(Ordering::Relaxed) {
            0 => Self::Waiting,
            1 => Self::Ready,
            2 => Self::Verified,
            _ => Self::Failed,
        }
    }
    fn store(self) {
        PROBE.store(self as u8, Ordering::Relaxed);
    }
}

static LINK: AtomicU8 = AtomicU8::new(Link::Unconfigured as u8);
static PROBE: AtomicU8 = AtomicU8::new(Probe::Waiting as u8);
static RECOVERY_REQUESTED: AtomicBool = AtomicBool::new(false);
static RECOVERY_GENERATION: AtomicU32 = AtomicU32::new(0);
static FAULT: AtomicU8 = AtomicU8::new(0);
static GENERATION: AtomicU32 = AtomicU32::new(0);
static ASSOCIATIONS: AtomicU32 = AtomicU32::new(0);
static PROBE_SUCCESSES: AtomicU32 = AtomicU32::new(0);
static PROBE_FAILURES: AtomicU32 = AtomicU32::new(0);
static RESOURCES: StaticCell<StackResources<4>> = StaticCell::new();

pub fn state() -> WifiState {
    match Link::load() {
        Link::Unconfigured => WifiState::Unconfigured,
        Link::Connecting => WifiState::Connecting,
        Link::Associated => match Probe::load() {
            Probe::Waiting => WifiState::Associated,
            Probe::Ready => WifiState::AddressReady,
            Probe::Verified => WifiState::Verified,
            Probe::Failed => WifiState::Retrying,
        },
        Link::Retrying => WifiState::Retrying,
        Link::Failed => WifiState::Failed,
    }
}
#[cfg(feature = "debug-harness")]
pub fn set_fault(fault: u8) {
    FAULT.store(fault, Ordering::Relaxed);
}
pub fn stats() -> WifiStats {
    WifiStats {
        associations: ASSOCIATIONS.load(Ordering::Relaxed),
        probe_successes: PROBE_SUCCESSES.load(Ordering::Relaxed),
        probe_failures: PROBE_FAILURES.load(Ordering::Relaxed),
        fault: FAULT.load(Ordering::Relaxed),
    }
}

pub fn online() -> bool {
    !suspended() && Link::load() == Link::Associated
}

/// Initialize the station and return Embassy's actual DNS/TCP/UDP capability.
///
/// None means unconfigured or failed radio initialization. Some means the stack
/// and its runner exist, not that association, DHCP or internet access succeeded.
/// Await `stack.wait_config_up()` under a caller deadline before data operations.
/// The same handle survives reconnects. Compare `connection_generation()` around
/// awaited operations when a result must belong to the same association.
///
/// Stack is Copy but !Send/!Sync: pass it only among tasks on this executor. Do
/// not wrap it in an unsafe Send/global singleton. Callers own socket buffers,
/// bound DNS/connect/read/write waits, and cancel by dropping their operation and
/// socket. The four-socket pool is shared with DHCP, DNS, probe and time clients;
/// additional consumers must coordinate socket use rather than assume capacity.
/// Consuming WIFI prevents a second initialization through the safe interface.
pub async fn initialize(peripheral: WIFI<'static>, spawner: Spawner) -> Option<Stack<'static>> {
    let station = StationConfig::default();
    let controller = match WifiController::new(
        peripheral,
        ControllerConfig::default().with_initial_config(Config::Station(station)),
    ) {
        Ok(controller) => controller,
        Err(_) => {
            Link::Failed.store();
            log::warn!(target: "wifi", "CYCLING_WIFI init_failed");
            return None;
        }
    };
    log::info!(target: "wifi",
        "CYCLING_WIFI radio_ready heap_free={}",
        esp_alloc::HEAP.free()
    );
    let rng = Rng::new();
    let seed = (u64::from(rng.random()) << 32) | u64::from(rng.random());
    let (stack, runner) = embassy_net::new(
        Interface::station(),
        embassy_net::Config::dhcpv4(Default::default()),
        RESOURCES.init(StackResources::new()),
        seed,
    );
    CONTROL.set_ready();
    spawner.spawn(connection(controller).unwrap());
    spawner.spawn(network(runner).unwrap());
    spawner.spawn(verify(stack).unwrap());
    Some(stack)
}

/// Association generation used by existing probe/SNTP recovery validation.
/// A changed generation invalidates results tied to the previous connection.
pub fn connection_generation() -> u32 {
    GENERATION.load(Ordering::Acquire)
}

/// The next user command or an automatic recovery request, or None once a
/// power transition needs the owner's attention.
async fn next_command() -> Option<(WifiOperation, bool)> {
    loop {
        if CONTROL.with(|c| c.suspended() || c.power() == PeripheralState::Pending) {
            return None;
        }
        if let Some(operation) = CONTROL.take_command() {
            return Some((operation, false));
        }
        if RECOVERY_REQUESTED.swap(false, Ordering::AcqRel)
            && RECOVERY_GENERATION.load(Ordering::Acquire) == GENERATION.load(Ordering::Acquire)
        {
            return Some((WifiOperation::Connect, true));
        }
        CONTROL.changed().await;
    }
}

fn request_recovery(generation: u32) {
    if !suspended()
        && generation == GENERATION.load(Ordering::Acquire)
        && Link::load() == Link::Associated
    {
        RECOVERY_GENERATION.store(generation, Ordering::Release);
        RECOVERY_REQUESTED.store(true, Ordering::Release);
        CONTROL.notify();
    }
}

/// Owner loop: power transitions first, then one command, automatic recovery
/// or link-loss event at a time.
#[embassy_executor::task]
async fn connection(mut controller: WifiController<'static>) {
    let mut retry = Reconnect::default();
    loop {
        if suspended() {
            if power_status() == PeripheralState::Pending {
                quiesce(&mut controller).await;
            } else {
                CONTROL.changed().await;
            }
            continue;
        }
        if CONTROL.complete_resume() == PeripheralState::Failed {
            CONTROL.changed().await;
            continue;
        }
        let Some((command, automatic)) = next_work(&mut controller, &mut retry).await else {
            continue;
        };
        // A suspend request may arrive while an automatic operation is waiting.
        if suspended() {
            continue;
        }
        perform(&mut controller, &mut retry, command, automatic).await;
    }
}

/// Disconnect the station and report whether it is confirmed inactive.
async fn quiesce(controller: &mut WifiController<'static>) {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    Probe::Waiting.store();
    let disconnected = if controller.is_connected() {
        matches!(
            with_timeout(Duration::from_secs(5), controller.disconnect_async()).await,
            Ok(Ok(_))
        )
    } else {
        true
    };
    if !disconnected {
        CONTROL.mark_uncertain();
    }
    let state = CONTROL.suspend_outcome(disconnected && !controller.is_connected());
    if state == PeripheralState::Quiescent {
        Link::Unconfigured.store();
    } else {
        Link::Failed.store();
    }
    // esp-radio exposes no public stop API. This confirms station inactivity,
    // not that the Wi-Fi controller or RF domain has been powered off.
    CONTROL.power_complete(true, state);
}

/// Wait for a command, a reconnect deadline or the loss of the association.
async fn next_work(
    controller: &mut WifiController<'static>,
    retry: &mut Reconnect,
) -> Option<(WifiOperation, bool)> {
    // Controller events are authoritative here. Embassy's link flag is updated by
    // another task and can remain true after the disconnect event; polling an
    // already-disconnected controller against that stale flag would never yield.
    if controller.is_connected() {
        match select(controller.wait_for_disconnect_async(), next_command()).await {
            Either::First(_) => {
                GENERATION.fetch_add(1, Ordering::AcqRel);
                Link::Failed.store();
                Probe::Waiting.store();
                CONTROL.with(|c| {
                    if !c.has_command() {
                        c.complete(Some(ControlError::LinkLost));
                    }
                });
                Timer::after_millis(1).await;
                None
            }
            Either::Second(operation) => operation,
        }
    } else if retry.requested() {
        match select(next_command(), Timer::after_secs(retry.delay())).await {
            Either::First(command) => command,
            Either::Second(()) => Some((WifiOperation::Connect, true)),
        }
    } else {
        next_command().await
    }
}

async fn perform(
    controller: &mut WifiController<'static>,
    retry: &mut Reconnect,
    command: WifiOperation,
    automatic: bool,
) {
    match command {
        WifiOperation::Connect if !automatic => retry.connect(),
        WifiOperation::Configure(_) | WifiOperation::Disconnect => retry.disconnect(),
        _ => {}
    }
    match command {
        WifiOperation::Scan => scan(controller, automatic).await,
        WifiOperation::Configure(config) => {
            if leave_association(controller, automatic).await {
                configure(controller, config, automatic);
            }
        }
        WifiOperation::Disconnect => {
            if leave_association(controller, automatic).await {
                CONTROL.finish(automatic, None);
            }
        }
        WifiOperation::Connect => {
            if leave_association(controller, automatic).await {
                connect(controller, retry, automatic).await;
            }
        }
    }
}

async fn scan(controller: &mut WifiController<'static>, automatic: bool) {
    CONTROL.clear_discoveries();
    match with_timeout(
        Duration::from_secs(15),
        controller.scan_async(&ScanConfig::default().with_max(DISCOVERIES)),
    )
    .await
    {
        Ok(Ok(aps)) => {
            CONTROL.update_discoveries(|found| {
                for (slot, ap) in found.iter_mut().zip(aps.iter()) {
                    if let Some(ssid) = Text::new(ap.ssid.as_str().as_bytes()) {
                        *slot = Some(NetworkDiscovery {
                            ssid,
                            rssi: ap.signal_strength,
                        });
                    }
                }
            });
            CONTROL.finish(automatic, None);
        }
        Err(_) => {
            CONTROL.mark_uncertain();
            CONTROL.finish(automatic, Some(ControlError::ScanTimeout));
        }
        Ok(Err(_)) => CONTROL.finish(automatic, Some(ControlError::ScanFailed)),
    }
}

/// Invalidate the current association and disconnect before a station change.
/// Returns false, with the failure reported, when the disconnect is unconfirmed.
async fn leave_association(controller: &mut WifiController<'static>, automatic: bool) -> bool {
    GENERATION.fetch_add(1, Ordering::AcqRel);
    Probe::Waiting.store();
    if controller.is_connected() {
        Link::Retrying.store();
        if !matches!(
            with_timeout(Duration::from_secs(5), controller.disconnect_async()).await,
            Ok(Ok(_))
        ) {
            CONTROL.mark_uncertain();
            Link::Failed.store();
            CONTROL.finish(automatic, Some(ControlError::DisconnectFailed));
            return false;
        }
    }
    Link::Unconfigured.store();
    true
}

fn configure(
    controller: &mut WifiController<'static>,
    config: Option<WifiConfig>,
    automatic: bool,
) {
    if let Some(config) = config {
        let station = StationConfig::default()
            .with_ssid(config.ssid.text())
            .with_password(config.password.text().into())
            .with_auth_method(if config.wpa3 {
                AuthenticationMethod::Wpa3Personal
            } else {
                AuthenticationMethod::Wpa2Personal
            });
        if controller.set_config(&Config::Station(station)).is_err() {
            CONTROL.finish(automatic, Some(ControlError::ConfigurationFailed));
            return;
        }
    }
    critical_section::with(|cs| *CONFIGURATION.borrow_ref_mut(cs) = config);
    CONTROL.finish(automatic, None);
}

async fn connect(controller: &mut WifiController<'static>, retry: &mut Reconnect, automatic: bool) {
    if critical_section::with(|cs| CONFIGURATION.borrow_ref(cs).is_none()) {
        retry.disconnect();
        CONTROL.finish(automatic, Some(ControlError::Unconfigured));
        return;
    }
    Link::Connecting.store();
    match with_timeout(Duration::from_secs(20), controller.connect_async()).await {
        Ok(Ok(_)) => {
            retry.recovered();
            ASSOCIATIONS.fetch_add(1, Ordering::Relaxed);
            GENERATION.fetch_add(1, Ordering::AcqRel);
            Link::Associated.store();
            CONTROL.finish(automatic, None);
        }
        result => {
            // disconnect_async refuses a connecting station, so a timeout
            // cannot prove that the attempt has stopped. Keep power blocked.
            if result.is_err() {
                CONTROL.mark_uncertain();
            }
            if controller.is_connected()
                && !matches!(
                    with_timeout(Duration::from_secs(5), controller.disconnect_async()).await,
                    Ok(Ok(_))
                )
            {
                CONTROL.mark_uncertain();
            }
            Link::Failed.store();
            CONTROL.finish(
                automatic,
                Some(if result.is_err() {
                    ControlError::ConnectTimeout
                } else {
                    ControlError::ConnectFailed
                }),
            );
        }
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
        // Do not spin on a stale DHCP snapshot after the radio has disconnected.
        // The network runner needs executor time to publish configuration-down.
        if !online() {
            Timer::after_millis(50).await;
            continue;
        }

        if with_timeout(Duration::from_secs(20), stack.wait_config_up())
            .await
            .is_err()
        {
            log::warn!(target: "wifi", "CYCLING_WIFI dhcp_failed kind=timeout");
            Probe::Failed.store();
            if Link::load() == Link::Associated {
                let generation = GENERATION.load(Ordering::Relaxed);
                let delay = firmware_services::network::retry_delay_secs(dhcp_failures);
                dhcp_failures = dhcp_failures.saturating_add(1);
                log::info!(target: "wifi", "CYCLING_WIFI dhcp_retry_in_s={}", delay);
                Timer::after_secs(delay).await;
                if generation == GENERATION.load(Ordering::Relaxed)
                    && Link::load() == Link::Associated
                    && !stack.is_config_up()
                {
                    request_recovery(generation);
                }
            }
            continue;
        }
        if !online() {
            continue;
        }
        dhcp_failures = 0;
        let generation = GENERATION.load(Ordering::Relaxed);
        Probe::Ready.store();
        log::info!(target: "wifi", "CYCLING_WIFI dhcp_ready");
        let result = with_timeout(Duration::from_secs(15), probe(stack)).await;
        if suspended()
            || generation != GENERATION.load(Ordering::Relaxed)
            || Link::load() != Link::Associated
            || !stack.is_config_up()
        {
            log::info!(target: "wifi", "CYCLING_WIFI request_stale");
            Probe::Waiting.store();
            continue;
        }
        match result {
            Ok(Ok(())) => {
                successes = successes.saturating_add(1);
                PROBE_SUCCESSES.fetch_add(1, Ordering::Relaxed);
                probe_failures = 0;
                Probe::Verified.store();
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
                Probe::Failed.store();
                log::warn!(target: "wifi", "CYCLING_WIFI request_failed kind={}", kind);
            }
            Err(_) => {
                PROBE_FAILURES.fetch_add(1, Ordering::Relaxed);
                Probe::Failed.store();
                log::warn!(target: "wifi", "CYCLING_WIFI request_failed kind=timeout");
            }
        }
        let delay = firmware_services::network::retry_delay_secs(probe_failures);
        probe_failures = probe_failures.saturating_add(1);
        log::info!(target: "wifi", "CYCLING_WIFI request_retry_in_s={}", delay);
        let _ = with_timeout(Duration::from_secs(delay), stack.wait_config_down()).await;
    }
}

/// Returns the failure kind logged by `verify`.
async fn probe(stack: Stack<'static>) -> Result<(), &'static str> {
    // Harness-only fault injection; FAULT stays zero without debug-harness.
    #[cfg(feature = "debug-harness")]
    if FAULT.load(Ordering::Relaxed) == 1 {
        return Err("injected_dns");
    }
    let addresses = stack
        .dns_query("example.com", DnsQueryType::A)
        .await
        .map_err(|_| "dns")?;
    #[cfg(feature = "debug-harness")]
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
    firmware_services::network::verified_response(&response[..used])
        .then_some(())
        .ok_or("response")
}

#[embassy_executor::task]
pub async fn time_sync(stack: Stack<'static>) {
    let mut retry_not_before = 0u64;
    loop {
        stack.wait_config_up().await;
        let mut failures = 0u8;
        while stack.is_config_up() {
            if suspended() {
                Timer::after_millis(50).await;
                continue;
            }
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
            firmware_services::network_time::set_syncing(true);
            let generation = connection_generation();
            let result = with_timeout(Duration::from_secs(10), sync_time(stack)).await;
            let current =
                !suspended() && generation == connection_generation() && stack.is_config_up();
            match result {
                Ok(Ok((timestamp, rtt_ms))) if current => {
                    let now = Instant::now().as_millis();
                    firmware_services::network_time::update(timestamp, now);
                    failures = 0;
                    log::info!(target: "wifi",
                        "CYCLING_TIME synced unix={} stratum={} rtt_ms={}",
                        timestamp.unix_seconds, timestamp.stratum, rtt_ms
                    );
                    match select(stack.wait_config_down(), Timer::after_secs(60 * 60)).await {
                        Either::First(_) => firmware_services::network_time::set_syncing(false),
                        Either::Second(_) => {}
                    }
                }
                result => {
                    firmware_services::network_time::set_syncing(false);
                    if current {
                        if matches!(result, Ok(Err(SyncError::Denied))) {
                            log::info!(target: "wifi", "CYCLING_TIME sync_disabled kind=denied until=restart");
                            return;
                        }
                        let (kind, delay) = if matches!(result, Ok(Err(SyncError::RateLimited))) {
                            retry_not_before = Instant::now().as_millis() + 15 * 60 * 1_000;
                            ("rate_limited", 15 * 60)
                        } else {
                            let delay = firmware_services::network::retry_delay_secs(failures);
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

/// Why one SNTP exchange failed. Only server refusals change the retry policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum SyncError {
    /// DNS, socket or response transport failed; retry with backoff.
    Request,
    /// The server answered with an unusable packet; retry with backoff.
    InvalidResponse,
    /// Kiss-o'-death RATE: wait the fixed rate-limit interval.
    RateLimited,
    /// Kiss-o'-death DENY/RSTR: stop syncing until restart.
    Denied,
}

async fn sync_time(stack: Stack<'static>) -> Result<(Timestamp, u64), SyncError> {
    let addresses = with_timeout(
        Duration::from_secs(5),
        stack.dns_query("time.cloudflare.com", DnsQueryType::A),
    )
    .await
    .map_err(|_| SyncError::Request)?
    .map_err(|_| SyncError::Request)?;
    let host = *addresses.first().ok_or(SyncError::Request)?;
    let mut rx_meta = [PacketMetadata::EMPTY];
    let mut tx_meta = [PacketMetadata::EMPTY];
    let mut socket_rx = [0; 512];
    let mut tx = [0; 48];
    let mut socket = UdpSocket::new(stack, &mut rx_meta, &mut socket_rx, &mut tx_meta, &mut tx);
    socket.bind(0).map_err(|_| SyncError::Request)?;
    let mut request = [0u8; 48];
    request[0] = 0x23;
    let sent_ms = Instant::now().as_millis();
    let origin = sent_ms.to_be_bytes();
    request[40..48].copy_from_slice(&origin);
    socket
        .send_to(&request, (host, 123))
        .await
        .map_err(|_| SyncError::Request)?;
    let mut response = [0; 512];
    let (length, source) = with_timeout(Duration::from_secs(5), socket.recv_from(&mut response))
        .await
        .map_err(|_| SyncError::Request)?
        .map_err(|_| SyncError::Request)?;
    let timestamp = firmware_services::network_time::parse_response(
        &response[..length],
        &origin,
        source.endpoint == (host, 123).into(),
    )
    .map_err(|error| match error {
        ParseError::RateLimited => SyncError::RateLimited,
        ParseError::Denied => SyncError::Denied,
        _ => SyncError::InvalidResponse,
    })?;
    Ok((
        timestamp,
        Instant::now().as_millis().saturating_sub(sent_ms),
    ))
}
