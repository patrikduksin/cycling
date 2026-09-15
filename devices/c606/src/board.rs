//! Verified C606 wiring and one-time resource construction.
//!
//! HAL initialization claims the singleton peripheral set. Consuming its tokens
//! assigns LCD DMA_CH0, GNSS UHCI0/DMA_CH1, and companion UART2 interrupts once.
//! MMC remains opt-in read-only; D1-D3 are reserved candidates, not verified lanes.

use crate::drivers::companion_uart;
use crate::drivers::crash_rtc;
use crate::drivers::display;
use crate::drivers::gps_uart;
use crate::drivers::psram;
use crate::drivers::sdmmc;
use crate::drivers::touch;
use esp_hal::clock::CpuClock;
use esp_hal::dma_tx_buffer;
use esp_hal::gpio::DriveMode;
use esp_hal::gpio::Level;
use esp_hal::gpio::Output;
use esp_hal::gpio::OutputConfig;
use esp_hal::lcd_cam::LcdCam;
use esp_hal::lcd_cam::lcd::i8080::Config;
use esp_hal::lcd_cam::lcd::i8080::I8080;
use esp_hal::ledc::LSGlobalClkSource;
use esp_hal::ledc::Ledc;
use esp_hal::ledc::LowSpeed;
use esp_hal::ledc::channel;
use esp_hal::ledc::channel::ChannelIFace;
use esp_hal::ledc::timer;
use esp_hal::ledc::timer::TimerIFace;
use esp_hal::time::Rate;
use esp_println::println;
use static_cell::StaticCell;

// LEDC channels borrow their timer. Keep that timer in permanent board-owned
// storage, so moving Resources cannot invalidate the channel's reference.
static BACKLIGHT_TIMER: StaticCell<timer::Timer<'static, LowSpeed>> = StaticCell::new();

