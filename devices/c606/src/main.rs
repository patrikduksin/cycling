#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]
#![no_main]
use c606_firmware as device;
mod bringup;
mod commands;
mod logging;
use esp_backtrace as _;
#[cfg(not(feature = "bulk-maintenance"))]
esp_bootloader_esp_idf::esp_app_desc!();
// The descriptor version marks the binary maintenance protocol, so host tools
// pick the right readiness probe from the stored image alone.
#[cfg(feature = "bulk-maintenance")]
esp_bootloader_esp_idf::esp_app_desc!(
    concat!(env!("CARGO_PKG_VERSION"), "+maintenance"),
    env!("CARGO_PKG_NAME"),
    esp_bootloader_esp_idf::BUILD_TIME,
    esp_bootloader_esp_idf::BUILD_DATE,
    esp_bootloader_esp_idf::ESP_IDF_COMPATIBLE_VERSION,
    esp_bootloader_esp_idf::MMU_PAGE_SIZE,
    0,
    u16::MAX,
    esp_bootloader_esp_idf::SECURE_VERSION
);

type FirmwareShell = firmware_shell::shell::Shell<
    device::drivers::display::Display<'static>,
    device::board::Input,
    device::board::Power,
    device::drivers::storage::Backend<'static>,
>;

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    device::stack::paint();
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
    system.clock = Some(|| embassy_time::Instant::now().as_millis());
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
                server: board.server,
                access_point: board.access_point,
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
/// Back to start's breadcrumb (48 KiB) and its state live in PSRAM: main's
/// stack shares DRAM with `Runtime`'s static future and cannot spare them.
/// Without PSRAM the back-to-start page says NO TRACK.
#[cfg(feature = "cycling")]
async fn attach_back(sdk: &mut vana::app::Runtime) {
    use vana::app::{BACK_CRUMBS, BackToStart, Crumb};
    let Ok(layout) = core::alloc::Layout::array::<Crumb>(BACK_CRUMBS) else {
        return;
    };
    // SAFETY: the layout has a non-zero size; the block is never freed.
    let pointer = unsafe { device::drivers::psram::allocate_external(layout) };
    if pointer.is_null() {
        log::warn!("back to start: no PSRAM for the breadcrumb");
        return;
    }
    // Zero in chunks and yield between them, as for the harness capture, so
    // the companion UART is never starved.
    for offset in (0..layout.size()).step_by(4096) {
        let len = (layout.size() - offset).min(4096);
        // SAFETY: the allocator returned `layout.size()` writable bytes at
        // `pointer`; `offset + len` stays within them.
        unsafe { core::ptr::write_bytes(pointer.add(offset), 0, len) };
        embassy_futures::yield_now().await;
    }
    // SAFETY: the block holds BACK_CRUMBS crumbs at the allocator's
    // alignment for `Crumb`, all zero bytes, which is a valid `Crumb`. It is
    // never deallocated and this is its only reference.
    let crumbs = unsafe { core::slice::from_raw_parts_mut(pointer.cast::<Crumb>(), BACK_CRUMBS) };
    let state = core::alloc::Layout::new::<BackToStart>();
    // SAFETY: `BackToStart` has a non-zero size; the block is never freed.
    let back = unsafe { device::drivers::psram::allocate_external(state) }.cast::<BackToStart>();
    if back.is_null() {
        log::warn!("back to start: no PSRAM for its state");
        return;
    }
    // SAFETY: the allocator returned a block with `BackToStart`'s size and
    // alignment. It is initialized by this write, never deallocated, and
    // this is its only reference.
    let back = unsafe {
        back.write(BackToStart::new(crumbs));
        &mut *back
    };
    sdk.attach_back(back);
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
    let sdk = external(|| vana::app::Runtime::new(profile));
    #[cfg(feature = "cycling")]
    {
        // Every saved ride of both journals, for ride history: about 20 KB
        // per journal, in PSRAM; VANA keeps only the pointer.
        use vana::ride::directory::{ENTRY, RIDES, RideIndex};
        let index = external(|| {
            RideIndex::new(external_bytes(RIDES * ENTRY), external_bytes(RIDES * ENTRY))
        });
        sdk.attach_ride_index(index);
    }
    #[cfg(feature = "cycling")]
    {
        sdk.load_preferences(&system.settings.app);
        sdk.set_clock(|| embassy_time::Instant::now().as_millis());
        sdk.draw_boot(&mut system);
        // The terrain look-ahead's tile cache lives in PSRAM. Without it
        // the climb page says "NO MAP" and keeps to the barometer.
        let layout =
            core::alloc::Layout::from_size_align(vana::app::TERRAIN_ARENA_BYTES, 4).unwrap();
        // SAFETY: the layout has a non-zero size; the block is never freed.
        let pointer = unsafe { device::drivers::psram::allocate_external(layout) };
        if pointer.is_null() {
            log::warn!("terrain: no PSRAM for the tile cache");
        } else {
            // Zero in chunks and yield between them, as for the harness
            // capture, so the companion UART is never starved.
            for offset in (0..layout.size()).step_by(4096) {
                let len = (layout.size() - offset).min(4096);
                // SAFETY: the allocator returned `layout.size()` writable
                // bytes at `pointer`; `offset + len` stays within them.
                unsafe { core::ptr::write_bytes(pointer.add(offset), 0, len) };
                embassy_futures::yield_now().await;
            }
            // SAFETY: every byte was initialized above. The block is never
            // deallocated and this is its only reference.
            let arena = unsafe { core::slice::from_raw_parts_mut(pointer, layout.size()) };
            // The look-ahead's state (about 7 KB) gets a block of its own,
            // like `Runtime`, but a missing one only means "NO MAP" (unlike
            // `external`, which panics).
            let state = core::alloc::Layout::new::<vana::app::Terrain>();
            // SAFETY: `Terrain` has a non-zero size; the block is never freed.
            let terrain = unsafe { device::drivers::psram::allocate_external(state) }
                .cast::<vana::app::Terrain>();
            if terrain.is_null() {
                log::warn!("terrain: no PSRAM for the look-ahead");
            } else {
                // SAFETY: the allocator returned a block with `Terrain`'s size
                // and alignment. It is initialized by this write, never
                // deallocated, and this is its only reference.
                let terrain = unsafe {
                    terrain.write(vana::app::Terrain::new(arena));
                    &mut *terrain
                };
                sdk.attach_terrain(terrain);
            }
        }
        attach_back(sdk).await;
        // The phone session's indexes and response state live in external
        // RAM, copied from a constant, so they cost no task stack.
        const PHONE_WORK: vana::app::PhoneWork = vana::app::PhoneWork::new();
        if let Some(work) = device::drivers::psram::place(PHONE_WORK) {
            sdk.attach_phone(work);
        }
        sdk.draw_boot(&mut system);
    }
    #[cfg(feature = "cycling")]
    let mut terrain_step_max_us = 0u64;
    // Only VANA selects a saved BLE peer at startup.
    #[cfg(feature = "cycling")]
    let ble_selection = system.settings.ble.and_then(|saved| {
        vana::sensors::ble::selection(
            vana::sensors::ble_profile::Profile::from_u8(system.settings.ble_profile),
            saved.name.bytes(),
            saved.address,
        )
    });
    #[cfg(not(feature = "cycling"))]
    let ble_selection = None;
    let mut bringup = bringup::Bringup::start(
        embassy_time::Instant::now().as_millis(),
        system.settings.wifi,
        &mut devices.network,
        ble_selection,
        &mut devices.ble,
    );
    #[cfg(feature = "cycling")]
    system.set_app_active(sdk.input_active());
    let started = embassy_time::Instant::now();
    system.present();
    system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
    loop {
        let now = embassy_time::Instant::now().as_millis();
        system.heap_min_sampled = system.heap_min_sampled.min(esp_alloc::HEAP.free());
        bringup.poll(now, &mut devices.network, &mut devices.ble);
        let power = device_api::power::PowerControl::status(&devices.power);
        // While the companion starts, VANA scans its ride journal so Home
        // opens with storage ready. Charging standby and transitions skip it.
        #[cfg(feature = "cycling")]
        if power.state == device_api::power::State::Initializing {
            sdk.prepare(&mut system.data_storage(), &mut devices.bulk, now);
        }
        // On USB power with VANA off, VANA shows the charge for a while,
        // then the panel goes dark; holding top left starts VANA as usual.
        #[cfg(feature = "cycling")]
        if power.state == device_api::power::State::Charging {
            let lit = sdk.standby(&mut system, &devices.input, now);
            device::capabilities::power::standby_light(if lit {
                system.settings.brightness
            } else {
                0
            });
        }
        // Diagnostic power requests enter only with no active domain work.
        // Keep USB observations alive while device owners complete the transition.
        if power.ready {
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
                sdk.input_received(edge.input, edge.received_ms, now, &mut devices.ant);
            }
            system.observe_position(&devices.position, now);
            let started = embassy_time::Instant::now();
            system.present();
            system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
            #[cfg(feature = "cycling")]
            sdk.tick(
                &mut system.data_storage(),
                &mut devices.bulk,
                now,
                &mut devices.ant,
                &mut devices.ble,
                vana::app::Observations {
                    position: &devices.position,
                    network_online: device_api::network::Network::online(&devices.network),
                    input: &devices.input,
                    physical: device_api::sensors::Sensors::snapshot(&devices.sensors, now),
                },
            );
            // One bounded look-ahead step a pass (a tile chunk, a match or a
            // prediction), before the frame that may show its result.
            #[cfg(feature = "cycling")]
            {
                let started = embassy_time::Instant::now();
                sdk.look_ahead(&mut devices.bulk, now);
                terrain_step_max_us = terrain_step_max_us.max(started.elapsed().as_micros());
            }
            #[cfg(feature = "cycling")]
            sdk.serve_phone(
                &mut devices.server,
                &mut devices.access_point,
                &mut system.data_storage(),
                &mut devices.bulk,
                now,
            );
            #[cfg(feature = "cycling")]
            sdk.present(&mut system, now, &devices.ant, &devices.position);
            #[cfg(feature = "cycling")]
            sdk.save_preferences(&mut system, now);
            #[cfg(feature = "cycling")]
            sdk.alert(&mut devices.sound, now);
            // After VANA's "TURNING OFF" frame, the device's coordinated
            // shutdown takes over; this loop then only serves the terminal.
            #[cfg(feature = "cycling")]
            sdk.power(&mut system, &mut devices.power, now);
        }
        terminal.pump(logging::take, || embassy_time::Instant::now().as_micros());
        if terminal.transfer_due() {
            // The ACCEPTED reply went out; the host now speaks binary frames.
            commands::transfer(terminal.transport(), &mut devices.bulk).await;
            terminal.end_transfer();
        }
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
                sdk,
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
            #[cfg(feature = "cycling")]
            {
                log::info!(target: "terrain",
                    "status={:?} step_max_us={}",
                    sdk.terrain_status(),
                    terrain_step_max_us
                );
                terrain_step_max_us = 0;
            }
        }
        embassy_time::Timer::after_millis(5).await;
    }
}

