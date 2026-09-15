//! Bounded ESP32-S3 SD/MMC backend with explicit raw maintenance writes.
//!
//! The low-level host sequence is adapted from the Apache-2.0 OR MIT licensed
//! `esp-hal` 1.2.1 SDMMC driver. The project remains pinned to HAL 1.1.2, so
//! this module uses the already-matched ESP32-S3 PAC directly while retaining
//! ownership of the HAL peripheral and recovered GPIO tokens.

use core::sync::atomic::Ordering;
use core::sync::atomic::compiler_fence;

use crate::drivers::sdmmc_probe::ProtocolError;
use crate::drivers::sdmmc_probe::check_app_command;
use crate::drivers::sdmmc_probe::check_r1;
use crate::drivers::sdmmc_probe::check_r6;
use crate::drivers::sdmmc_probe::sd_sector_count;

use esp_hal::gpio::DriveMode;
use esp_hal::gpio::Flex;
use esp_hal::gpio::InputConfig;
use esp_hal::gpio::InputSignal;
use esp_hal::gpio::OutputConfig;
use esp_hal::gpio::OutputSignal;
use esp_hal::gpio::Pin;
use esp_hal::gpio::Pull;
use esp_hal::peripherals::SDHOST;
use esp_hal::time::Duration;
use esp_hal::time::Instant;

const SLOT: u8 = 1;
/// Maximum contiguous raw maintenance read (one IDMAC descriptor).
pub const MAX_READ_SECTORS: usize = 7;
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
    Readback,
}

impl From<ProtocolError> for Error {
    fn from(value: ProtocolError) -> Self {
        Self::Protocol(value)
    }
}

