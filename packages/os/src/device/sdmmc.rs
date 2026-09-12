//! Bounded, read-only ESP32-S3 SD/MMC identification probe.
//!
//! The low-level host sequence is adapted from the Apache-2.0 OR MIT licensed
//! `esp-hal` 1.2.1 SDMMC driver. The project remains pinned to HAL 1.1.2, so
//! this module uses the already-matched ESP32-S3 PAC directly while retaining
//! ownership of the HAL peripheral and recovered GPIO tokens.

use core::sync::atomic::{Ordering, compiler_fence};

use cycling_os::sdmmc_probe::{
    ProtocolError, check_app_command, check_r1, check_r6, response_u128, sd_sector_count,
};

use esp_hal::{
    gpio::{DriveMode, Flex, InputConfig, InputSignal, OutputConfig, OutputSignal, Pin, Pull},
    peripherals::SDHOST,
    time::{Duration, Instant},
};

const SLOT: u8 = 1;
const COMMAND_TIMEOUT: Duration = Duration::from_millis(150);
const BUSY_TIMEOUT: Duration = Duration::from_millis(500);
const INIT_TIMEOUT: Duration = Duration::from_millis(1_200);

const EVT_RESP_ERR: u32 = 1 << 1;
const EVT_CMD_DONE: u32 = 1 << 2;
const EVT_DATA_OVER: u32 = 1 << 3;
const EVT_RCRC: u32 = 1 << 6;
const EVT_DCRC: u32 = 1 << 7;
const EVT_RTO: u32 = 1 << 8;
const EVT_DTO: u32 = 1 << 9;
const EVT_HTO: u32 = 1 << 10;
const EVT_FRUN: u32 = 1 << 11;
const EVT_HLE: u32 = 1 << 12;
const EVT_SBE: u32 = 1 << 13;
const EVT_EBE: u32 = 1 << 15;
const CTRL_DMA_ENABLE: u32 = 1 << 5;
const CTRL_USE_INTERNAL_DMA: u32 = 1 << 25;
const DESC_LAST: u32 = 1 << 2;
const DESC_FIRST: u32 = 1 << 3;
const DESC_CHAINED: u32 = 1 << 4;
const DESC_OWN: u32 = 1 << 31;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Sd,
    Mmc,
}

impl Kind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Sd => "sd",
            Self::Mmc => "mmc",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    ResponseTimeout,
    ResponseCrc,
    Response,
    HardwareLocked,
    DataTimeout,
    DataCrc,
    Fifo,
    StartBit,
    Dma,
    Protocol(ProtocolError),
    InterfaceCondition,
    InitializationTimeout,
}

impl From<ProtocolError> for Error {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Report {
    pub kind: Kind,
    pub high_capacity: bool,
    pub rca: u16,
    pub cid: [u32; 4],
    pub csd: [u32; 4],
    pub sectors: u64,
    pub sector_size: u16,
    pub product: [u8; 6],
    pub reads: u8,
    pub repeated_equal: bool,
    pub first_hash: u32,
    pub second_hash: u32,
    pub last_hash: u32,
}

#[derive(Clone, Copy)]
enum ResponseLen {
    None,
    Short,
    Long,
}

#[repr(C, align(4))]
struct Descriptor {
    flags: u32,
    sizes: u32,
    buffer: u32,
    next: u32,
}

#[repr(C, align(4))]
struct Sector([u8; 512]);

struct Host<'d> {
    _peripheral: SDHOST<'d>,
}

struct Pins<'d> {
    clk: Flex<'d>,
    cmd: Flex<'d>,
    data0: Flex<'d>,
    data1: Flex<'d>,
    data2: Flex<'d>,
    data3: Flex<'d>,
}

