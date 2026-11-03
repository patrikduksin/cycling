#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]
#![no_main]
use c606_firmware as device;
use device::activity::{Stage, stage_ended};
mod bringup;
mod commands;
mod logging;
#[cfg(feature = "cycling")]
mod trace;
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

#[cfg(not(feature = "cycling"))]
type FirmwareShell = firmware_shell::shell::Shell<
    device::drivers::display::Display<'static>,
    device::board::Input,
    device::board::Power,
    device::drivers::storage::Backend<'static>,
>;

#[cfg(feature = "cycling")]
type FirmwareShell = firmware_shell::shell::Shell<
    firmware_services::trace::shell::ShellDisplay<device::drivers::display::Display<'static>>,
    ShellInput,
    firmware_services::trace::shell::ShellPower<device::board::Power>,
    firmware_services::trace::shell::ShellFlash<device::drivers::storage::Backend<'static>>,
>;

/// The edges the shell takes, kept for the trace until the pass records
/// them, in PSRAM: the shell lives in the console task's future, where 16
/// edges would take main stack. Base builds record no trace.
#[cfg(feature = "cycling")]
struct ShellInput(
    &'static mut firmware_services::trace::frozen::RecordedEdges<device::board::Input>,
);

#[cfg(feature = "cycling")]
impl device_api::input::InputSource for ShellInput {
    fn controls(&self) -> device_api::input::Controls {
        self.0.controls()
    }
    fn take_edge(&mut self) -> Option<device_api::input::Edge> {
        self.0.take_edge()
    }
}

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    use device::activity::Task;
    device::stack::paint();
    device::activity::name_current(Task::Main).await;
    let board = device::board::start(spawner).await;
    let boot = esp_hal::rng::Rng::new().random();
    logging::init(boot);
    logging::metadata(false);
    device::activity::calibrate();
    #[cfg(feature = "cycling")]
    let pass_log: &'static firmware_services::trace::shell::PassLog = external(Default::default);
    #[cfg(feature = "cycling")]
    let mut system = {
        use firmware_services::trace::shell::{ShellDisplay, ShellFlash, ShellPower};
        let input = ShellInput(external(|| {
            firmware_services::trace::frozen::RecordedEdges::new(board.input)
        }));
        firmware_shell::shell::Shell::new(
            ShellDisplay::new(board.display, pass_log),
            input,
            ShellPower::new(board.power, pass_log),
            ShellFlash::new(board.storage, pass_log),
            embassy_time::Instant::now().as_millis(),
        )
        .unwrap()
    };
    #[cfg(not(feature = "cycling"))]
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
        device::activity::spawn(
            spawner,
            Task::Maintenance,
            device::drivers::bulk_maintenance::run(board.terminal, board.bulk).unwrap(),
        );
    }
    #[cfg(not(feature = "bulk-maintenance"))]
    device::activity::spawn(
        spawner,
        Task::Console,
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
            #[cfg(feature = "cycling")]
            pass_log,
        )
        .unwrap(),
    );
    core::future::pending().await
}
/// A block of PSRAM for `layout`, zeroed in chunks with a yield between
/// them, as for the harness capture, so the companion UART is never starved.
/// Null without PSRAM or for an empty layout; the block is never freed.
#[cfg(feature = "cycling")]
async fn zeroed_external(layout: core::alloc::Layout) -> *mut u8 {
    if layout.size() == 0 {
        return core::ptr::null_mut();
    }
    // SAFETY: the layout has a non-zero size.
    let pointer = unsafe { device::drivers::psram::allocate_external(layout) };
    if pointer.is_null() {
        return pointer;
    }
    for offset in (0..layout.size()).step_by(4096) {
        let len = (layout.size() - offset).min(4096);
        // SAFETY: the allocator returned `layout.size()` writable bytes at
        // `pointer`; `offset + len` stays within them.
        unsafe { core::ptr::write_bytes(pointer.add(offset), 0, len) };
        embassy_futures::yield_now().await;
    }
    pointer
}

