#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]
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
        // SAFETY: the layout has a non-zero size; the block is never freed.
        let pointer = unsafe { device::drivers::psram::allocate_external(layout) };
        if !pointer.is_null() {
            // Zero in chunks and yield between them: one blocking 600 KB PSRAM
            // write starves the companion UART during its startup probe.
            for offset in (0..layout.size()).step_by(4096) {
                let len = (layout.size() - offset).min(4096);
                // SAFETY: the allocator returned `layout.size()` writable bytes
                // at `pointer`; `offset + len` stays within them.
                unsafe { core::ptr::write_bytes(pointer.add(offset), 0, len) };
                embassy_futures::yield_now().await;
            }
            // SAFETY: every byte was initialized above. The block is never
            // deallocated and this is its only reference, so a 'static
            // exclusive slice is sound.
            let capture = unsafe { core::slice::from_raw_parts_mut(pointer, layout.size()) };
            system.harness.buffer = Some(firmware_shell::harness::CaptureBuffer::Borrowed(capture));
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
            if device_api::power::PowerControl::status(&board.power_control).ready {
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
            commands::Devices {
                ant: board.ant,
                ble: board.ble,
                position: board.positioning,
                bulk: board.bulk,
                sound: board.sound,
                sensors: board.sensors,
                network: board.network,
                input: board.input_observation,
                power: board.power_control,
            },
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
    mut devices: commands::Devices,
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
        if devices
            .network
            .request(device_api::connectivity::WifiOperation::Configure(Some(
                configuration,
            )))
            .is_ok()
        {
            let _ = embassy_time::with_timeout(embassy_time::Duration::from_secs(3), async {
                while devices.network.control().operation
                    == device_api::connectivity::Operation::Pending
                {
                    embassy_time::Timer::after_millis(10).await;
                }
            })
            .await;
            if devices.network.control().operation == device_api::connectivity::Operation::Completed
            {
                let _ = devices
                    .network
                    .request(device_api::connectivity::WifiOperation::Connect);
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
        if devices
            .ble
            .request(device_api::ble_transport::Operation::Select(Some(
                selection,
            )))
            .is_ok()
        {
            let _ = embassy_time::with_timeout(embassy_time::Duration::from_secs(3), async {
                while devices.ble.control().operation
                    == device_api::connectivity::Operation::Pending
                {
                    embassy_time::Timer::after_millis(10).await;
                }
            })
            .await;
            let _ = devices
                .ble
                .request(device_api::ble_transport::Operation::Connect);
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
        if device_api::power::PowerControl::status(&devices.power).ready {
            #[cfg(feature = "cycling")]
            {
                sdk.set_startup_status(device_api::sensors::Sensors::startup_status(
                    &devices.sensors,
                ));
                system.set_app_active(sdk.input_active());
            }
            system.tick(now);
            #[cfg(feature = "cycling")]
            for _ in 0..16 {
                let Some(edge) = system.take_app_input() else {
                    break;
                };
                sdk.input(edge.input, now, &mut devices.ant);
            }
            system.observe_position(&devices.position, now);
            let started = embassy_time::Instant::now();
            system.present();
            system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
            #[cfg(feature = "cycling")]
            sdk.tick(
                &mut system.data_storage(),
                now,
                &mut devices.ant,
                &mut devices.ble,
                &devices.position,
                device_api::network::Network::online(&devices.network),
                &devices.input,
                device_api::sensors::Sensors::snapshot(&devices.sensors, now),
            );
            #[cfg(feature = "cycling")]
            sdk.present(&mut system, now, &devices.ant, &devices.position);
            #[cfg(feature = "cycling")]
            sdk.alert(&mut devices.sound, now);
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
                &mut devices,
                &FirmwareDiagnostics,
                #[cfg(feature = "cycling")]
                &mut sdk,
            );
        }
        if now >= next_health {
            next_health = now + 5_000;
            let position = device::capabilities::positioning::snapshot(now);
            let inputs = device::capabilities::io::snapshot(now);
            let gps = position.map(|s| s.gps);
            log::info!(target: "progress",
                "gps_valid={} gps_dma={} gps_uart={} companion={} input_uart={}",
                gps.map_or(0, |gps| gps.valid_sentences),
                gps.map_or(0, |gps| gps.overflows),
                gps.map_or(0, |gps| gps.uart_errors),
                inputs.map_or(0, |s| s.companion_valid),
                inputs.map_or(0, |s| s.uart_errors)
            );
            log::info!(target: "health",
                "uptime_ms={} heap_free={} heap_min_sampled={} terminal_sent={}",
                now,
                esp_alloc::HEAP.free(),
                system.heap_min_sampled,
                terminal.sent()
            );
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
