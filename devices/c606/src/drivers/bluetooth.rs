//! One-peer BLE discovery, bounded notification bytes and echo transport.

use bt_hci::cmd::le::LeSetAdvEnable;
use bt_hci::cmd::le::LeSetScanEnable;
use bt_hci::cmd::le::LeSetScanParams;
use bt_hci::controller::ControllerCmdSync;
use core::cell::RefCell;
use core::sync::atomic::AtomicI32;
use core::sync::atomic::AtomicU32;
use core::sync::atomic::Ordering;
use critical_section::Mutex;
use embassy_futures::select::Either;
use embassy_futures::select::Either3;
use embassy_futures::select::select;
use embassy_futures::select::select3;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Duration;
use embassy_time::Timer;
use esp_hal::peripherals::BT;
use esp_radio::ble::controller::BleConnector;
use firmware_services::connectivity::Reconnect;
use trouble_host::prelude::*;

use crate::drivers::radio_control::DISCOVERIES;
use crate::drivers::radio_control::Shared;
use device_api::ble_transport::Discovery;
use device_api::ble_transport::Link;
use device_api::ble_transport::Operation as BleOperation;
use device_api::ble_transport::Packet;
use device_api::ble_transport::Selection;
use device_api::ble_transport::Snapshot;
use device_api::connectivity::ControlError;
use device_api::connectivity::ControlStatus;
use device_api::connectivity::Text;
use device_api::observation::Availability;
use device_api::observation::Error;
use device_api::power::PeripheralState;

type BleController = ExternalController<BleConnector<'static>, 10>;
type BleStack<'s> = Stack<'s, BleController, DefaultPacketPool>;
type BleCentral<'s> = Central<'s, BleController, DefaultPacketPool>;
type BlePeripheral<'s> = Peripheral<'s, BleController, DefaultPacketPool>;

/// Power, command admission and discoveries. Cancelled connects and failed
/// disconnects mark it uncertain for the rest of the boot; see `radio_control`.
static CONTROL: Shared<BleOperation, Discovery> = Shared::new();

pub fn power_status() -> PeripheralState {
    CONTROL.power()
}
pub fn power_request(suspend: bool) -> Result<(), Error> {
    CONTROL.power_request(suspend, || {})
}
pub fn control() -> ControlStatus {
    CONTROL.status()
}
pub fn discoveries() -> [Option<Discovery>; DISCOVERIES] {
    CONTROL.discoveries()
}
pub fn request(operation: BleOperation) -> Result<(), Error> {
    CONTROL.request(operation)
}

static SELECTION: Mutex<RefCell<Option<Selection>>> = Mutex::new(RefCell::new(None));
static PACKETS: Channel<CriticalSectionRawMutex, Packet, 4> = Channel::new();
pub fn take_packet() -> Option<Packet> {
    PACKETS.try_receive().ok()
}

const CONNECTIONS: usize = 1;
const CHANNELS: usize = 2;
const ECHO_NAME: &[u8] = b"Cycling Echo";
static SCAN_REPORTS: AtomicU32 = AtomicU32::new(0);
/// Signal range of the current scan only; reset when each scan starts.
static SCAN_RSSI_MIN: AtomicI32 = AtomicI32::new(i32::MAX);
static SCAN_RSSI_MAX: AtomicI32 = AtomicI32::new(i32::MIN);
static TARGET: Mutex<RefCell<Option<Address>>> = Mutex::new(RefCell::new(None));

static STATE: Mutex<RefCell<Snapshot>> = Mutex::new(RefCell::new(Snapshot {
    link: Link::Off,
    connections: 0,
    disconnections: 0,
    notifications: 0,
    dropped: 0,
    scan_reports: 0,
}));

#[gatt_server]
struct Server {
    echo: EchoService,
}

#[gatt_service(uuid = "7e570001-2a6f-4d75-9f6a-5afbf0f50c06")]
struct EchoService {
    #[characteristic(
        uuid = "7e570002-2a6f-4d75-9f6a-5afbf0f50c06",
        read,
        write,
        notify,
        value = 0u64
    )]
    value: u64,
}

struct ScanCounter;

