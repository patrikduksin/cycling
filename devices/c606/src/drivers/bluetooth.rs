//! BLE: one sensor as central (discovery, bounded notification bytes), and
//! beside it the phone's sync channel as peripheral.
//!
//! The phone link advertises the sync service, about once a second, only
//! while the device holds a pairing and the application listens on
//! Bluetooth; the scan response carries the pairing's id, so the phone tells
//! its own device apart. The phone reads the L2CAP PSM from the service and
//! opens a channel that `exchange::serve_channel` seals and serves. It runs
//! on its own connection beside the sensor's, so opening it costs the rider
//! no sensor. A new pairing ends the link the old one made.

use bt_hci::cmd::le::LeSetAdvEnable;
use bt_hci::cmd::le::LeSetScanEnable;
use bt_hci::cmd::le::LeSetScanParams;
use bt_hci::controller::ControllerCmdSync;
use core::cell::RefCell;
use core::sync::atomic::AtomicBool;
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

use crate::drivers::exchange;
use crate::drivers::radio_control::DISCOVERIES;
use crate::drivers::radio_control::Shared;
use device_api::ble_transport::BatteryLevel;
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
use device_api::server::{Listen, Pairing, PhoneLink};

type BleController = ExternalController<BleConnector<'static>, 10>;
type BleStack<'s> = Stack<'s, BleController, DefaultPacketPool>;
type BleCentral<'s> = Central<'s, BleController, DefaultPacketPool>;
type BlePeripheral<'s> = Peripheral<'s, BleController, DefaultPacketPool>;

/// Power, command admission and discoveries. Cancelled connects and failed
/// disconnects mark it uncertain for the rest of the boot; see `radio_control`.
static CONTROL: Shared<BleOperation, Discovery> = Shared::new();

pub fn power_status() -> PeripheralState {
    CONTROL.with(|c| c.power())
}
pub fn power_request(suspend: bool) -> Result<(), Error> {
    CONTROL.power_request(suspend, || {})
}
pub fn control() -> ControlStatus {
    CONTROL.with(|c| c.status())
}
pub fn discoveries() -> [Option<Discovery>; DISCOVERIES] {
    CONTROL.discoveries()
}
pub fn request(operation: BleOperation) -> Result<(), Error> {
    let _access = crate::capabilities::power::ACCESS.enter()?;
    CONTROL.request(operation)
}

static SELECTION: Mutex<RefCell<Option<Selection>>> = Mutex::new(RefCell::new(None));
static PACKETS: Channel<CriticalSectionRawMutex, Packet, 4> = Channel::new();
pub fn take_packet() -> Option<Packet> {
    PACKETS.try_receive().ok()
}

/// The newest Battery Level read; one value, since it changes over hours.
static BATTERY: Mutex<RefCell<Option<BatteryLevel>>> = Mutex::new(RefCell::new(None));
pub fn take_battery() -> Option<BatteryLevel> {
    critical_section::with(|cs| BATTERY.borrow_ref_mut(cs).take())
}

const BATTERY_SERVICE: u16 = 0x180f;
const BATTERY_LEVEL: u16 = 0x2a19;
/// A battery drains over hours, so a minute-old reading is as good as a
/// notification and costs no second subscription.
const BATTERY_REREAD_MS: u64 = 60_000;
/// One bound for each battery step, so a slow peer cannot starve notifications.
const BATTERY_STEP: Duration = Duration::from_secs(2);