struct Resources {
    pub reset: device_api::crash::Reset,
    pub crash: device_api::crash::Marker,
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
    crate::drivers::sleep::initialize(p.LPWR);
    let reset = crash_rtc::reset();
    let crash = crash_rtc::take();
    println!(
        "CYCLING_BOOT version={} board=magene-c606 harness={}",
        env!("CARGO_PKG_VERSION"),
        cfg!(feature = "debug-harness")
    );
    match crash {
        device_api::crash::Marker::Valid(report) => println!(
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
            use device_api::bulk::Read;
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
    crate::capabilities::sensors::startup_begin(embassy_time::Instant::now().as_millis());
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
impl device_api::input::InputSource for Input {
    fn controls(&self) -> device_api::input::Controls {
        use device_api::input::Button;
        use device_api::input::Controls;
        use device_api::observation::Availability;
        Controls {
            touch: match crate::capabilities::io::snapshot(embassy_time::Instant::now().as_millis())
            {
                Some(s) if s.touch_available => Availability::Ready,
                Some(_) => Availability::Failed,
                None => Availability::Initializing,
            },
            buttons: &[Button::TopLeft, Button::BottomLeft, Button::BottomRight],
        }
    }
    fn take_edge(&mut self) -> Option<device_api::input::Edge> {
        crate::capabilities::io::take_edge()
    }
}
pub struct Power;
impl device_api::power::Power for Power {
    fn availability(&self) -> device_api::observation::Availability {
        if crate::capabilities::power::light_available() {
            device_api::observation::Availability::Ready
        } else {
            device_api::observation::Availability::Failed
        }
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        use device_api::observation::Observation;
        crate::capabilities::io::snapshot(embassy_time::Instant::now().as_millis()).and_then(|s| {
            match s.battery {
                Observation::Fresh {
                    value: (percent, mv),
                    received_ms,
                }
                | Observation::Stale {
                    value: (percent, mv),
                    received_ms,
                } => Some((percent, mv, received_ms)),
                Observation::Unavailable => None,
            }
        })
    }
    fn brightness(&mut self, percent: u8) -> Result<(), device_api::observation::Error> {
        if percent > 100 {
            return Err(device_api::observation::Error::Invalid);
        }
        crate::capabilities::power::brightness(percent)
    }
}
pub struct Positioning;
impl device_api::positioning::Positioning for Positioning {
    fn availability(&self) -> device_api::observation::Availability {
        device_api::observation::Availability::Ready
    }
    fn snapshot(&self, now_ms: u64) -> Option<device_api::positioning::Snapshot> {
        crate::capabilities::positioning::snapshot(now_ms)
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
    pub storage: crate::drivers::storage::Backend<'static>,
    pub terminal: crate::drivers::usb::Usb,
    pub reset: device_api::crash::Reset,
    pub crash: device_api::crash::Marker,
}
pub async fn start(spawner: embassy_executor::Spawner) -> Parts {
    let board = init();
    spawner.spawn(crate::capabilities::power::run(board.backlight).unwrap());
    spawner.spawn(crate::capabilities::positioning::run(board.gps_receiver).unwrap());
    spawner.spawn(crate::capabilities::io::run(board.touch, board.touch_available).unwrap());
    let stack = crate::drivers::wifi::initialize(board.wifi, spawner).await;
    if let Some(stack) = stack {
        spawner.spawn(crate::drivers::wifi::time_sync(stack).unwrap());
    }
    spawner.spawn(crate::drivers::bluetooth::start(board.bluetooth).unwrap());
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
        storage: crate::drivers::storage::Backend::new(board.flash),
        terminal: crate::drivers::usb::Usb::new(board.usb),
        reset: board.reset,
        crash: board.crash,
    }
}

impl device_api::input::InputObservation for Input {
    fn snapshot(&self, now_ms: u64) -> Option<device_api::input::InputSnapshot> {
        crate::capabilities::io::snapshot(now_ms)
    }
}
pub struct Ble;
impl device_api::ble_transport::Ble for Ble {
    fn availability(&self) -> device_api::observation::Availability {
        crate::drivers::bluetooth::availability()
    }
    fn snapshot(&self) -> device_api::ble_transport::Snapshot {
        crate::drivers::bluetooth::snapshot()
    }
    fn take_packet(&mut self) -> Option<device_api::ble_transport::Packet> {
        crate::drivers::bluetooth::take_packet()
    }
    fn reconnect(&mut self) -> Result<(), device_api::observation::Error> {
        let _access = crate::capabilities::power::ACCESS.enter()?;
        crate::drivers::bluetooth::request(device_api::ble_transport::Operation::Connect)
    }
    fn request(
        &mut self,
        operation: device_api::ble_transport::Operation,
    ) -> Result<(), device_api::observation::Error> {
        let _access = crate::capabilities::power::ACCESS.enter()?;
        crate::drivers::bluetooth::request(operation)
    }
    fn control(&self) -> device_api::connectivity::ControlStatus {
        crate::drivers::bluetooth::control()
    }
    fn discoveries(&self) -> [Option<device_api::ble_transport::Discovery>; 8] {
        crate::drivers::bluetooth::discoveries()
    }
}
pub use crate::capabilities::ant::Ant;
pub struct Network(Option<embassy_net::Stack<'static>>);
impl device_api::network::Network for Network {
    fn availability(&self) -> device_api::observation::Availability {
        if self.0.is_some() {
            device_api::observation::Availability::Ready
        } else if crate::drivers::wifi::state() == 0 {
            device_api::observation::Availability::Unconfigured
        } else {
            device_api::observation::Availability::Failed
        }
    }
    fn stack(&self) -> Option<embassy_net::Stack<'static>> {
        self.0
    }
    fn online(&self) -> bool {
        crate::drivers::wifi::online()
    }
    fn state(&self) -> u8 {
        crate::drivers::wifi::state()
    }
    fn stats(&self) -> (u32, u32, u32, u8) {
        crate::drivers::wifi::stats()
    }
    fn connection_generation(&self) -> u32 {
        crate::drivers::wifi::connection_generation()
    }
    fn reconnect(&mut self) -> Result<(), device_api::observation::Error> {
        let _access = crate::capabilities::power::ACCESS.enter()?;
        crate::drivers::wifi::request(device_api::connectivity::WifiOperation::Connect)
    }
    fn request(
        &mut self,
        operation: device_api::connectivity::WifiOperation,
    ) -> Result<(), device_api::observation::Error> {
        let _access = crate::capabilities::power::ACCESS.enter()?;
        crate::drivers::wifi::request(operation)
    }
    fn control(&self) -> device_api::connectivity::ControlStatus {
        crate::drivers::wifi::control()
    }
    fn discoveries(&self) -> [Option<device_api::connectivity::NetworkDiscovery>; 8] {
        crate::drivers::wifi::discoveries()
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
impl device_api::bulk::Read for Bulk {
    fn info(&self) -> Result<device_api::bulk::Info, device_api::bulk::Error> {
        self.0
            .as_ref()
            .ok_or(device_api::bulk::Error::Unavailable)?
            .info()
    }
    fn read(&mut self, sector: u64, output: &mut [u8; 512]) -> Result<(), device_api::bulk::Error> {
        let _access = crate::capabilities::power::ACCESS
            .enter()
            .map_err(|_| device_api::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(device_api::bulk::Error::Unavailable)?
            .read(sector, output)
    }
    fn clock(&mut self, hz: u32) -> Result<(), device_api::bulk::Error> {
        let _access = crate::capabilities::power::ACCESS
            .enter()
            .map_err(|_| device_api::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(device_api::bulk::Error::Unavailable)?
            .clock(hz)
    }
    fn recover(&mut self) -> Result<(), device_api::bulk::Error> {
        let _access = crate::capabilities::power::ACCESS
            .enter()
            .map_err(|_| device_api::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(device_api::bulk::Error::Unavailable)?
            .recover()
    }
}

impl device_api::bulk::ReadWrite for Bulk {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        let _access = crate::capabilities::power::ACCESS
            .enter()
            .map_err(|_| device_api::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(device_api::bulk::Error::Unavailable)?
            .owned_info()
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        let _access = crate::capabilities::power::ACCESS
            .enter()
            .map_err(|_| device_api::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(device_api::bulk::Error::Unavailable)?
            .owned_read(sector, output)
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        let _access = crate::capabilities::power::ACCESS
            .enter()
            .map_err(|_| device_api::bulk::Error::Unavailable)?;
        self.0
            .as_mut()
            .ok_or(device_api::bulk::Error::Unavailable)?
            .owned_write(sector, input)
    }
}

pub struct Sound;
impl device_api::sound::Sound for Sound {
    fn patterns(&self) -> &'static [device_api::sound::Pattern] {
        crate::drivers::sound_protocol::PATTERNS
    }
    fn snapshot(&self) -> device_api::sound::Snapshot {
        crate::capabilities::sound::snapshot()
    }
    fn play(&mut self, id: u8, now: u64) -> Result<(), device_api::observation::Error> {
        crate::capabilities::sound::play(id, now)
    }
    fn stop(&mut self, now: u64) -> Result<(), device_api::observation::Error> {
        crate::capabilities::sound::stop(now)
    }
}

pub struct Sensors;
impl device_api::sensors::Sensors for Sensors {
    fn startup_status(&self) -> &'static str {
        crate::capabilities::sensors::startup_status()
    }
    fn startup_reason(&self) -> Option<u8> {
        crate::capabilities::sensors::startup_reason()
    }
    fn snapshot(&self, now: u64) -> device_api::sensors::Snapshot {
        crate::capabilities::sensors::snapshot(now)
    }
    fn identity_status(&self) -> &'static str {
        crate::capabilities::sensors::query_status()
    }
    fn query_identity(&mut self, now: u64) -> Result<(), device_api::observation::Error> {
        crate::capabilities::sensors::query(now)
    }
}

impl device_api::position_control::Control for Positioning {
    fn control(&self) -> device_api::position_control::Snapshot {
        crate::capabilities::position_control::snapshot()
    }
    fn pause(&mut self, ms: u32, now: u64) -> Result<(), device_api::observation::Error> {
        crate::capabilities::position_control::pause(ms, now)
    }
    fn resume(&mut self, now: u64) -> Result<(), device_api::observation::Error> {
        crate::capabilities::position_control::resume(now)
    }
}

impl device_api::power::Control for Power {
    fn capabilities(&self) -> device_api::power::Capabilities {
        crate::capabilities::power::capabilities()
    }
    fn status(&self) -> device_api::power::Status {
        crate::capabilities::power::status()
    }
    fn request(
        &mut self,
        operation: device_api::power::Operation,
    ) -> Result<(), device_api::observation::Error> {
        crate::capabilities::power::request(operation, embassy_time::Instant::now().as_millis())
    }
    fn request_after(
        &mut self,
        operation: device_api::power::Operation,
        delay_ms: u32,
    ) -> Result<(), device_api::observation::Error> {
        crate::capabilities::power::request_after(
            operation,
            delay_ms,
            embassy_time::Instant::now().as_millis(),
        )
    }
}