/// Back to start's breadcrumb (48 KiB), the loaded course (16 KiB), the
/// course in the ride's frame (24 KiB), its turns (16 KiB), what finds them
/// while a course uploads (12 KiB) and their state live in PSRAM: main's
/// stack shares DRAM with `Runtime`'s static future and cannot spare them.
/// Without PSRAM the back-to-start page says NO TRACK and no course loads;
/// false then.
#[cfg(feature = "cycling")]
async fn attach_back(sdk: &mut vana::app::Runtime) -> bool {
    use vana::app::{
        BACK_CRUMBS, BackToStart, COURSE_POINTS, COURSE_TURNS, Course, CourseFinder, Crumb,
        MapFrame, Turn,
    };
    let Ok(layout) = core::alloc::Layout::array::<Crumb>(BACK_CRUMBS) else {
        return false;
    };
    let pointer = zeroed_external(layout).await;
    if pointer.is_null() {
        log::warn!("back to start: no PSRAM for the breadcrumb");
        return false;
    }
    // SAFETY: the block holds BACK_CRUMBS crumbs at the allocator's
    // alignment for `Crumb`, all zero bytes, which is a valid `Crumb`. It is
    // never deallocated and this is its only reference.
    let crumbs = unsafe { core::slice::from_raw_parts_mut(pointer.cast::<Crumb>(), BACK_CRUMBS) };
    let Ok(layout) = core::alloc::Layout::array::<Crumb>(COURSE_POINTS) else {
        return false;
    };
    let line = zeroed_external(layout).await;
    if line.is_null() {
        log::warn!("back to start: no PSRAM for the course's line");
        return false;
    }
    // SAFETY: as for the breadcrumb, with COURSE_POINTS crumbs.
    let line = unsafe { core::slice::from_raw_parts_mut(line.cast::<Crumb>(), COURSE_POINTS) };
    let Ok(layout) = core::alloc::Layout::array::<Turn>(COURSE_TURNS) else {
        return false;
    };
    let turns = zeroed_external(layout).await;
    if turns.is_null() {
        log::warn!("back to start: no PSRAM for the course's turns");
        return false;
    }
    // SAFETY: as for the breadcrumb, with COURSE_TURNS turns; all zero bytes
    // are a valid `Turn`.
    let turns = unsafe { core::slice::from_raw_parts_mut(turns.cast::<Turn>(), COURSE_TURNS) };
    // The map's track is zero bytes to begin with too: nothing shown yet.
    let map = zeroed_external(core::alloc::Layout::new::<MapFrame>()).await;
    if map.is_null() {
        log::warn!("back to start: no PSRAM for the map");
        return false;
    }
    // SAFETY: the block holds a `MapFrame` at its alignment, all zero bytes,
    // which is a valid `MapFrame`. It is never deallocated and this is its
    // only reference.
    let map = unsafe { &mut *map.cast::<MapFrame>() };
    let course = zeroed_external(core::alloc::Layout::new::<Course>()).await;
    if course.is_null() {
        log::warn!("back to start: no PSRAM for the course");
        return false;
    }
    // SAFETY: the block holds a `Course` at its alignment, all zero bytes,
    // which is an empty `Course`. It is never deallocated and this is its
    // only reference.
    let course = unsafe { &mut *course.cast::<Course>() };
    let finder = zeroed_external(core::alloc::Layout::new::<CourseFinder>()).await;
    if finder.is_null() {
        log::warn!("back to start: no PSRAM for the course's turn finder");
        return false;
    }
    // SAFETY: the block holds a `CourseFinder` at its alignment, all zero
    // bytes, which is a valid `CourseFinder`. It is never deallocated and
    // this is its only reference.
    let finder = unsafe { &mut *finder.cast::<CourseFinder>() };
    let state = core::alloc::Layout::new::<BackToStart>();
    // SAFETY: `BackToStart` has a non-zero size; the block is never freed.
    let back = unsafe { device::drivers::psram::allocate_external(state) }.cast::<BackToStart>();
    if back.is_null() {
        log::warn!("back to start: no PSRAM for its state");
        return false;
    }
    // SAFETY: the allocator returned a block with `BackToStart`'s size and
    // alignment. It is initialized by this write, never deallocated, and
    // this is its only reference.
    let back = unsafe {
        back.write(BackToStart::new(crumbs, line, turns, map, course, finder));
        &mut *back
    };
    sdk.attach_back(back);
    true
}

