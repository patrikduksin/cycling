#![no_std]
#![deny(clippy::undocumented_unsafe_blocks)]
#![no_main]
// The maintenance image runs only its binary protocol; the console it
// replaces is still compiled.
#![cfg_attr(feature = "bulk-maintenance", allow(dead_code, unused_variables))]
use c606_firmware as device;
use device::activity::{Stage, stage_ended};
mod bringup;
mod commands;
mod logging;
mod trace;
mod world;
use esp_backtrace as _;
use firmware_console::session::Reboot;
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
    device_api::display::DisplayRecorder<
        device::drivers::display::Display<'static>,
        trace::Journal,
        trace::Kept<device_api::display::DisplayObserved>,
    >,
    device_api::input::InputSourceRecorder<
        device::board::Input,
        trace::Journal,
        trace::Kept<device_api::input::InputSourceObserved>,
    >,
    device_api::power::PowerRecorder<
        device::board::Power,
        trace::Journal,
        trace::Kept<device_api::power::PowerObserved>,
    >,
    ShellFlash,
>;

type ShellFlash = device_api::storage::OwnedFlashRecorder<
    SharedFlash,
    trace::Journal,
    trace::Kept<device_api::storage::OwnedFlashObserved>,
>;

/// The settings journal on the flash itself: the log records what a load
/// and a save gave, not the sectors behind them.
type ShellSettings = firmware_shell::storage::SettingsRecorder<
    firmware_shell::storage::Store<SharedFlash>,
    trace::Journal,
    trace::Kept<firmware_shell::storage::SettingsObserved>,
>;

/// The one flash driver, shared by the shell's owned bytes and its settings
/// journal. Both run on the console task, one call at a time.
struct SharedFlash(&'static core::cell::RefCell<device::drivers::storage::Backend<'static>>);

impl device_api::storage::OwnedFlash for SharedFlash {
    type Error = device::drivers::storage::Error;
    fn availability(
        &self,
        region: device_api::storage::Region,
    ) -> device_api::observation::Availability {
        self.0.borrow().availability(region)
    }
    fn geometry(&self, region: device_api::storage::Region) -> device_api::storage::Geometry {
        self.0.borrow().geometry(region)
    }
    fn read(
        &mut self,
        region: device_api::storage::Region,
        offset: usize,
        output: &mut [u8],
    ) -> Result<(), Self::Error> {
        self.0.borrow_mut().read(region, offset, output)
    }
    fn program(
        &mut self,
        region: device_api::storage::Region,
        offset: usize,
        bytes: &[u8],
    ) -> Result<(), Self::Error> {
        self.0.borrow_mut().program(region, offset, bytes)
    }
    fn erase(
        &mut self,
        region: device_api::storage::Region,
        offset: usize,
        length: usize,
    ) -> Result<(), Self::Error> {
        self.0.borrow_mut().erase(region, offset, length)
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
    // Composition reserves the harness's capture storage in external RAM.
    // The library receives only an owned byte slice; DMA still uses
    // device-owned strips.
    let capture = zeroed_external(
        core::alloc::Layout::from_size_align(firmware_shell::harness::CAPTURE_BYTES, 4).unwrap(),
    )
    .await;
    // VANA's working memory comes before the trace, which opens by saying
    // what of it there is, and the trace before the shell, so that it
    // holds the settings the shell loads, with their secrets replaced.
    let working = working_memory().await;
    let trace = external(|| trace::Trace::start(boot, working.memory()));
    // Passed by value, the shell would sit in internal RAM three times:
    // twice in the console task's future (rust-lang/rust#62958) and once
    // in main's (#375).
    static SHELL: static_cell::StaticCell<FirmwareShell> = static_cell::StaticCell::new();
    static FLASH: static_cell::StaticCell<
        core::cell::RefCell<device::drivers::storage::Backend<'static>>,
    > = static_cell::StaticCell::new();
    static SETTINGS: static_cell::StaticCell<ShellSettings> = static_cell::StaticCell::new();
    let system = SHELL.init_with(|| {
        let (log, observed) = (trace.log, trace.observed);
        let flash = &*FLASH.init(core::cell::RefCell::new(board.storage));
        let settings = SETTINGS.init_with(|| {
            firmware_shell::storage::SettingsRecorder::new(
                firmware_shell::storage::Store::new(SharedFlash(flash)),
                log,
                &observed.settings,
            )
        });
        firmware_shell::shell::Shell::new(
            device_api::display::DisplayRecorder::new(board.display, log, &observed.display),
            device_api::input::InputSourceRecorder::new(board.input, log, &observed.input),
            device_api::power::PowerRecorder::new(board.power, log, &observed.backlight),
            device_api::storage::OwnedFlashRecorder::new(SharedFlash(flash), log, &observed.flash),
            settings,
            embassy_time::Instant::now().as_millis(),
        )
        .unwrap()
    });
    system.harness = firmware_shell::harness::State::new(boot);
    system.harness.clock = Some(|| embassy_time::Instant::now().as_millis());
    if !capture.is_null() {
        // SAFETY: `zeroed_external` returned `CAPTURE_BYTES` initialized
        // bytes. The block is never deallocated and this is its only
        // reference, so a 'static exclusive slice is sound.
        let capture = unsafe {
            core::slice::from_raw_parts_mut(capture, firmware_shell::harness::CAPTURE_BYTES)
        };
        system.harness.buffer = Some(firmware_shell::harness::CaptureBuffer::Borrowed(capture));
    }
    system.boot_id = boot;
    system.clock = Some(|| embassy_time::Instant::now().as_millis());
    system.reset = board.reset;
    system.crash = board.crash;
    system.heap_min_sampled = esp_alloc::HEAP.free();
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
            world::World::new(
                board.ant,
                board.ble,
                board.positioning,
                board.bulk,
                board.sound,
                board.sensors,
                board.server,
                board.access_point,
                board.input_observation,
                board.power_control,
            ),
            profile,
            trace,
            working,
        )
        .unwrap(),
    );
    core::future::pending().await
}
/// A block of PSRAM for `layout`, zeroed in chunks with a yield between
/// them, as for the harness capture, so the companion UART is never starved.
/// Null without PSRAM or for an empty layout; the block is never freed.
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

