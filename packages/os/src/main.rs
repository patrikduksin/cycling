#![no_std]
#![no_main]

mod display;
mod touch;
mod wifi;

use cycling_os::{
    coin,
    companion::{Button, Decoder, Event, Status},
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
        "CYCLING_BOOT version={} board=magene-c606",
        env!("CARGO_PKG_VERSION")
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
    let (mut usb_rx, _usb_tx) = esp_hal::usb_serial_jtag::UsbSerialJtag::new(p.USB_DEVICE).split();
    let mut screenshot_command = cycling_os::screenshot::Command::default();
    let mut snapshot = [0u16; coin::PIXELS];
    let mut screenshot_row = coin::HEIGHT;
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
                                if code == 1 {
                                    match button {
                                        Button::BottomLeft => {
                                            ui.brightness = ui.brightness.saturating_sub(5).max(5)
                                        }
                                        Button::BottomRight => {
                                            ui.brightness = ui.brightness.saturating_add(5).min(100)
                                        }
                                        Button::TopLeft => {}
                                    }
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
        if last_battery.elapsed().as_millis() > 5000 {
            status.battery = None;
        }
        if last_power.elapsed().as_millis() > 5000 {
            status.power = None;
        }
        match touch.poll() {
            Ok(Report::Press(point)) => {
                available = true;
                last_touch = Instant::now();
                ui.update(Some(point));
            }
            Ok(Report::Release) => {
                available = true;
                ui.update(None);
            }
            Ok(Report::Invalid) => {}
            Err(error) => {
                errors = errors.saturating_add(1);
                if errors % 120 == 1 {
                    println!("CYCLING_TOUCH error={:?} count={}", error, errors);
                }
                available = false;
                ui.update(None);
            }
        }
        if last_touch.elapsed().as_millis() > 250 {
            ui.update(None);
        }
        if ui.point != previous {
            println!("CYCLING_TOUCH point={:?}", ui.point);
        }
        if ui.brightness != brightness {
            backlight.set_duty(ui.brightness).unwrap();
            println!("CYCLING_BRIGHTNESS percent={}", ui.brightness);
        }
        ui.render(&mut canvas, available, &status);
        cycling_os::controls::wifi_label(&mut canvas, wifi::label());
        screen.draw(&canvas);
        // Bound input work and stream one row per frame so input and Wi-Fi keep running.
        for _ in 0..64 {
            let Ok(byte) = usb_rx.read_byte() else { break };
            if screenshot_command.push(byte) && screenshot_row == coin::HEIGHT {
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
        }
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
        if elapsed < 42 {
            embassy_time::Timer::after_millis(42 - elapsed).await;
        }
        embassy_futures::yield_now().await;
        frame = frame.wrapping_add(1);
    }
}
