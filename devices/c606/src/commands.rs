//! Single bounded USB owner for ordinary commands and JSON logs.
use c606_firmware as device;
use core::fmt;
use core::fmt::Write;
use device::drivers::wifi;
use firmware_console::commands::{Board, Capabilities, Card, Identity};
use firmware_console::protocol::Command;
use firmware_console::protocol::Request;
use firmware_console::session::{Reboot, Terminal};

const TEST_REBOOT: u8 = 11;
const TEST_REBOOT_UNSYNCED: u8 = 12;
const TEST_COMPOSE: u8 = 31;

struct Text {
    bytes: [u8; 1280],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self {
            bytes: [0; 1280],
            len: 0,
        }
    }
    fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("encoding_failed")
    }
}
impl Write for Text {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        if self.len + s.len() > self.bytes.len() {
            return Err(fmt::Error);
        }
        self.bytes[self.len..self.len + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len();
        Ok(())
    }
}
pub trait Diagnostics {
    fn board(&self) -> &'static str;
    /// Application slot that booted: `A`, `B` or `unknown`.
    fn slot(&self) -> &'static str;
    fn heap_free(&self) -> usize;
    fn external_free(&self) -> usize;
    fn restart(&self, panic: bool) -> !;
    fn set_fault(&self, fault: u8);
    fn stall(&self);
}
pub fn execute<C: device_api::console::Console>(
    terminal: &mut Terminal<C>,
    request: Request,
    system: &mut crate::FirmwareShell,
    now: u64,
    world: &mut crate::world::World,
    diagnostics: &impl Diagnostics,
    sdk: &mut vana::app::Runtime,
    trace: &mut crate::trace::Trace,
) {
    let crate::world::Console {
        ant,
        ble,
        position,
        bulk,
        sound,
        sensors,
        input,
        power,
    } = world.unrecorded_console();
    let started = embassy_time::Instant::now();
    let mut output = Text::new();
    // The radios' control status decides what a SELECT, FORGET or CONFIG
    // answers, so the world log holds what the command read of them, as
    // the simulator's session records it.
    let (log, observed) = (trace.log, trace.observed);
    let mut ble = device_api::ble_transport::BleRecorder::new(ble, log, &observed.ble);
    let mut wifi = device_api::connectivity::WifiRecorder::new(Station, log, &observed.wifi);
    let status = if matches!(
        request.command,
        Command::Test(TEST_REBOOT | TEST_REBOOT_UNSYNCED)
    ) && sdk.holds_unsynced()
    {
        "UNSYNCED"
    } else {
        firmware_console::commands::execute(
            request.command,
            now,
            Capabilities {
                ant,
                ble: &mut ble,
                wifi: &mut wifi,
                power,
                input,
            },
            system,
            sdk,
            &mut Composition {
                terminal,
                trace,
                bulk,
                sound,
                position,
                sensors,
                diagnostics,
            },
            &mut output,
        )
    };
    let elapsed = started.elapsed().as_millis();
    match request.command {
        Command::Save => system.storage_max_ms = system.storage_max_ms.max(elapsed),
        Command::Display(_) => system.display_max_ms = system.display_max_ms.max(elapsed),
        _ => {}
    }
    terminal.reply(request.id, status, output.as_str(), now);
    terminal.command_completed();
}

struct Station;

impl device_api::connectivity::Wifi for Station {
    fn request(
        &mut self,
        operation: device_api::connectivity::WifiOperation,
    ) -> Result<(), device_api::observation::Error> {
        wifi::request(operation)
    }
    fn control(&self) -> device_api::connectivity::ControlStatus {
        wifi::control()
    }
    fn discoveries(&self) -> [Option<device_api::connectivity::NetworkDiscovery>; 8] {
        wifi::discoveries()
    }
    fn state(&self) -> device_api::connectivity::WifiState {
        wifi::state()
    }
    fn online(&self) -> bool {
        wifi::online()
    }
    fn stats(&self) -> device_api::connectivity::WifiStats {
        wifi::stats()
    }
}

struct Composition<'a, C: device_api::console::Console, G> {
    terminal: &'a mut Terminal<C>,
    trace: &'a mut crate::trace::Trace,
    bulk: &'a mut device::board::Bulk,
    sound: &'a mut device::board::Sound,
    position: &'a mut device::board::Positioning,
    sensors: &'a mut device::board::Sensors,
    diagnostics: &'a G,
}

