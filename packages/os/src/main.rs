#![no_std]
#![no_main]

mod bluetooth;
mod crash_rtc;
#[cfg(feature = "debug-harness")]
mod debug_usb;
mod display;
mod gps_uart;
mod persistent;
mod psram;
mod ride_recorder;
mod sdmmc;
mod touch;
mod wifi;

use core::{alloc::Layout, ptr, slice};
use cycling_os::{
    coin,
    companion::{Decoder, Event, Status},
    idle::{Config as IdleConfig, Gate, Idle},
    input::Report,
    metrics::Snapshot as Metrics,
    preferences::{Saver, Settings},
    redraw::Tracker,
    ride_log::Sample as RideSample,
    ui::App,
};
#[cfg(feature = "debug-harness")]
use embassy_time::Timer;
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    dma_tx_buffer,
    gpio::{DriveMode, Level, Output, OutputConfig},
    lcd_cam::{
        LcdCam,
        lcd::i8080::{Config, I8080},
    },
    ledc::{
        LSGlobalClkSource, Ledc, LowSpeed,
        channel::{self, ChannelIFace},
        timer::{self, TimerIFace},
    },
    time::{Instant, Rate},
};
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

#[esp_rtos::main]
async fn main(spawner: embassy_executor::Spawner) -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz));
    let reset = crash_rtc::reset();
    let crash = crash_rtc::take();
    println!(
        "CYCLING_BOOT version={} board=magene-c606 harness={}",
        env!("CARGO_PKG_VERSION"),
        cfg!(feature = "debug-harness")
    );
    match crash {
        cycling_os::crash::Marker::Valid(report) => println!(
            "CYCLING_RESET reason={} marker={} version={}",
            reset.name(),
            crash.name(),
            report.version()
        ),
        _ => println!(
            "CYCLING_RESET reason={} marker={}",
            reset.name(),
            crash.name()
        ),
    }
    esp_alloc::heap_allocator!(#[esp_hal::ram(reclaimed)] size: 64 * 1024);
    esp_alloc::heap_allocator!(size: 96 * 1024);
    let psram = psram::init(p.PSRAM);
    println!(
        "CYCLING_PSRAM ready mode=quad ram_mhz=40 capacity={} tested={} passes={} internal_before={} internal_after={} external_free={} allocator_probe={} allocator_alignment={}",
        psram.capacity,
        psram.tested,
        psram.passes,
        psram.internal_before,
        psram.internal_after,
        psram.external_free,
        psram.allocator_probe,
        psram.allocator_alignment
    );
    let (mut settings_store, loaded) = persistent::Store::open(p.FLASH);
    let loaded_settings = match loaded {
        Ok(loaded) => {
            println!(
                "CYCLING_STORAGE ready backend=ota_1_tail base=0x{:08x} sectors=2 capacity={} source={:?} sequence={:?} length={} brightness={}",
                persistent::BASE,
                cycling_os::storage::CAPACITY,
                loaded.source,
                loaded.sequence,
                loaded.length,
                loaded.settings.brightness
            );
            loaded.settings
        }
        Err(error) => {
            println!(
                "CYCLING_STORAGE load_failed error={:?} using=defaults",
                error
            );
            Settings::default()
        }
    };
    if option_env!("CYCLING_SDMMC_PROBE") == Some("1") {
        println!(
            "CYCLING_SDMMC probe=start mode=native-read-only slot=1 width=1 clock_hz=400000 pins=13,14,16"
        );
        match sdmmc::probe(
            p.SDHOST, p.GPIO13, p.GPIO14, p.GPIO16, p.GPIO17, p.GPIO18, p.GPIO15,
        ) {
            Ok(report) => {
                let model = core::str::from_utf8(&report.product).unwrap_or("??????");
                println!(
                    "CYCLING_SDMMC ready kind={} model={} high_capacity={} capacity_bytes={} sector_size={} reads={} repeated_equal={}",
                    report.kind.name(),
                    model,
                    report.high_capacity,
                    report.sectors * u64::from(report.sector_size),
                    report.sector_size,
                    report.reads,
                    report.repeated_equal
                );
                println!(
                    "CYCLING_SDMMC_PRIVATE rca={:04x} cid={:08x},{:08x},{:08x},{:08x} csd={:08x},{:08x},{:08x},{:08x} first_hash={:08x} second_hash={:08x} last_hash={:08x}",
                    report.rca,
                    report.cid[0],
                    report.cid[1],
                    report.cid[2],
                    report.cid[3],
                    report.csd[0],
                    report.csd[1],
                    report.csd[2],
                    report.csd[3],
                    report.first_hash,
                    report.second_hash,
                    report.last_hash
                );
            }
            Err(error) => println!("CYCLING_SDMMC probe_failed error={:?}", error),
        }
    } else {
        // Own the recovered storage pins even in ordinary builds so later
        // changes cannot silently assign them to another peripheral.
        let _sdhost = p.SDHOST;
        let _storage_pins = (p.GPIO13, p.GPIO14, p.GPIO16, p.GPIO17, p.GPIO18, p.GPIO15);
    }
    let timg0 = esp_hal::timer::timg::TimerGroup::new(p.TIMG0);
    let interrupts = esp_hal::interrupt::software::SoftwareInterruptControl::new(p.SW_INTERRUPT);
    esp_rtos::start(timg0.timer0, interrupts.software_interrupt0);
    let _rd = Output::new(p.GPIO39, Level::High, OutputConfig::default());
    let lcd = LcdCam::new(p.LCD_CAM);
    let bus = I8080::new(
        lcd.lcd,
        p.DMA_CH0,
        Config::default().with_frequency(Rate::from_mhz(10)),
    )
    .unwrap()
    .with_cs(p.GPIO2)
    .with_dc(p.GPIO40)
    .with_wrx(p.GPIO3)
    .with_data0(p.GPIO4)
    .with_data1(p.GPIO38)
    .with_data2(p.GPIO5)
    .with_data3(p.GPIO37)
    .with_data4(p.GPIO6)
    .with_data5(p.GPIO36)
    .with_data6(p.GPIO7)
    .with_data7(p.GPIO35)
    .with_data8(p.GPIO8)
    .with_data9(p.GPIO34)
    .with_data10(p.GPIO9)
    .with_data11(p.GPIO33)
    .with_data12(p.GPIO10)
    .with_data13(p.GPIO47)
    .with_data14(p.GPIO11)
    .with_data15(p.GPIO48);
    let mut screen = display::Display::new(bus, dma_tx_buffer!(3840).unwrap());
    screen.init();

    let touch_bus = esp_hal::i2c::master::I2c::new(
        p.I2C0,
        esp_hal::i2c::master::Config::default().with_frequency(Rate::from_khz(100)),
    )
    .unwrap()
    .with_sda(p.GPIO21)
    .with_scl(p.GPIO12);
    let mut touch = touch::Touch::new(touch_bus);
    let probe = touch.probe();
    println!("CYCLING_TOUCH probe={:02x?}", probe);
    let mut available = probe.is_ok();

    let companion_uart = esp_hal::uart::Uart::new(
        p.UART2,
        esp_hal::uart::Config::default().with_baudrate(115200),
    )
    .unwrap()
    .with_rx(p.GPIO41)
    .with_tx(p.GPIO42);
    let (mut companion, mut companion_tx) = companion_uart.split();
    println!("CYCLING_COMPANION listening uart=2 tx=42 rx=41 baud=115200");
    const GPS_OPEN: [u8; 16] = [
        0xa5, 0x0c, 0x6f, 0xf1, 0x02, 0x10, 0xe2, 0x02, 0x07, 0x00, 0x00, 0x01, 0x00, 0x00, 0x89,
        0xe5,
    ];
    match companion_tx.write(&GPS_OPEN) {
        Ok(16) => println!("CYCLING_GPS companion_open=sent source=stock_candidate"),
        Ok(count) => println!("CYCLING_GPS companion_open_short bytes={}", count),
        Err(error) => println!("CYCLING_GPS companion_open_failed error={:?}", error),
    }
    let mut gps_receiver = gps_uart::init(p.UART0, p.GPIO0, p.UHCI0, p.DMA_CH1);
    let mut gps_parser = cycling_os::gps::Parser::default();
    let mut gps_ring_overflow = 0u32;
    let mut gps_uart_errors_seen = 0u32;
    println!(
        "CYCLING_GPS ready uart=0 rx=0 baud=921600 dma=uhci0 channel=1 buffer=8192 mode=stock_open_candidate"
    );

    let mut ledc = Ledc::new(p.LEDC);
    ledc.set_global_slow_clock(LSGlobalClkSource::APBClk);
    let mut timer = ledc.timer::<LowSpeed>(timer::Number::Timer0);
    timer
        .configure(timer::config::Config {
            duty: timer::config::Duty::Duty10Bit,
            clock_source: timer::LSClockSource::APBClk,
            frequency: Rate::from_khz(20),
        })
        .unwrap();
    let mut backlight = ledc.channel(channel::Number::Channel0, p.GPIO45);
    backlight
        .configure(channel::config::Config {
            timer: &timer,
            duty_pct: loaded_settings.brightness,
            drive_mode: DriveMode::PushPull,
        })
        .unwrap();

    println!("CYCLING_DISPLAY ready canvas=80x106 heap=163840");
    let mut canvas = [0; coin::PIXELS];
    let previous_layout = Layout::new::<[u16; coin::PIXELS]>();
    let previous_pointer = unsafe { psram::allocate_external(previous_layout) };
    if previous_pointer.is_null() {
        panic!("display history allocation failed");
    }
    unsafe { ptr::write_bytes(previous_pointer, 0, previous_layout.size()) };
    let previous_canvas =
        unsafe { slice::from_raw_parts_mut(previous_pointer.cast::<u16>(), coin::PIXELS) };
    let mut redraw = Tracker::default();
    println!(
        "CYCLING_DISPLAY history=exact bytes={} memory=external external_free={}",
        previous_layout.size(),
        psram::external_free()
    );
    spawner.spawn(wifi::start(p.WIFI, spawner).unwrap());
    spawner.spawn(bluetooth::start(p.BT).unwrap());
    #[cfg(feature = "debug-harness")]
    let (mut usb_rx, _usb_tx) = esp_hal::usb_serial_jtag::UsbSerialJtag::new(p.USB_DEVICE).split();
    #[cfg(feature = "debug-harness")]
    let mut debug = debug_usb::Debug::new();
    #[cfg(feature = "debug-harness")]
    let mut debug_lines = cycling_os::debug::Lines::default();
    #[cfg(feature = "debug-harness")]
    let mut screenshot_command = cycling_os::screenshot::Command::default();
    #[cfg(feature = "debug-harness")]
    let mut snapshot = [0u16; coin::PIXELS];
    #[cfg(feature = "debug-harness")]
    let mut screenshot_row = coin::HEIGHT;
    #[cfg(feature = "debug-harness")]
    let mut screenshot_frame = 0u32;
    let mut frame = 0u32;
    let mut app = App::default();
    app.controls.brightness = loaded_settings.brightness;
    app.dim_timeout_secs = loaded_settings.dim_timeout_secs;
    app.dim_brightness = loaded_settings.dim_brightness;
    app.timezone_minutes = loaded_settings.timezone_minutes;
    let mut settings_saver = Saver::new(loaded_settings);
    let mut runtime_settings = loaded_settings;
    let mut idle = Idle::new(Instant::now().duration_since_epoch().as_millis());
    let mut applied_brightness = loaded_settings.brightness;
    let mut last_touch = Instant::now();
    let mut errors = 0u32;
    let mut decoder = Decoder::default();
    let mut status = Status::default();
    let mut last_rx = Instant::now();
    let mut last_battery = Instant::now();
    let mut last_power = Instant::now();
    let mut uart_errors = 0u32;
    let mut heap_min_sampled = esp_alloc::HEAP.free();
    let mut last_frame_ms = 0u32;
    let mut max_frame_ms = 0u32;
    let mut display_draws = 0u32;
    let mut display_skips = 0u32;
    let mut display_metrics = Metrics::default();
    let mut next_metrics = 0u64;
    let mut ride_recorder = ride_recorder::Recorder::default();
    let mut last_recording_status = ride_recorder.status();
    loop {
        let start = Instant::now();
        let now = start.duration_since_epoch().as_millis();
        #[cfg(feature = "debug-harness")]
        debug.tick(now, &mut app, &mut status, &mut runtime_settings, &mut idle);
        let mut idle_config = IdleConfig {
            timeout_ms: (runtime_settings.dim_timeout_secs != 0)
                .then_some(u64::from(runtime_settings.dim_timeout_secs) * 1_000),
            dim_level: runtime_settings.dim_brightness,
        };
        idle.tick(now, idle_config);
        let heap_free = esp_alloc::HEAP.free();
        heap_min_sampled = heap_min_sampled.min(heap_free);
        let mut metrics = Metrics {
            reset,
            crash,
            ride_recording: ride_recorder.status(),
            ride_source: ride_recorder.source(),
            recording_active_ms: ride_recorder.active_ms(now),
            recorded_samples: ride_recorder.written_samples(),
            recording_dropped: ride_recorder.dropped_samples(),
            recorded_rides: ride_recorder.completed(),
            recording_slot: ride_recorder.next_slot().min(u16::MAX as usize) as u16,
            recording_write_ms: ride_recorder.max_write_ms(),
            recording_erase_ms: ride_recorder.max_erase_ms(),
            ride_summaries: ride_recorder.summaries(),
            ride_summary_count: ride_recorder.summary_count(),
            gps: cycling_os::gps::Snapshot::default(),
            ble: bluetooth::snapshot(now),
            uptime_ms: now,
            frame_ms: last_frame_ms,
            max_frame_ms,
            display_draws,
            display_skips,
            heap_free,
            heap_min_sampled,
            psram_capacity: psram::CAPACITY,
            psram_free: psram::external_free(),
            companion_valid: decoder.valid_frames,
            companion_bad_crc: decoder.bad_crc,
            uart_errors,
            touch_errors: errors,
            harness: cfg!(feature = "debug-harness"),
            #[cfg(feature = "debug-harness")]
            recording: debug.recording(),
            #[cfg(not(feature = "debug-harness"))]
            recording: false,
            selected_brightness: app.controls.brightness,
            effective_brightness: idle.effective(app.controls.brightness, idle_config),
            dimmed: idle.dimmed(),
            idle_ms: idle.age_ms(now),
            dim_timeout_secs: runtime_settings.dim_timeout_secs,
            dim_brightness: runtime_settings.dim_brightness,
        };
        let previous = app.point();
        let ride_before_input = app.ride;
        let brightness = app.controls.brightness;
        if last_rx.elapsed().as_millis() > 250 {
            decoder.reset();
        }
        let mut bytes = [0u8; 128];
        match companion.read_buffered(&mut bytes) {
            Ok(n) if n > 0 => {
                last_rx = Instant::now();
                for &byte in &bytes[..n] {
                    if let Some(event) = decoder.push(byte) {
                        match event {
                            Event::Battery {
                                percent,
                                millivolts,
                            } => {
                                if status.battery.is_none()
                                    || last_battery.elapsed().as_millis() > 1000
                                {
                                    println!(
                                        "CYCLING_BATTERY percent={} millivolts={}",
                                        percent, millivolts
                                    );
                                }
                                last_battery = Instant::now();
                            }
                            Event::Power { status: value } => {
                                if status.power != Some(value) {
                                    println!("CYCLING_POWER status={}", value);
                                }
                                last_power = Instant::now();
                            }
                            Event::Button { button, code } => {
                                println!("CYCLING_BUTTON button={:?} code={}", button, code);
                                if code != 1 || idle.button(now) == Gate::Forward {
                                    app.button_at(button, code, now);
                                }
                            }
                        }
                        status.update(event);
                    }
                }
            }
            Err(e) => {
                uart_errors = uart_errors.saturating_add(1);
                decoder.reset();
                if uart_errors % 120 == 1 {
                    println!("CYCLING_COMPANION error={:?} count={}", e, uart_errors);
                }
            }
            _ => {}
        }
        let mut gps_bytes = [0u8; 2048];
        let (gps_count, gps_overflow, gps_uart_errors) = gps_receiver.drain(&mut gps_bytes);
        gps_parser.overflow(gps_overflow.saturating_sub(gps_ring_overflow));
        gps_ring_overflow = gps_overflow;
        if gps_uart_errors != gps_uart_errors_seen {
            gps_parser.data_loss();
            gps_uart_errors_seen = gps_uart_errors;
        }
        for &byte in &gps_bytes[..gps_count] {
            gps_parser.push(byte, now);
        }
        let mut gps = gps_parser.snapshot(now);
        gps.uart_errors = gps_uart_errors;
        metrics.gps = gps;
        if frame % 240 == 0 {
            println!(
                "CYCLING_GPS state={:?} identity={} bytes={} valid={} checksum_errors={} parse_errors={} dma_losses={} line_overflow={} uart_errors={}",
                gps.state,
                gps.identity.name(),
                gps.bytes,
                gps.valid_sentences,
                gps.checksum_errors,
                gps.parse_errors,
                gps.overflows,
                gps.line_overflows,
                gps_uart_errors
            );
        }
        if last_battery.elapsed().as_millis() > 5000 {
            status.battery = None;
        }
        if last_power.elapsed().as_millis() > 5000 {
            status.power = None;
        }
        #[cfg(feature = "debug-harness")]
        let touch_injected = debug.touch_injected();
        #[cfg(not(feature = "debug-harness"))]
        let touch_injected = false;
        match touch.poll() {
            Ok(Report::Press(point)) => {
                available = true;
                last_touch = Instant::now();
                if !touch_injected {
                    if idle.contact(now) == Gate::Forward {
                        app.pointer(point);
                    } else {
                        app.cancel();
                    }
                }
            }
            Ok(Report::Release) => {
                available = true;
                if !touch_injected {
                    if idle.release(now) == Gate::Forward {
                        app.release_at(now);
                    } else {
                        app.cancel();
                    }
                }
            }
            Ok(Report::Invalid) => {}
            Err(error) => {
                errors = errors.saturating_add(1);
                if errors % 120 == 1 {
                    println!("CYCLING_TOUCH error={:?} count={}", error, errors);
                }
                available = false;
                if !touch_injected {
                    idle.release(now);
                    app.cancel();
                }
            }
        }
        if !touch_injected && last_touch.elapsed().as_millis() > 250 {
            idle.release(now);
            app.cancel();
        }
        #[cfg(feature = "debug-harness")]
        let mut screenshot_requested = false;
        // Bound input work and stream one row per frame so input and Wi-Fi keep running.
        #[cfg(feature = "debug-harness")]
        for _ in 0..64 {
            let Ok(byte) = usb_rx.read_byte() else { break };
            if screenshot_command.push(byte) && screenshot_row == coin::HEIGHT && !debug.recording()
            {
                screenshot_requested = true;
            }
            if let Some(command) = debug_lines.push(byte) {
                match command {
                    Ok((id, action)) => debug.command(
                        id,
                        action,
                        start.duration_since_epoch().as_millis(),
                        &mut app,
                        &mut status,
                        &mut runtime_settings,
                        &mut idle,
                        screenshot_requested || screenshot_row != coin::HEIGHT,
                    ),
                    Err(()) => println!("CYCLING_DEBUG 0 INVALID {{}}"),
                }
                break;
            }
        }
        let ride_action = app.take_ride_action();
        #[cfg(feature = "debug-harness")]
        let temporary_ride = debug.active;
        #[cfg(not(feature = "debug-harness"))]
        let temporary_ride = false;
        if let Some(action) = ride_action
            && !temporary_ride
        {
            let _accepted = ride_recorder.request(action, app.selected_ride_source(), now, 0);
            // Durable controls change the visible demo only after the record is
            // committed and read back. Rejected and failed operations stay put.
            app.ride = ride_before_input;
        }
        #[cfg(feature = "debug-harness")]
        if let Some((id, action)) = debug.take_ride() {
            let accepted = match action {
                Some((action, source)) => ride_recorder.request(action, source, now, id),
                None => ride_recorder.initialize(id),
            };
            if !accepted {
                debug.ride_result(id, false);
            }
        }
        if app.point() != previous {
            println!("CYCLING_TOUCH point={:?}", app.point());
        }
        if app.controls.brightness != brightness {
            println!("CYCLING_BRIGHTNESS percent={}", app.controls.brightness);
        }
        runtime_settings = runtime_settings
            .with_brightness(app.controls.brightness)
            .unwrap()
            .with_idle_preferences(app.dim_timeout_secs, app.dim_brightness)
            .unwrap()
            .with_timezone(app.timezone_minutes)
            .unwrap();
        // The previous LCD transfer is complete here. Explicit persistence ends
        // any temporary session before changing the live App and PWM state.
        #[cfg(feature = "debug-harness")]
        let explicit_settings = if let Some((id, brightness)) = debug.take_persistence() {
            let settings = runtime_settings.with_brightness(brightness).unwrap();
            let save_started = Instant::now();
            match settings_store.save(settings) {
                Ok(record) => {
                    app.controls.brightness = brightness;
                    runtime_settings = settings;
                    settings_saver.saved(settings);
                    println!(
                        "CYCLING_SETTINGS persisted sequence={} brightness={} write_ms={}",
                        record.sequence,
                        brightness,
                        save_started.elapsed().as_millis()
                    );
                    debug.persistence_result(id, "OK");
                }
                Err(error) => {
                    println!("CYCLING_SETTINGS persist_failed error={:?}", error);
                    debug.persistence_result(id, "PERSIST_FAILED");
                }
            }
            true
        } else {
            false
        };
        #[cfg(not(feature = "debug-harness"))]
        let explicit_settings = false;
        // The prior LCD DMA is complete. Service one bounded flash operation,
        // apply its verified result, and only then render/ack the new state.
        let ride_clock = cycling_os::network_time::snapshot(now, 0, wifi::online());
        let (ride_heart_bpm, ride_cadence_tenths) =
            cycling_os::ble_sensor::ride_fields(metrics.ble, ride_recorder.source());
        let ride_sample = RideSample {
            active_ms: 0,
            utc_ms: ride_clock.unix_seconds.and_then(|seconds| {
                seconds
                    .checked_mul(1_000)
                    .and_then(|value| value.checked_add(u64::from(ride_clock.millis)))
            }),
            location_e7: (metrics.gps.state == cycling_os::gps::FixState::Fresh)
                .then(|| Some((metrics.gps.latitude_e7?, metrics.gps.longitude_e7?)))
                .flatten(),
            demo_speed_mm_s: Some(cycling_os::ride::DEMO_SPEED_MM_S),
            heart_bpm: ride_heart_bpm,
            cadence_tenths: ride_cadence_tenths,
            battery_percent: status.battery.map(|(percent, _)| percent),
        };
        let mut ride_flash = ride_recorder.service(&mut settings_store, now, ride_sample);
        if ride_recorder.status() != last_recording_status {
            last_recording_status = ride_recorder.status();
            println!(
                "CYCLING_RIDE_RECORD status={} source={} slot={} rides={} samples={} dropped={} write_ms={} erase_ms={}",
                last_recording_status.name(),
                ride_recorder
                    .source()
                    .map(|source| source.name())
                    .unwrap_or("none"),
                ride_recorder.next_slot(),
                ride_recorder.completed(),
                ride_recorder.written_samples(),
                ride_recorder.dropped_samples(),
                ride_recorder.max_write_ms(),
                ride_recorder.max_erase_ms()
            );
        }
        if let Some(result) = ride_recorder.take_result() {
            if result.ok
                && let Some(action) = result.action
            {
                app.apply_ride_action(action, now);
            }
            #[cfg(feature = "debug-harness")]
            if result.token != 0 {
                debug.ride_result(result.token, result.ok);
            }
        }
        #[cfg(feature = "debug-harness")]
        if let Some((id, slot)) = debug.take_export() {
            if !ride_recorder.exportable() {
                debug.export_error(id, "BUSY");
            } else if let Some(index) = slot {
                if usize::from(index) >= ride_recorder.next_slot() {
                    debug.export_error(id, "BOUNDS");
                } else {
                    let mut bytes = cycling_os::ride_log::Slot::default();
                    match settings_store.ride_read_slot(usize::from(index), &mut bytes) {
                        Ok(()) => debug.export_slot(id, index, &bytes),
                        Err(_) => debug.export_error(id, "READ"),
                    }
                    ride_flash = true;
                }
            } else {
                debug.export_info(id, ride_recorder.next_slot(), ride_recorder.status().name());
            }
        }
        metrics.ride_recording = ride_recorder.status();
        metrics.ride_source = ride_recorder.source();
        metrics.recording_active_ms = ride_recorder.active_ms(now);
        metrics.recorded_samples = ride_recorder.written_samples();
        metrics.recording_dropped = ride_recorder.dropped_samples();
        metrics.recorded_rides = ride_recorder.completed();
        metrics.recording_slot = ride_recorder.next_slot().min(u16::MAX as usize) as u16;
        metrics.recording_write_ms = ride_recorder.max_write_ms();
        metrics.recording_erase_ms = ride_recorder.max_erase_ms();
        metrics.ride_summaries = ride_recorder.summaries();
        metrics.ride_summary_count = ride_recorder.summary_count();
        // Recorder transitions are rendered in the same acknowledged frame;
        // unrelated diagnostics retain their bounded one-second refresh.
        display_metrics.ride_recording = metrics.ride_recording;
        display_metrics.ride_source = metrics.ride_source;
        display_metrics.recording_active_ms = metrics.recording_active_ms;
        display_metrics.recorded_samples = metrics.recorded_samples;
        display_metrics.recording_dropped = metrics.recording_dropped;
        display_metrics.recorded_rides = metrics.recorded_rides;
        display_metrics.recording_slot = metrics.recording_slot;
        display_metrics.recording_write_ms = metrics.recording_write_ms;
        display_metrics.recording_erase_ms = metrics.recording_erase_ms;
        display_metrics.ride_summaries = metrics.ride_summaries;
        display_metrics.ride_summary_count = metrics.ride_summary_count;
        idle_config = IdleConfig {
            timeout_ms: (runtime_settings.dim_timeout_secs != 0)
                .then_some(u64::from(runtime_settings.dim_timeout_secs) * 1_000),
            dim_level: runtime_settings.dim_brightness,
        };
        idle.tick(now, idle_config);
        let effective_brightness = idle.effective(app.controls.brightness, idle_config);
        metrics.selected_brightness = app.controls.brightness;
        metrics.effective_brightness = effective_brightness;
        metrics.dimmed = idle.dimmed();
        metrics.idle_ms = idle.age_ms(now);
        metrics.dim_timeout_secs = runtime_settings.dim_timeout_secs;
        metrics.dim_brightness = runtime_settings.dim_brightness;
        if now >= next_metrics {
            display_metrics = metrics;
            next_metrics = now + 1_000;
        }
        if effective_brightness != applied_brightness {
            backlight.set_duty(effective_brightness).unwrap();
            applied_brightness = effective_brightness;
            println!(
                "CYCLING_BACKLIGHT selected={} effective={} dimmed={}",
                app.controls.brightness,
                effective_brightness,
                idle.dimmed()
            );
        }
        #[cfg(feature = "debug-harness")]
        let visible_status = debug.status(&status);
        #[cfg(not(feature = "debug-harness"))]
        let visible_status = status;
        let clock = cycling_os::network_time::snapshot(
            now,
            runtime_settings.timezone_minutes,
            wifi::online(),
        );
        app.render(
            &mut canvas,
            available,
            &visible_status,
            wifi::label(),
            &display_metrics,
            &clock,
        );
        if redraw.changed(&canvas, previous_canvas) {
            screen.draw(&canvas);
            redraw.commit(&canvas, previous_canvas);
            display_draws = display_draws.saturating_add(1);
        } else {
            display_skips = display_skips.saturating_add(1);
        }
        #[cfg(feature = "debug-harness")]
        {
            metrics.display_draws = display_draws;
            metrics.display_skips = display_skips;
        }
        #[cfg(feature = "debug-harness")]
        let temporary_settings = debug.active;
        #[cfg(not(feature = "debug-harness"))]
        let temporary_settings = false;
        let current_settings = runtime_settings;
        if !ride_flash
            && !explicit_settings
            && settings_saver.ready(
                now,
                current_settings,
                temporary_settings,
                app.point().is_some(),
            )
        {
            let save_started = Instant::now();
            match settings_store.save(current_settings) {
                Ok(record) => {
                    settings_saver.saved(current_settings);
                    println!(
                        "CYCLING_SETTINGS saved sequence={} brightness={} write_ms={}",
                        record.sequence,
                        current_settings.brightness,
                        save_started.elapsed().as_millis()
                    );
                }
                Err(error) => {
                    settings_saver.failed(now);
                    println!("CYCLING_SETTINGS save_failed error={:?}", error);
                }
            }
        }
        #[cfg(feature = "debug-harness")]
        if screenshot_requested {
            snapshot.copy_from_slice(&canvas);
            screenshot_frame = frame;
            screenshot_row = 0;
            println!(
                "CYCLING_SHOT BEGIN {} {} {} {:08x}",
                screenshot_frame,
                coin::WIDTH,
                coin::HEIGHT,
                cycling_os::screenshot::checksum(&snapshot)
            );
        }
        #[cfg(feature = "debug-harness")]
        debug.drawn(
            frame,
            Instant::now().duration_since_epoch().as_millis(),
            &canvas,
            &app,
            &visible_status,
            available,
            &metrics,
        );
        #[cfg(feature = "debug-harness")]
        if let Some(reboot) = debug.take_reboot() {
            Timer::after_millis(50).await;
            match reboot {
                debug_usb::Reboot::Panic => crash_rtc::controlled_panic(),
                debug_usb::Reboot::Restart => esp_hal::system::software_reset(),
            }
        }
        #[cfg(feature = "debug-harness")]
        if screenshot_row < coin::HEIGHT {
            let mut hex = [0u8; coin::WIDTH * 4];
            const DIGITS: &[u8; 16] = b"0123456789abcdef";
            for (pixel, output) in snapshot[screenshot_row * coin::WIDTH..][..coin::WIDTH]
                .iter()
                .zip(hex.chunks_exact_mut(4))
            {
                for (i, byte) in output.iter_mut().enumerate() {
                    *byte = DIGITS[((pixel >> (12 - i * 4)) & 15) as usize];
                }
            }
            println!(
                "CYCLING_SHOT ROW {} {} {}",
                screenshot_frame,
                screenshot_row,
                core::str::from_utf8(&hex).unwrap()
            );
            screenshot_row += 1;
            if screenshot_row == coin::HEIGHT {
                println!("CYCLING_SHOT END {}", screenshot_frame);
            }
        }
        if frame % 240 == 0 {
            println!(
                "CYCLING_COMPANION valid={} bad_crc={} uart_errors={}",
                decoder.valid_frames, decoder.bad_crc, uart_errors
            );
        }
        if frame % 24 == 0 {
            println!(
                "CYCLING_FRAME frame={} render_ms={} draws={} skipped={}",
                frame,
                start.elapsed().as_millis(),
                display_draws,
                display_skips
            );
        }
        let elapsed = start.elapsed().as_millis();
        last_frame_ms = elapsed.min(u64::from(u32::MAX)) as u32;
        max_frame_ms = max_frame_ms.max(last_frame_ms);
        if elapsed < 42 {
            embassy_time::Timer::after_millis(42 - elapsed).await;
        }
        embassy_futures::yield_now().await;
        frame = frame.wrapping_add(1);
    }
}