#[embassy_executor::task]
async fn console(
    usb: device::drivers::usb::Usb,
    mut system: FirmwareShell,
    mut devices: commands::Devices,
    #[cfg(feature = "cycling")] profile: vana::sensors::ble_profile::Profile,
    #[cfg(feature = "cycling")] pass_log: &'static firmware_services::trace::shell::PassLog,
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
        // Each saved ride's facts, 32 KB in PSRAM.
        sdk.attach_ride_facts(external_bytes(vana::ride::facts::SHELF_BYTES));
        // The rides the rider discarded or deleted, 8 KB in PSRAM.
        sdk.attach_deleted_rides(external_bytes(vana::ride::deleted::SHELF_BYTES));
    }
    #[cfg(feature = "cycling")]
    let mut memory = firmware_services::trace::origin::Memory {
        ride_index: true,
        ride_facts: true,
        ..Default::default()
    };
    #[cfg(feature = "cycling")]
    {
        sdk.load_preferences(&system.settings.app);
        sdk.set_latency_clock(|| embassy_time::Instant::now().as_millis());
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
                memory.terrain = true;
            }
        }
        memory.back_to_start = attach_back(sdk).await;
        // The phone session's indexes and response state live in external
        // RAM, copied from a constant, so they cost no task stack.
        const PHONE_WORK: vana::app::PhoneWork = vana::app::PhoneWork::new();
        if let Some(work) = device::drivers::psram::place(PHONE_WORK) {
            sdk.attach_phone(work);
            memory.phone = true;
        }
        sdk.draw_boot(&mut system);
    }
    // This session's trace, staged in PSRAM from the first pass; it takes
    // what PSRAM VANA left.
    #[cfg(feature = "cycling")]
    let trace = external(|| trace::Trace::start(system.boot_id, system.settings, memory, pass_log));
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
    system.forget_frame();
    loop {
        device::activity::pass_began();
        device::capabilities::io::pass_began();
        let now = embassy_time::Instant::now().as_millis();
        system.heap_min_sampled = system.heap_min_sampled.min(esp_alloc::HEAP.free());
        bringup.poll(now, &mut devices.network, &mut devices.ble);
        stage_ended(Stage::Bringup);
        #[cfg(feature = "cycling")]
        {
            let quiet = vana_pass(
                sdk,
                trace,
                &mut system,
                &mut devices,
                now,
                &mut terrain_step_max_us,
            );
            trace.vitals(&devices, system.heap_min_sampled, now, quiet);
            stage_ended(Stage::Vitals);
        }
        #[cfg(not(feature = "cycling"))]
        let power = device_api::power::PowerControl::status(&devices.power);
        // Diagnostic power requests enter only with no active domain work.
        // Keep USB observations alive while device owners complete the transition.
        #[cfg(not(feature = "cycling"))]
        if power.ready {
            system.tick(now);
            system.observe_position(&devices.position, now);
            let started = embassy_time::Instant::now();
            system.present();
            system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
        }
        terminal.pump(logging::take, || embassy_time::Instant::now().as_micros());
        if terminal.transfer_due() {
            let handing = embassy_time::Instant::now();
            // The trace is whole on the card before the host can read it.
            #[cfg(feature = "cycling")]
            trace.before_host(&mut devices.bulk, now);
            // The ACCEPTED reply went out; the host now speaks binary frames.
            let handover_us = handing.elapsed().as_micros();
            commands::transfer(terminal.transport(), &mut devices.bulk, handover_us).await;
            terminal.end_transfer();
            device::activity::pass_began();
            #[cfg(feature = "cycling")]
            trace.after_host(
                &mut devices.bulk,
                true,
                embassy_time::Instant::now().as_millis(),
            );
        }
        if let Some(panic) = terminal.reboot_due(now) {
            #[cfg(feature = "cycling")]
            trace.sync(&mut devices.bulk, now);
            commands::Diagnostics::restart(&FirmwareDiagnostics, panic);
        }
        let request = terminal.request(now);
        stage_ended(Stage::Terminal);
        if let Some(request) = request {
            // The trace holds the line and the power status the command's
            // gate reads, as the simulator records them.
            #[cfg(feature = "cycling")]
            {
                trace.command(terminal.line());
                trace.power.freeze(&devices.power, now, &trace.record);
            }
            commands::execute(
                &mut terminal,
                request,
                &mut system,
                now,
                &mut devices,
                &FirmwareDiagnostics,
                #[cfg(feature = "cycling")]
                sdk,
                #[cfg(feature = "cycling")]
                trace,
            );
            #[cfg(feature = "cycling")]
            trace.log.record(&trace.record);
            stage_ended(Stage::Command);
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
            stage_ended(Stage::Log);
        }
        device::activity::pass_ended();
        #[cfg(feature = "cycling")]
        let rest_ms = sdk.pass_rest_ms(
            &system,
            trace.power.frozen().state,
            terminal.take_traffic(),
            embassy_time::Instant::now().as_millis(),
        );
        #[cfg(not(feature = "cycling"))]
        let rest_ms = 5;
        device::capabilities::io::rest(rest_ms).await;
    }
}

