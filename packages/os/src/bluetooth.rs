//! Small BLE echo peripheral and bounded startup scan for the C606.

use core::{
    cell::RefCell,
    sync::atomic::{AtomicI32, AtomicU32, Ordering},
};

use bt_hci::cmd::le::LeSetScanParams;
use bt_hci::controller::ControllerCmdSync;
use critical_section::Mutex;
use embassy_futures::select::{Either, select};
use embassy_time::{Duration, Timer};
use esp_hal::peripherals::BT;
use esp_println::println;
use esp_radio::ble::controller::BleConnector;
use trouble_host::prelude::*;

const CONNECTIONS: usize = 1;
const CHANNELS: usize = 2;
const ECHO_NAME: &[u8] = b"Cycling Echo";

static SCAN_REPORTS: AtomicU32 = AtomicU32::new(0);
static SCAN_RSSI_MIN: AtomicI32 = AtomicI32::new(i32::MAX);
static SCAN_RSSI_MAX: AtomicI32 = AtomicI32::new(i32::MIN);
static CONNECTION_COUNT: AtomicU32 = AtomicU32::new(0);
static DISCONNECTION_COUNT: AtomicU32 = AtomicU32::new(0);
static WRITE_COUNT: AtomicU32 = AtomicU32::new(0);
static NOTIFICATION_COUNT: AtomicU32 = AtomicU32::new(0);
static SIMULATOR: Mutex<RefCell<Option<Address>>> = Mutex::new(RefCell::new(None));

#[gatt_server]
struct Server {
    echo: EchoService,
}

#[gatt_service(uuid = "7e570001-2a6f-4d75-9f6a-5afbf0f50c06")]
struct EchoService {
    /// Eight-byte little-endian test value. Accepted writes are echoed by read and notify.
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
            if AdStructure::decode(report.data)
                .any(|item| matches!(item, Ok(AdStructure::CompleteLocalName(b"Cycling Sim"))))
            {
                critical_section::with(|cs| {
                    *SIMULATOR.borrow_ref_mut(cs) = Some(Address {
                        kind: report.addr_kind,
                        addr: report.addr,
                    });
                });
            }
        }
    }
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
    // A non-identifying static random address keeps the public test protocol reproducible.
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
    println!(
        "CYCLING_BLE ready role=scan_then_peripheral att_mtu_max=60 heap_before={} heap_after={}",
        heap_before,
        esp_alloc::HEAP.free()
    );

    let app = async {
        let mut central = startup_scan(central).await;
        if let Some(target) =
            critical_section::with(|cs| SIMULATOR.borrow_ref(cs).as_ref().copied())
        {
            println!("CYCLING_BLE simulator_found connecting=owned");
            match embassy_time::with_timeout(
                Duration::from_secs(30),
                sensor_test(&stack, &mut central, target),
            )
            .await
            {
                Ok(Ok(())) => {}
                Ok(Err(())) => println!("CYCLING_BLE simulator_test result=failed"),
                Err(_) => println!("CYCLING_BLE simulator_test result=timeout"),
            }
        } else {
            println!("CYCLING_BLE simulator_found=false");
        }
        loop {
            if advertise_and_echo(&mut peripheral, &server).await.is_err() {
                println!("CYCLING_BLE peripheral_error retry_ms=1000");
                Timer::after_secs(1).await;
            }
        }
    };
    match select(runner.run_with_handler(&ScanCounter), app).await {
        Either::First(Ok(())) => println!("CYCLING_BLE runner_stopped result=ok"),
        Either::First(Err(_)) => println!("CYCLING_BLE runner_stopped result=error"),
        Either::Second(()) => println!("CYCLING_BLE application_stopped"),
    }
}

async fn startup_scan<C>(
    central: Central<'_, C, DefaultPacketPool>,
) -> Central<'_, C, DefaultPacketPool>
where
    C: Controller + ControllerCmdSync<LeSetScanParams>,
{
    let mut scanner = Scanner::new(central);
    let config = ScanConfig {
        active: true,
        phys: PhySet::M1,
        interval: Duration::from_millis(100),
        window: Duration::from_millis(50),
        ..Default::default()
    };
    println!("CYCLING_BLE scan_start seconds=10 active=true");
    match scanner.scan(&config).await {
        Ok(session) => {
            Timer::after_secs(10).await;
            drop(session);
            Timer::after_millis(100).await;
            let reports = SCAN_REPORTS.load(Ordering::Relaxed);
            let min = SCAN_RSSI_MIN.load(Ordering::Relaxed);
            let max = SCAN_RSSI_MAX.load(Ordering::Relaxed);
            println!(
                "CYCLING_BLE scan_done reports={} rssi_min={} rssi_max={}",
                reports,
                if reports == 0 { 0 } else { min },
                if reports == 0 { 0 } else { max }
            );
        }
        Err(_) => println!("CYCLING_BLE scan_failed"),
    }
    scanner.into_inner()
}