impl<'d> Host<'d> {
    fn new(peripheral: SDHOST<'d>) -> Result<Self, Error> {
        let mut host = Self {
            _peripheral: peripheral,
        };
        let system = unsafe { &*esp32s3::SYSTEM::ptr() };
        system
            .perip_clk_en1()
            .modify(|_, w| w.sdio_host_clk_en().set_bit());
        system
            .perip_rst_en1()
            .modify(|_, w| w.sdio_host_rst().set_bit());
        system
            .perip_rst_en1()
            .modify(|_, w| w.sdio_host_rst().clear_bit());

        let r = registers();
        // 160 MHz PLL / 2 = 80 MHz host module clock, matching esp-hal 1.2.1.
        r.clk_edge_sel().write(|w| unsafe {
            w.cclkin_edge_drv_sel().bits(1);
            w.cclkin_edge_sam_sel().bits(0);
            w.cclkin_edge_slf_sel().bits(0);
            w.ccllkin_edge_h().bits(0);
            w.ccllkin_edge_l().bits(1);
            w.ccllkin_edge_n().bits(1);
            w.cclk_en().set_bit()
        });
        r.ctrl().modify(|_, w| {
            w.controller_reset().set_bit();
            w.fifo_reset().set_bit();
            w.dma_reset().set_bit()
        });
        poll(COMMAND_TIMEOUT, || {
            let value = r.ctrl().read();
            !value.controller_reset().bit_is_set()
                && !value.fifo_reset().bit_is_set()
                && !value.dma_reset().bit_is_set()
        })?;
        r.tmout().write(|w| unsafe {
            w.response_timeout().bits(0xff);
            w.data_timeout().bits(0xff_ffff)
        });
        r.rintsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.idsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.intmask().write(|w| unsafe { w.bits(0) });
        r.idinten().write(|w| unsafe { w.bits(0) });
        r.ctrl().modify(|_, w| w.int_enable().clear_bit());

        host.set_bus_1bit_400khz()?;
        Ok(host)
    }

    fn set_bus_1bit_400khz(&mut self) -> Result<(), Error> {
        let r = registers();
        r.ctype().modify(|rd, w| unsafe {
            w.card_width4().bits(rd.card_width4().bits() & !(1 << SLOT));
            w.card_width8().bits(rd.card_width8().bits() & !(1 << SLOT))
        });
        r.clkena().modify(|rd, w| unsafe {
            w.cclk_enable().bits(rd.cclk_enable().bits() & !(1 << SLOT))
        });
        self.update_clock()?;
        r.clksrc().modify(|rd, w| unsafe {
            let mut value = rd.clksrc().bits() & !0b1100;
            value |= 1 << 2;
            w.clksrc().bits(value)
        });
        // card_clk = 80 MHz / (2 * 100) = 400 kHz.
        r.clkdiv()
            .modify(|_, w| unsafe { w.clk_divider1().bits(100) });
        r.clkena().modify(|rd, w| unsafe {
            w.cclk_enable().bits(rd.cclk_enable().bits() | (1 << SLOT));
            w.lp_enable().bits(rd.lp_enable().bits() | (1 << SLOT))
        });
        self.update_clock()
    }

    fn update_clock(&mut self) -> Result<(), Error> {
        let r = registers();
        r.cmdarg().write(|w| unsafe { w.bits(0) });
        r.cmd().write(|w| unsafe {
            w.update_clock_registers_only().set_bit();
            w.wait_prvdata_complete().set_bit();
            w.card_number().bits(SLOT);
            w.start_cmd().set_bit()
        });
        wait_command_accepted()
    }

    fn init_clocks(&mut self) -> Result<(), Error> {
        let r = registers();
        r.rintsts().write(|w| unsafe { w.bits(EVT_CMD_DONE) });
        r.cmdarg().write(|w| unsafe { w.bits(0) });
        r.cmd().write(|w| unsafe {
            w.send_initialization().set_bit();
            w.wait_prvdata_complete().set_bit();
            w.card_number().bits(SLOT);
            w.start_cmd().set_bit()
        });
        wait_command_accepted()?;
        poll(COMMAND_TIMEOUT, || {
            r.rintsts().read().bits() & EVT_CMD_DONE != 0
        })?;
        r.rintsts().write(|w| unsafe { w.bits(EVT_CMD_DONE) });
        Ok(())
    }