/// Moves long-lived state into PSRAM so it stays out of the console task
/// future. Task futures are internal DRAM statics, and every byte there comes
/// out of the main stack (#167). Not inlined, so the value is built in this
/// frame, which is gone before the task's loop, rather than in the console
/// task's poll frame, which stays under every frame VANA prepares.
#[cfg(feature = "cycling")]
#[inline(never)]
fn external<T>(make: impl FnOnce() -> T) -> &'static mut T {
    let value = make();
    let layout = core::alloc::Layout::new::<T>();
    // SAFETY: `T` is a sized, non-zero-size state type, so the layout is
    // non-zero; the block is never freed.
    let pointer = unsafe { device::drivers::psram::allocate_external(layout) }.cast::<T>();
    assert!(!pointer.is_null(), "PSRAM exhausted");
    // SAFETY: the allocator returned a block valid and aligned for `T`. It is
    // written once here and never deallocated, and the returned reference is
    // its only one, so a 'static exclusive reference is sound.
    unsafe {
        pointer.write(value);
        &mut *pointer
    }
}

/// `len` zeroed bytes of PSRAM, never freed.
#[cfg(feature = "cycling")]
fn external_bytes(len: usize) -> &'static mut [u8] {
    let layout = core::alloc::Layout::from_size_align(len, 4).expect("byte layout");
    // SAFETY: callers pass a non-zero length; the block is never freed.
    let pointer = unsafe { device::drivers::psram::allocate_external(layout) };
    assert!(!pointer.is_null(), "PSRAM exhausted");
    // SAFETY: the allocator returned `len` writable bytes, zeroed here before
    // the only reference to them is made; they are never deallocated.
    unsafe {
        core::ptr::write_bytes(pointer, 0, len);
        core::slice::from_raw_parts_mut(pointer, len)
    }
}

struct FirmwareDiagnostics;
impl commands::Diagnostics for FirmwareDiagnostics {
    fn board(&self) -> &'static str {
        "c606"
    }
    fn slot(&self) -> &'static str {
        device::drivers::boot_slot::running()
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