/// The sensor, and the phone beside it.
const CONNECTIONS: usize = 2;
const CHANNELS: usize = 2;
const PHONE_NAME: &[u8] = b"VANA";
/// The L2CAP PSM the phone opens its channel on, published in `SyncService`.
const SYNC_PSM: u16 = 0x0081;
/// The sync service's UUID, little-endian as advertised.
const SYNC_SERVICE_LE: [u8; 16] = [
    0x06, 0xc6, 0x06, 0xc6, 0x06, 0xc6, 0x6b, 0x6e, 0x69, 0x4c, 0x01, 0x00, 0x41, 0x4e, 0x41, 0x56,
];
/// Advertising this slowly costs little power and still finds the phone
/// within seconds: one of the intervals Apple's accessory guidelines name.
const PHONE_INTERVAL: Duration = Duration::from_micros(1_022_500);
/// Channel memory in external RAM: an SDU (at least the channel MTU, which
/// the packet pool caps), one of the phone's sealed frames, a request head,
/// and body and response pieces.
const SDU: usize = 256;
const PIECE: usize = 512;
const CHANNEL_MEMORY: usize =
    SDU + exchange::FRAME_MAX + firmware_services::http::MAX_HEAD + PIECE + PIECE + SDU;
/// The peripheral role holds no advertiser and no connection.
static ROLE_QUIET: AtomicBool = AtomicBool::new(true);
static SCAN_REPORTS: AtomicU32 = AtomicU32::new(0);
/// Signal range of the current scan only; reset when each scan starts.
static SCAN_RSSI_MIN: AtomicI32 = AtomicI32::new(i32::MAX);
static SCAN_RSSI_MAX: AtomicI32 = AtomicI32::new(i32::MIN);
static TARGET: Mutex<RefCell<Option<Address>>> = Mutex::new(RefCell::new(None));
/// The chosen sensor's signal in its last advertisement this scan found;
/// `i32::MIN` until one does.
static TARGET_RSSI: AtomicI32 = AtomicI32::new(i32::MIN);

static STATE: Mutex<RefCell<Snapshot>> = Mutex::new(RefCell::new(Snapshot {
    link: Link::Off,
    connections: 0,
    disconnections: 0,
    notifications: 0,
    dropped: 0,
    scan_reports: 0,
}));

#[gatt_server(connections_max = 2)]
struct Server {
    sync: SyncService,
}

#[gatt_service(uuid = "56414e41-0001-4c69-6e6b-c606c606c606")]
struct SyncService {
    /// The L2CAP PSM the phone connects its byte stream to.
    #[characteristic(uuid = "56414e41-0002-4c69-6e6b-c606c606c606", read, value = SYNC_PSM)]
    psm: u16,
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
                TARGET_RSSI.store(i32::from(report.rssi), Ordering::Relaxed);
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

/// The chosen sensor's signal at its last advertisement.
pub fn target_rssi() -> Option<i8> {
    let rssi = TARGET_RSSI.load(Ordering::Relaxed);
    (rssi != i32::MIN).then(|| rssi.clamp(i32::from(i8::MIN), i32::from(i8::MAX)) as i8)
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
            exchange::set_link(PhoneLink::Failed);
            log::info!(target: "ble", "CYCLING_BLE init_failed error={:?}", error);
            return;
        }
    };
    let controller: BleController = ExternalController::new(connector);
    let mut resources: HostResources<BleController, DefaultPacketPool, CONNECTIONS, CHANNELS> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random([0x15, 0xc6, 0x06, 0x00, 0x00, 0xc0]))
        .register_l2cap_spsm(SYNC_PSM)
        .build();
    let central = stack.central();
    let peripheral = stack.peripheral();
    let mut runner = stack.runner();
    let server = match Server::new_with_config(GapConfig::Peripheral(PeripheralConfig {
        name: "VANA",
        appearance: &appearance::computer::GENERIC_COMPUTER,
    })) {
        Ok(server) => server,
        Err(_) => {
            set_link(Link::Failed);
            exchange::set_link(PhoneLink::Failed);
            log::info!(target: "ble", "CYCLING_BLE server_failed");
            return;
        }
    };

    log::info!(target: "ble",
        "CYCLING_BLE ready mode={} att_mtu_max=60 heap_before={} heap_after={} psm_handle={}",
        "runtime",
        heap_before,
        esp_alloc::HEAP.free(),
        server.sync.psm.handle
    );

    CONTROL.with(|c| c.set_ready());
    let outcome = select3(
        runner.run_with_handler(&ScanCounter),
        application(&stack, central, peripheral, &server),
        peripheral_role(&stack, &server),
    )
    .await;
    CONTROL.with(|c| c.stop(ControlError::RunnerStopped));
    set_link(Link::Failed);
    exchange::set_link(PhoneLink::Failed);
    match outcome {
        Either3::First(Ok(())) => {
            log::info!(target: "ble", "CYCLING_BLE runner_stopped result=ok")
        }
        Either3::First(Err(_)) => {
            log::info!(target: "ble", "CYCLING_BLE runner_stopped result=error")
        }
        Either3::Second(()) | Either3::Third(()) => {
            log::info!(target: "ble", "CYCLING_BLE application_stopped")
        }
    }
}

