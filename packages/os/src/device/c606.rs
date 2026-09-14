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

struct Resources {
    pub reset: cycling_os::crash::Reset,
    pub crash: cycling_os::crash::Marker,
    pub bulk: Option<sdmmc::Reader<'static>>,
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
fn init() -> Resources {
    let p = esp_hal::init(esp_hal::Config::default().with_cpu_clock(CpuClock::_160MHz));
    super::sleep::initialize(p.LPWR);
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
    // Ordinary read-only access. No mount, card register writes or implicit
    // application namespace: vendor data remains owned by stock.
    let bulk = match sdmmc::open(
        p.SDHOST, p.GPIO13, p.GPIO14, p.GPIO16, p.GPIO17, p.GPIO18, p.GPIO15,
    ) {
        Ok(reader) => {
            use cycling_os::bulk::Read;
            let info = reader.info().unwrap();
            println!(
                "CYCLING_SDMMC ready mode=read-only sectors={} sector_size={} width={} clock_hz={}",
                info.sectors, info.sector_size, info.bus_width, info.clock_hz
            );
            Some(reader)
        }
        Err(error) => {
            println!("CYCLING_SDMMC initialization_failed error={:?}", error);
            None
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
    let touch_available = probe.is_ok();

    match companion_uart::init(p.UART2, p.GPIO41, p.GPIO42) {
        Ok(16) => println!("CYCLING_GPS companion_open=sent source=stock_candidate"),
        Ok(count) => println!("CYCLING_GPS companion_open_short bytes={}", count),
        Err(()) => println!("CYCLING_GPS companion_open_failed"),
    }
    super::services::sensors::startup_begin(embassy_time::Instant::now().as_millis());
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
        bulk,
        gps_receiver,
        backlight,
        _lcd_read: _rd,
    }
}

/// Handles expose capabilities, never raw peripheral tokens to consumers.
pub struct Input;
impl cycling_os::capabilities::InputSource for Input {
    fn controls(&self) -> cycling_os::capabilities::Controls {
        use cycling_os::capabilities::{Availability, Button, Controls};
        Controls {
            touch: match super::services::io::snapshot(embassy_time::Instant::now().as_millis()) {
                Some(s) if s.touch_available => Availability::Ready,
                Some(_) => Availability::Failed,
                None => Availability::Initializing,
            },
            buttons: &[Button::TopLeft, Button::BottomLeft, Button::BottomRight],
        }
    }
    fn take_edge(&mut self) -> Option<cycling_os::capabilities::Edge> {
        super::services::io::take_edge()
    }
}
pub struct Power;
impl cycling_os::capabilities::Power for Power {
    fn availability(&self) -> cycling_os::capabilities::Availability {
        if super::services::power::light_available() {
            cycling_os::capabilities::Availability::Ready
        } else {
            cycling_os::capabilities::Availability::Failed
        }
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        use cycling_os::capabilities::Observation;
        super::services::io::snapshot(embassy_time::Instant::now().as_millis()).and_then(
            |s| match s.battery {
                Observation::Fresh {
                    value: (percent, mv),
                    received_ms,
                }
                | Observation::Stale {
                    value: (percent, mv),
                    received_ms,
                } => Some((percent, mv, received_ms)),
                Observation::Unavailable => None,
            },
        )
    }
    fn brightness(&mut self, percent: u8) -> Result<(), cycling_os::capabilities::Error> {
        if percent > 100 {
            return Err(cycling_os::capabilities::Error::Invalid);
        }
        super::services::power::brightness(percent)
    }
}
pub struct Positioning;
impl cycling_os::capabilities::Positioning for Positioning {
    fn availability(&self) -> cycling_os::capabilities::Availability {
        cycling_os::capabilities::Availability::Ready
    }
    fn snapshot(&self, now_ms: u64) -> Option<cycling_os::positioning::Snapshot> {
        super::services::positioning::snapshot(now_ms)
    }
}
/// Composition receives exclusive initialized handles. Acquisition is already
/// running and outlives foreground presentation and terminal connections.
pub struct Parts {
    pub display: display::Display<'static>,
    pub input: Input,
    pub input_observation: Input,
    pub ant: Ant,
    pub ble: Ble,
    pub positioning: Positioning,
    pub network: Network,
    pub power: Power,
    pub power_control: Power,
    pub sensors: Sensors,
    pub sound: Sound,
    pub bulk: Bulk,
    pub storage: super::storage::Backend<'static>,
    pub terminal: super::usb::Usb,
    pub reset: cycling_os::crash::Reset,
    pub crash: cycling_os::crash::Marker,
}
pub async fn start(spawner: embassy_executor::Spawner) -> Parts {
    let board = init();
    spawner.spawn(super::services::power::run(board.backlight).unwrap());
    spawner.spawn(super::services::positioning::run(board.gps_receiver).unwrap());
    spawner.spawn(super::services::io::run(board.touch, board.touch_available).unwrap());
    let stack = super::wifi::initialize(board.wifi, spawner).await;
    if let Some(stack) = stack {
        spawner.spawn(super::wifi::time_sync(stack).unwrap());
    }
    spawner.spawn(super::bluetooth::start(board.bluetooth).unwrap());
    // Output intentionally remains asserted for the device lifetime.
    core::mem::forget(board._lcd_read);
    Parts {
        display: board.screen,
        input: Input,
        input_observation: Input,
        ant: Ant,
        ble: Ble,
        positioning: Positioning,
        network: Network(stack),
        power: Power,
        power_control: Power,
        sensors: Sensors,
        sound: Sound,
        bulk: Bulk(board.bulk),
        storage: super::storage::Backend::new(board.flash),
        terminal: super::usb::Usb::new(board.usb),
        reset: board.reset,
        crash: board.crash,
    }
}

impl cycling_os::capabilities::InputObservation for Input {
    fn snapshot(&self, now_ms: u64) -> Option<cycling_os::capabilities::InputSnapshot> {
        super::services::io::snapshot(now_ms)
    }
}
pub struct Ble;
impl cycling_os::capabilities::Ble for Ble {
    fn availability(&self) -> cycling_os::capabilities::Availability {
        super::bluetooth::availability()
    }
    fn snapshot(&self) -> cycling_os::ble_transport::Snapshot {
        super::bluetooth::snapshot()
    }
    fn take_packet(&mut self) -> Option<cycling_os::ble_transport::Packet> {
        super::bluetooth::take_packet()
    }
    fn reconnect(&mut self) -> Result<(), cycling_os::capabilities::Error> {
        let _access = super::services::power::ACCESS.enter()?;
        super::bluetooth::request(cycling_os::ble_transport::Operation::Connect)
    }
    fn request(
        &mut self,
        operation: cycling_os::ble_transport::Operation,
    ) -> Result<(), cycling_os::capabilities::Error> {
        let _access = super::services::power::ACCESS.enter()?;
        super::bluetooth::request(operation)
    }
    fn control(&self) -> cycling_os::connectivity::ControlStatus {
        super::bluetooth::control()
    }
    fn discoveries(&self) -> [Option<cycling_os::ble_transport::Discovery>; 8] {
        super::bluetooth::discoveries()
    }
}
pub struct Ant;
impl cycling_os::capabilities::Ant for Ant {
    fn availability(&self) -> cycling_os::capabilities::Availability {
        cycling_os::capabilities::Availability::Ready
    }
    fn channels(
        &self,
        now_ms: u64,
    ) -> [Option<cycling_os::ant::Snapshot>; cycling_os::ant::CHANNEL_CAPACITY] {
        super::services::ant::snapshots(now_ms)
    }
    fn take_packet(&mut self) -> Option<cycling_os::ant::Packet> {
        super::services::ant::take_packet()
    }
    fn scanning(&self) -> bool {
        super::services::ant::scanning()
    }
    fn discoveries(&self) -> [Option<cycling_os::ant::Discovery>; 8] {
        super::services::ant::discoveries()
    }
    fn request(
        &mut self,
        operation: cycling_os::capabilities::AntOperation,
        now_ms: u64,
    ) -> &'static str {
        let Ok(_access) = super::services::power::ACCESS.enter() else {
            return "BUSY";
        };
        super::services::ant::request(operation, now_ms)
    }
}
pub struct Network(Option<embassy_net::Stack<'static>>);
impl cycling_os::capabilities::Network for Network {
    fn availability(&self) -> cycling_os::capabilities::Availability {
        if self.0.is_some() {
            cycling_os::capabilities::Availability::Ready
        } else if super::wifi::state() == 0 {
            cycling_os::capabilities::Availability::Unconfigured
        } else {
            cycling_os::capabilities::Availability::Failed
        }
    }
    fn stack(&self) -> Option<embassy_net::Stack<'static>> {
        self.0
    }
    fn online(&self) -> bool {
        super::wifi::online()
    }
    fn state(&self) -> u8 {
        super::wifi::state()
    }
    fn stats(&self) -> (u32, u32, u32, u8) {
        super::wifi::stats()
    }
    fn connection_generation(&self) -> u32 {
        super::wifi::connection_generation()
    }
    fn reconnect(&mut self) -> Result<(), cycling_os::capabilities::Error> {
        let _access = super::services::power::ACCESS.enter()?;
        super::wifi::request(cycling_os::connectivity::WifiOperation::Connect)
    }
    fn request(
        &mut self,
        operation: cycling_os::connectivity::WifiOperation,
    ) -> Result<(), cycling_os::capabilities::Error> {
        let _access = super::services::power::ACCESS.enter()?;
        super::wifi::request(operation)
    }
    fn control(&self) -> cycling_os::connectivity::ControlStatus {
        super::wifi::control()
    }
    fn discoveries(&self) -> [Option<cycling_os::connectivity::NetworkDiscovery>; 8] {
        super::wifi::discoveries()
    }
}

/// A missing medium remains explicitly unavailable. It never falls through to
/// the owned boot-flash journals.
pub struct Bulk(Option<sdmmc::Reader<'static>>);
#[cfg(feature = "bulk-maintenance")]
impl Bulk {
    pub(super) fn maintenance_reader(self) -> Option<sdmmc::Reader<'static>> {
        self.0
    }
}
impl cycling_os::bulk::Read for Bulk {
    fn info(&self) -> Result<cycling_os::bulk::Info, cycling_os::bulk::Error> {
        self.0
            .as_ref()
            .ok_or(cycling_os::bulk::Error::Unavailable)?
            .info()
    }
    fn read(&mut self, sector: u64, output: &mut [u8; 512]) -> Result<(), cycling_os::bulk::Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| cycling_os::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(cycling_os::bulk::Error::Unavailable)?
            .read(sector, output)
    }
    fn clock(&mut self, hz: u32) -> Result<(), cycling_os::bulk::Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| cycling_os::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(cycling_os::bulk::Error::Unavailable)?
            .clock(hz)
    }
    fn recover(&mut self) -> Result<(), cycling_os::bulk::Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| cycling_os::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(cycling_os::bulk::Error::Unavailable)?
            .recover()
    }
}