struct TracedCard<'a> {
    trace: &'a mut crate::trace::Trace,
    bulk: &'a mut device::board::Bulk,
}

impl Card for TracedCard<'_> {
    fn media(&mut self) -> impl device_api::bulk::ReadWrite + '_ {
        device_api::bulk::ReadWriteRecorder::new(
            &mut *self.bulk,
            self.trace.log,
            &self.trace.observed.bulk,
        )
    }
    fn store(&mut self) -> impl device_api::bulk::ReadWrite + '_ {
        &mut *self.bulk
    }
    fn trace_in_store(&self) -> bool {
        self.trace.open()
    }
    fn trace_status(&self, out: &mut dyn Write) {
        self.trace.status(out);
    }
    fn after_volume(&mut self, now: u64) {
        self.trace.after_host(&mut *self.bulk, false, now);
        let (log, observed) = (self.trace.log, self.trace.observed);
        let mut media =
            device_api::bulk::ReadWriteRecorder::new(&mut *self.bulk, log, &observed.bulk);
        self.trace.read_volumes(&mut media);
    }
    fn sync_trace(&mut self, now: u64) {
        self.trace.sync(&mut *self.bulk, now);
    }
}

impl<C: device_api::console::Console, G: Diagnostics> Board<crate::FirmwareShell>
    for Composition<'_, C, G>
{
    const HARDWARE: &'static str = ",MMC,SOUND,GNSS,PRESSURE,MOTION,COMPANION,FITTED,TEST";
    fn startup(&self) -> device_api::sensors::Startup {
        device_api::sensors::Sensors::startup_status(self.sensors)
    }
    fn card(&mut self) -> impl Card + '_ {
        TracedCard {
            trace: &mut *self.trace,
            bulk: &mut *self.bulk,
        }
    }
    fn transfer(&mut self) -> &'static str {
        self.terminal.schedule_transfer();
        "ACCEPTED"
    }
    fn restart(&mut self, now: u64) {
        self.terminal.schedule_reboot(now, Reboot::Restart);
    }
    fn identity(&mut self, recording: bool) -> Identity {
        crate::logging::metadata(recording);
        Identity {
            board: self.diagnostics.board(),
            slot: self.diagnostics.slot(),
            commit: option_env!("CYCLING_BUILD_COMMIT").unwrap_or("unknown"),
        }
    }
    fn status(&self, mut out: &mut dyn Write) {
        let (frame_us, wait_us, frame_max_us) = crate::device::drivers::display::TIMING.snapshot();
        let _ = write!(
            out,
            " display_frame_us={frame_us} display_wait_us={wait_us} display_frame_max_us={frame_max_us} stack_untouched={} ",
            crate::device::stack::untouched()
        );
        let _ = crate::device::drivers::sdmmc::HEALTH.write(&mut out);
        let logs = crate::logging::stats();
        let _ = write!(
            out,
            " heap_free={} psram_free={} terminal_rejected={} log_attempts={} log_total_us={} log_max_us={} log_lost={} log_depth={} log_buffer_bytes={} usb_poll_max_us={}",
            self.diagnostics.heap_free(),
            self.diagnostics.external_free(),
            self.terminal.rejected(),
            logs.0,
            logs.1,
            logs.2,
            logs.3,
            logs.4,
            logs.5,
            self.terminal.tx_max_us()
        );
    }
    fn clock(&self) -> firmware_services::network_time::Clock {
        firmware_services::network_time::clock()
    }
    fn hardware(
        &mut self,
        system: &mut crate::FirmwareShell,
        command: Command,
        now: u64,
        mut out: &mut dyn Write,
    ) -> &'static str {
        match command {
            Command::Peripheral(command) => {
                use firmware_console::commands::peripherals;
                let status = peripherals::execute(
                    command,
                    now,
                    &mut *self.bulk,
                    &mut *self.sound,
                    &mut *self.position,
                    &mut *self.sensors,
                    &mut out,
                );
                // The test wrote the card unseen by the trace.
                if let peripherals::Command::MmcOwnedTest { .. } = command {
                    self.trace.forget_card();
                    self.card().after_volume(now);
                }
                status
            }
            Command::Fitted => {
                fitted(system, out);
                "OK"
            }
            Command::Crash => crash(system, out),
            Command::CrashClear => {
                match crate::device::drivers::crash_rtc::clear(&mut crash_journal(system)) {
                    Ok(()) => "OK",
                    Err(device_api::storage::AccessError::Unavailable) => "UNAVAILABLE",
                    Err(_) => "FAILED",
                }
            }
            Command::Test(n) => match n {
                0..=4 => {
                    self.diagnostics.set_fault(n);
                    "ACCEPTED"
                }
                20 => {
                    log::warn!(target:"diagnostic","controlled executor stall ms=6000");
                    self.diagnostics.stall();
                    log::info!(target:"diagnostic","executor stall ended");
                    "ACCEPTED"
                }
                TEST_REBOOT => {
                    self.terminal.schedule_reboot(now, Reboot::Panic);
                    "ACCEPTED"
                }
                TEST_REBOOT_UNSYNCED => {
                    self.terminal.schedule_reboot(now, Reboot::PanicUnsynced);
                    "ACCEPTED"
                }
                10 => {
                    for _ in 0..64 {
                        log::warn!(target:"diagnostic","controlled log saturation");
                    }
                    "ACCEPTED"
                }
                TEST_COMPOSE => {
                    const AT: vana::screens::scaled::Scaled = vana::screens::scaled::Scaled::of(
                        device::fitted::PANEL_WIDTH,
                        device::fitted::PANEL_HEIGHT,
                    );
                    let now = || embassy_time::Instant::now().as_micros();
                    vana::compose::bench::report(&AT, &now, out);
                    "OK"
                }
                _ => "UNSUPPORTED",
            },
            _ => "UNSUPPORTED",
        }
    }
}