/// Transport intent that survives individual operations and power cycles.
struct Session {
    retry: Reconnect,
}

/// Owner loop of the central role: power transitions first, then one command
/// or automatic reconnect, then a reconnect backoff or waiting for new work.
async fn application<'a>(
    stack: &'a BleStack<'_>,
    mut central: BleCentral<'a>,
    mut peripheral: BlePeripheral<'a>,
    server: &Server<'_>,
) {
    let mut session = Session {
        retry: Reconnect::default(),
    };
    loop {
        if !CONTROL.with(|c| c.ready()) {
            return;
        }
        if CONTROL.with(|c| c.suspended()) {
            // Re-check from the top after quiescing: a disconnect timeout can
            // stop the owner, and then nothing would ever signal it again.
            if CONTROL.with(|c| c.power()) == PeripheralState::Pending {
                quiesce(stack, &mut peripheral).await;
            } else {
                CONTROL.changed().await;
            }
            continue;
        }
        // Keep selection and reconnect intent unchanged across power
        // preparation. Fresh transport work resumes below.
        if CONTROL.with(|c| c.complete_resume()) == PeripheralState::Failed {
            CONTROL.changed().await;
            continue;
        }
        let requested = CONTROL.with(|c| c.take_command());
        let automatic = requested.is_none() && session.retry.requested();
        let operation = if automatic {
            Some(BleOperation::Connect)
        } else {
            requested
        };
        if let Some(operation) = operation {
            central = perform(operation, automatic, &mut session, stack, central, server).await;
        }
        if !CONTROL.with(|c| c.ready()) {
            return;
        }
        if CONTROL.with(|c| c.suspended()) {
            continue;
        }
        idle(&mut session).await;
    }
}

/// Stop scanning and advertising with the runner alive, and report whether the
/// controller acknowledged inactivity.
async fn quiesce(stack: &BleStack<'_>, peripheral: &mut BlePeripheral<'_>) {
    // The peripheral role sees the suspend and lets go of its link first.
    let parked = embassy_time::with_timeout(Duration::from_secs(6), async {
        while !ROLE_QUIET.load(Ordering::Relaxed) {
            Timer::after_millis(20).await;
        }
    })
    .await
    .is_ok();
    // Dropping an advertiser/scan session only queues cancellation.
    // Wait for actual controller acknowledgements with the runner alive.
    let result = embassy_time::with_timeout(Duration::from_secs(5), async {
        stack.command(LeSetScanEnable::new(false, false)).await?;
        stack.command(LeSetAdvEnable::new(false)).await
    })
    .await;
    let mut quiet = parked && matches!(result, Ok(Ok(_)));
    if !quiet {
        CONTROL.with(|c| c.mark_uncertain());
    }
    // A peer can connect just before the advertising disable. Drain
    // that accepted controller connection before claiming inactivity.
    while let Some(connection) = peripheral.try_accept() {
        connection.disconnect();
        quiet &= wait_disconnected(&connection).await;
    }
    while PACKETS.try_receive().is_ok() {}
    let state = CONTROL.with(|c| c.suspend_outcome(quiet));
    set_link(if state == PeripheralState::Quiescent {
        Link::Off
    } else {
        Link::Failed
    });
    CONTROL.with(|c| c.power_complete(true, state));
}

