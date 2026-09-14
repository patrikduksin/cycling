//! One-peer BLE discovery, bounded notification bytes and echo transport.

use bt_hci::{cmd::le::LeSetScanParams, controller::ControllerCmdSync};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicI32, AtomicU32, Ordering},
};
use critical_section::Mutex;
use embassy_futures::select::{Either, select};
use embassy_sync::{blocking_mutex::raw::CriticalSectionRawMutex, channel::Channel};
use embassy_time::{Duration, Timer};
use esp_hal::peripherals::BT;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

use cycling_os::ble_transport::{Link, Packet, Selection, Snapshot};
static SELECTION: Mutex<RefCell<Option<Selection>>> = Mutex::new(RefCell::new(None));
static PACKETS: Channel<CriticalSectionRawMutex, Packet, 4> = Channel::new();
pub fn take_packet() -> Option<Packet> {
    PACKETS.try_receive().ok()
}

const CONNECTIONS: usize = 1;
const CHANNELS: usize = 2;
const ECHO_NAME: &[u8] = b"Cycling Echo";
static SCAN_REPORTS: AtomicU32 = AtomicU32::new(0);
static SCAN_RSSI_MIN: AtomicI32 = AtomicI32::new(i32::MAX);
static SCAN_RSSI_MAX: AtomicI32 = AtomicI32::new(i32::MIN);
static TARGET: Mutex<RefCell<Option<Address>>> = Mutex::new(RefCell::new(None));
use cycling_os::{
    ble_transport::{Discovery, Operation as BleOperation},
    connectivity::{ControlStatus, Operation, Text},
};
static READY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);
static CONTROL: Mutex<RefCell<ControlStatus>> = Mutex::new(RefCell::new(ControlStatus::new()));
static COMMAND: Mutex<RefCell<Option<BleOperation>>> = Mutex::new(RefCell::new(None));
static DISCOVERIES: Mutex<RefCell<[Option<Discovery>; 8]>> = Mutex::new(RefCell::new([None; 8]));
pub fn control() -> ControlStatus {
    critical_section::with(|cs| *CONTROL.borrow_ref(cs))
}
pub fn discoveries() -> [Option<Discovery>; 8] {
    critical_section::with(|cs| *DISCOVERIES.borrow_ref(cs))
}
pub fn request(operation: BleOperation) -> Result<(), cycling_os::capabilities::Error> {
    if !READY.load(Ordering::Acquire) {
        return Err(cycling_os::capabilities::Error::Unavailable);
    }
    critical_section::with(|cs| {
        let mut status = CONTROL.borrow_ref_mut(cs);
        if status.operation == Operation::Pending || COMMAND.borrow_ref(cs).is_some() {
            return Err(cycling_os::capabilities::Error::Unavailable);
        }
        status.sequence = status.sequence.wrapping_add(1);
        status.operation = Operation::Pending;
        status.error = "none";
        *COMMAND.borrow_ref_mut(cs) = Some(operation);
        Ok(())
    })
}
fn complete(error: Option<&'static str>) {
    critical_section::with(|cs| {
        let mut s = CONTROL.borrow_ref_mut(cs);
        s.operation = if error.is_some() {
            Operation::Failed
        } else {
            Operation::Completed
        };
        s.error = error.unwrap_or("none");
    });
}
async fn command_pending() {
    loop {
        if critical_section::with(|cs| COMMAND.borrow_ref(cs).is_some()) {
            return;
        }
        Timer::after_millis(50).await;
    }
}

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
            let address: [u8; 6] = report.addr.raw().try_into().unwrap();
            critical_section::with(|cs| {
                let mut discoveries = DISCOVERIES.borrow_ref_mut(cs);
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
    let controller: ExternalController<_, 10> = ExternalController::new(connector);
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS, CHANNELS> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random([0x15, 0xc6, 0x06, 0x00, 0x00, 0xc0]));
    let Host {
        mut central,
        mut peripheral,
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

    READY.store(true, Ordering::Release);
    let app = async {
        let mut echo = true;
        let mut retry = cycling_os::connectivity::Reconnect::default();
        loop {
            if !READY.load(Ordering::Acquire) {
                return;
            }
            let requested = critical_section::with(|cs| COMMAND.borrow_ref_mut(cs).take());
            let automatic = requested.is_none() && retry.requested();
            let operation = if automatic {
                Some(BleOperation::Connect)
            } else {
                requested
            };
            let finish = |error| {
                if !automatic && critical_section::with(|cs| COMMAND.borrow_ref(cs).is_none()) {
                    complete(error);
                }
            };
            match operation {
                Some(BleOperation::Select(selection)) => {
                    retry.disconnect();
                    critical_section::with(|cs| *SELECTION.borrow_ref_mut(cs) = selection);
                    while PACKETS.try_receive().is_ok() {}
                    echo = false;
                    set_link(Link::Off);
                    finish(None);
                }
                Some(BleOperation::Disconnect) => {
                    retry.disconnect();
                    echo = false;
                    set_link(Link::Off);
                    finish(None);
                }
                Some(BleOperation::Echo) => {
                    retry.disconnect();
                    echo = true;
                    finish(None);
                }
                Some(BleOperation::Scan) => {
                    retry.disconnect();
                    echo = false;
                    critical_section::with(|cs| *DISCOVERIES.borrow_ref_mut(cs) = [None; 8]);
                    let (returned, _, success) = scan_target(central).await;
                    central = returned;
                    set_link(Link::Off);
                    finish(if success { None } else { Some("scan_failed") });
                }
                Some(BleOperation::Connect) => {
                    if !automatic {
                        retry.connect();
                    }
                    echo = false;
                    let selection = critical_section::with(|cs| *SELECTION.borrow_ref(cs));
                    if let Some(selection) = selection {
                        let (returned, target, _) = scan_target(central).await;
                        central = returned;
                        if automatic
                            && critical_section::with(|cs| COMMAND.borrow_ref(cs).is_some())
                        {
                            continue;
                        }
                        if let Some(target) = target {
                            set_link(Link::Connecting);
                            let before = snapshot().connections;
                            let result =
                                client_attempt(&stack, &mut central, target, selection, !automatic)
                                    .await;
                            if snapshot().connections != before {
                                retry.recovered();
                                disconnected();
                            }
                            match result {
                                Ok(())
                                    if critical_section::with(|cs| {
                                        COMMAND.borrow_ref(cs).is_some()
                                    }) =>
                                {
                                    set_link(Link::Off);
                                }
                                result => {
                                    set_link(Link::Failed);
                                    finish(Some(result.err().unwrap_or("connection_ended")));
                                }
                            }
                        } else {
                            set_link(Link::Failed);
                            finish(Some("peer_not_found"));
                        }
                    } else {
                        set_link(Link::Off);
                        retry.disconnect();
                        finish(Some("unconfigured"));
                    }
                }
                None => {}
            }
            if !READY.load(Ordering::Acquire) {
                return;
            }
            if echo {
                if advertise_and_echo(&mut peripheral, &server).await.is_err() {
                    let _ = select(command_pending(), Timer::after_secs(1)).await;
                }
            } else if retry.requested() {
                let _ = select(command_pending(), Timer::after_secs(retry.delay())).await;
            } else {
                command_pending().await;
            }
        }
    };
    let outcome = select(runner.run_with_handler(&ScanCounter), app).await;
    READY.store(false, Ordering::Release);
    critical_section::with(|cs| *COMMAND.borrow_ref_mut(cs) = None);
    complete(Some("runner_stopped"));
    update_state(|state| {
        state.link = Link::Failed;
    });
    match outcome {
        Either::First(Ok(())) => log::info!(target: "ble", "CYCLING_BLE runner_stopped result=ok"),
        Either::First(Err(_)) => {
            log::info!(target: "ble", "CYCLING_BLE runner_stopped result=error")
        }
        Either::Second(()) => log::info!(target: "ble", "CYCLING_BLE application_stopped"),
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
            Timer::after_secs(10).await;
            drop(session);
            Timer::after_millis(100).await;
            true
        }
        Err(_) => false,
    };
    let target = critical_section::with(|cs| TARGET.borrow_ref_mut(cs).take());
    let reports = SCAN_REPORTS.load(Ordering::Relaxed);
    log::info!(target: "ble",
        "CYCLING_BLE scan_done found={} reports={} rssi_min={} rssi_max={}",
        target.is_some(),
        reports,
        if reports == 0 {
            0
        } else {
            SCAN_RSSI_MIN.load(Ordering::Relaxed)
        },
        if reports == 0 {
            0
        } else {
            SCAN_RSSI_MAX.load(Ordering::Relaxed)
        }
    );
    (scanner.into_inner(), target, success)
}