/// The ride's breadcrumb (48 KiB), the loaded course (16 KiB), the
/// course in the ride's frame (24 KiB), its turns (16 KiB), what finds them
/// while a course uploads (48 KiB) and their state live in PSRAM: main's
/// stack shares DRAM with `Runtime`'s static future and cannot spare them.
/// Without PSRAM the map says NO TRACK and no course loads; `None` then.
async fn ride_track() -> Option<&'static mut vana::app::RideTrack> {
    use vana::app::{
        COURSE_POINTS, COURSE_TURNS, Course, CourseFinder, Crumb, MapFrame, RideTrack,
        TRACK_CRUMBS, Turn,
    };
    let Ok(layout) = core::alloc::Layout::array::<Crumb>(TRACK_CRUMBS) else {
        return None;
    };
    let pointer = zeroed_external(layout).await;
    if pointer.is_null() {
        log::warn!("ride track: no PSRAM for the breadcrumb");
        return None;
    }
    // SAFETY: the block holds TRACK_CRUMBS crumbs at the allocator's
    // alignment for `Crumb`, all zero bytes, which is a valid `Crumb`. It is
    // never deallocated and this is its only reference.
    let crumbs = unsafe { core::slice::from_raw_parts_mut(pointer.cast::<Crumb>(), TRACK_CRUMBS) };
    let Ok(layout) = core::alloc::Layout::array::<Crumb>(COURSE_POINTS) else {
        return None;
    };
    let line = zeroed_external(layout).await;
    if line.is_null() {
        log::warn!("ride track: no PSRAM for the course's line");
        return None;
    }
    // SAFETY: as for the breadcrumb, with COURSE_POINTS crumbs.
    let line = unsafe { core::slice::from_raw_parts_mut(line.cast::<Crumb>(), COURSE_POINTS) };
    let Ok(layout) = core::alloc::Layout::array::<Turn>(COURSE_TURNS) else {
        return None;
    };
    let turns = zeroed_external(layout).await;
    if turns.is_null() {
        log::warn!("ride track: no PSRAM for the course's turns");
        return None;
    }
    // SAFETY: as for the breadcrumb, with COURSE_TURNS turns; all zero bytes
    // are a valid `Turn`.
    let turns = unsafe { core::slice::from_raw_parts_mut(turns.cast::<Turn>(), COURSE_TURNS) };
    // The map's track is zero bytes to begin with too: nothing shown yet.
    let map = zeroed_external(core::alloc::Layout::new::<MapFrame>()).await;
    if map.is_null() {
        log::warn!("ride track: no PSRAM for the map");
        return None;
    }
    // SAFETY: the block holds a `MapFrame` at its alignment, all zero bytes,
    // which is a valid `MapFrame`. It is never deallocated and this is its
    // only reference.
    let map = unsafe { &mut *map.cast::<MapFrame>() };
    let course = zeroed_external(core::alloc::Layout::new::<Course>()).await;
    if course.is_null() {
        log::warn!("ride track: no PSRAM for the course");
        return None;
    }
    // SAFETY: the block holds a `Course` at its alignment, all zero bytes,
    // which is an empty `Course`. It is never deallocated and this is its
    // only reference.
    let course = unsafe { &mut *course.cast::<Course>() };
    let finder = zeroed_external(core::alloc::Layout::new::<CourseFinder>()).await;
    if finder.is_null() {
        log::warn!("ride track: no PSRAM for the course's turn finder");
        return None;
    }
    // SAFETY: the block holds a `CourseFinder` at its alignment, all zero
    // bytes, which is a valid `CourseFinder`. It is never deallocated and
    // this is its only reference.
    let finder = unsafe { &mut *finder.cast::<CourseFinder>() };
    let state = core::alloc::Layout::new::<RideTrack>();
    // SAFETY: `RideTrack` has a non-zero size; the block is never freed.
    let track = unsafe { device::drivers::psram::allocate_external(state) }.cast::<RideTrack>();
    if track.is_null() {
        log::warn!("ride track: no PSRAM for its state");
        return None;
    }
    // SAFETY: the allocator returned a block with `RideTrack`'s size and
    // alignment. It is initialized by this write, never deallocated, and
    // this is its only reference.
    let track = unsafe {
        track.write(RideTrack::new(crumbs, line, turns, map, course, finder));
        &mut *track
    };
    Some(track)
}