impl EventHandler for ScanCounter {
    fn on_adv_reports(&self, mut reports: LeAdvReportsIter<'_>) {
        while let Some(Ok(report)) = reports.next() {
            SCAN_REPORTS.fetch_add(1, Ordering::Relaxed);
            SCAN_RSSI_MIN.fetch_min(i32::from(report.rssi), Ordering::Relaxed);
            SCAN_RSSI_MAX.fetch_max(i32::from(report.rssi), Ordering::Relaxed);
            let name = AdStructure::decode(report.data)
                .find_map(|item| match item {
                    Ok(
                        AdStructure::CompleteLocalName(name)
                        | AdStructure::ShortenedLocalName(name),
                    ) => Text::new(name),
                    _ => None,
                })
                .unwrap_or(Text::empty());
            // BdAddr is always six bytes; the conversion cannot fail.
            if let Ok(address) = <[u8; 6]>::try_from(report.addr.raw()) {
                CONTROL.update_discoveries(|discoveries| {
                    let slot = discoveries
                        .iter()
                        .position(|item| item.is_some_and(|item| item.address == address))
                        .or_else(|| discoveries.iter().position(Option::is_none));
                    if let Some(slot) = slot {
                        discoveries[slot] = Some(Discovery {
                            name,
                            address,
                            random: report.addr_kind != AddrKind::PUBLIC,
                            rssi: report.rssi,
                        });
                    }
                });
            }
            let Some(selection) = critical_section::with(|cs| *SELECTION.borrow_ref(cs)) else {
                continue;
            };
            let name_matches = (selection.name.is_empty() && selection.address.is_some())
                || AdStructure::decode(report.data).any(|item| {
                    matches!(item,
                    Ok(AdStructure::CompleteLocalName(name) | AdStructure::ShortenedLocalName(name))
                    if name == selection.name.bytes())
                });
            let address_matches = selection
                .address
                .is_none_or(|address| report.addr.raw() == address.as_slice());
            if name_matches && address_matches {
                critical_section::with(|cs| {
                    *TARGET.borrow_ref_mut(cs) = Some(Address {
                        kind: report.addr_kind,
                        addr: report.addr,
                    });
                });
            }
        }
    }
}

pub fn snapshot() -> Snapshot {
    let mut state = critical_section::with(|cs| *STATE.borrow_ref(cs));
    state.scan_reports = SCAN_REPORTS.load(Ordering::Relaxed);
    state
}
fn update_state(update: impl FnOnce(&mut Snapshot)) {
    critical_section::with(|cs| update(&mut STATE.borrow_ref_mut(cs)));
}

fn set_link(link: Link) {
    update_state(|state| state.link = link);
}

fn disconnected() {
    update_state(|state| {
        state.link = Link::Retrying;
        state.disconnections = state.disconnections.saturating_add(1);
    });
}

#[embassy_executor::task]
pub async fn start(bt: BT<'static>) {
    let heap_before = esp_alloc::HEAP.free();
    let connector = match BleConnector::new(bt, Default::default()) {
        Ok(connector) => connector,
        Err(error) => {
            set_link(Link::Failed);
            log::info!(target: "ble", "CYCLING_BLE init_failed error={:?}", error);
            return;
        }
    };
    let controller: BleController = ExternalController::new(connector);
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS, CHANNELS> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random([0x15, 0xc6, 0x06, 0x00, 0x00, 0xc0]));
    let Host {
        central,
        peripheral,
        mut runner,
        ..
    } = stack.build();
    let server = match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "Cycling Echo",
        appearance: &appearance::computer::GENERIC_COMPUTER,
    })) {
        Ok(server) => server,
        Err(_) => {
            set_link(Link::Failed);
            log::info!(target: "ble", "CYCLING_BLE server_failed");
            return;
        }
    };

    log::info!(target: "ble",
        "CYCLING_BLE ready mode={} att_mtu_max=60 heap_before={} heap_after={}",
        "runtime",
        heap_before,
        esp_alloc::HEAP.free()
    );

    CONTROL.set_ready();
    let outcome = select(
        runner.run_with_handler(&ScanCounter),
        application(&stack, central, peripheral, &server),
    )
    .await;
    CONTROL.stop(ControlError::RunnerStopped);
    set_link(Link::Failed);
    match outcome {
        Either::First(Ok(())) => log::info!(target: "ble", "CYCLING_BLE runner_stopped result=ok"),
        Either::First(Err(_)) => {
            log::info!(target: "ble", "CYCLING_BLE runner_stopped result=error")
        }
        Either::Second(()) => log::info!(target: "ble", "CYCLING_BLE application_stopped"),
    }
}

