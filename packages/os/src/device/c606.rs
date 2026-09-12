//! Verified C606 wiring and one-time resource construction.
//!
//! HAL initialization claims the singleton peripheral set. Consuming its tokens
//! assigns LCD DMA_CH0, GNSS UHCI0/DMA_CH1, and companion UART2 interrupts once.
//! MMC remains opt-in read-only; D1-D3 are reserved candidates, not verified lanes.

use super::{companion_uart, crash_rtc, display, gps_uart, psram, sdmmc, touch};
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
    time::Rate,
};
use esp_println::println;
use static_cell::StaticCell;

// LEDC channels borrow their timer. Keep that timer in permanent board-owned
// storage, so moving Resources cannot invalidate the channel's reference.
static BACKLIGHT_TIMER: StaticCell<timer::Timer<'static, LowSpeed>> = StaticCell::new();

pub struct Resources {
    pub reset: cycling_os::crash::Reset,
    pub crash: cycling_os::crash::Marker,
    pub flash: esp_hal::peripherals::FLASH<'static>,
    pub wifi: esp_hal::peripherals::WIFI<'static>,
    pub bluetooth: esp_hal::peripherals::BT<'static>,
    pub usb: esp_hal::usb_serial_jtag::UsbSerialJtag<'static, esp_hal::Blocking>,
    pub screen: display::Display<'static>,
    pub touch: touch::Touch<'static>,
    pub touch_available: bool,
    pub gps_receiver: gps_uart::Receiver,
    pub backlight: channel::Channel<'static, LowSpeed>,
    // Keep the LCD read strobe driven high throughout the board's lifetime.
    pub _lcd_read: Output<'static>,
}

/// Claim the board once. A second call is rejected by esp_hal's singleton guard.
/// Backlight starts off; composition applies the persisted brightness after load.
pub fn init() -> Resources {
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
    let touch_available = probe.is_ok();

    match companion_uart::init(p.UART2, p.GPIO41, p.GPIO42) {
        Ok(16) => println!("CYCLING_GPS companion_open=sent source=stock_candidate"),
        Ok(count) => println!("CYCLING_GPS companion_open_short bytes={}", count),
        Err(()) => println!("CYCLING_GPS companion_open_failed"),
    }
    println!("CYCLING_COMPANION listening uart=2 tx=42 rx=41 baud=115200 buffer=2048");
    let gps_receiver = gps_uart::init(p.UART0, p.GPIO0, p.UHCI0, p.DMA_CH1);
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
            timer: BACKLIGHT_TIMER.init(timer),
            duty_pct: 0,
            drive_mode: DriveMode::PushPull,
        })
        .unwrap();

    Resources {
        reset,
        crash,
        flash: p.FLASH,
        wifi: p.WIFI,
        bluetooth: p.BT,
        usb: esp_hal::usb_serial_jtag::UsbSerialJtag::new(p.USB_DEVICE),
        screen,
        touch,
        touch_available,
        gps_receiver,
        backlight,
        _lcd_read: _rd,
    }
}