async fn sensor_test<'stack, C>(
    stack: &'stack Stack<'stack, C, DefaultPacketPool>,
    central: &mut Central<'stack, C, DefaultPacketPool>,
    target: Address,
) -> Result<(), ()>
where
    C: Controller,
{
    let filter = [(target.kind, &target.addr)];
    let config = ConnectConfig {
        connect_params: Default::default(),
        scan_config: ScanConfig {
            filter_accept_list: &filter,
            timeout: Duration::from_secs(10),
            ..Default::default()
        },
    };
    let connection = embassy_time::with_timeout(Duration::from_secs(12), central.connect(&config))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
    println!("CYCLING_BLE simulator_connected");
    let client = GattClient::<C, DefaultPacketPool, 12>::new(stack, &connection)
        .await
        .map_err(|_| ())?;
    let protocol = sensor_protocol(&client);
    match select(client.task(), protocol).await {
        Either::First(_) => Err(()),
        Either::Second(result) => result,
    }
}

async fn sensor_protocol<C>(client: &GattClient<'_, C, DefaultPacketPool, 12>) -> Result<(), ()>
where
    C: Controller,
{
    let hrs = client
        .services_by_uuid(&Uuid::new_short(0x180d))
        .await
        .map_err(|_| ())?
        .first()
        .cloned()
        .ok_or(())?;
    let cscs = client
        .services_by_uuid(&Uuid::new_short(0x1816))
        .await
        .map_err(|_| ())?
        .first()
        .cloned()
        .ok_or(())?;
    let hr_measurement: Characteristic<[u8]> = client
        .characteristic_by_uuid(&hrs, &Uuid::new_short(0x2a37))
        .await
        .map_err(|_| ())?;
    let body_location: Characteristic<u8> = client
        .characteristic_by_uuid(&hrs, &Uuid::new_short(0x2a38))
        .await
        .map_err(|_| ())?;
    let csc_measurement: Characteristic<[u8]> = client
        .characteristic_by_uuid(&cscs, &Uuid::new_short(0x2a5b))
        .await
        .map_err(|_| ())?;
    let csc_feature: Characteristic<u16> = client
        .characteristic_by_uuid(&cscs, &Uuid::new_short(0x2a5c))
        .await
        .map_err(|_| ())?;
    let mut body = [0; 1];
    let mut feature = [0; 2];
    let body_len = client
        .read_characteristic(&body_location, &mut body)
        .await
        .map_err(|_| ())?;
    let feature_len = client
        .read_characteristic(&csc_feature, &mut feature)
        .await
        .map_err(|_| ())?;
    if body_len != 1 || body[0] != 1 || feature_len != 2 || u16::from_le_bytes(feature) != 2 {
        return Err(());
    }
    let mut hr = client
        .subscribe(&hr_measurement, false)
        .await
        .map_err(|_| ())?;
    let mut csc = client
        .subscribe(&csc_measurement, false)
        .await
        .map_err(|_| ())?;
    let mut last_bpm = 0;
    for _ in 0..2 {
        let notification = embassy_time::with_timeout(Duration::from_secs(5), hr.next())
            .await
            .map_err(|_| ())?;
        last_bpm = cycling_os::ble_sensor::HeartRate::parse(notification.as_ref())
            .ok_or(())?
            .bpm;
    }
    let mut cadence = cycling_os::ble_sensor::CrankCadence::default();
    let mut rpm_tenths = None;
    for _ in 0..3 {
        let notification = embassy_time::with_timeout(Duration::from_secs(5), csc.next())
            .await
            .map_err(|_| ())?;
        let measurement =
            cycling_os::ble_sensor::CscMeasurement::parse(notification.as_ref()).ok_or(())?;
        if let Some((revolutions, event_time)) = measurement.crank {
            rpm_tenths = cadence
                .update(
                    revolutions,
                    event_time,
                    embassy_time::Instant::now().as_millis(),
                )
                .or(rpm_tenths);
        }
    }
    if !(72..=73).contains(&last_bpm) || rpm_tenths != Some(600) {
        return Err(());
    }
    println!(
        "CYCLING_BLE simulator_test result=ok hrs_notifications=2 csc_notifications=3 bpm={} cadence_tenths={}",
        last_bpm,
        rpm_tenths.unwrap_or(0)
    );
    Ok(())
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
    let connections = CONNECTION_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    println!(
        "CYCLING_BLE connected count={} heap_free={}",
        connections,
        esp_alloc::HEAP.free()
    );

    loop {
        match connection.next().await {
            GattConnectionEvent::Disconnected { reason } => {
                let count = DISCONNECTION_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                println!(
                    "CYCLING_BLE disconnected count={} reason={:?}",
                    count, reason
                );
                return Ok(());
            }
            GattConnectionEvent::Gatt { event } => {
                if let GattEvent::Write(write) = event {
                    if write.handle() != server.echo.value.handle {
                        write.accept()?.send().await;
                        continue;
                    }
                    if write.data().len() != 8 {
                        write
                            .reject(AttErrorCode::INVALID_ATTRIBUTE_VALUE_LENGTH)?
                            .send()
                            .await;
                        println!("CYCLING_BLE echo_rejected reason=length");
                        continue;
                    }
                    write.accept()?.send().await;
                    let value = server.get(&server.echo.value)?;
                    let writes = WRITE_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                    if server.echo.value.notify(&connection, &value).await.is_ok() {
                        let notifications = NOTIFICATION_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
                        println!(
                            "CYCLING_BLE echo writes={} notifications={}",
                            writes, notifications
                        );
                    } else {
                        println!("CYCLING_BLE notify_failed writes={}", writes);
                    }
                } else {
                    event.accept()?.send().await;
                }
            }
            _ => {}
        }
    }
}