async fn perform<'a>(
    operation: BleOperation,
    automatic: bool,
    session: &mut Session,
    stack: &'a BleStack<'_>,
    central: BleCentral<'a>,
    server: &Server<'_>,
) -> BleCentral<'a> {
    match operation {
        BleOperation::Select(selection) => {
            session.retry.disconnect();
            critical_section::with(|cs| *SELECTION.borrow_ref_mut(cs) = selection);
            while PACKETS.try_receive().is_ok() {}
            critical_section::with(|cs| *BATTERY.borrow_ref_mut(cs) = None);
            set_link(Link::Off);
            CONTROL.with(|c| c.finish(automatic, None));
            central
        }
        BleOperation::Disconnect => {
            session.retry.disconnect();
            set_link(Link::Off);
            CONTROL.with(|c| c.finish(automatic, None));
            central
        }
        BleOperation::Scan => {
            session.retry.disconnect();
            CONTROL.clear_discoveries();
            let (central, _, success) = scan_target(central).await;
            set_link(Link::Off);
            CONTROL.with(|c| c.finish(automatic, (!success).then_some(ControlError::ScanFailed)));
            central
        }
        BleOperation::Connect => connect(automatic, session, stack, central, server).await,
    }
}

async fn connect<'a>(
    automatic: bool,
    session: &mut Session,
    stack: &'a BleStack<'_>,
    central: BleCentral<'a>,
    server: &Server<'_>,
) -> BleCentral<'a> {
    if !automatic {
        session.retry.connect();
    }
    let Some(selection) = critical_section::with(|cs| *SELECTION.borrow_ref(cs)) else {
        set_link(Link::Off);
        session.retry.disconnect();
        CONTROL.with(|c| c.finish(automatic, Some(ControlError::Unconfigured)));
        return central;
    };
    let (mut central, target, _) = scan_target(central).await;
    // A suspend, or a user command overriding an automatic retry, wins.
    if CONTROL.with(|c| c.suspended()) || (automatic && CONTROL.with(|c| c.has_command())) {
        return central;
    }
    let Some(target) = target else {
        set_link(Link::Failed);
        CONTROL.with(|c| c.finish(automatic, Some(ControlError::PeerNotFound)));
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
        Ok(()) if CONTROL.with(|c| c.has_command()) => set_link(Link::Off),
        result => {
            set_link(Link::Failed);
            CONTROL.with(|c| {
                c.finish(
                    automatic,
                    Some(result.err().unwrap_or(ControlError::ConnectionEnded)),
                )
            });
        }
    }
    central
}

