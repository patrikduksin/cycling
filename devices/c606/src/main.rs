#![no_std]
#![no_main]
use c606_firmware as device;
mod commands;
mod logging;
use esp_backtrace as _;
esp_bootloader_esp_idf::esp_app_desc!();

type FirmwareShell = firmware_shell::shell::Shell<
    device::drivers::display::Display<'static>,
    device::board::Input,
    device::board::Power,
    device::drivers::storage::Backend<'static>,
>;

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    let board = device::board::start(spawner).await;
    let boot = esp_hal::rng::Rng::new().random();
    logging::init(boot);
    logging::metadata(false);
    let mut system = firmware_shell::shell::Shell::new(
        board.display,
        board.input,
        board.power,
        board.storage,
        embassy_time::Instant::now().as_millis(),
    )
    .unwrap();
    #[cfg(feature = "debug-harness")]
    {
        system.harness = firmware_shell::harness::State::new(boot);
        system.harness.clock = Some(|| embassy_time::Instant::now().as_millis());
        // Composition reserves capture storage in external RAM. The library
        // receives only an owned byte slice; DMA still uses device-owned strips.
        let layout =
            core::alloc::Layout::from_size_align(firmware_shell::harness::CAPTURE_BYTES, 4)
                .unwrap();
        let pointer = unsafe { device::drivers::psram::allocate_external(layout) };
        if !pointer.is_null() {
            system.harness.buffer =
                Some(firmware_shell::harness::CaptureBuffer::Borrowed(unsafe {
                    core::slice::from_raw_parts_mut(pointer, layout.size())
                }));
        }
    }
    system.boot_id = boot;
    system.reset = board.reset;
    system.crash = board.crash;
    system.heap_min_sampled = esp_alloc::HEAP.free();
    #[cfg(feature = "cycling")]
    let profile = vana::sensors::ble_profile::Profile::from_u8(system.settings.ble_profile);
    #[cfg(feature = "bulk-maintenance")]
    {
        for _ in 0..300 {
            if device_api::power::Control::status(&board.power_control).ready {
                break;
            }
            embassy_time::Timer::after_millis(10).await;
        }
        system.tick(embassy_time::Instant::now().as_millis());
        system.present();
        spawner.spawn(device::drivers::bulk_maintenance::run(board.terminal, board.bulk).unwrap());
    }
    #[cfg(not(feature = "bulk-maintenance"))]
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
            board.power_control,
            #[cfg(feature = "cycling")]
            profile,
        )
        .unwrap(),
    );
    core::future::pending().await
}
#[embassy_executor::task]
async fn console(
    usb: device::drivers::usb::Usb,
    mut system: FirmwareShell,
    mut ant: device::board::Ant,
    mut ble: device::board::Ble,
    mut position: device::board::Positioning,
    mut bulk: device::board::Bulk,
    mut sound: device::board::Sound,
    mut sensors: device::board::Sensors,
    mut network: device::board::Network,
    input: device::board::Input,
    mut power: device::board::Power,
    #[cfg(feature = "cycling")] profile: vana::sensors::ble_profile::Profile,
) {
    let mut terminal = firmware_console::session::Terminal::new(usb);
    let mut next_health = 0;
    #[cfg(feature = "cycling")]
    let mut sdk = vana::app::Runtime::new(profile);
    #[cfg(feature = "cycling")]
    {
        sdk.set_clock(|| embassy_time::Instant::now().as_millis());
        let view =
            vana::screens::workout::View::new(vana::screens::workout::Page::Boot, false, None);
        system.draw_scaled(240, 320, |x, y| view.pixel(x, y));
    }
    #[cfg(feature = "cycling")]
    use device_api::ble_transport::Ble as _;
    use device_api::network::Network as _;
    if let Some(configuration) = system.settings.wifi {
        if network
            .request(device_api::connectivity::WifiOperation::Configure(Some(
                configuration,
            )))
            .is_ok()
        {
            let _ = embassy_time::with_timeout(embassy_time::Duration::from_secs(3), async {
                while network.control().operation == device_api::connectivity::Operation::Pending {
                    embassy_time::Timer::after_millis(10).await;
                }
            })
            .await;
            if network.control().operation == device_api::connectivity::Operation::Completed {
                let _ = network.request(device_api::connectivity::WifiOperation::Connect);
            }
        }
    }
    #[cfg(feature = "cycling")]
    if let Some(selection) = system.settings.ble.and_then(|saved| {
        vana::sensors::ble::selection(
            vana::sensors::ble_profile::Profile::from_u8(system.settings.ble_profile),
            saved.name.bytes(),
            saved.address,
        )
    }) {
        if ble
            .request(device_api::ble_transport::Operation::Select(Some(
                selection,
            )))
            .is_ok()
        {
            let _ = embassy_time::with_timeout(embassy_time::Duration::from_secs(3), async {
                while ble.control().operation == device_api::connectivity::Operation::Pending {
                    embassy_time::Timer::after_millis(10).await;
                }
            })
            .await;
            let _ = ble.request(device_api::ble_transport::Operation::Connect);
        }
    }
    #[cfg(feature = "cycling")]
    system.set_app_active(sdk.input_active());
    let started = embassy_time::Instant::now();
    system.present();
    system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
    loop {
        let now = embassy_time::Instant::now().as_millis();
        system.heap_min_sampled = system.heap_min_sampled.min(esp_alloc::HEAP.free());
        // Diagnostic power requests enter only with no active domain work.
        // Keep USB observations alive while device owners complete the transition.
        if device_api::power::Control::status(&power).ready {
            #[cfg(feature = "cycling")]
            {
                sdk.set_startup_status(device_api::sensors::Sensors::startup_status(&sensors));
                system.set_app_active(sdk.input_active());
            }
            system.tick(now);
            #[cfg(feature = "cycling")]
            for _ in 0..16 {
                let Some(edge) = system.take_app_input() else {
                    break;
                };
                sdk.input(edge.input, now, &mut ant);
            }
            system.observe_position(&position, now);
            let started = embassy_time::Instant::now();
            system.present();
            system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
            #[cfg(feature = "cycling")]
            sdk.tick(
                &mut system.data_storage(),
                now,
                &mut ant,
                &mut ble,
                &position,
                device_api::network::Network::online(&network),
                &input,
                device_api::sensors::Sensors::snapshot(&sensors, now),
            );
            #[cfg(feature = "cycling")]
            sdk.present(&mut system, now, &ant, &position);
            #[cfg(feature = "cycling")]
            sdk.alert(&mut sound, now);
        }
        terminal.pump(logging::take, || embassy_time::Instant::now().as_micros());
        if let Some(panic) = terminal.reboot_due(now) {
            commands::Diagnostics::restart(&FirmwareDiagnostics, panic);
        }
        if let Some(request) = terminal.request(now) {
            commands::execute(
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
                &mut power,
                &FirmwareDiagnostics,
                #[cfg(feature = "cycling")]
                &mut sdk,
            );
        }
        if now >= next_health {
            next_health = now + 5_000;
            let position = device::capabilities::positioning::snapshot(now);
            let inputs = device::capabilities::io::snapshot(now);
            log::info!(target:"progress","gps_valid={} gps_dma={} gps_uart={} companion={} input_uart={}",position.map(|s|s.gps.valid_sentences).unwrap_or(0),position.map(|s|s.gps.overflows).unwrap_or(0),position.map(|s|s.gps.uart_errors).unwrap_or(0),inputs.map(|s|s.companion_valid).unwrap_or(0),inputs.map(|s|s.uart_errors).unwrap_or(0));
            log::info!(target:"health","uptime_ms={} heap_free={} heap_min_sampled={} terminal_sent={}",now,esp_alloc::HEAP.free(),system.heap_min_sampled,terminal.sent());
        }
        embassy_time::Timer::after_millis(5).await;
    }
}

struct FirmwareDiagnostics;
impl commands::Diagnostics for FirmwareDiagnostics {
    fn board(&self) -> &'static str {
        "c606"
    }
    fn heap_free(&self) -> usize {
        esp_alloc::HEAP.free()
    }
    fn external_free(&self) -> usize {
        device::drivers::psram::external_free()
    }
    fn restart(&self, panic: bool) -> ! {
        #[cfg(feature = "debug-harness")]
        if panic {
            device::drivers::crash_rtc::controlled_panic();
        }
        let _ = panic;
        esp_hal::system::software_reset()
    }
    fn set_fault(&self, fault: u8) {
        #[cfg(feature = "debug-harness")]
        device::drivers::wifi::set_fault(fault);
        let _ = fault;
    }
    fn stall(&self) {
        esp_hal::delay::Delay::new().delay_millis(6000);
    }
}
