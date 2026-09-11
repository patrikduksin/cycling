//! BLE echo peripheral or one explicitly selected continuous standard sensor.

use bt_hci::{cmd::le::LeSetScanParams, controller::ControllerCmdSync};
use core::{
    cell::RefCell,
    sync::atomic::{AtomicBool, AtomicI32, AtomicU32, Ordering},
};
use critical_section::Mutex;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_hal::peripherals::BT;
use esp_println::println;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

use cycling_os::ble_sensor::{
    CadenceReading, CscMeasurement, HeartRate, Link, Profile, Snapshot, fresh_u16, usable_heart,
};

mod config {
    include!(env!("CYCLING_BLE_CONFIG"));
}

const CONNECTIONS: usize = 1;
const CHANNELS: usize = 2;
const ECHO_NAME: &[u8] = b"Cycling Echo";
static SCAN_REPORTS: AtomicU32 = AtomicU32::new(0);
static SCAN_RSSI_MIN: AtomicI32 = AtomicI32::new(i32::MAX);
static SCAN_RSSI_MAX: AtomicI32 = AtomicI32::new(i32::MIN);
static TARGET: Mutex<RefCell<Option<Address>>> = Mutex::new(RefCell::new(None));
static RECONNECT: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy)]
struct SensorState {
    link: Link,
    heart: Option<(u16, u64)>,
    cadence: Option<(u16, u64)>,
    connections: u32,
    disconnections: u32,
    notifications: u32,
    invalid: u32,
    rr_dropped: u32,
}

impl SensorState {
    const fn new() -> Self {
        Self {
            link: Link::Off,
            heart: None,
            cadence: None,
            connections: 0,
            disconnections: 0,
            notifications: 0,
            invalid: 0,
            rr_dropped: 0,
        }
    }
}

static SENSOR: Mutex<RefCell<SensorState>> = Mutex::new(RefCell::new(SensorState::new()));

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
            if config::PROFILE == 0 {
                continue;
            }
            let name_matches = AdStructure::decode(report.data).any(|item| {
                matches!(item,
                    Ok(AdStructure::CompleteLocalName(name) | AdStructure::ShortenedLocalName(name))
                    if name == config::TARGET_NAME)
            });
            let address_matches = config::TARGET_ADDRESS
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

pub fn snapshot(now: u64) -> Snapshot {
    critical_section::with(|cs| {
        let state = *SENSOR.borrow_ref(cs);
        let (heart_bpm, heart_age_ms) = state
            .heart
            .map(|(value, at)| fresh_u16(value, at, now))
            .unwrap_or((None, None));
        let (cadence_tenths, cadence_age_ms) = state
            .cadence
            .map(|(value, at)| fresh_u16(value, at, now))
            .unwrap_or((None, None));
        Snapshot {
            profile: Profile::from_u8(config::PROFILE),
            link: state.link,
            heart_bpm,
            heart_age_ms,
            cadence_tenths,
            cadence_age_ms,
            connections: state.connections,
            disconnections: state.disconnections,
            notifications: state.notifications,
            invalid: state.invalid,
            rr_dropped: state.rr_dropped,
        }
    })
}

pub fn request_reconnect() -> bool {
    if config::PROFILE == 0 {
        false
    } else {
        RECONNECT.store(true, Ordering::Relaxed);
        true
    }
}

fn update_sensor(update: impl FnOnce(&mut SensorState)) {
    critical_section::with(|cs| update(&mut SENSOR.borrow_ref_mut(cs)));
}

fn set_link(link: Link) {
    update_sensor(|state| state.link = link);
}

fn disconnected() {
    update_sensor(|state| {
        state.link = Link::Retrying;
        state.heart = None;
        state.cadence = None;
        state.disconnections = state.disconnections.saturating_add(1);
    });
}