#[derive(Clone, Copy, Debug)]
struct Report {
    kind: Kind,
    high_capacity: bool,
    rca: u16,
    cache_enabled: bool,
    cid: [u32; 4],
    sectors: u64,
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

#[cfg(feature = "bulk-maintenance")]
#[repr(C, align(4))]
struct Blocks([u8; MAX_READ_SECTORS * 512]);

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
        host.initialize()?;
        Ok(host)
    }

    fn initialize(&mut self) -> Result<(), Error> {
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

        self.set_bus_1bit_400khz()
    }

    fn set_bus_1bit_400khz(&mut self) -> Result<(), Error> {
        self.set_bus_width(false);
        self.set_clock(100)
    }

    fn set_bus_width(&mut self, four: bool) {
        registers().ctype().modify(|rd, w| unsafe {
            let width4 = rd.card_width4().bits() & !(1 << SLOT);
            w.card_width4()
                .bits(width4 | if four { 1 << SLOT } else { 0 });
            w.card_width8().bits(rd.card_width8().bits() & !(1 << SLOT))
        });
    }

    fn set_clock(&mut self, divider: u8) -> Result<(), Error> {
        let r = registers();
        r.clkena().modify(|rd, w| unsafe {
            w.cclk_enable().bits(rd.cclk_enable().bits() & !(1 << SLOT))
        });
        self.update_clock()?;
        r.clksrc().modify(|rd, w| unsafe {
            let mut value = rd.clksrc().bits() & !0b1100;
            value |= 1 << 2;
            w.clksrc().bits(value)
        });
        // card_clk = 80 MHz / (2 * divider). No card register changes.
        r.clkdiv()
            .modify(|_, w| unsafe { w.clk_divider1().bits(divider) });
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
        // RTO can precede the controller's command-done/turnaround phase.
        // Wait for idle before clearing status and starting a new command.
        poll(COMMAND_TIMEOUT, || {
            r.status().read().command_fsm_states().bits() == 0
        })?;
        let consume = EVT_CMD_DONE | EVT_RTO | EVT_RCRC | EVT_RESP_ERR | EVT_HLE;
        r.rintsts().write(|w| unsafe { w.bits(consume) });
        r.cmdarg().write(|w| unsafe { w.bits(argument) });
        r.cmd().write(|w| unsafe {
            w.index().bits(index);
            w.stop_abort_cmd().bit(index == 12);
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
        self.data_command(17, argument, &mut sector.0, false)
    }

    fn reset_card(&mut self) -> Result<(), Error> {
        self.command(0, 0, ResponseLen::None, false, false, false)?;
        // ESP-IDF sdmmc_send_cmd_go_idle_state allows 20ms after CMD0.
        // Do not rely on boot diagnostic USB output to supply reset settling.
        esp_hal::delay::Delay::new().delay_millis(20);
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

pub fn open<'d>(
    peripheral: SDHOST<'d>,
    clk: impl Pin + 'd,
    cmd: impl Pin + 'd,
    data0: impl Pin + 'd,
    data1: impl Pin + 'd,
    data2: impl Pin + 'd,
    data3: impl Pin + 'd,
) -> Result<Reader<'d>, Error> {
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
    log::info!(target: "mmc", "CYCLING_SDMMC stage=host_ready");
    host.init_clocks()?;
    log::info!(target: "mmc", "CYCLING_SDMMC stage=idle_clocks_sent");
    host.reset_card()?;
    log::info!(target: "mmc", "CYCLING_SDMMC stage=cmd0_sent");
    let report = match identify_sd(&mut host)? {
        Some(report) => {
            log::info!(target: "mmc", "CYCLING_SDMMC stage=sd_identified");
            Ok(report)
        }
        None => {
            log::info!(target: "mmc", "CYCLING_SDMMC stage=sd_unsupported trying=mmc");
            host.reset_card()?;
            identify_mmc(&mut host)
        }
    };
    let report = report?;
    Ok(Reader {
        host,
        _pins: pins,
        report,
        failed: false,
        reads: 0,
        failures: 0,
        last_read_us: 0,
        clock_hz: 400_000,
        bus_width: 1,
        owned_binding: None,
    })
}

/// Host and pin tokens remain owned for the complete media lifetime. All DMA
/// buffers are stack-allocated internal RAM, never caller-owned external RAM.
pub struct Reader<'d> {
    host: Host<'d>,
    _pins: Pins<'d>,
    report: Report,
    failed: bool,
    reads: u32,
    failures: u32,
    last_read_us: u64,
    clock_hz: u32,
    bus_width: u8,
    owned_binding: Option<u32>,
}
impl Reader<'_> {
    /// Explicit maintenance-only MMC SDR four-bit mode. CMD6 selects volatile
    /// POWER_CLASS and BUS_WIDTH; clock rate and persistent settings stay
    /// unchanged. Failure locks media access until CMD0-based recovery.
    #[cfg(feature = "bulk-maintenance")]
    pub fn wide_read_mode(&mut self) -> Result<(), device_api::bulk::Error> {
        if self.failed {
            return Err(device_api::bulk::Error::Failed);
        }
        if self.report.kind != Kind::Mmc {
            return Err(device_api::bulk::Error::Unsupported);
        }
        if self.bus_width == 4 {
            return Ok(());
        }
        let mut before = Sector([0; 512]);
        let mut ext_csd = Sector([0; 512]);
        let result = (|| {
            self.ready()?;
            self.host.read_sector(0, &mut before)?;
            self.host.read_sector_command(8, 0, &mut ext_csd)?;
            Ok(())
        })();
        self.complete(result)?;
        // ESP-IDF v5.5 sdmmc_init_mmc_read_ext_csd selects the low nibble of
        // PWR_CL_26_360 for four-bit SDR at <=26MHz and the recovered 3.3V rail.
        // POWER_CLASS is R/W/E_P: CMD0 resets it, unlike persistent boot settings.
        let power_class = ext_csd.0[203] & 0x0f;
        let previous_class = ext_csd.0[187];
        let identity = [
            ext_csd.0[192],
            ext_csd.0[194],
            ext_csd.0[196],
            ext_csd.0[231],
        ];
        let capacity = u32::from_le_bytes(ext_csd.0[212..216].try_into().unwrap());
        let result = (|| {
            if previous_class != power_class {
                // Set the card-advertised power class before widening the bus,
                // as sdmmc_init_mmc_bus_width does. It changes no voltage rail.
                let argument = 0x03bb_0001 | (u32::from(power_class) << 8);
                check_r1(
                    self.host
                        .command(6, argument, ResponseLen::Short, true, true, true)?[0],
                )?;
                self.ready()?;
                self.host.read_sector_command(8, 0, &mut ext_csd)?;
                if ext_csd.0[187] != power_class {
                    return Err(Error::Readback);
                }
            }
            // ESP-IDF v5.5 sdmmc_mmc_switch: WRITE_BYTE=3, BUS_WIDTH=183,
            // four-bit SDR=1, normal command set=1. BUS_WIDTH is write-only,
            // so validate actual data transport instead of reading that byte.
            check_r1(
                self.host
                    .command(6, 0x03b7_0101, ResponseLen::Short, true, true, true)?[0],
            )?;
            self.ready()?;
            self._pins.connect_four_bit();
            self.host.set_bus_width(true);
            self.host.read_sector_command(8, 0, &mut ext_csd)?;
            if ext_csd.0[187] != power_class
                || identity
                    != [
                        ext_csd.0[192],
                        ext_csd.0[194],
                        ext_csd.0[196],
                        ext_csd.0[231],
                    ]
                || capacity != u32::from_le_bytes(ext_csd.0[212..216].try_into().unwrap())
            {
                return Err(Error::Readback);
            }
            // A sector read exercises every data lane and is compared against
            // the one-bit read taken immediately before this volatile switch.
            self.host.read_sector(0, &mut ext_csd)?;
            if before.0 != ext_csd.0 {
                return Err(Error::Readback);
            }
            self.ready()
        })();
        self.complete(result)?;
        self.bus_width = 4;
        Ok(())
    }

    /// Bounded raw read. Callers may use external RAM; only the internal bounce
    /// buffer is exposed to DMA. No output is copied on an incomplete transfer.
    #[cfg(feature = "bulk-maintenance")]
    pub fn read_blocks(
        &mut self,
        start: u64,
        output: &mut [u8],
    ) -> Result<(), device_api::bulk::Error> {
        let address = self.range(start, output.len())?;
        let mut buffer = Blocks([0; MAX_READ_SECTORS * 512]);
        let started = Instant::now();
        let index = if output.len() == 512 { 17 } else { 18 };
        let result = (|| {
            if index == 18 && start + (output.len() / 512) as u64 == self.report.sectors {
                // Hardware showed open-ended CMD18 can read beyond the final
                // address before CMD12 reaches the card. Bound this edge with
                // CMD17 rather than issuing configuration/count commands.
                for (i, sector) in buffer.0[..output.len()].chunks_exact_mut(512).enumerate() {
                    let argument = firmware_services::bulk::address(
                        start + i as u64,
                        self.report.sectors,
                        self.report.high_capacity,
                    )
                    .map_err(|_| Error::Protocol(ProtocolError::Capacity))?;
                    self.host.data_command(17, argument, sector, false)?;
                }
            } else {
                self.host
                    .data_command(index, address, &mut buffer.0[..output.len()], false)?;
            }
            self.ready()
        })();
        self.last_read_us = started.elapsed().as_micros();
        self.complete(result)?;
        output.copy_from_slice(&buffer.0[..output.len()]);
        self.reads = self.reads.saturating_add((output.len() / 512) as u32);
        Ok(())
    }

    /// Explicit maintenance primitive: one CMD24, programming completion and
    /// CMD13 status, then byte-for-byte readback. Failure is ambiguous: never
    /// automatically retry a write. Recovery must precede further media access.
    #[cfg(feature = "bulk-maintenance")]
    pub fn write_sector_verified(
        &mut self,
        start: u64,
        input: &[u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        self.write_raw_verified(start, input)
    }

    fn write_raw_verified(
        &mut self,
        start: u64,
        input: &[u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        // An enabled eMMC cache needs an explicit persistent flush protocol.
        // Until that is supported, neither maintenance nor owned writes proceed.
        if self.report.cache_enabled {
            return Err(device_api::bulk::Error::Unsupported);
        }
        let address = self.range(start, 512)?;
        let mut buffer = Sector(*input);
        let result = (|| {
            self.host.data_command(24, address, &mut buffer.0, true)?;
            self.ready()?;
            self.host.read_sector(address, &mut buffer)?;
            if buffer.0 != *input {
                return Err(Error::Readback);
            }
            Ok(())
        })();
        self.complete(result)
    }

    fn validate_owned(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        use device_api::bulk::Read;
        let total = self.info()?.sectors;
        let mut mbr = [0; 512];
        self.read(0, &mut mbr)?;
        let layout = firmware_services::bulk::parse_mbr(&mbr, total)?;
        let mut marker = [0; 512];
        self.read(layout.custom_start, &mut marker)?;
        let info = firmware_services::bulk::parse_owned(&mbr, &marker, total)?;
        let binding = firmware_services::bulk::crc32(&mbr);
        if self
            .owned_binding
            .is_some_and(|previous| previous != binding)
        {
            self.failed = true;
            self.failures = self.failures.saturating_add(1);
            return Err(device_api::bulk::Error::Failed);
        }
        self.owned_binding = Some(binding);
        Ok(info)
    }

    fn range(&self, start: u64, bytes: usize) -> Result<u32, device_api::bulk::Error> {
        use device_api::bulk::Error;
        use firmware_services::bulk::address;
        if self.failed {
            return Err(Error::Failed);
        }
        if bytes == 0 || bytes % 512 != 0 || bytes > MAX_READ_SECTORS * 512 {
            return Err(Error::Range);
        }
        let last = start
            .checked_add((bytes / 512 - 1) as u64)
            .ok_or(Error::Range)?;
        address(last, self.report.sectors, self.report.high_capacity)?;
        address(start, self.report.sectors, self.report.high_capacity)
    }

    fn complete(&mut self, result: Result<(), Error>) -> Result<(), device_api::bulk::Error> {
        result.map_err(|error| {
            self.failed = true;
            self.failures = self.failures.saturating_add(1);
            log::warn!(target: "mmc", "maintenance transfer failed error={:?}; recover required", error);
            device_api::bulk::Error::Failed
        })
    }

    fn ready(&mut self) -> Result<(), Error> {
        poll(BUSY_TIMEOUT, || {
            !registers().status().read().data_busy().bit_is_set()
        })?;
        let started = Instant::now();
        loop {
            let status = self.host.command(
                13,
                u32::from(self.report.rca) << 16,
                ResponseLen::Short,
                true,
                false,
                false,
            )?[0];
            check_r1(status)?;
            // READY_FOR_DATA and CURRENT_STATE == TRAN, not merely DATA_OVER.
            if status & (1 << 8) != 0 && (status >> 9) & 15 == 4 {
                return Ok(());
            }
            if started.elapsed() >= BUSY_TIMEOUT {
                return Err(Error::Timeout);
            }
        }
    }
}

impl device_api::bulk::ReadWrite for Reader<'_> {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        if self.report.cache_enabled {
            return Err(device_api::bulk::Error::Unsupported);
        }
        self.validate_owned()
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        let info = self.validate_owned()?;
        device_api::bulk::Read::read(self, info.address(sector)?, output)
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; 512],
    ) -> Result<(), device_api::bulk::Error> {
        // Re-read both ownership sectors immediately before every write. There
        // is one host owner and no await between validation and CMD24.
        let info = self.validate_owned()?;
        self.write_raw_verified(info.address(sector)?, input)
    }
}