/// The panic journal, read past the shell's recorder: `CRASH` and `FITTED`
/// answer on this device only, so a replay asks its world for none of what
/// they read.
fn crash_journal(
    system: &mut crate::FirmwareShell,
) -> firmware_services::storage::RegionAccess<'_, crate::SharedFlash> {
    firmware_services::storage::RegionAccess::new(
        system.flash().unrecorded_mut(),
        device_api::storage::Region::Crash,
    )
}

fn crash(system: &mut crate::FirmwareShell, out: &mut dyn Write) -> &'static str {
    let record = match crate::device::drivers::crash_rtc::last(&mut crash_journal(system)) {
        Err(device_api::storage::AccessError::Unavailable) => return "UNAVAILABLE",
        Err(_) => return "FAILED",
        Ok(None) => {
            let _ = write!(out, "none");
            return "OK";
        }
        Ok(Some(record)) => record,
    };
    let (record, source) = record;
    let _ = write!(
        out,
        "kind={} this_boot={} source={} version={} commit={} elf={}{} uptime_ms={} location={}:{} frames={} pcs=",
        record.kind.name(),
        // Boot marks the record reported, so ask what this boot's
        // reset was blamed on.
        matches!(system.crash, device_api::crash::Marker::Valid(_)),
        source.name(),
        record.version(),
        if record.commit().is_empty() {
            "unknown"
        } else {
            record.commit()
        },
        // The archived ELF's name, for `cycling backtrace`.
        record.commit(),
        record.build.suffix(),
        record.uptime_ms,
        record.file(),
        record.line,
        record.pcs().len(),
    );
    for (index, pc) in record.pcs().iter().enumerate() {
        let _ = write!(out, "{}0x{pc:08x}", if index == 0 { "" } else { "," });
    }
    // Last, so a reader can take the rest of the line as it is.
    let _ = write!(out, " message={}", record.message());
    "OK"
}

/// The card for one transfer session. Its ownership is checked at the
/// session's first read instead of before every read, which reads two more
/// sectors each time; a write still checks it. The app is paused, so
/// nothing else uses the card meanwhile.
struct SessionCard<'a> {
    bulk: &'a mut device::board::Bulk,
    owned: Option<device_api::bulk::OwnedInfo>,
}

impl device_api::bulk::ReadWrite for SessionCard<'_> {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        if let Some(owned) = self.owned {
            return Ok(owned);
        }
        let owned = self.bulk.owned_info()?;
        self.owned = Some(owned);
        Ok(owned)
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; device_api::bulk::SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        let address = self.owned_info()?.address(sector)?;
        device_api::bulk::Read::read(self.bulk, address, output)
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; device_api::bulk::SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        self.bulk.owned_write(sector, input)
    }
    fn entropy(&mut self) -> u64 {
        self.bulk.entropy()
    }
}