/// Transport intent that survives individual operations and power cycles.
struct Session {
    echo: bool,
    retry: Reconnect,
}

/// Owner loop: power transitions first, then one command or automatic
/// reconnect, then the idle activity until new work arrives.
async fn application<'s>(
    stack: &'s BleStack<'s>,
    mut central: BleCentral<'s>,
    mut peripheral: BlePeripheral<'s>,
    server: &Server<'_>,
) {
    let mut session = Session {
        echo: true,
        retry: Reconnect::default(),
    };
    loop {
        if !CONTROL.ready() {
            return;
        }
        if CONTROL.suspended() {
            // Re-check from the top after quiescing: a disconnect timeout can
            // stop the owner, and then nothing would ever signal it again.
            if CONTROL.power() == PeripheralState::Pending {
                quiesce(stack, &mut peripheral).await;
            } else {
                CONTROL.changed().await;
            }
            continue;
        }
        // Keep echo, selection and reconnect intent unchanged across power
        // preparation. Fresh transport work resumes below.
        if CONTROL.complete_resume() == PeripheralState::Failed {
            CONTROL.changed().await;
            continue;
        }
        let requested = CONTROL.take_command();
        let automatic = requested.is_none() && session.retry.requested();
        let operation = if automatic {
            Some(BleOperation::Connect)
        } else {
            requested
        };
        if let Some(operation) = operation {
            central = perform(operation, automatic, &mut session, stack, central, server).await;
        }
        if !CONTROL.ready() {
            return;
        }
        if CONTROL.suspended() {
            continue;
        }
        idle(&mut session, &mut peripheral, server).await;
    }
}

/// Stop scanning and advertising with the runner alive, and report whether the
/// controller acknowledged inactivity.
async fn quiesce(stack: &BleStack<'_>, peripheral: &mut BlePeripheral<'_>) {
    // Dropping an advertiser/scan session only queues cancellation.
    // Wait for actual controller acknowledgements with the runner alive.
    let result = embassy_time::with_timeout(Duration::from_secs(5), async {
        stack.command(LeSetScanEnable::new(false, false)).await?;
        stack.command(LeSetAdvEnable::new(false)).await
    })
    .await;
    let mut quiet = matches!(result, Ok(Ok(_)));
    if !quiet {
        CONTROL.mark_uncertain();
    }
    // A peer can connect just before the advertising disable. Drain
    // that accepted controller connection before claiming inactivity.
    while let Some(connection) = peripheral.try_accept() {
        connection.disconnect();
        quiet &= wait_disconnected(&connection).await;
    }
    while PACKETS.try_receive().is_ok() {}
    let state = CONTROL.suspend_outcome(quiet);
    set_link(if state == PeripheralState::Quiescent {
        Link::Off
    } else {
        Link::Failed
    });
    CONTROL.power_complete(true, state);
}

async fn perform<'s>(
    operation: BleOperation,
    automatic: bool,
    session: &mut Session,
    stack: &'s BleStack<'s>,
    central: BleCentral<'s>,
    server: &Server<'_>,
) -> BleCentral<'s> {
    match operation {
        BleOperation::Select(selection) => {
            session.retry.disconnect();
            critical_section::with(|cs| *SELECTION.borrow_ref_mut(cs) = selection);
            while PACKETS.try_receive().is_ok() {}
            session.echo = false;
            set_link(Link::Off);
            CONTROL.finish(automatic, None);
            central
        }
        BleOperation::Disconnect => {
            session.retry.disconnect();
            session.echo = false;
            set_link(Link::Off);
            CONTROL.finish(automatic, None);
            central
        }
        BleOperation::Echo => {
            session.retry.disconnect();
            session.echo = true;
            CONTROL.finish(automatic, None);
            central
        }
        BleOperation::Scan => {
            session.retry.disconnect();
            session.echo = false;
            CONTROL.clear_discoveries();
            let (central, _, success) = scan_target(central).await;
            set_link(Link::Off);
            CONTROL.finish(automatic, (!success).then_some(ControlError::ScanFailed));
            central
        }
        BleOperation::Connect => connect(automatic, session, stack, central, server).await,
    }
}