#[embassy_executor::task]
pub async fn start(bt: BT<'static>) {
    let heap_before = esp_alloc::HEAP.free();
    let connector = match BleConnector::new(bt, Default::default()) {
        Ok(connector) => connector,
        Err(error) => {
            println!("CYCLING_BLE init_failed error={:?}", error);
            return;
        }
    };
    let controller: ExternalController<_, 10> = ExternalController::new(connector);
    let mut resources: HostResources<DefaultPacketPool, CONNECTIONS, CHANNELS> =
        HostResources::new();
    let stack = trouble_host::new(controller, &mut resources)
        .set_random_address(Address::random([0x15, 0xc6, 0x06, 0x00, 0x00, 0xc0]));
    let Host {
        central,
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
            println!("CYCLING_BLE server_failed");
            return;
        }
    };
    let profile = Profile::from_u8(config::PROFILE);
    println!(
        "CYCLING_BLE ready mode={} att_mtu_max=60 heap_before={} heap_after={}",
        profile.name(),
        heap_before,
        esp_alloc::HEAP.free()
    );

    let app = async {
        if profile == Profile::Echo {
            set_link(Link::Off);
            loop {
                if advertise_and_echo(&mut peripheral, &server).await.is_err() {
                    println!("CYCLING_BLE peripheral_error retry_ms=1000");
                    Timer::after_secs(1).await;
                }
            }
        } else {
            sensor_loop(&stack, central, profile).await;
        }
    };
    let outcome = select(runner.run_with_handler(&ScanCounter), app).await;
    update_sensor(|state| {
        state.link = Link::Off;
        state.heart = None;
        state.cadence = None;
    });
    match outcome {
        Either::First(Ok(())) => println!("CYCLING_BLE runner_stopped result=ok"),
        Either::First(Err(_)) => println!("CYCLING_BLE runner_stopped result=error"),
        Either::Second(()) => println!("CYCLING_BLE application_stopped"),
    }
}

async fn sensor_loop<'stack, C>(
    stack: &'stack Stack<'stack, C, DefaultPacketPool>,
    mut central: Central<'stack, C, DefaultPacketPool>,
    profile: Profile,
) where
    C: Controller + ControllerCmdSync<LeSetScanParams>,
{
    let mut retry_seconds = 1u64;
    loop {
        let (returned, target) = scan_target(central).await;
        central = returned;
        let Some(target) = target else {
            set_link(Link::Retrying);
            println!("CYCLING_BLE sensor_found=false retry_s={}", retry_seconds);
            Timer::after_secs(retry_seconds).await;
            retry_seconds = (retry_seconds * 2).min(30);
            continue;
        };
        set_link(Link::Connecting);
        RECONNECT.store(false, Ordering::Relaxed);
        let before = snapshot(embassy_time::Instant::now().as_millis());
        match sensor_attempt(stack, &mut central, target, profile).await {
            Ok(()) => println!("CYCLING_BLE sensor_disconnected requested=true"),
            Err(()) => println!("CYCLING_BLE sensor_disconnected requested=false"),
        }
        let after = snapshot(embassy_time::Instant::now().as_millis());
        if after.connections != before.connections {
            disconnected();
        } else {
            set_link(Link::Retrying);
        }
        if after.notifications != before.notifications {
            retry_seconds = 1;
        }
        Timer::after_secs(retry_seconds).await;
        retry_seconds = (retry_seconds * 2).min(30);
    }
}

async fn scan_target<C>(
    central: Central<'_, C, DefaultPacketPool>,
) -> (Central<'_, C, DefaultPacketPool>, Option<Address>)
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
    match scanner.scan(&scan_config).await {
        Ok(session) => {
            Timer::after_secs(10).await;
            drop(session);
            Timer::after_millis(100).await;
        }
        Err(_) => println!("CYCLING_BLE scan_failed"),
    }
    let target = critical_section::with(|cs| TARGET.borrow_ref_mut(cs).take());
    let reports = SCAN_REPORTS.load(Ordering::Relaxed);
    println!(
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
    (scanner.into_inner(), target)
}

async fn sensor_attempt<'stack, C>(
    stack: &'stack Stack<'stack, C, DefaultPacketPool>,
    central: &mut Central<'stack, C, DefaultPacketPool>,
    target: Address,
    profile: Profile,
) -> Result<(), ()>
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
            .map_err(|_| ())?
            .map_err(|_| ())?;
    update_sensor(|state| state.connections = state.connections.saturating_add(1));
    println!("CYCLING_BLE sensor_connected profile={}", profile.name());
    let client = embassy_time::with_timeout(
        Duration::from_secs(10),
        GattClient::<C, DefaultPacketPool, 12>::new(stack, &connection),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    let result = match profile {
        Profile::HeartRate => match select(client.task(), heart_protocol(&client)).await {
            Either::First(_) => Err(()),
            Either::Second(result) => result,
        },
        Profile::Cadence => match select(client.task(), cadence_protocol(&client)).await {
            Either::First(_) => Err(()),
            Either::Second(result) => result,
        },
        Profile::Echo => Err(()),
    };
    connection.disconnect();
    result
}