impl device_api::bulk::Read for Reader<'_> {
    fn info(&self) -> Result<device_api::bulk::Info, device_api::bulk::Error> {
        if self.failed {
            return Err(device_api::bulk::Error::Failed);
        }
        Ok(device_api::bulk::Info {
            sectors: self.report.sectors,
            sector_size: 512,
            clock_hz: self.clock_hz,
            bus_width: self.bus_width,
            reads: self.reads,
            failures: self.failures,
            last_read_us: self.last_read_us,
        })
    }
    fn read(&mut self, sector: u64, output: &mut [u8; 512]) -> Result<(), device_api::bulk::Error> {
        let address = firmware_services::bulk::address(
            sector,
            self.report.sectors,
            self.report.high_capacity,
        )?;
        if self.failed {
            return Err(device_api::bulk::Error::Failed);
        }
        let mut buffer = Sector([0; 512]);
        let started = Instant::now();
        let result = self.host.read_sector(address, &mut buffer);
        self.last_read_us = started.elapsed().as_micros();
        if let Err(error) = result {
            self.failed = true;
            self.failures = self.failures.saturating_add(1);
            log::warn!(target: "mmc", "read failed error={:?}; recover before another read", error);
            return Err(device_api::bulk::Error::Failed);
        }
        output.copy_from_slice(&buffer.0);
        self.reads = self.reads.saturating_add(1);
        Ok(())
    }
    fn clock(&mut self, hz: u32) -> Result<(), device_api::bulk::Error> {
        let divider = match hz {
            400_000 => 100,
            4_000_000 => 10,
            20_000_000 => 2,
            _ => return Err(device_api::bulk::Error::Range),
        };
        if self.failed {
            return Err(device_api::bulk::Error::Failed);
        }
        if self.host.set_clock(divider).is_err() {
            self.failed = true;
            self.failures = self.failures.saturating_add(1);
            return Err(device_api::bulk::Error::Failed);
        }
        self.clock_hz = hz;
        Ok(())
    }
    fn recover(&mut self) -> Result<(), device_api::bulk::Error> {
        self.failed = true;
        // CMD0 uses CMD only and resets the card's volatile bus width. Release
        // extra data outputs even when a previous CMD6 completion was uncertain.
        self._pins.idle_extra_data();
        let result = (|| {
            self.host.initialize()?;
            self.host.init_clocks()?;
            self.host.reset_card()?;
            match self.report.kind {
                Kind::Mmc => identify_mmc(&mut self.host),
                Kind::Sd => identify_sd(&mut self.host)?.ok_or(Error::InterfaceCondition),
            }
        })();
        let report = result.map_err(|_| device_api::bulk::Error::Failed)?;
        // A changed medium cannot silently replace the one selected at boot.
        if report.cid != self.report.cid || report.sectors != self.report.sectors {
            return Err(device_api::bulk::Error::Failed);
        }
        self.report = report;
        self.clock_hz = 400_000;
        self.bus_width = 1;
        self.failed = false;
        Ok(())
    }
}