    fn command(
        &mut self,
        index: u8,
        argument: u32,
        response: ResponseLen,
        check_crc: bool,
        wait_complete: bool,
        busy: bool,
    ) -> Result<[u32; 4], Error> {
        let r = registers();
        let consume = EVT_CMD_DONE | EVT_RTO | EVT_RCRC | EVT_RESP_ERR | EVT_HLE;
        r.rintsts().write(|w| unsafe { w.bits(consume) });
        r.cmdarg().write(|w| unsafe { w.bits(argument) });
        r.cmd().write(|w| unsafe {
            w.index().bits(index);
            w.response_expect()
                .bit(!matches!(response, ResponseLen::None));
            w.response_length()
                .bit(matches!(response, ResponseLen::Long));
            w.check_response_crc().bit(check_crc);
            w.wait_prvdata_complete().bit(wait_complete);
            w.use_hole().set_bit();
            w.card_number().bits(SLOT);
            w.start_cmd().set_bit()
        });
        wait_command_accepted()?;

        let mut status = 0;
        poll(COMMAND_TIMEOUT, || {
            status = r.rintsts().read().bits();
            status & (EVT_CMD_DONE | EVT_RTO | EVT_RCRC | EVT_RESP_ERR) != 0
        })?;
        map_status(status)?;
        let result = read_response(response);
        r.rintsts().write(|w| unsafe { w.bits(consume) });
        if busy {
            poll(BUSY_TIMEOUT, || !r.status().read().data_busy().bit_is_set())?;
        }
        Ok(result)
    }

    fn read_sector(&mut self, argument: u32, sector: &mut Sector) -> Result<[u32; 4], Error> {
        self.read_data_command(17, argument, sector)
    }

    fn reset_card(&mut self) -> Result<(), Error> {
        self.command(0, 0, ResponseLen::None, false, false, false)?;
        Ok(())
    }
}

impl Drop for Host<'_> {
    fn drop(&mut self) {
        let r = registers();
        let _ = stop_dma();
        r.intmask().write(|w| unsafe { w.bits(0) });
        r.idinten().write(|w| unsafe { w.bits(0) });
        r.clkena().modify(|rd, w| unsafe {
            w.cclk_enable().bits(rd.cclk_enable().bits() & !(1 << SLOT));
            w.lp_enable().bits(rd.lp_enable().bits() & !(1 << SLOT))
        });
        let _ = self.update_clock();
        let system = unsafe { &*esp32s3::SYSTEM::ptr() };
        system
            .perip_rst_en1()
            .modify(|_, w| w.sdio_host_rst().set_bit());
        system
            .perip_clk_en1()
            .modify(|_, w| w.sdio_host_clk_en().clear_bit());
    }
}

pub fn probe<'d>(
    peripheral: SDHOST<'d>,
    clk: impl Pin + 'd,
    cmd: impl Pin + 'd,
    data0: impl Pin + 'd,
    data1: impl Pin + 'd,
    data2: impl Pin + 'd,
    data3: impl Pin + 'd,
) -> Result<Report, Error> {
    let mut pins = Pins {
        clk: Flex::new(clk),
        cmd: Flex::new(cmd),
        data0: Flex::new(data0),
        data1: configure_idle(data1),
        data2: configure_idle(data2),
        data3: configure_idle(data3),
    };
    pins.clk.set_high();
    pins.clk.apply_output_config(&OutputConfig::default());
    pins.clk.set_output_enable(true);
    OutputSignal::SDHOST_CCLK_OUT_2.connect_to(&pins.clk);
    configure_bidir(
        &mut pins.cmd,
        InputSignal::SDHOST_CCMD_IN_2,
        OutputSignal::SDHOST_CCMD_OUT_2,
    );
    configure_bidir(
        &mut pins.data0,
        InputSignal::SDHOST_CDATA_IN_20,
        OutputSignal::SDHOST_CDATA_OUT_20,
    );

    let mut host = Host::new(peripheral)?;
    esp_println::println!("CYCLING_SDMMC stage=host_ready");
    host.init_clocks()?;
    esp_println::println!("CYCLING_SDMMC stage=idle_clocks_sent");
    host.reset_card()?;
    esp_println::println!("CYCLING_SDMMC stage=cmd0_sent");
    let report = match identify_sd(&mut host)? {
        Some(report) => {
            esp_println::println!("CYCLING_SDMMC stage=sd_identified");
            Ok(report)
        }
        None => {
            esp_println::println!("CYCLING_SDMMC stage=sd_unsupported trying=mmc");
            host.reset_card()?;
            identify_mmc(&mut host)
        }
    };
    let _ = host.reset_card();
    report
}