/// The working memory VANA runs on, in PSRAM. Without a block VANA does
/// without its feature: the trace says which it has.
struct Working {
    /// Every saved ride of both journals, for ride history: about 20 KB
    /// per journal; VANA keeps only the pointer.
    index: Option<&'static mut vana::ride::directory::RideIndex>,
    /// Each saved ride's facts, 32 KB.
    facts: Option<&'static mut [u8]>,
    /// The rides the rider discarded or deleted, 8 KB.
    deleted: Option<&'static mut [u8]>,
    terrain: Option<&'static mut vana::app::Terrain>,
    track: Option<&'static mut vana::app::RideTrack>,
    phone: Option<&'static mut vana::app::PhoneWork>,
}

impl Working {
    fn memory(&self) -> firmware_services::trace::origin::Memory {
        firmware_services::trace::origin::Memory {
            terrain: self.terrain.is_some(),
            track: self.track.is_some(),
            phone: self.phone.is_some(),
            ride_index: self.index.is_some(),
            ride_facts: self.facts.is_some(),
        }
    }

    fn attach(&mut self, sdk: &mut vana::app::Runtime) {
        if let Some(index) = self.index.take() {
            sdk.attach_ride_index(index);
        }
        if let Some(facts) = self.facts.take() {
            sdk.attach_ride_facts(facts);
        }
        if let Some(deleted) = self.deleted.take() {
            sdk.attach_deleted_rides(deleted);
        }
        if let Some(terrain) = self.terrain.take() {
            sdk.attach_terrain(terrain);
        }
        if let Some(track) = self.track.take() {
            sdk.attach_track(track);
        }
        if let Some(work) = self.phone.take() {
            sdk.attach_phone(work);
        }
    }
}