impl Pins<'_> {
    #[cfg(feature = "bulk-maintenance")]
    fn connect_four_bit(&mut self) {
        configure_bidir(
            &mut self.data1,
            InputSignal::SDHOST_CDATA_IN_21,
            OutputSignal::SDHOST_CDATA_OUT_21,
        );
        configure_bidir(
            &mut self.data2,
            InputSignal::SDHOST_CDATA_IN_22,
            OutputSignal::SDHOST_CDATA_OUT_22,
        );
        configure_bidir(
            &mut self.data3,
            InputSignal::SDHOST_CDATA_IN_23,
            OutputSignal::SDHOST_CDATA_OUT_23,
        );
    }

    fn idle_extra_data(&mut self) {
        for (pin, input, output) in [
            (
                &mut self.data1,
                InputSignal::SDHOST_CDATA_IN_21,
                OutputSignal::SDHOST_CDATA_OUT_21,
            ),
            (
                &mut self.data2,
                InputSignal::SDHOST_CDATA_IN_22,
                OutputSignal::SDHOST_CDATA_OUT_22,
            ),
            (
                &mut self.data3,
                InputSignal::SDHOST_CDATA_IN_23,
                OutputSignal::SDHOST_CDATA_OUT_23,
            ),
        ] {
            output.disconnect_from(pin);
            input.connect_to(&esp_hal::gpio::Level::High);
            pin.set_output_enable(false);
            pin.apply_input_config(&InputConfig::default().with_pull(Pull::Up));
            pin.set_input_enable(true);
        }
    }
}