/// One pass of VANA and the shell, with every handle VANA reads frozen for
/// the pass and recorded into the session's trace in the order the
/// simulator records them (`tools/simulator/src/session.rs`), then this
/// pass's share of the trace to the card. Inlined, as the pass was before
/// it had a name: a frame of its own would sit under every frame VANA
/// prepares. True for a pass in dark charging standby.
#[cfg(feature = "cycling")]
#[inline(always)]
fn vana_pass(
    sdk: &mut vana::app::Runtime,
    trace: &mut trace::Trace,
    system: &mut FirmwareShell,
    devices: &mut commands::Devices,
    now: u64,
    terrain_step_max_us: &mut u64,
) -> bool {
    use device_api::power::{Power as _, State};
    use device_api::sensors::Sensors as _;
    use firmware_services::trace::Event;
    use firmware_services::trace::frozen::RecordedBulk;
    // In dark charging standby a pass reads only the startup and power
    // status, and only a change records it, so hours of standby fit the
    // spool; the simulator runs the same quiet pass.
    let quiet = trace.power.frozen().state == State::Charging && sdk.standby_dark();
    if quiet {
        trace.quiet_pass(now);
    } else {
        trace.record(&Event::Pass { now_ms: now });
    }
    let record = &trace.record;
    let companion = (!quiet).then(|| trace.companion.freeze(&devices.input, now, record));
    let startup = devices.sensors.startup_status();
    let startup = trace.startup.observe(startup, now, record, Event::Startup);
    let power = trace.power.freeze(&devices.power, now, record);
    if quiet && power.state == State::Charging {
        stage_ended(Stage::Inputs);
        return quiet;
    }
    let companion =
        companion.unwrap_or_else(|| trace.companion.freeze(&devices.input, now, record));
    let backlight = device::board::Power.availability();
    trace.log.begin_pass(backlight, &companion);
    stage_ended(Stage::Inputs);
    // While the companion starts, VANA scans its ride journal so Home
    // opens with storage ready. Charging standby and transitions skip it.
    if power.state == State::Initializing {
        let mut bulk = RecordedBulk::counted(&mut devices.bulk, record, trace.log);
        sdk.prepare(&mut system.data_storage(), &mut bulk, now);
    }
    // On USB power with VANA off, VANA shows the charge for a while,
    // then the panel goes dark; holding top left starts VANA as usual.
    if power.state == State::Charging {
        let lit = sdk.standby(system, &companion, now);
        device::capabilities::power::standby_light(if lit {
            system.settings.brightness
        } else {
            0
        });
    }
    stage_ended(Stage::Standby);
    // Diagnostic power requests enter only with no active domain work.
    // Keep USB observations alive while device owners complete the transition.
    if power.ready {
        trace
            .backlight
            .observe(backlight, now, record, Event::Backlight);
        let position = trace.position.freeze(&devices.position, now, record);
        let physical = devices.sensors.snapshot(now);
        let physical = trace
            .physical
            .observe(physical, now, record, Event::Physical);
        let online = device_api::network::Network::online(&devices.network);
        let network_time = firmware_services::network_time::snapshot(now, 0, online);
        let network_time =
            trace
                .network_time
                .observe(network_time, now, record, Event::NetworkTime);
        let token = sdk.phone_status().map(|status| status.token);
        trace
            .server
            .redact(token.as_ref().map(|token| token.bytes()));
        let mut ant = trace.ant.bind(&mut devices.ant, now, record);
        let mut ble = trace.ble.bind(&mut devices.ble, now, record);
        let mut bulk = RecordedBulk::counted(&mut devices.bulk, record, trace.log);
        let mut access_point = trace
            .access_point
            .bind(&mut devices.access_point, now, record);
        let mut server = trace.server.bind(&mut devices.server, now, record);
        let mut sound = trace.sound.bind(&mut devices.sound, now, record);
        let mut control = trace.power.bind(&mut devices.power, now, record);
        stage_ended(Stage::Observe);
        sdk.set_startup_status(startup);
        system.set_app_active(sdk.input_active());
        system.tick(now);
        system.input_source().0.record(&mut *record.borrow_mut());
        #[cfg(feature = "debug-harness")]
        for input in system.take_synthetic() {
            firmware_services::trace::frozen::Record::record(
                &mut *record.borrow_mut(),
                &Event::Synthetic(input),
            );
        }
        for _ in 0..16 {
            let Some(edge) = system.take_app_input() else {
                break;
            };
            sdk.input_received(edge.input, edge.received_ms, now, &mut ant);
        }
        system.observe_position(&position, now);
        stage_ended(Stage::Shell);
        let started = embassy_time::Instant::now();
        system.present();
        system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis());
        stage_ended(Stage::ShellPresent);
        sdk.tick(
            &mut system.data_storage(),
            &mut bulk,
            now,
            &mut ant,
            &mut ble,
            vana::app::Observations {
                position: &position,
                network_time,
                input: &companion,
                physical,
            },
        );
        stage_ended(Stage::VanaTick);
        // One bounded look-ahead step a pass (a tile chunk, a match or a
        // prediction), before the frame that may show its result.
        let started = embassy_time::Instant::now();
        sdk.look_ahead(&mut bulk, now);
        *terrain_step_max_us = (*terrain_step_max_us).max(started.elapsed().as_micros());
        stage_ended(Stage::LookAhead);
        sdk.serve_phone(
            &mut server,
            &mut access_point,
            &mut system.data_storage(),
            &mut bulk,
            now,
        );
        stage_ended(Stage::Phone);
        sdk.present(system, now, &ant, &position);
        stage_ended(Stage::VanaPresent);
        sdk.save_preferences(system, now);
        sdk.alert(&mut sound, now);
        // After VANA's "TURNING OFF" frame, the device's coordinated
        // shutdown takes over; this loop then only serves the terminal.
        sdk.power(system, &mut control, now);
        stage_ended(Stage::VanaAfter);
    }
    trace.log.record(&trace.record);
    trace.seen(sdk, system.take_frame(), now);
    stage_ended(Stage::Seen);
    trace.drain(
        &mut devices.bulk,
        sdk.journals_at_boot(),
        firmware_services::trace::spool::Card::of(
            !device::capabilities::power::ACCESS.closed(),
            sdk.recording(),
            sdk.shutdown_due(),
        ),
        now,
    );
    stage_ended(Stage::Drain);
    quiet
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