impl Drop for Pins<'_> {
    fn drop(&mut self) {
        OutputSignal::SDHOST_CCLK_OUT_2.disconnect_from(&self.clk);
        OutputSignal::SDHOST_CCMD_OUT_2.disconnect_from(&self.cmd);
        OutputSignal::SDHOST_CDATA_OUT_20.disconnect_from(&self.data0);
        InputSignal::SDHOST_CCMD_IN_2.connect_to(&esp_hal::gpio::Level::Low);
        InputSignal::SDHOST_CDATA_IN_20.connect_to(&esp_hal::gpio::Level::Low);
        self.clk.set_output_enable(false);
        self.cmd.set_output_enable(false);
        self.cmd.set_input_enable(false);
        self.data0.set_output_enable(false);
        self.data0.set_input_enable(false);
        self.data1.set_input_enable(false);
        self.data2.set_input_enable(false);
        self.data3.set_input_enable(false);
    }
}

fn configure_bidir(pin: &mut Flex<'_>, input: InputSignal, output: OutputSignal) {
    pin.set_high();
    pin.apply_output_config(
        &OutputConfig::default()
            .with_pull(Pull::Up)
            .with_drive_mode(DriveMode::PushPull),
    );
    pin.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
    pin.set_output_enable(true);
    pin.set_input_enable(true);
    input.connect_to(pin);
    output.connect_to(pin);
}

fn configure_idle<'d>(pin: impl Pin + 'd) -> Flex<'d> {
    let mut pin = Flex::new(pin);
    pin.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
    pin.set_output_enable(false);
    pin.set_input_enable(true);
    pin
}

fn identify_sd(host: &mut Host<'_>) -> Result<Option<Report>, Error> {
    match host.command(8, 0x1aa, ResponseLen::Short, true, false, false) {
        Ok(interface) if interface[0] & 0xfff != 0x1aa => {
            return Err(Error::InterfaceCondition);
        }
        Ok(_) => esp_println::println!("CYCLING_SDMMC stage=sd_cmd8_ok"),
        Err(Error::ResponseTimeout) => {
            esp_println::println!("CYCLING_SDMMC stage=sd_cmd8_legacy_or_unsupported")
        }
        Err(error) => return Err(error),
    }
    let start = Instant::now();
    let first_app = match host.command(55, 0, ResponseLen::Short, true, false, false) {
        Ok(response) => response[0],
        Err(Error::ResponseTimeout) => return Ok(None),
        Err(error) => return Err(error),
    };
    if check_app_command(first_app).is_err() {
        return Ok(None);
    }
    esp_println::println!("CYCLING_SDMMC stage=sd_cmd55_ok");
    let mut value = match host.command(41, 0x40ff_8000, ResponseLen::Short, false, false, false) {
        Ok(response) => response[0],
        Err(Error::ResponseTimeout) => return Ok(None),
        Err(error) => return Err(error),
    };
    let ocr = loop {
        if value & (1 << 31) != 0 {
            break value;
        }
        if start.elapsed() >= INIT_TIMEOUT {
            return Err(Error::InitializationTimeout);
        }
        let app = host.command(55, 0, ResponseLen::Short, true, false, false)?[0];
        check_app_command(app)?;
        value = host.command(41, 0x40ff_8000, ResponseLen::Short, false, false, false)?[0];
    };
    let cid = host.command(2, 0, ResponseLen::Long, true, false, false)?;
    let rca_response = host.command(3, 0, ResponseLen::Short, true, false, false)?[0];
    let rca = check_r6(rca_response)?;
    let csd = host.command(
        9,
        u32::from(rca) << 16,
        ResponseLen::Long,
        true,
        false,
        false,
    )?;
    let sectors = sd_sector_count(csd)?;
    check_r1(
        host.command(
            7,
            u32::from(rca) << 16,
            ResponseLen::Short,
            true,
            true,
            true,
        )?[0],
    )?;
    let high_capacity = ocr & (1 << 30) != 0;
    if !high_capacity {
        check_r1(host.command(16, 512, ResponseLen::Short, true, true, false)?[0])?;
    }
    read_report(host, Kind::Sd, high_capacity, rca, cid, csd, sectors).map(Some)
}