async fn connect<'s>(
    automatic: bool,
    session: &mut Session,
    stack: &'s BleStack<'s>,
    central: BleCentral<'s>,
    server: &Server<'_>,
) -> BleCentral<'s> {
    if !automatic {
        session.retry.connect();
    }
    session.echo = false;
    let Some(selection) = critical_section::with(|cs| *SELECTION.borrow_ref(cs)) else {
        set_link(Link::Off);
        session.retry.disconnect();
        CONTROL.finish(automatic, Some(ControlError::Unconfigured));
        return central;
    };
    let (mut central, target, _) = scan_target(central).await;
    // A suspend, or a user command overriding an automatic retry, wins.
    if CONTROL.suspended() || (automatic && CONTROL.has_command()) {
        return central;
    }
    let Some(target) = target else {
        set_link(Link::Failed);
        CONTROL.finish(automatic, Some(ControlError::PeerNotFound));
        return central;
    };
    set_link(Link::Connecting);
    let before = snapshot().connections;
    let result = client_attempt(stack, &mut central, server, target, selection, !automatic).await;
    if snapshot().connections != before {
        session.retry.recovered();
        disconnected();
    }
    match result {
        Ok(()) if CONTROL.has_command() => set_link(Link::Off),
        result => {
            set_link(Link::Failed);
            CONTROL.finish(
                automatic,
                Some(result.err().unwrap_or(ControlError::ConnectionEnded)),
            );
        }
    }
    central
}

/// Background activity between operations: echo advertising, a reconnect
/// backoff, or simply waiting for the next command or power request.
async fn idle(session: &mut Session, peripheral: &mut BlePeripheral<'_>, server: &Server<'_>) {
    if session.echo {
        if advertise_and_echo(peripheral, server).await.is_err() {
            // A GATT error can drop a live connection and only queue its
            // disconnect. Do not claim power quiescence without completion.
            CONTROL.mark_uncertain();
            let _ = select(CONTROL.work_pending(), Timer::after_secs(1)).await;
        }
    } else if session.retry.requested() {
        let _ = select(
            CONTROL.work_pending(),
            Timer::after_secs(session.retry.delay()),
        )
        .await;
    } else {
        CONTROL.work_pending().await;
    }
}