impl cycling_os::bulk::ReadWrite for Bulk {
    fn owned_info(&mut self) -> Result<cycling_os::bulk::OwnedInfo, cycling_os::bulk::Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| cycling_os::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(cycling_os::bulk::Error::Unavailable)?
            .owned_info()
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; 512],
    ) -> Result<(), cycling_os::bulk::Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| cycling_os::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(cycling_os::bulk::Error::Unavailable)?
            .owned_read(sector, output)
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; 512],
    ) -> Result<(), cycling_os::bulk::Error> {
        let _access = super::services::power::ACCESS
            .enter()
            .map_err(|_| cycling_os::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(cycling_os::bulk::Error::Unavailable)?
            .owned_write(sector, input)
    }
}

pub struct Sound;
impl cycling_os::sound::Sound for Sound {
    fn patterns(&self) -> &'static [cycling_os::sound::Pattern] {
        super::sound_protocol::PATTERNS
    }
    fn snapshot(&self) -> cycling_os::sound::Snapshot {
        super::services::sound::snapshot()
    }
    fn play(&mut self, id: u8, now: u64) -> Result<(), cycling_os::capabilities::Error> {
        super::services::sound::play(id, now)
    }
    fn stop(&mut self, now: u64) -> Result<(), cycling_os::capabilities::Error> {
        super::services::sound::stop(now)
    }
}