fn identify_mmc(host: &mut Host<'_>) -> Result<Report, Error> {
    esp_println::println!("CYCLING_SDMMC stage=mmc_cmd1_start");
    let start = Instant::now();
    let ocr = loop {
        let value = host.command(1, 0x40ff_8080, ResponseLen::Short, false, false, false)?[0];
        if value & (1 << 31) != 0 {
            break value;
        }
        if start.elapsed() >= INIT_TIMEOUT {
            return Err(Error::InitializationTimeout);
        }
    };
    let cid = host.command(2, 0, ResponseLen::Long, true, false, false)?;
    let rca = 1u16;
    check_r1(
        host.command(
            3,
            u32::from(rca) << 16,
            ResponseLen::Short,
            true,
            false,
            false,
        )?[0],
    )?;
    let csd = host.command(
        9,
        u32::from(rca) << 16,
        ResponseLen::Long,
        true,
        false,
        false,
    )?;
    check_r1(
        host.command(
            7,
            u32::from(rca) << 16,
            ResponseLen::Short,
            true,
            true,
            true,
        )?[0],
    )?;

    // MMC CMD8 reads EXT_CSD. It is read-only; unlike a high-level acquire
    // helper this probe never issues CMD6 bus-width or partition writes.
    let mut ext_csd = Sector([0; 512]);
    host.read_sector_command(8, 0, &mut ext_csd)?;
    let sectors = u32::from_le_bytes(ext_csd.0[212..216].try_into().unwrap()) as u64;
    if sectors == 0 {
        return Err(ProtocolError::Capacity.into());
    }
    read_report(
        host,
        Kind::Mmc,
        ocr & (1 << 30) != 0,
        rca,
        cid,
        csd,
        sectors,
    )
}