async fn scan_target<C>(
    central: Central<'_, C, DefaultPacketPool>,
) -> (Central<'_, C, DefaultPacketPool>, Option<Address>, bool)
where
    C: Controller + ControllerCmdSync<LeSetScanParams>,
{
    critical_section::with(|cs| *TARGET.borrow_ref_mut(cs) = None);
    set_link(Link::Scanning);
    let reports_before = SCAN_REPORTS.load(Ordering::Relaxed);
    SCAN_RSSI_MIN.store(i32::MAX, Ordering::Relaxed);
    SCAN_RSSI_MAX.store(i32::MIN, Ordering::Relaxed);
    let mut scanner = Scanner::new(central);
    let scan_config = ScanConfig {
        active: true,
        phys: PhySet::M1,
        interval: Duration::from_millis(100),
        window: Duration::from_millis(50),
        ..Default::default()
    };
    let success = match scanner.scan(&scan_config).await {
        Ok(session) => {
            let _ = select(Timer::after_secs(10), CONTROL.wait_until(|c| c.suspended())).await;
            drop(session);
            Timer::after_millis(100).await;
            true
        }
        Err(_) => false,
    };
    let target = critical_section::with(|cs| TARGET.borrow_ref_mut(cs).take());
    let reports = SCAN_REPORTS.load(Ordering::Relaxed);
    let observed = reports != reports_before;
    log::info!(target: "ble",
        "CYCLING_BLE scan_done found={} reports={} rssi_min={} rssi_max={}",
        target.is_some(),
        reports,
        if observed {
            SCAN_RSSI_MIN.load(Ordering::Relaxed)
        } else {
            0
        },
        if observed {
            SCAN_RSSI_MAX.load(Ordering::Relaxed)
        } else {
            0
        }
    );
    (scanner.into_inner(), target, success)
}

async fn client_attempt<'stack, C>(
    stack: &'stack Stack<'stack, C, DefaultPacketPool>,
    central: &mut Central<'stack, C, DefaultPacketPool>,
    server: &Server<'_>,
    target: Address,
    selection: Selection,
    report_completion: bool,
) -> Result<(), ControlError>
where
    C: Controller,
{
    let filter = [(target.kind, &target.addr)];
    let connect_config = ConnectConfig {
        connect_params: Default::default(),
        scan_config: ScanConfig {
            filter_accept_list: &filter,
            timeout: Duration::from_secs(10),
            ..Default::default()
        },
    };
    let connection =
        embassy_time::with_timeout(Duration::from_secs(12), central.connect(&connect_config))
            .await
            .map_err(|_| {
                // Cancellation can race a controller connection-complete event.
                // The public host API cannot prove that late connection absent.
                CONTROL.mark_uncertain();
                ControlError::ConnectTimeout
            })?
            .map_err(|_| ControlError::ConnectFailed)?;
    update_state(|state| state.connections = state.connections.saturating_add(1));
    log::info!(target: "ble", "CYCLING_BLE transport_connected");
    // Serve the peer too: it may be a GATT client of ours (BlueZ discovers
    // every LE peer) and drops the link when a request stays unanswered for
    // the 30 s ATT transaction timeout. This also observes the disconnect.
    let connection = connection
        .with_attribute_server(server)
        .map_err(|_| ControlError::GattFailed)?;
    let exchange = async {
        let client = embassy_time::with_timeout(
            Duration::from_secs(10),
            GattClient::<C, DefaultPacketPool, 12>::new(stack, connection.raw()),
        )
        .await
        .map_err(|_| ControlError::GattFailed)?
        .map_err(|_| ControlError::GattFailed)?;
        match select3(
            client.task(),
            data_protocol(&client, selection, report_completion),
            serve_peer(stack, &connection),
        )
        .await
        {
            Either3::First(_) => Err(ControlError::GattFailed),
            Either3::Second(result) => result.map_err(|_| ControlError::GattFailed),
            Either3::Third(error) => Err(error),
        }
    };
    let result = match select(exchange, CONTROL.work_pending()).await {
        Either::First(result) => result,
        Either::Second(()) => Ok(()),
    };
    connection.raw().disconnect();
    if !wait_disconnected(connection.raw()).await {
        return Err(ControlError::DisconnectTimeout);
    }
    result
}

/// Answer the peer's ATT requests and connection parameter requests until the
/// link ends. Returns only when the peer or controller ended the link.
async fn serve_peer<C>(
    stack: &Stack<'_, C, DefaultPacketPool>,
    connection: &GattConnection<'_, '_, DefaultPacketPool>,
) -> ControlError
where
    C: Controller,
{
    loop {
        match connection.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                log::info!(target: "ble", "CYCLING_BLE link_lost reason={:?}", reason);
                return ControlError::LinkLost;
            }
            GattConnectionEvent::Gatt { event } => {
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
            }
            GattConnectionEvent::RequestConnectionParams(request) => {
                let _ = request.accept(None, stack).await;
            }
            _ => {}
        }
    }
}

