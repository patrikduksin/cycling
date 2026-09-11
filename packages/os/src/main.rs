#![no_std]
#![no_main]

#[cfg(feature = "debug-harness")]
mod debug_usb;
mod display;
mod persistent;
mod psram;
mod touch;
mod wifi;

use cycling_os::{
    coin,
    companion::{Decoder, Event, Status},
    input::Report,
    metrics::Snapshot as Metrics,
    preferences::{Saver, Settings},
    ui::App,
};
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
    println!(
        "CYCLING_BOOT version={} board=magene-c606 harness={}",
        env!("CARGO_PKG_VERSION"),
        cfg!(feature = "debug-harness")
    );
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

    let mut companion = esp_hal::uart::UartRx::new(
        p.UART2,
        esp_hal::uart::Config::default().with_baudrate(115200),
    )
    .unwrap()
    .with_rx(p.GPIO41);
    println!("CYCLING_COMPANION listening uart=2 rx=41 baud=115200");

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
    spawner.spawn(wifi::start(p.WIFI, spawner).unwrap());
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
    let mut settings_saver = Saver::new(loaded_settings);
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
    let mut display_metrics = Metrics::default();
    let mut next_metrics = 0u64;
    loop {
        let start = Instant::now();
        let now = start.duration_since_epoch().as_millis();
        let heap_free = esp_alloc::HEAP.free();
        heap_min_sampled = heap_min_sampled.min(heap_free);
        let metrics = Metrics {
            uptime_ms: now,
            frame_ms: last_frame_ms,
            max_frame_ms,
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
        };
        if now >= next_metrics {
            display_metrics = metrics;
            next_metrics = now + 1_000;
        }
        let previous = app.point();
        let brightness = app.controls.brightness;
        #[cfg(feature = "debug-harness")]
        debug.tick(
            start.duration_since_epoch().as_millis(),
            &mut app,
            &mut status,
        );
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
                                app.button(button, code);
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
                    app.pointer(point);
                }
            }
            Ok(Report::Release) => {
                available = true;
                if !touch_injected {
                    app.release();
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
                    app.cancel();
                }
            }
        }
        if !touch_injected && last_touch.elapsed().as_millis() > 250 {
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
                        screenshot_requested || screenshot_row != coin::HEIGHT,
                    ),
                    Err(()) => println!("CYCLING_DEBUG 0 INVALID {{}}"),
                }
                break;
            }
        }
        if app.point() != previous {
            println!("CYCLING_TOUCH point={:?}", app.point());
        }
        if app.controls.brightness != brightness {
            backlight.set_duty(app.controls.brightness).unwrap();
            println!("CYCLING_BRIGHTNESS percent={}", app.controls.brightness);
        }
        // The previous LCD transfer is complete here. Explicit persistence ends
        // any temporary session before changing the live App and PWM state.
        #[cfg(feature = "debug-harness")]
        let explicit_settings = if let Some((id, brightness)) = debug.take_persistence() {
            let settings = Settings::new(brightness).unwrap();
            let save_started = Instant::now();
            match settings_store.save(settings) {
                Ok(record) => {
                    app.controls.brightness = brightness;
                    backlight.set_duty(brightness).unwrap();
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
        #[cfg(feature = "debug-harness")]
        let visible_status = debug.status(&status);
        #[cfg(not(feature = "debug-harness"))]
        let visible_status = status;
        app.render(
            &mut canvas,
            available,
            &visible_status,
            wifi::label(),
            &display_metrics,
        );
        screen.draw(&canvas);
        #[cfg(feature = "debug-harness")]
        let temporary_settings = debug.active;
        #[cfg(not(feature = "debug-harness"))]
        let temporary_settings = false;
        let current_settings = Settings::new(app.controls.brightness).unwrap();
        if !explicit_settings
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
                "CYCLING_FRAME frame={} render_ms={}",
                frame,
                start.elapsed().as_millis()
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