async fn working_memory() -> &'static mut Working {
    use vana::ride::directory::{ENTRY, RIDES, RideIndex};
    let index =
        external(|| RideIndex::new(external_bytes(RIDES * ENTRY), external_bytes(RIDES * ENTRY)));
    let facts = external_bytes(vana::kept::facts::record::SHELF_BYTES);
    let deleted = external_bytes(vana::kept::deleted::SHELF_BYTES);
    let terrain = terrain().await;
    let track = ride_track().await;
    // The phone session's indexes and response state are copied from a
    // constant, so they cost no task stack.
    const PHONE_WORK: vana::app::PhoneWork = vana::app::PhoneWork::new();
    let phone = device::drivers::psram::place(PHONE_WORK);
    external(|| Working {
        index: Some(index),
        facts: Some(facts),
        deleted: Some(deleted),
        terrain,
        track,
        phone,
    })
}

/// The terrain look-ahead and its tile cache. Without them the climb page
/// says "NO MAP" and keeps to the barometer.
async fn terrain() -> Option<&'static mut vana::app::Terrain> {
    let layout = core::alloc::Layout::from_size_align(vana::app::TERRAIN_ARENA_BYTES, 4).unwrap();
    let pointer = zeroed_external(layout).await;
    if pointer.is_null() {
        log::warn!("terrain: no PSRAM for the tile cache");
        return None;
    }
    // SAFETY: `zeroed_external` returned `layout.size()` initialized bytes.
    // The block is never deallocated and this is its only reference.
    let arena = unsafe { core::slice::from_raw_parts_mut(pointer, layout.size()) };
    // Allocate the roughly 7 KB look-ahead state in PSRAM to conserve internal RAM. Allocation
    // failure disables terrain instead of aborting boot.
    let state = core::alloc::Layout::new::<vana::app::Terrain>();
    // SAFETY: `Terrain` has a non-zero size; the block is never freed.
    let terrain =
        unsafe { device::drivers::psram::allocate_external(state) }.cast::<vana::app::Terrain>();
    if terrain.is_null() {
        log::warn!("terrain: no PSRAM for the look-ahead");
        return None;
    }
    // SAFETY: the allocator returned a block with `Terrain`'s size and
    // alignment. It is initialized by this write, never deallocated, and
    // this is its only reference.
    Some(unsafe {
        terrain.write(vana::app::Terrain::new(arena));
        &mut *terrain
    })
}

const PRODUCT: vana::Product = vana::Product {
    name: device::fitted::PRODUCT_NAME,
    fit_id: device::fitted::FIT_PRODUCT,
};

static LAYOUT: vana::screens::Layout = vana::screens::Layout::of(
    vana::screens::scaled::Scaled::of(device::fitted::PANEL_WIDTH, device::fitted::PANEL_HEIGHT),
    &device::fitted::BUTTONS,
);

