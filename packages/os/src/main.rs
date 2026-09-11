#![no_std]
#![no_main]

#[cfg(feature = "debug-harness")]
mod debug_usb;
mod display;
mod touch;
mod wifi;

use cycling_os::{
    coin,
    companion::{Decoder, Event, Status},
    controls::Controls,
    input::Report,
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
            duty_pct: 50,
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
    let mut ui = Controls::default();
    let mut last_touch = Instant::now();
    let mut errors = 0u32;
    let mut decoder = Decoder::default();
    let mut status = Status::default();
    let mut last_rx = Instant::now();
    let mut last_battery = Instant::now();
    let mut last_power = Instant::now();
    let mut uart_errors = 0u32;
    loop {
        let start = Instant::now();
        let previous = ui.point;
        let brightness = ui.brightness;
        #[cfg(feature = "debug-harness")]
        debug.tick(
            start.duration_since_epoch().as_millis(),
            &mut ui,
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
                                ui.button(button, code);
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
        let touch_injected = debug.touch_injected;
        #[cfg(not(feature = "debug-harness"))]
        let touch_injected = false;
        match touch.poll() {
            Ok(Report::Press(point)) => {
                available = true;
                last_touch = Instant::now();
                if !touch_injected {
                    ui.update(Some(point));
                }
            }
            Ok(Report::Release) => {
                available = true;
                if !touch_injected {
                    ui.update(None);
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
                    ui.update(None);
                }
            }
        }
        if !touch_injected && last_touch.elapsed().as_millis() > 250 {
            ui.update(None);
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
                        &mut ui,
                        &mut status,
                        screenshot_requested || screenshot_row != coin::HEIGHT,
                    ),
                    Err(()) => println!("CYCLING_DEBUG 0 INVALID {{}}"),
                }
                break;
            }
        }
        if ui.point != previous {
            println!("CYCLING_TOUCH point={:?}", ui.point);
        }
        if ui.brightness != brightness {
            backlight.set_duty(ui.brightness).unwrap();
            println!("CYCLING_BRIGHTNESS percent={}", ui.brightness);
        }
        #[cfg(feature = "debug-harness")]
        let visible_status = debug.status(&status);
        #[cfg(not(feature = "debug-harness"))]
        let visible_status = status;
        ui.render(&mut canvas, available, &visible_status);
        cycling_os::controls::wifi_label(&mut canvas, wifi::label());
        screen.draw(&canvas);
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
            &ui,
            &visible_status,
            available,
            decoder.valid_frames,
            decoder.bad_crc,
            uart_errors,
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
        #[cfg(feature = "debug-harness")]
        {
            debug.frame_ms = elapsed;
            debug.max_frame_ms = debug.max_frame_ms.max(elapsed);
        }
        if elapsed < 42 {
            embassy_time::Timer::after_millis(42 - elapsed).await;
        }
        embassy_futures::yield_now().await;
        frame = frame.wrapping_add(1);
    }
}