/// Serve the binary volume transfer session on the terminal's USB until the
/// host ends it or stays silent for `IDLE_MS`. The app does not run meanwhile;
/// `VOLUME TRANSFER` refused while a ride records.
pub async fn transfer(
    usb: &mut device::drivers::usb::Usb,
    bulk: &mut device::board::Bulk,
    handover_us: u64,
) {
    use device_api::console::Console;
    use firmware_console::transfer::{self, HEADER, MAX_PAYLOAD, Session};
    use firmware_services::checksum::crc32;
    // The request and reply buffers (8 KiB) and the session, which keeps
    // STORE mounted between requests, live in external RAM for the session
    // only: held across awaits they would sit in the console task's static
    // future and take internal RAM from the main stack.
    struct Block(*mut u8);
    const SESSION: usize = 2 * MAX_PAYLOAD;
    const LAYOUT: core::alloc::Layout = match core::alloc::Layout::from_size_align(
        SESSION + core::mem::size_of::<Session>(),
        core::mem::align_of::<Session>(),
    ) {
        Ok(layout) => layout,
        Err(_) => panic!("transfer block layout"),
    };
    const _: () = assert!(SESSION.is_multiple_of(core::mem::align_of::<Session>()));
    const _: () = assert!(!core::mem::needs_drop::<Session>());
    impl Drop for Block {
        fn drop(&mut self) {
            // SAFETY: the block came from `allocate_external` with `LAYOUT`,
            // is freed only here, and no reference into it outlives `self`.
            unsafe { device::drivers::psram::deallocate_external(self.0, LAYOUT) };
        }
    }
    // SAFETY: `LAYOUT` has a non-zero size.
    let pointer = unsafe { device::drivers::psram::allocate_external(LAYOUT) };
    if pointer.is_null() {
        // No external RAM: the host times out and reports the session failed.
        return;
    }
    let block = Block(pointer);
    // SAFETY: the allocator returned `LAYOUT.size()` writable bytes aligned
    // for `Session` at `SESSION`, initialized here before any reference
    // exists; `block` frees them only after `payload`, `out` and `session`
    // (declared later, dropped first) are gone.
    let (payload, out, session) = unsafe {
        core::ptr::write_bytes(block.0, 0, SESSION);
        let session = block.0.add(SESSION).cast::<Session>();
        session.write(Session::default());
        (
            &mut *block.0.cast::<[u8; MAX_PAYLOAD]>(),
            &mut *block.0.add(MAX_PAYLOAD).cast::<[u8; MAX_PAYLOAD]>(),
            &mut *session,
        )
    };
    session.spent.before_us = micros(handover_us);
    let mut card = SessionCard { bulk, owned: None };
    let mut header = [0u8; HEADER];
    let mut last = embassy_time::Instant::now();
    let idle = embassy_time::Duration::from_millis(transfer::IDLE_MS);
    // The host answers and takes each packet within a USB frame or so: for
    // that long, only yield to the other tasks between looks, as a timer's
    // wake-up costs more than the wait.
    let spin = embassy_time::Duration::from_millis(1);
    fn micros(us: u64) -> u32 {
        u32::try_from(us).unwrap_or(u32::MAX)
    }
    // Bytes until `deadline`, or `None` if the host went quiet.
    async fn receive(
        usb: &mut device::drivers::usb::Usb,
        bytes: &mut [u8],
        deadline: embassy_time::Instant,
    ) -> Option<()> {
        for byte in bytes {
            loop {
                if let Some(value) = usb.read() {
                    *byte = value;
                    break;
                }
                if embassy_time::Instant::now() >= deadline {
                    return None;
                }
                embassy_time::Timer::after_micros(50).await;
            }
        }
        Some(())
    }
    async fn transmit(
        usb: &mut device::drivers::usb::Usb,
        bytes: &[u8],
        deadline: embassy_time::Instant,
        spin: embassy_time::Duration,
    ) -> Option<()> {
        // 63-byte packets: every packet is short, so cdc-acm hands each one up
        // at once instead of waiting for a ZLP after a full 64-byte packet.
        for packet in bytes.chunks(63) {
            for byte in packet {
                while !usb.write(*byte) {
                    if embassy_time::Instant::now() >= deadline {
                        return None;
                    }
                    embassy_time::Timer::after_micros(50).await;
                }
            }
            usb.flush();
            let flushed = embassy_time::Instant::now();
            while !usb.tx_ready() {
                let now = embassy_time::Instant::now();
                if now >= deadline {
                    return None;
                }
                if now - flushed < spin {
                    embassy_futures::yield_now().await;
                } else {
                    embassy_time::Timer::after_micros(50).await;
                }
            }
        }
        Some(())
    }
    while !session.ended() {
        // Find the request magic, so stray bytes cannot become a request.
        let mut found = 0;
        let waiting = embassy_time::Instant::now();
        while found < 4 {
            let Some(byte) = usb.read() else {
                let quiet = last.elapsed();
                if quiet >= idle {
                    return;
                }
                if quiet < spin {
                    embassy_futures::yield_now().await;
                } else {
                    embassy_time::Timer::after_micros(50).await;
                }
                continue;
            };
            if byte == transfer::REQUEST[found] {
                header[found] = byte;
                found += 1;
            } else {
                found = usize::from(byte == transfer::REQUEST[0]);
                header[0] = byte;
            }
        }
        let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_secs(5);
        if receive(usb, &mut header[4..], deadline).await.is_none() {
            continue;
        }
        let Some(request) = transfer::request(&header) else {
            continue;
        };
        let body = &mut payload[..request.length as usize];
        if receive(usb, body, deadline).await.is_none() {
            continue;
        }
        last = embassy_time::Instant::now();
        let spent = &mut session.spent;
        spent.waiting_us = spent
            .waiting_us
            .saturating_add(micros((last - waiting).as_micros()));
        spent.requests = spent.requests.saturating_add(1);
        let reply = if crc32(body) != request.payload_crc {
            transfer::Reply {
                status: transfer::status::INVALID,
                arg: 0,
                count: 0,
                length: 0,
            }
        } else {
            let reply = session.handle(&request, body, &mut card, out);
            let spent = &mut session.spent;
            spent.serving_us = spent
                .serving_us
                .saturating_add(micros(last.elapsed().as_micros()));
            reply
        };
        let data = &out[..reply.length];
        let head = transfer::encode_reply(&request, &reply, data);
        let sending = embassy_time::Instant::now();
        let deadline = sending + embassy_time::Duration::from_secs(5);
        if transmit(usb, &head, deadline, spin).await.is_none()
            || transmit(usb, data, deadline, spin).await.is_none()
        {
            return;
        }
        last = embassy_time::Instant::now();
        let spent = &mut session.spent;
        spent.sending_us = spent
            .sending_us
            .saturating_add(micros((last - sending).as_micros()));
    }
}