/// Between central operations: a reconnect backoff, or simply waiting for the
/// next command or power request.
async fn idle(session: &mut Session) {
    if session.retry.requested() {
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
    TARGET_RSSI.store(i32::MIN, Ordering::Relaxed);
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

async fn client_attempt<'a, C>(
    stack: &'a Stack<'_, C, DefaultPacketPool>,
    central: &mut Central<'a, C, DefaultPacketPool>,
    server: &Server<'_>,
    target: Address,
    selection: Selection,
    report_completion: bool,
) -> Result<(), ControlError>
where
    C: Controller,
{
    let filter = [target];
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
                CONTROL.with(|c| c.mark_uncertain());
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
    let mut battery = find_battery(client).await;
    let mut battery_due_ms = 0;
    set_link(Link::Connected);
    if report_completion {
        CONTROL.with(|c| c.complete(None));
    }
    loop {
        if let Some(characteristic) = &battery {
            let now = embassy_time::Instant::now().as_millis();
            if now >= battery_due_ms {
                battery_due_ms = now.saturating_add(BATTERY_REREAD_MS);
                if !read_battery(client, characteristic).await {
                    battery = None;
                }
            }
        }
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

/// The peer's Battery Level characteristic, when it has one. Any failure means
/// no battery reading, never a failed connection.
async fn find_battery<C>(
    client: &GattClient<'_, C, DefaultPacketPool, 12>,
) -> Option<Characteristic<[u8]>>
where
    C: Controller,
{
    let services = embassy_time::with_timeout(
        BATTERY_STEP,
        client.services_by_uuid(&Uuid::new_short(BATTERY_SERVICE)),
    )
    .await
    .ok()?
    .ok()?;
    let service = services.first()?.clone();
    embassy_time::with_timeout(
        BATTERY_STEP,
        client.characteristic_by_uuid(&service, &Uuid::new_short(BATTERY_LEVEL)),
    )
    .await
    .ok()?
    .ok()
}

/// Read the level once. Returns false after a timeout: trouble-host's
/// `request` is not cancel-safe, so a timed-out read leaves its reply queued
/// and the next request on this connection would take that stale reply for its
/// own. No request of any kind may follow a timed-out one, so the caller stops
/// reading for the rest of the connection; a new connection starts clean.
async fn read_battery<C>(
    client: &GattClient<'_, C, DefaultPacketPool, 12>,
    characteristic: &Characteristic<[u8]>,
) -> bool
where
    C: Controller,
{
    let mut value = [0u8; 4];
    let read = embassy_time::with_timeout(
        BATTERY_STEP,
        client.read_characteristic(characteristic, &mut value),
    )
    .await;
    let Ok(Ok(1)) = read else {
        return read.is_ok();
    };
    let level = BatteryLevel {
        connection: snapshot().connections,
        raw: value[0],
        received_ms: embassy_time::Instant::now().as_millis(),
    };
    critical_section::with(|cs| *BATTERY.borrow_ref_mut(cs) = Some(level));
    true
}

/// The pairing the phone link is wanted for: the device holds one, the
/// application listens on Bluetooth and the radio is not suspended.
fn wanted() -> Option<Pairing> {
    let on =
        CONTROL.with(|c| c.ready() && !c.suspended()) && exchange::listening() == Listen::Bluetooth;
    exchange::pairing().filter(|_| on)
}

/// Returns once the phone link is no longer wanted for `pairing`.
async fn unwanted(pairing: Pairing) {
    while wanted() == Some(pairing) {
        Timer::after_millis(200).await;
    }
}

/// Owner loop of the peripheral role, beside the central's: advertise and
/// serve the phone while it is wanted.
async fn peripheral_role(stack: &BleStack<'_>, server: &Server<'_>) {
    let mut peripheral = stack.peripheral();
    exchange::install(
        || crate::drivers::psram::zeroed(exchange::BUFFER),
        || crate::drivers::psram::zeroed(exchange::INBOX),
    );
    let mut memory = crate::drivers::psram::zeroed(CHANNEL_MEMORY);
    loop {
        let Some(pairing) = wanted() else {
            exchange::set_link(PhoneLink::Off);
            ROLE_QUIET.store(true, Ordering::Relaxed);
            Timer::after_millis(200).await;
            continue;
        };
        ROLE_QUIET.store(false, Ordering::Relaxed);
        let result = match memory.as_deref_mut() {
            Some(memory) => phone_link(stack, &mut peripheral, server, memory, pairing).await,
            None => {
                log::warn!(target: "ble", "CYCLING_BLE phone no external RAM for the channel");
                exchange::set_link(PhoneLink::Failed);
                unwanted(pairing).await;
                Ok(())
            }
        };
        if result.is_err() {
            // A GATT error can drop a live connection and only queue its
            // disconnect. Do not claim power quiescence without completion.
            CONTROL.with(|c| c.mark_uncertain());
            Timer::after_secs(1).await;
        }
    }
}

/// Advertise the sync service as `VANA` with the pairing's id, then serve
/// one phone's connection and its channel until the link ends or the phone
/// link is unwanted for `pairing`.
async fn phone_link<'a>(
    stack: &'a BleStack<'_>,
    peripheral: &mut BlePeripheral<'a>,
    server: &Server<'_>,
    memory: &mut [u8],
    pairing: Pairing,
) -> Result<(), ()> {
    let mut adv_data = [0; 31];
    let adv_len = AdStructure::encode_slice(
        &[
            AdStructure::Flags(LE_GENERAL_DISCOVERABLE | BR_EDR_NOT_SUPPORTED),
            AdStructure::IncompleteServiceUuids128(&[SYNC_SERVICE_LE]),
        ],
        &mut adv_data,
    )
    .map_err(drop)?;
    // Service data for a 128-bit UUID: the sync service, then the id.
    let mut service_data = [0; 20];
    service_data[..16].copy_from_slice(&SYNC_SERVICE_LE);
    service_data[16..].copy_from_slice(&pairing.id);
    let mut scan_data = [0; 31];
    let scan_len = AdStructure::encode_slice(
        &[
            AdStructure::CompleteLocalName(PHONE_NAME),
            AdStructure::Unknown {
                ty: 0x21,
                data: &service_data,
            },
        ],
        &mut scan_data,
    )
    .map_err(drop)?;
    let parameters = AdvertisementParameters {
        interval_min: PHONE_INTERVAL,
        interval_max: PHONE_INTERVAL,
        ..Default::default()
    };
    let advertiser = peripheral
        .advertise(
            &parameters,
            Advertisement::ConnectableScannableUndirected {
                adv_data: &adv_data[..adv_len],
                scan_data: &scan_data[..scan_len],
            },
        )
        .await
        .map_err(drop)?;
    exchange::set_link(PhoneLink::Advertising);
    log::info!(target: "ble", "CYCLING_BLE phone_advertising");
    let connection = match select(advertiser.accept(), unwanted(pairing)).await {
        Either::First(result) => result
            .map_err(drop)?
            .with_attribute_server(server)
            .map_err(drop)?,
        Either::Second(()) => return Ok(()),
    };
    exchange::set_link(PhoneLink::Connected);
    log::info!(target: "ble", "CYCLING_BLE phone_connected heap_free={}", esp_alloc::HEAP.free());
    fast_link(stack, connection.raw()).await;
    let listener = L2capChannel::listen(stack, connection.raw());
    // One future for the channel's whole life, polled beside the GATT loop and
    // never restarted: dropping it mid-send would cut a response short.
    let mut sync = core::pin::pin!(serve_sync(stack, &listener, memory, &pairing.key));
    loop {
        match select3(connection.next(), unwanted(pairing), sync.as_mut()).await {
            Either3::First(GattConnectionEvent::Disconnected { reason }) => {
                exchange::set_link(PhoneLink::Off);
                log::info!(target: "ble", "CYCLING_BLE phone_disconnected reason={:?}", reason);
                return Ok(());
            }
            Either3::First(GattConnectionEvent::Gatt { event }) => {
                if let Ok(reply) = event.accept() {
                    reply.send().await;
                }
            }
            Either3::First(GattConnectionEvent::RequestConnectionParams(request)) => {
                let _ = request.accept(None, stack).await;
            }
            Either3::First(_) => {}
            Either3::Second(()) => {
                connection.raw().disconnect();
                wait_disconnected(connection.raw()).await;
                exchange::set_link(PhoneLink::Off);
                return Ok(());
            }
            Either3::Third(never) => match never {},
        }
    }
}

/// Ask for the fastest link an iPhone grants: 2M PHY, 251-byte packets, a
/// 15 ms interval (iOS's floor). Each is a request; log the outcome.
async fn fast_link(stack: &BleStack<'_>, connection: &Connection<'_, DefaultPacketPool>) {
    let phy = connection.set_phy(stack, PhyKind::Le2M).await.is_ok();
    let length = connection
        .update_data_length(stack, 251, 2120)
        .await
        .is_ok();
    let params = RequestedConnParams {
        min_connection_interval: Duration::from_millis(15),
        max_connection_interval: Duration::from_millis(15),
        max_latency: 0,
        min_event_length: Duration::from_millis(0),
        max_event_length: Duration::from_millis(15),
        supervision_timeout: Duration::from_secs(4),
    };
    let interval = connection
        .update_connection_params(stack, &params)
        .await
        .is_ok();
    log::info!(target: "ble",
        "CYCLING_BLE fast_link phy_req={} dle_req={} interval_req={}",
        phy, length, interval
    );
}

struct Incoming<'a, 's> {
    reader: L2capChannelReader<'a, DefaultPacketPool>,
    stack: &'a BleStack<'s>,
}

impl exchange::ReceiveSdu for Incoming<'_, '_> {
    async fn receive(&mut self, sdu: &mut [u8]) -> Option<usize> {
        self.reader.receive(self.stack, sdu).await.ok()
    }
}

struct Outgoing<'a, 's> {
    writer: L2capChannelWriter<'a, DefaultPacketPool>,
    stack: &'a BleStack<'s>,
}

impl exchange::SendSdu for Outgoing<'_, '_> {
    async fn send(&mut self, sdu: &[u8]) -> bool {
        self.writer.send(self.stack, sdu).await.is_ok()
    }
}

/// Accept the phone's channels on this connection, one at a time, and serve
/// each through the exchange. Runs for the connection's life.
async fn serve_sync<'a>(
    stack: &'a BleStack<'_>,
    listener: &L2capChannelListener<'a, BleController, DefaultPacketPool>,
    memory: &mut [u8],
    key: &[u8; 32],
) -> core::convert::Infallible {
    loop {
        let channel = match listener.accept(&L2capChannelConfig::default()).await {
            Ok(channel) => channel,
            Err(_) => {
                log::info!(target: "ble", "CYCLING_BLE sync_accept_failed");
                Timer::after_secs(1).await;
                continue;
            }
        };
        // `send` refuses an SDU over the peer's MTU; the cap at our own MTU as
        // well is the size the device runs were measured with.
        let piece = usize::from(channel.mtu().min(channel.peer_mtu()));
        log::info!(target: "ble",
            "CYCLING_BLE sync_open mtu={} mps={} peer_mtu={} peer_mps={}",
            channel.mtu(), channel.mps(), channel.peer_mtu(), channel.peer_mps()
        );
        let (writer, reader) = channel.split();
        let (sdu, rest) = memory.split_at_mut(SDU);
        let (frame, rest) = rest.split_at_mut(exchange::FRAME_MAX);
        let (head, rest) = rest.split_at_mut(firmware_services::http::MAX_HEAD);
        let (into, out) = rest.split_at_mut(PIECE);
        let rng = esp_hal::rng::Rng::new();
        let mut hello = [0; crate::drivers::seal::HELLO];
        for word in hello.chunks_mut(4) {
            word.copy_from_slice(&rng.random().to_le_bytes());
        }
        let ended = exchange::serve_channel(
            Incoming { reader, stack },
            Outgoing { writer, stack },
            piece,
            exchange::ChannelBuffers {
                sdu,
                frame,
                head,
                into,
                out,
            },
            key,
            hello,
        )
        .await;
        log::info!(target: "ble", "CYCLING_BLE sync_closed ended={:?}", ended);
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
        CONTROL.with(|c| c.mark_uncertain());
        set_link(Link::Failed);
        CONTROL.with(|c| c.stop(ControlError::DisconnectTimeout));
        false
    } else {
        true
    }
}
