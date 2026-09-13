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
    #[cfg(feature = "cycling")]
    let (selection, profile) = configured();
    #[cfg(not(feature = "cycling"))]
    let selection = None;
    let board = device::c606::start(spawner, selection).await;
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
    spawner.spawn(
        console(
            board.terminal,
            system,
            board.ant,
            board.ble,
            board.positioning,
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
    position: device::c606::Positioning,
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
    let started = embassy_time::Instant::now();
    system.present();
    system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
    loop {
        let now = embassy_time::Instant::now().as_millis();
        system.heap_min_sampled = system.heap_min_sampled.min(esp_alloc::HEAP.free());
        system.tick(now);
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
                &position,
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

#[cfg(feature = "cycling")]
pub fn configured() -> (
    Option<cycling_os::ble_transport::Selection>,
    cycling_os::sdk::ble_sensor::Profile,
) {
    mod config {
        include!(env!("CYCLING_BLE_CONFIG"));
    }
    let profile = cycling_os::sdk::ble_sensor::Profile::from_u8(config::PROFILE);
    (
        cycling_os::sdk::ble::selection(profile, config::TARGET_NAME, config::TARGET_ADDRESS),
        profile,
    )
}
