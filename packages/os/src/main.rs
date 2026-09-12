#![no_std]
#![no_main]
mod bluetooth;
mod core_system;
mod device;
mod logging;
mod persistent;
#[cfg(feature = "cycling")]
mod sdk_runtime;
mod services;
mod terminal;
mod wifi;
use esp_backtrace as _;
esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    let board = device::c606::init();
    logging::init(esp_hal::rng::Rng::new().random());
    logging::metadata(false);
    log::info!(target:"boot","reset={} crash={}",board.reset.name(),board.crash.name());
    let system = core_system::System::new(
        board.flash,
        board.backlight,
        board.screen,
        board.reset,
        board.crash,
    );
    spawner.spawn(services::positioning::run(board.gps_receiver).unwrap());
    spawner.spawn(services::io::run(board.touch, board.touch_available).unwrap());
    spawner.spawn(wifi::start(board.wifi, spawner).unwrap());
    #[cfg(feature = "cycling")]
    let (selection, profile) = cycling_os::sdk::ble::configured();
    #[cfg(not(feature = "cycling"))]
    let selection = None;
    spawner.spawn(bluetooth::start(board.bluetooth, selection).unwrap());
    spawner.spawn(
        console(
            board.usb,
            system,
            #[cfg(feature = "cycling")]
            profile,
        )
        .unwrap(),
    );
    let _read = board._lcd_read;
    core::future::pending().await
}
#[embassy_executor::task]
async fn console(
    usb: esp_hal::usb_serial_jtag::UsbSerialJtag<'static, esp_hal::Blocking>,
    mut system: core_system::System,
    #[cfg(feature = "cycling")] profile: cycling_os::sdk::ble_sensor::Profile,
) {
    let mut terminal = terminal::Terminal::new(usb);
    let mut next_health = 0;
    #[cfg(feature = "cycling")]
    let mut sdk = sdk_runtime::Runtime::new(profile);
    system.fill(0);
    loop {
        let now = embassy_time::Instant::now().as_millis();
        system.tick(now);
        #[cfg(feature = "cycling")]
        sdk.tick(&mut system.store, now);
        terminal.pump();
        terminal.finish_reboot(now);
        if let Some(request) = terminal.request(now) {
            terminal::execute(
                &mut terminal,
                request,
                &mut system,
                now,
                #[cfg(feature = "cycling")]
                &mut sdk,
            );
        }
        if now >= next_health {
            next_health = now + 5_000;
            let position = services::positioning::snapshot(now);
            let inputs = services::io::snapshot(now);
            log::info!(target:"progress","gps_valid={} gps_dma={} gps_uart={} companion={} input_uart={}",position.map(|s|s.gps.valid_sentences).unwrap_or(0),position.map(|s|s.gps.overflows).unwrap_or(0),position.map(|s|s.gps.uart_errors).unwrap_or(0),inputs.map(|s|s.companion_valid).unwrap_or(0),inputs.map(|s|s.uart_errors).unwrap_or(0));
            log::info!(target:"health","uptime_ms={} heap_free={} heap_min_sampled={} terminal_sent={}",now,esp_alloc::HEAP.free(),system.heap_min_sampled,terminal.sent);
        }
        embassy_time::Timer::after_millis(5).await;
    }
}