#[embassy_executor::task]
async fn console(
    usb: device::drivers::usb::Usb,
    system: &'static mut FirmwareShell,
    mut world: world::World,
    profile: vana::sensors::ble_profile::Profile,
    trace: &'static mut trace::Trace,
    working: &'static mut Working,
) {
    let mut terminal = firmware_console::session::Terminal::new(usb);
    let mut next_health = 0;
    let sdk = external(|| vana::app::Runtime::new(profile, PRODUCT, &LAYOUT));
    device::capabilities::ant::wake_on(vana::app::WAKING_ANT_TYPE);
    working.attach(sdk);
    sdk.load_preferences(&system.settings.app, system.settings_source);
    sdk.set_latency_clock(|| embassy_time::Instant::now().as_millis());
    sdk.set_micros_clock(|| embassy_time::Instant::now().as_micros());
    sdk.draw_boot(system);
    // Last, so scripts take only PSRAM nothing else at boot needs.
    start_scripts().await;
    let mut terrain_step_max_us = 0u64;
    let ble_selection = system.settings.ble.and_then(|saved| {
        vana::sensors::ble::selection(
            vana::sensors::ble_profile::Profile::from_u8(system.settings.ble_profile),
            saved.name.bytes(),
            saved.address,
        )
    });
    let mut bringup = bringup::Bringup::start(
        embassy_time::Instant::now().as_millis(),
        system.settings.wifi,
        ble_selection,
        world.ble_mut(),
    );
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
        bringup.poll(now, world.ble_mut());
        stage_ended(Stage::Bringup);
        {
            let quiet = traced_pass(
                sdk,
                trace,
                system,
                &mut world,
                now,
                &mut terrain_step_max_us,
            );
            trace.vitals(&world, system.heap_min_sampled, now, quiet, || {
                sdk.take_map_work()
            });
            stage_ended(Stage::Vitals);
        }
        // Diagnostic power requests enter only with no active domain work.
        // Keep USB observations alive while device owners complete the transition.
        terminal.pump(logging::take, || embassy_time::Instant::now().as_micros());
        if terminal.transfer_due() {
            let handing = embassy_time::Instant::now();
            // The trace is whole on the card before the host can read it.
            trace.before_host(world.unrecorded_bulk(), now);
            // The ACCEPTED reply went out; the host now speaks binary frames.
            let handover_us = handing.elapsed().as_micros();
            commands::transfer(terminal.transport(), world.unrecorded_bulk(), handover_us).await;
            terminal.end_transfer();
            device::activity::pass_began();
            let after = embassy_time::Instant::now().as_millis();
            if trace.after_host(world.unrecorded_bulk(), true, after) {
                trace.read_volumes(&mut world.recorded_bulk(trace.log, trace.observed));
            }
        }
        if sdk.restart_due() {
            trace.sync(world.unrecorded_bulk(), now);
            commands::Diagnostics::restart(&FirmwareDiagnostics, false);
        }
        if let Some(reboot) = terminal.reboot_due(now) {
            if reboot != Reboot::PanicUnsynced {
                trace.sync(world.unrecorded_bulk(), now);
            }
            commands::Diagnostics::restart(&FirmwareDiagnostics, reboot != Reboot::Restart);
        }
        let request = terminal.request(now);
        stage_ended(Stage::Terminal);
        if let Some(request) = request {
            // The trace holds the line and the power status the command's
            // gate reads, as the simulator records them.
            trace.command(terminal.line());
            trace.status = world.power(trace);
            commands::execute(
                &mut terminal,
                request,
                system,
                now,
                &mut world,
                &FirmwareDiagnostics,
                sdk,
                trace,
            );
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
        let rest_ms = sdk.pass_rest_ms(
            system,
            trace.status.state,
            terminal.take_traffic(),
            embassy_time::Instant::now().as_millis(),
        );
        device::capabilities::io::rest(rest_ms).await;
    }
}