impl Host<'_> {
    fn read_sector_command(
        &mut self,
        index: u8,
        argument: u32,
        sector: &mut Sector,
    ) -> Result<[u32; 4], Error> {
        if index == 17 {
            return self.read_sector(argument, sector);
        }
        // MMC EXT_CSD uses the same single-block data phase with CMD8.
        self.read_data_command(index, argument, sector)
    }

    fn read_data_command(
        &mut self,
        index: u8,
        argument: u32,
        sector: &mut Sector,
    ) -> Result<[u32; 4], Error> {
        let r = registers();
        stop_dma()?;
        r.ctrl().modify(|_, w| w.fifo_reset().set_bit());
        poll(COMMAND_TIMEOUT, || {
            !r.ctrl().read().fifo_reset().bit_is_set()
        })?;
        r.rintsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.idsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.blksiz().write(|w| unsafe { w.bits(512) });
        r.bytcnt().write(|w| unsafe { w.bits(512) });
        let mut descriptor = Descriptor {
            flags: DESC_OWN | DESC_CHAINED | DESC_FIRST | DESC_LAST,
            sizes: 512,
            buffer: sector.0.as_mut_ptr() as u32,
            next: 0,
        };
        compiler_fence(Ordering::SeqCst);
        enable_dma(&mut descriptor as *mut Descriptor as u32);
        r.pldmnd().write(|w| unsafe { w.bits(1) });
        let result = (|| {
            r.cmdarg().write(|w| unsafe { w.bits(argument) });
            r.cmd().write(|w| unsafe {
                w.index().bits(index);
                w.response_expect().set_bit();
                w.check_response_crc().set_bit();
                w.data_expected().set_bit();
                w.read_write().clear_bit();
                w.wait_prvdata_complete().set_bit();
                w.use_hole().set_bit();
                w.card_number().bits(SLOT);
                w.start_cmd().set_bit()
            });
            wait_command_accepted()?;
            let mut status = 0;
            poll(BUSY_TIMEOUT, || {
                status = r.rintsts().read().bits();
                let idmac = r.idsts().read();
                status
                    & (EVT_DATA_OVER
                        | EVT_RTO
                        | EVT_RCRC
                        | EVT_RESP_ERR
                        | EVT_DCRC
                        | EVT_DTO
                        | EVT_HTO
                        | EVT_SBE
                        | EVT_EBE
                        | EVT_FRUN)
                    != 0
                    || idmac.fbe().bit_is_set()
                    || idmac.du().bit_is_set()
            })?;
            if r.idsts().read().fbe().bit_is_set() || r.idsts().read().du().bit_is_set() {
                return Err(Error::Dma);
            }
            map_status(status)?;
            if status & EVT_DATA_OVER == 0 {
                return Err(Error::DataTimeout);
            }
            let response = read_response(ResponseLen::Short);
            check_r1(response[0])?;
            Ok(response)
        })();
        let cleanup = stop_dma();
        compiler_fence(Ordering::SeqCst);
        r.rintsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.idsts().write(|w| unsafe { w.bits(u32::MAX) });
        cleanup?;
        result
    }
}

fn read_report(
    host: &mut Host<'_>,
    kind: Kind,
    high_capacity: bool,
    rca: u16,
    cid: [u32; 4],
    csd: [u32; 4],
    sectors: u64,
) -> Result<Report, Error> {
    if sectors == 0 || sectors > u64::from(u32::MAX) {
        return Err(ProtocolError::Capacity.into());
    }
    let mut first = Sector([0; 512]);
    let mut repeated = Sector([0; 512]);
    let mut last = Sector([0; 512]);
    let address = |sector: u64| -> Result<u32, Error> {
        let value = if high_capacity {
            sector
        } else {
            sector.checked_mul(512).ok_or(ProtocolError::Capacity)?
        };
        value
            .try_into()
            .map_err(|_| Error::Protocol(ProtocolError::Capacity))
    };
    host.read_sector(address(0)?, &mut first)?;
    host.read_sector(address(0)?, &mut repeated)?;
    host.read_sector(address(sectors - 1)?, &mut last)?;
    let product = product_name(kind, cid);
    Ok(Report {
        kind,
        high_capacity,
        rca,
        cid,
        csd,
        sectors,
        sector_size: 512,
        product,
        reads: 3,
        repeated_equal: first.0 == repeated.0,
        first_hash: hash(&first.0),
        second_hash: hash(&repeated.0),
        last_hash: hash(&last.0),
    })
}

fn product_name(kind: Kind, cid: [u32; 4]) -> [u8; 6] {
    let bytes = response_u128(cid).to_be_bytes();
    let mut product = [b' '; 6];
    match kind {
        Kind::Sd => product[..5].copy_from_slice(&bytes[3..8]),
        Kind::Mmc => product.copy_from_slice(&bytes[3..9]),
    }
    for byte in &mut product {
        if !byte.is_ascii_graphic() {
            *byte = b'?';
        }
    }
    product
}

