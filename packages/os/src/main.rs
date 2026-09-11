#![no_std]
#![no_main]

mod display;
mod touch;

use cycling_os::{coin, controls::Controls, input::Report};
use esp_backtrace as _;
use esp_hal::{
    clock::CpuClock,
    delay::Delay,
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
    main,
    time::{Instant, Rate},
};
use esp_println::println;

esp_bootloader_esp_idf::esp_app_desc!();

#[main]
fn main() -> ! {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz));
    println!(
        "CYCLING_BOOT version={} board=magene-c606",
        env!("CARGO_PKG_VERSION")
    );
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

    println!("CYCLING_DISPLAY ready canvas=80x106 heap=0");
    let mut canvas = [0; coin::PIXELS];
    let delay = Delay::new();
    let mut frame = 0u32;
    let mut ui = Controls::default();
    let mut last_touch = Instant::now();
    let mut errors = 0u32;
    loop {
        let start = Instant::now();
        let previous = ui.point;
        let brightness = ui.brightness;
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
        ui.render(&mut canvas, available);
        screen.draw(&canvas);
        if frame % 24 == 0 {
            println!(
                "CYCLING_FRAME frame={} render_ms={}",
                frame,
                start.elapsed().as_millis()
            );
        }
        let elapsed = start.elapsed().as_millis();
        if elapsed < 42 {
            delay.delay_millis((42 - elapsed) as u32);
        }
        frame = frame.wrapping_add(1);
    }
}
