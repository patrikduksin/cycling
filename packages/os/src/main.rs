#![no_std]
#![no_main]
mod device;
mod logging;
#[cfg(feature = "cycling")]
use cycling_os::sdk_runtime;
mod terminal;
use esp_backtrace as _;
esp_bootloader_esp_idf::esp_app_desc!();

type FirmwareShell = cycling_os::shell::Shell<
    device::display::Display<'static>,
    device::c606::Input,
    device::c606::Power,
    device::storage::Backend<'static>,
>;

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    let board = device::c606::start(spawner).await;
    let boot = esp_hal::rng::Rng::new().random();
    logging::init(boot);
    logging::metadata(false);
    let mut system = cycling_os::shell::Shell::new(
        board.display,
        board.input,
        board.power,
        board.storage,
        embassy_time::Instant::now().as_millis(),
    )
    .unwrap();
    #[cfg(feature = "debug-harness")]
    {
        system.harness = cycling_os::harness::State::new(boot);
        system.harness.clock = Some(|| embassy_time::Instant::now().as_millis());
        // Composition reserves capture storage in external RAM. The library
        // receives only an owned byte slice; DMA still uses device-owned strips.
        let layout =
            core::alloc::Layout::from_size_align(cycling_os::harness::CAPTURE_BYTES, 4).unwrap();
        let pointer = unsafe { device::psram::allocate_external(layout) };
        if !pointer.is_null() {
            system.harness.buffer = Some(cycling_os::harness::CaptureBuffer::Borrowed(unsafe {
                core::slice::from_raw_parts_mut(pointer, layout.size())
            }));
        }
    }
    system.boot_id = boot;
    system.reset = board.reset;
    system.crash = board.crash;
    system.heap_min_sampled = esp_alloc::HEAP.free();
    #[cfg(feature = "cycling")]
    let profile = cycling_os::sdk::ble_sensor::Profile::from_u8(system.settings.ble_profile);
    spawner.spawn(
        console(
            board.terminal,
            system,
            board.ant,
            board.ble,
            board.positioning,
            board.bulk,
            board.sound,
            board.sensors,
            board.network,
            board.input_observation,
            #[cfg(feature = "cycling")]
            profile,
        )
        .unwrap(),
    );
    core::future::pending().await
}
#[embassy_executor::task]
async fn console(
    usb: device::usb::Usb,
    mut system: FirmwareShell,
    mut ant: device::c606::Ant,
    mut ble: device::c606::Ble,
    mut position: device::c606::Positioning,
    mut bulk: device::c606::Bulk,
    mut sound: device::c606::Sound,
    mut sensors: device::c606::Sensors,
    mut network: device::c606::Network,
    input: device::c606::Input,
    #[cfg(feature = "cycling")] profile: cycling_os::sdk::ble_sensor::Profile,
) {
    let mut terminal = terminal::Terminal::new(usb);
    let mut next_health = 0;
    #[cfg(feature = "cycling")]
    let mut sdk = sdk_runtime::Runtime::new(profile);
    #[cfg(feature = "cycling")]
    {
        sdk.clock = Some(|| embassy_time::Instant::now().as_millis());
    }
    #[cfg(feature = "cycling")]
    use cycling_os::capabilities::Ble as _;
    use cycling_os::capabilities::Network as _;
    if let Some(configuration) = system.settings.wifi {
        if network
            .request(cycling_os::connectivity::WifiOperation::Configure(Some(
                configuration,
            )))
            .is_ok()
        {
            let _ = embassy_time::with_timeout(embassy_time::Duration::from_secs(3), async {
                while network.control().operation == cycling_os::connectivity::Operation::Pending {
                    embassy_time::Timer::after_millis(10).await;
                }
            })
            .await;
            if network.control().operation == cycling_os::connectivity::Operation::Completed {
                let _ = network.request(cycling_os::connectivity::WifiOperation::Connect);
            }
        }
    }
    #[cfg(feature = "cycling")]
    if let Some(selection) = system.settings.ble.and_then(|saved| {
        cycling_os::sdk::ble::selection(
            cycling_os::sdk::ble_sensor::Profile::from_u8(system.settings.ble_profile),
            saved.name.bytes(),
            saved.address,
        )
    }) {
        if ble
            .request(cycling_os::ble_transport::Operation::Select(Some(
                selection,
            )))
            .is_ok()
        {
            let _ = embassy_time::with_timeout(embassy_time::Duration::from_secs(3), async {
                while ble.control().operation == cycling_os::connectivity::Operation::Pending {
                    embassy_time::Timer::after_millis(10).await;
                }
            })
            .await;
            let _ = ble.request(cycling_os::ble_transport::Operation::Connect);
        }
    }
    let started = embassy_time::Instant::now();
    system.present();
    system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
    loop {
        let now = embassy_time::Instant::now().as_millis();
        system.heap_min_sampled = system.heap_min_sampled.min(esp_alloc::HEAP.free());
        #[cfg(feature = "cycling")]
        system.set_app_active(sdk.input_active());
        system.tick(now);
        #[cfg(feature = "cycling")]
        for _ in 0..16 {
            let Some(edge) = system.take_app_input() else {
                break;
            };
            sdk.input(edge.input, now, &mut ant, &mut system.store.data());
        }
        system.observe_position(&position, now);
        let started = embassy_time::Instant::now();
        system.present();
        system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
        #[cfg(feature = "cycling")]
        sdk.tick(
            &mut system.store.data(),
            now,
            &mut ant,
            &mut ble,
            &position,
            cycling_os::capabilities::Network::online(&network),
            &input,
            cycling_os::capabilities::Sensors::snapshot(&sensors, now),
        );
        #[cfg(feature = "cycling")]
        sdk.test_display(&mut system, now, &ant, &position);
        terminal.pump();
        terminal.finish_reboot(now, &FirmwareDiagnostics);
        if let Some(request) = terminal.request(now) {
            terminal::execute(
                &mut terminal,
                request,
                &mut system,
                now,
                &mut ant,
                &mut ble,
                &mut position,
                &mut bulk,
                &mut sound,
                &mut sensors,
                &mut network,
                &input,
                &FirmwareDiagnostics,
                #[cfg(feature = "cycling")]
                &mut sdk,
            );
        }
        if now >= next_health {
            next_health = now + 5_000;
            let position = device::services::positioning::snapshot(now);
            let inputs = device::services::io::snapshot(now);
            log::info!(target:"progress","gps_valid={} gps_dma={} gps_uart={} companion={} input_uart={}",position.map(|s|s.gps.valid_sentences).unwrap_or(0),position.map(|s|s.gps.overflows).unwrap_or(0),position.map(|s|s.gps.uart_errors).unwrap_or(0),inputs.map(|s|s.companion_valid).unwrap_or(0),inputs.map(|s|s.uart_errors).unwrap_or(0));
            log::info!(target:"health","uptime_ms={} heap_free={} heap_min_sampled={} terminal_sent={}",now,esp_alloc::HEAP.free(),system.heap_min_sampled,terminal.sent);
        }
        embassy_time::Timer::after_millis(5).await;
    }
}

struct FirmwareDiagnostics;
impl terminal::Diagnostics for FirmwareDiagnostics {
    fn board(&self) -> &'static str {
        "c606"
    }
    fn heap_free(&self) -> usize {
        esp_alloc::HEAP.free()
    }
    fn external_free(&self) -> usize {
        device::psram::external_free()
    }
    fn restart(&self, panic: bool) -> ! {
        #[cfg(feature = "debug-harness")]
        if panic {
            device::crash_rtc::controlled_panic();
        }
        let _ = panic;
        esp_hal::system::software_reset()
    }
    fn set_fault(&self, fault: u8) {
        #[cfg(feature = "debug-harness")]
        device::wifi::set_fault(fault);
        let _ = fault;
    }
    fn stall(&self) {
        esp_hal::delay::Delay::new().delay_millis(6000);
    }
}