fn hash(bytes: &[u8]) -> u32 {
    let mut value = 0x811c_9dc5u32;
    for &byte in bytes {
        value ^= u32::from(byte);
        value = value.wrapping_mul(0x0100_0193);
    }
    value
}

fn registers() -> &'static esp32s3::sdhost::RegisterBlock {
    unsafe { &*esp32s3::SDHOST::ptr() }
}

fn poll(timeout: Duration, mut condition: impl FnMut() -> bool) -> Result<(), Error> {
    let start = Instant::now();
    while !condition() {
        if start.elapsed() >= timeout {
            return Err(Error::Timeout);
        }
    }
    Ok(())
}

fn wait_command_accepted() -> Result<(), Error> {
    let r = registers();
    let start = Instant::now();
    while r.cmd().read().start_cmd().bit_is_set() {
        if r.rintsts().read().bits() & EVT_HLE != 0 {
            r.rintsts().write(|w| unsafe { w.bits(EVT_HLE) });
            return Err(Error::HardwareLocked);
        }
        if start.elapsed() >= COMMAND_TIMEOUT {
            return Err(Error::Timeout);
        }
    }
    Ok(())
}

fn map_status(status: u32) -> Result<(), Error> {
    if status & EVT_HLE != 0 {
        Err(Error::HardwareLocked)
    } else if status & EVT_RTO != 0 {
        Err(Error::ResponseTimeout)
    } else if status & EVT_RCRC != 0 {
        Err(Error::ResponseCrc)
    } else if status & EVT_RESP_ERR != 0 {
        Err(Error::Response)
    } else if status & (EVT_DTO | EVT_HTO) != 0 {
        Err(Error::DataTimeout)
    } else if status & (EVT_DCRC | EVT_EBE) != 0 {
        Err(Error::DataCrc)
    } else if status & EVT_SBE != 0 {
        Err(Error::StartBit)
    } else if status & EVT_FRUN != 0 {
        Err(Error::Fifo)
    } else {
        Ok(())
    }
}

fn read_response(response: ResponseLen) -> [u32; 4] {
    let r = registers();
    match response {
        ResponseLen::None => [0; 4],
        ResponseLen::Short => [r.resp0().read().bits(), 0, 0, 0],
        ResponseLen::Long => [
            r.resp0().read().bits(),
            r.resp1().read().bits(),
            r.resp2().read().bits(),
            r.resp3().read().bits(),
        ],
    }
}

fn enable_dma(descriptor: u32) {
    let r = registers();
    r.ctrl()
        .modify(|rd, w| unsafe { w.bits(rd.bits() | CTRL_DMA_ENABLE | CTRL_USE_INTERNAL_DMA) });
    r.bmod().modify(|_, w| w.swr().set_bit());
    r.idinten().write(|w| unsafe { w.bits(0) });
    r.dbaddr().write(|w| unsafe { w.bits(descriptor) });
    r.bmod().modify(|_, w| {
        w.de().set_bit();
        w.fb().set_bit()
    });
}

fn stop_dma() -> Result<(), Error> {
    let r = registers();
    r.idinten().write(|w| unsafe { w.bits(0) });
    r.bmod().modify(|_, w| {
        w.de().clear_bit();
        w.fb().clear_bit()
    });
    r.ctrl()
        .modify(|rd, w| unsafe { w.bits(rd.bits() & !CTRL_USE_INTERNAL_DMA) });
    r.ctrl().modify(|_, w| w.dma_reset().set_bit());
    match poll(COMMAND_TIMEOUT, || {
        !r.ctrl().read().dma_reset().bit_is_set()
    }) {
        Ok(()) => Ok(()),
        Err(error) => {
            // A module reset is the final fail-closed barrier before a
            // stack-backed descriptor or buffer can leave scope.
            unsafe { &*esp32s3::SYSTEM::ptr() }
                .perip_rst_en1()
                .modify(|_, w| w.sdio_host_rst().set_bit());
            Err(error)
        }
    }
}