/// Inlined: a frame of its own would sit under every frame VANA prepares.
/// True for a pass in dark charging standby.
#[inline(always)]
fn traced_pass(
    sdk: &mut vana::app::Runtime,
    trace: &mut trace::Trace,
    system: &mut FirmwareShell,
    world: &mut world::World,
    now: u64,
    terrain_step_max_us: &mut u64,
) -> bool {
    use device_api::power::State;
    use firmware_services::trace::Event;
    // In dark charging standby a pass reads only the startup and power
    // status, and only a change records it, so hours of standby fit the
    // spool; the simulator runs the same quiet pass.
    let quiet = trace.status.state == State::Charging && sdk.standby_dark();
    trace.begin_pass(now, quiet);
    let companion = (!quiet).then(|| world.companion(trace, now));
    let startup = world.startup(trace);
    let power = world.power(trace);
    trace.status = power;
    if quiet && power.state == State::Charging {
        stage_ended(Stage::Inputs);
        return quiet;
    }
    let companion = companion.unwrap_or_else(|| world.companion(trace, now));
    // Before VANA first reads the card, the trace stages its file there and
    // the log learns which sectors are TERRAIN and STORE.
    let admits = power.ready || power.state == State::Initializing;
    if admits && trace.waiting() && !device::capabilities::power::ACCESS.closed() {
        trace.stage(world.unrecorded_bulk());
        trace.read_volumes(&mut world.recorded_bulk(trace.log, trace.observed));
    }
    stage_ended(Stage::Inputs);
    // While the companion starts, VANA scans its ride journal so Home
    // opens with storage ready. Charging standby and transitions skip it.
    if power.state == State::Initializing {
        let mut bulk = world.recorded_bulk(trace.log, trace.observed);
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
        let position = world.position(trace, now);
        let (physical, light, fitted) = world.sensed(trace, now);
        let online = device::drivers::wifi::online();
        let network_time = firmware_services::network_time::snapshot(now, 0, online);
        trace.network_time(network_time, now);
        let token = sdk.phone_status().map(|status| status.token);
        let log = trace.log;
        log.borrow_mut()
            .session_token(token.as_ref().map(|token| token.bytes()));
        let handles = world.handles(log, trace.observed, &mut trace.started);
        stage_ended(Stage::Observe);
        let mut started = embassy_time::Instant::MIN;
        vana::app::pass(
            sdk,
            system,
            startup,
            light,
            vana::app::Observations {
                position: &position,
                network_time,
                input: &companion,
                physical,
                fitted,
            },
            now,
            handles,
            &mut |point, system| match point {
                vana::app::Point::InputsReady => {
                    for input in system.take_synthetic() {
                        log.borrow_mut().record(&Event::Synthetic(input));
                    }
                }
                vana::app::Point::Shell => {
                    stage_ended(Stage::Shell);
                    started = embassy_time::Instant::now();
                }
                vana::app::Point::ShellPresent => {
                    system.display_max_ms =
                        system.display_max_ms.max(started.elapsed().as_millis());
                    stage_ended(Stage::ShellPresent);
                }
                vana::app::Point::VanaTick => {
                    stage_ended(Stage::VanaTick);
                    started = embassy_time::Instant::now();
                }
                vana::app::Point::LookAhead => {
                    *terrain_step_max_us =
                        (*terrain_step_max_us).max(started.elapsed().as_micros());
                    stage_ended(Stage::LookAhead);
                }
                vana::app::Point::Phone => stage_ended(Stage::Phone),
                vana::app::Point::VanaPresent => stage_ended(Stage::VanaPresent),
                // After VANA's "TURNING OFF" frame, the device's coordinated
                // shutdown takes over; this loop then only serves the terminal.
                vana::app::Point::VanaAfter => stage_ended(Stage::VanaAfter),
            },
        );
        trace.status = world.power(trace);
    }
    trace.seen(sdk, system.take_frame(), now);
    stage_ended(Stage::Seen);
    if sdk.wipes_traces() {
        trace.storage_formatted();
    }
    trace.drain(
        world.unrecorded_bulk(),
        firmware_services::trace::spool::Card::of(
            !device::capabilities::power::ACCESS.closed(),
            sdk.recorder_busy(),
            sdk.shutdown_due(),
        ),
        now,
    );
    stage_ended(Stage::Drain);
    quiet
}

/// What a host call may take of the main stack below WAMR's last check of
/// it: a script call stops rather than go nearer the stack's end.
const SCRIPT_STACK_GUARD: usize = 4096;

/// Script tiles' runtime (#496): WAMR's pool and a 64 KB page for each
/// instance, in PSRAM. Without them no script tile is accepted, and the
/// rest of the firmware runs on.
async fn start_scripts() {
    use vana::compose::script::{INSTANCES, Memory, POOL, start};
    let pool = zeroed_external(core::alloc::Layout::from_size_align(POOL, 16).unwrap()).await;
    let memories = zeroed_external(core::alloc::Layout::new::<[Memory; INSTANCES]>()).await;
    if pool.is_null() || memories.is_null() {
        log::warn!("scripts: no PSRAM for the runtime");
        return;
    }
    let floor = core::ptr::NonNull::new(device::stack::floor(SCRIPT_STACK_GUARD) as *mut u8);
    // SAFETY: both blocks are zeroed, aligned for what they hold, never
    // freed, and referenced nowhere else; this is the only start.
    let started = unsafe {
        start(
            core::slice::from_raw_parts_mut(pool, POOL),
            &mut *memories.cast::<[Memory; INSTANCES]>(),
            floor,
        )
    };
    if !started {
        log::warn!("scripts: the runtime did not start");
    }
}

/// Moves long-lived state into PSRAM so it stays out of the console task
/// future. Task futures are internal DRAM statics, and every byte there comes
/// out of the main stack (#167). Not inlined, so the value is built in this
/// frame, which is gone before the task's loop, rather than in the console
/// task's poll frame, which stays under every frame VANA prepares.
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
        if panic {
            device::drivers::crash_rtc::controlled_panic();
        }
        let _ = panic;
        esp_hal::system::software_reset()
    }
    fn set_fault(&self, fault: u8) {
        device::drivers::wifi::set_fault(fault);
    }
    fn stall(&self) {
        esp_hal::delay::Delay::new().delay_millis(6000);
    }
}