async fn data_protocol<C>(
    client: &GattClient<'_, C, DefaultPacketPool, 12>,
    selection: Selection,
    report_completion: bool,
) -> Result<(), ()>
where
    C: Controller,
{
    let services = embassy_time::with_timeout(
        Duration::from_secs(10),
        client.services_by_uuid(&Uuid::new_short(selection.service)),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    let service = services.first().cloned().ok_or(())?;
    let measurement: Characteristic<[u8]> = embassy_time::with_timeout(
        Duration::from_secs(10),
        client.characteristic_by_uuid(&service, &Uuid::new_short(selection.characteristic)),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    let mut subscription = embassy_time::with_timeout(
        Duration::from_secs(10),
        client.subscribe(&measurement, false),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    set_link(Link::Connected);
    if report_completion {
        CONTROL.complete(None);
    }
    loop {
        let Ok(notification) =
            embassy_time::with_timeout(Duration::from_secs(1), subscription.next()).await
        else {
            continue;
        };
        let now = embassy_time::Instant::now().as_millis();
        let data: &[u8] = notification.as_ref();
        update_state(|state| state.notifications = state.notifications.saturating_add(1));
        let state = snapshot();
        let mut packet = Packet {
            connection: state.connections,
            sequence: state.notifications,
            dropped: state.dropped,
            received_ms: now,
            bytes: [0; 60],
            length: data.len().min(60) as u8,
        };
        if data.len() > packet.bytes.len() {
            update_state(|state| state.dropped = state.dropped.saturating_add(1));
            continue;
        }
        packet.bytes[..data.len()].copy_from_slice(data);
        // One bounded consumer queue. A slow client cannot stall BLE acquisition;
        // sequence/drop stamps let it reject queued data from before a loss.
        if PACKETS.try_send(packet).is_err() {
            update_state(|state| state.dropped = state.dropped.saturating_add(1));
        }
    }
}

async fn advertise_and_echo<C>(
    peripheral: &mut Peripheral<'_, C, DefaultPacketPool>,
    server: &Server<'_>,
) -> Result<(), BleHostError<C::Error>>
where
    C: Controller,
{
    let mut scan_data = [0; 31];
    let scan_len =
        AdStructure::encode_slice(&[AdStructure::CompleteLocalName(ECHO_NAME)], &mut scan_data)?;
    let mut adv_data = [0; 31];
    let adv_len = AdStructure::encode_slice(
        &[AdStructure::Flags(
            LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED,
        )],
        &mut adv_data,
    )?;
    set_link(Link::Advertising);
    log::info!(target: "ble", "CYCLING_BLE advertising");
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &adv_data[..adv_len],
                scan_data: &scan_data[..scan_len],
            },
        )
        .await?;
    let connection = match select(advertiser.accept(), CONTROL.work_pending()).await {
        Either::First(result) => result?.with_attribute_server(server)?,
        Either::Second(()) => return Ok(()),
    };
    update_state(|state| state.connections = state.connections.saturating_add(1));
    set_link(Link::Connected);
    log::info!(target: "ble", "CYCLING_BLE connected heap_free={}", esp_alloc::HEAP.free());
    loop {
        let event = match select(connection.next(), CONTROL.work_pending()).await {
            Either::First(event) => event,
            Either::Second(()) => {
                connection.raw().disconnect();
                if wait_disconnected(connection.raw()).await {
                    disconnected();
                }
                return Ok(());
            }
        };
        match event {
            GattConnectionEvent::Disconnected { reason } => {
                update_state(|state| {
                    state.disconnections = state.disconnections.saturating_add(1);
                    state.link = Link::Advertising;
                });
                log::info!(target: "ble", "CYCLING_BLE disconnected reason={:?}", reason);
                return Ok(());
            }
            GattConnectionEvent::Gatt { event } => {
                if let GattEvent::Write(write) = event {
                    if write.handle() != server.echo.value.handle {
                        write.accept()?.send().await;
                    } else if write.data().len() != 8 {
                        write
                            .reject(AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH)?
                            .send()
                            .await;
                        log::info!(target: "ble", "CYCLING_BLE echo_rejected reason=length");
                    } else {
                        write.accept()?.send().await;
                        let value = server.get(&server.echo.value)?;
                        update_state(|state| {
                            state.notifications = state.notifications.saturating_add(1)
                        });
                        if server.echo.value.notify(&connection, &value).await.is_err() {
                            log::info!(target: "ble", "CYCLING_BLE notify_failed");
                        }
                    }
                } else {
                    event.accept()?.send().await;
                }
            }
            _ => {}
        }
    }
}

pub fn availability() -> Availability {
    match snapshot().link {
        Link::Failed => Availability::Failed,
        _ => Availability::Ready,
    }
}

/// Wait for a disconnect the host only queued. A timeout means the controller
/// may still hold the link: stop serving requests and fail closed.
async fn wait_disconnected(connection: &Connection<'_, DefaultPacketPool>) -> bool {
    if embassy_time::with_timeout(Duration::from_secs(5), async {
        while connection.is_connected() {
            Timer::after_millis(20).await;
        }
    })
    .await
    .is_err()
    {
        CONTROL.mark_uncertain();
        set_link(Link::Failed);
        CONTROL.stop(ControlError::DisconnectTimeout);
        false
    } else {
        true
    }
}