pub struct Sensors;
impl cycling_os::capabilities::Sensors for Sensors {
    fn startup_status(&self) -> &'static str {
        super::services::sensors::startup_status()
    }
    fn startup_reason(&self) -> Option<u8> {
        super::services::sensors::startup_reason()
    }
    fn snapshot(&self, now: u64) -> cycling_os::companion_sensors::Snapshot {
        super::services::sensors::snapshot(now)
    }
    fn identity_status(&self) -> &'static str {
        super::services::sensors::query_status()
    }
    fn query_identity(&mut self, now: u64) -> Result<(), cycling_os::capabilities::Error> {
        super::services::sensors::query(now)
    }
}

impl cycling_os::position_control::Control for Positioning {
    fn control(&self) -> cycling_os::position_control::Snapshot {
        super::services::position_control::snapshot()
    }
    fn pause(&mut self, ms: u32, now: u64) -> Result<(), cycling_os::capabilities::Error> {
        super::services::position_control::pause(ms, now)
    }
    fn resume(&mut self, now: u64) -> Result<(), cycling_os::capabilities::Error> {
        super::services::position_control::resume(now)
    }
}

impl cycling_os::power::Control for Power {
    fn capabilities(&self) -> cycling_os::power::Capabilities {
        super::services::power::capabilities()
    }
    fn status(&self) -> cycling_os::power::Status {
        super::services::power::status()
    }
    fn request(
        &mut self,
        operation: cycling_os::power::Operation,
    ) -> Result<(), cycling_os::capabilities::Error> {
        super::services::power::request(operation, embassy_time::Instant::now().as_millis())
    }
    fn request_after(
        &mut self,
        operation: cycling_os::power::Operation,
        delay_ms: u32,
    ) -> Result<(), cycling_os::capabilities::Error> {
        super::services::power::request_after(
            operation,
            delay_ms,
            embassy_time::Instant::now().as_millis(),
        )
    }
}