impl Drop for Pins<'_> {
    fn drop(&mut self) {
        self.idle_extra_data();
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
        Ok(_) => log::info!(target: "mmc", "CYCLING_SDMMC stage=sd_cmd8_ok"),
        Err(Error::ResponseTimeout) => {
            log::info!(target: "mmc", "CYCLING_SDMMC stage=sd_cmd8_legacy_or_unsupported")
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
    log::info!(target: "mmc", "CYCLING_SDMMC stage=sd_cmd55_ok");
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
    read_report(host, Kind::Sd, high_capacity, rca, cid, csd, sectors, false).map(Some)
}

fn identify_mmc(host: &mut Host<'_>) -> Result<Report, Error> {
    log::info!(target: "mmc", "CYCLING_SDMMC stage=mmc_cmd1_start");
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
        ext_csd.0[33] != 0,
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
        self.data_command(index, argument, &mut sector.0, false)
    }

    fn data_command(
        &mut self,
        index: u8,
        argument: u32,
        buffer: &mut [u8],
        write: bool,
    ) -> Result<[u32; 4], Error> {
        let r = registers();
        // RTO can precede the controller's command-done/turnaround phase.
        // Wait for idle before clearing status and starting a new command.
        poll(COMMAND_TIMEOUT, || {
            r.status().read().command_fsm_states().bits() == 0
        })?;
        stop_dma()?;
        r.ctrl().modify(|_, w| w.fifo_reset().set_bit());
        poll(COMMAND_TIMEOUT, || {
            !r.ctrl().read().fifo_reset().bit_is_set()
        })?;
        r.rintsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.idsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.blksiz().write(|w| unsafe { w.bits(512) });
        r.bytcnt().write(|w| unsafe { w.bits(buffer.len() as u32) });
        let mut descriptor = Descriptor {
            flags: DESC_OWN | DESC_CHAINED | DESC_FIRST | DESC_LAST,
            sizes: buffer.len() as u32,
            buffer: buffer.as_mut_ptr() as u32,
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
                w.read_write().bit(write);
                w.wait_prvdata_complete().set_bit();
                w.use_hole().set_bit();
                w.card_number().bits(SLOT);
                w.start_cmd().set_bit()
            });
            wait_command_accepted()?;
            let mut status = 0;
            poll(COMMAND_TIMEOUT, || {
                status = r.rintsts().read().bits();
                status & (EVT_CMD_DONE | EVT_RTO | EVT_RCRC | EVT_RESP_ERR | EVT_HLE) != 0
            })?;
            map_status(status)?;
            let response = read_response(ResponseLen::Short);
            check_r1(response[0])?;
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
            // DATA_OVER is a card-side event. Wait for IDMAC to release the
            // descriptor before resetting DMA or exposing the received bytes.
            poll(COMMAND_TIMEOUT, || unsafe {
                core::ptr::read_volatile(core::ptr::addr_of!(descriptor.flags)) & DESC_OWN == 0
            })?;
            Ok(response)
        })();
        let cleanup = stop_dma();
        compiler_fence(Ordering::SeqCst);
        r.rintsts().write(|w| unsafe { w.bits(u32::MAX) });
        r.idsts().write(|w| unsafe { w.bits(u32::MAX) });
        cleanup?;
        // Open-ended CMD18 requires an explicit stop, even after a data error.
        // Stop never waits for the unfinished previous data phase.
        if index == 18 {
            let stopped = self
                .command(12, 0, ResponseLen::Short, true, false, true)
                .and_then(|response| check_r1(response[0]).map_err(Error::from));
            if result.is_ok() {
                stopped?;
            }
        }
        result
    }
}

fn read_report(
    host: &mut Host<'_>,
    kind: Kind,
    high_capacity: bool,
    rca: u16,
    cid: [u32; 4],
    _csd: [u32; 4],
    sectors: u64,
    cache_enabled: bool,
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
    if first.0 != repeated.0 {
        return Err(Error::Readback);
    }
    Ok(Report {
        kind,
        high_capacity,
        rca,
        cache_enabled,
        cid,
        sectors,
    })
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