async fn heart_protocol<C>(client: &GattClient<'_, C, DefaultPacketPool, 12>) -> Result<(), ()>
where
    C: Controller,
{
    let services = embassy_time::with_timeout(
        Duration::from_secs(10),
        client.services_by_uuid(&Uuid::new_short(0x180d)),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    let service = services.first().cloned().ok_or(())?;
    let measurement: Characteristic<[u8]> = embassy_time::with_timeout(
        Duration::from_secs(10),
        client.characteristic_by_uuid(&service, &Uuid::new_short(0x2a37)),
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
    loop {
        if RECONNECT.swap(false, Ordering::Relaxed) {
            return Ok(());
        }
        let Ok(notification) =
            embassy_time::with_timeout(Duration::from_secs(1), subscription.next()).await
        else {
            continue;
        };
        let now = embassy_time::Instant::now().as_millis();
        if let Some(value) = HeartRate::parse(notification.as_ref()) {
            update_sensor(|state| {
                state.notifications = state.notifications.saturating_add(1);
                state.rr_dropped = state.rr_dropped.saturating_add(u32::from(value.rr_dropped));
                if let Some(bpm) = usable_heart(value) {
                    state.heart = Some((bpm, now));
                } else {
                    state.heart = None;
                }
            });
        } else {
            update_sensor(|state| state.invalid = state.invalid.saturating_add(1));
        }
    }
}

async fn cadence_protocol<C>(client: &GattClient<'_, C, DefaultPacketPool, 12>) -> Result<(), ()>
where
    C: Controller,
{
    let services = embassy_time::with_timeout(
        Duration::from_secs(10),
        client.services_by_uuid(&Uuid::new_short(0x1816)),
    )
    .await
    .map_err(|_| ())?
    .map_err(|_| ())?;
    let service = services.first().cloned().ok_or(())?;
    let measurement: Characteristic<[u8]> = embassy_time::with_timeout(
        Duration::from_secs(10),
        client.characteristic_by_uuid(&service, &Uuid::new_short(0x2a5b)),
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
    let mut cadence = CadenceReading::default();
    set_link(Link::Connected);
    loop {
        if RECONNECT.swap(false, Ordering::Relaxed) {
            cadence.disconnected();
            return Ok(());
        }
        let Ok(notification) =
            embassy_time::with_timeout(Duration::from_secs(1), subscription.next()).await
        else {
            continue;
        };
        let now = embassy_time::Instant::now().as_millis();
        let Some(value) = CscMeasurement::parse(notification.as_ref()) else {
            update_sensor(|state| state.invalid = state.invalid.saturating_add(1));
            continue;
        };
        update_sensor(|state| state.notifications = state.notifications.saturating_add(1));
        if let Some((revolutions, event_time)) = value.crank
            && let Some(value) = cadence.update(revolutions, event_time, now)
        {
            update_sensor(|state| state.cadence = Some((value as u16, now)));
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
    println!("CYCLING_BLE advertising");
    let advertiser = peripheral
        .advertise(
            &Default::default(),
            Advertisement::ConnectableScannableUndirected {
                adv_data: &adv_data[..adv_len],
                scan_data: &scan_data[..scan_len],
            },
        )
        .await?;
    let connection = advertiser.accept().await?.with_attribute_server(server)?;
    update_sensor(|state| state.connections = state.connections.saturating_add(1));
    println!("CYCLING_BLE connected heap_free={}", esp_alloc::HEAP.free());
    loop {
        match connection.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                update_sensor(|state| {
                    state.disconnections = state.disconnections.saturating_add(1)
                });
                println!("CYCLING_BLE disconnected reason={:?}", reason);
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
                        println!("CYCLING_BLE echo_rejected reason=length");
                    } else {
                        write.accept()?.send().await;
                        let value = server.get(&server.echo.value)?;
                        update_sensor(|state| {
                            state.notifications = state.notifications.saturating_add(1)
                        });
                        if server.echo.value.notify(&connection, &value).await.is_err() {
                            println!("CYCLING_BLE notify_failed");
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