fn fitted(system: &mut crate::FirmwareShell, output: &mut dyn Write) {
    use device_api::input::{InputSource as _, Role};
    use device_api::observation::Availability;
    use device_api::power::{PowerControl, Support};
    use device_api::sensors::Sensors;
    use device_api::storage::OwnedFlash as _;
    let panel = system.display_geometry();
    let controls = system.input_source().unrecorded().controls();
    let touch = match controls.touch {
        Availability::Unsupported => "unsupported",
        Availability::Initializing => "initializing",
        Availability::Unconfigured => "unconfigured",
        Availability::Ready => "ready",
        Availability::Failed => "failed",
    };
    let _ = write!(
        output,
        "device={} width={} height={} touch={} buttons=",
        device::fitted::NAME,
        panel.width,
        panel.height,
        touch
    );
    for (index, button) in controls.buttons.as_slice().iter().enumerate() {
        let separator = if index == 0 { "" } else { "," };
        let _ = write!(output, "{}{}", separator, button.id);
        if let Some(role) = button.role {
            let role = match role {
                Role::Act => "act",
                Role::Back => "back",
                Role::Move => "move",
            };
            let _ = write!(output, ":{role}");
        }
    }
    let shutdown = match PowerControl::capabilities(&device::board::Power).shutdown {
        Support::Supported => "supported",
        Support::Unknown => "unknown",
        Support::Unsupported => "unsupported",
    };
    let sensors = Sensors::fitted(&device::board::Sensors);
    let fitted = |fitted: bool| if fitted { "fitted" } else { "absent" };
    let _ = write!(
        output,
        " shutdown={shutdown} barometer={} light={}",
        fitted(sensors.barometer),
        fitted(sensors.light)
    );
    for &(name, region) in device::fitted::REGIONS {
        let geometry = system.flash().unrecorded().geometry(region);
        let _ = write!(
            output,
            " {name}_offset={:#x} {name}_size={:#x} {name}_erase={:#x}",
            device::fitted::owned(region).0,
            geometry.capacity,
            geometry.erase_size
        );
    }
}