async fn client_attempt<'stack, C>(
    stack: &'stack Stack<'stack, C, DefaultPacketPool>,
    central: &mut Central<'stack, C, DefaultPacketPool>,
    target: Address,
    selection: Selection,
    report_completion: bool,
) -> Result<(), &'static str>
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
            .map_err(|_| "connect_failed")?
            .map_err(|_| "connect_failed")?;
    update_state(|state| state.connections = state.connections.saturating_add(1));
    log::info!(target: "ble", "CYCLING_BLE transport_connected");
    let exchange = async {
        let client = embassy_time::with_timeout(
            Duration::from_secs(10),
            GattClient::<C, DefaultPacketPool, 12>::new(stack, &connection),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
        match select(
            client.task(),
            data_protocol(&client, selection, report_completion),
        )
        .await
        {
            Either::First(_) => Err(()),
            Either::Second(result) => result,
        }
    };
    let result = match select(exchange, command_pending()).await {
        Either::First(result) => result.map_err(|_| "gatt_failed"),
        Either::Second(()) => Ok(()),
    };
    connection.disconnect();
    if !wait_disconnected(&connection).await {
        return Err("disconnect_timeout");
    }
    result
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
        complete(None);
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

async fn advertise_and_echo<'server, C>(
    peripheral: &mut Peripheral<'_, C, DefaultPacketPool>,
    server: &'server Server<'_>,
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
    let connection = match select(advertiser.accept(), command_pending()).await {
        Either::First(result) => result?.with_attribute_server(server)?,
        Either::Second(()) => return Ok(()),
    };
    update_state(|state| state.connections = state.connections.saturating_add(1));
    set_link(Link::Connected);
    log::info!(target: "ble", "CYCLING_BLE connected heap_free={}", esp_alloc::HEAP.free());
    loop {
        let event = match select(connection.next(), command_pending()).await {
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

pub fn availability() -> cycling_os::capabilities::Availability {
    use cycling_os::capabilities::Availability;
    match snapshot().link {
        Link::Off => Availability::Ready,
        Link::Failed => Availability::Failed,
        _ => Availability::Ready,
    }
}

async fn wait_disconnected(connection: &Connection<'_, DefaultPacketPool>) -> bool {
    if embassy_time::with_timeout(Duration::from_secs(5), async {
        while connection.is_connected() {
            Timer::after_millis(20).await;
        }
    })
    .await
    .is_err()
    {
        READY.store(false, Ordering::Release);
        critical_section::with(|cs| *COMMAND.borrow_ref_mut(cs) = None);
        set_link(Link::Failed);
        complete(Some("disconnect_timeout"));
        false
    } else {
        true
    }
}
