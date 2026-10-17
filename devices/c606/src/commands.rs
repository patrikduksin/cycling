//! Single bounded USB owner for ordinary commands and JSON logs.
use c606_firmware as device;
use core::fmt;
use core::fmt::Write;
use device_api::ble_transport::Ble as _;
use device_api::input::InputObservation as _;
use device_api::network::Network as _;
use device_api::power::PowerControl as _;
use firmware_console::protocol::Command;
use firmware_console::protocol::Request;
use firmware_console::session::Terminal;

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
/// Capability handles owned by the console task and shared with the terminal.
pub struct Devices {
    pub ant: device::board::Ant,
    pub ble: device::board::Ble,
    pub position: device::board::Positioning,
    pub bulk: device::board::Bulk,
    pub sound: device::board::Sound,
    pub sensors: device::board::Sensors,
    pub network: device::board::Network,
    pub input: device::board::Input,
    pub power: device::board::Power,
}

pub fn execute(
    terminal: &mut Terminal<impl device_api::console::Console>,
    request: Request,
    system: &mut crate::FirmwareShell,
    now: u64,
    devices: &mut Devices,
    diagnostics: &impl Diagnostics,
    #[cfg(feature = "cycling")] sdk: &mut vana::app::Runtime,
) {
    let Devices {
        ant,
        ble,
        position,
        bulk,
        sound,
        sensors,
        network,
        input,
        power,
    } = devices;
    let mut output = Text::new();
    let mut status = "OK";
    let started = embassy_time::Instant::now();
    if !power.status().ready
        && !matches!(
            request.command,
            Command::Power(_)
                | Command::PowerShutdownAfter(_)
                | Command::Info
                | Command::Status
                | Command::Input
                | Command::Battery
                | Command::Position
                | Command::Peripheral(_)
                | Command::Wifi
                | Command::Ble
                | Command::Ant
                | Command::AntCapabilities
                | Command::AntOperation(_)
                | Command::AntChannel(_)
                | Command::AntDevices
                | Command::AntRead
                | Command::Restart
                | Command::Help
        )
    {
        terminal.reply(request.id, "BUSY", "power transition", now);
        return;
    }
    if let Some(status) =
        firmware_console::commands::shell::execute(request.command, system, now, &mut output)
    {
        if matches!(
            request.command,
            Command::Harness(firmware_shell::harness::Command {
                operation: firmware_shell::harness::Operation::Caps,
                ..
            })
        ) {
            let _ = write!(
                output,
                " ordinary=HELP,INFO,STATUS,POWER,POSITION,INPUT,BATTERY,TIME,SETTINGS,BRIGHTNESS,TIMEZONE,IDLE,SAVE,ACTIVITY,WIFI,BLE,ANT,MMC,VOLUME,TERRAIN,FILES,SOUND,GNSS,PRESSURE,MOTION,COMPANION,STORAGE,DISPLAY,RESTART,TEST"
            );
            #[cfg(feature = "cycling")]
            {
                let _ = write!(output, ",RIDE,EXPORT,RADAR,PROFILE");
            }
        }
        match request.command {
            Command::Save => {
                system.storage_max_ms = system.storage_max_ms.max(started.elapsed().as_millis())
            }
            Command::Display(_) => {
                #[cfg(feature = "cycling")]
                sdk.suspend_display();
                system.display_max_ms = system.display_max_ms.max(started.elapsed().as_millis())
            }
            _ => {}
        }
        terminal.reply(request.id, status, output.as_str(), now);
        return;
    }
    match request.command {
        Command::Power(_) | Command::PowerShutdownAfter(_) => {
            let (operation, delay_ms) = match request.command {
                Command::Power(operation) => (operation, 0),
                Command::PowerShutdownAfter(delay_ms) => {
                    (Some(device_api::power::Operation::Shutdown), delay_ms)
                }
                _ => unreachable!(),
            };
            if let Some(operation) = operation {
                #[cfg(feature = "cycling")]
                if sdk.recording() {
                    terminal.reply(request.id, "BUSY", "active work", now);
                    return;
                }
                status = firmware_console::protocol::request_status(
                    power.request_after(operation, delay_ms),
                );
            }
            let _ = write!(
                output,
                "capabilities={:?} status={:?}",
                power.capabilities(),
                power.status()
            );
        }
        Command::Volume(command) => {
            use firmware_console::commands::volumes;
            // Transfers pause the app; refuse while a ride records. Volume
            // writes never reach RIDES, so other commands need no guard.
            #[cfg(feature = "cycling")]
            if command == volumes::Command::Transfer && sdk.recording() {
                terminal.reply(request.id, "BUSY", "active work", now);
                return;
            }
            status = volumes::execute(command, bulk, &mut output);
            if command == volumes::Command::Transfer && status == "ACCEPTED" {
                terminal.schedule_transfer();
            }
        }
        Command::Peripheral(command) => {
            status = firmware_console::commands::peripherals::execute(
                command,
                now,
                bulk,
                sound,
                position,
                sensors,
                &mut output,
            );
        }
        #[cfg(feature = "cycling")]
        Command::Domain { bytes, length } => {
            status = sdk.command(
                core::str::from_utf8(&bytes[..length]).unwrap_or(""),
                &mut system.data_storage(),
                bulk,
                now,
                &mut output,
            );
        }
        command @ (Command::Ant
        | Command::AntChannel(_)
        | Command::AntDevices
        | Command::AntRead
        | Command::AntScan(_)
        | Command::AntStop
        | Command::AntConnect(_)
        | Command::AntDisconnect(_)
        | Command::AntCapabilities
        | Command::AntOperation(_)
        | Command::AntSend { .. }) => {
            status = firmware_console::commands::ant::execute(command, ant, now, &mut output)
                // Only ANT commands reach this arm; the console handles all of them.
                .unwrap_or("UNSUPPORTED");
        }
        Command::Help => {
            #[cfg(feature = "debug-harness")]
            let _ = write!(
                output,
                "MMC OWNED TEST relative_sector expected_crc_hex fill_hex; "
            );
            let _ = write!(
                output,
                "MMC [READ sector offset length|CLOCK hz|RECOVER]; MMC OWNED [STATUS|READ sector offset length]; VOLUME [STATUS|INIT|TRANSFER]; TERRAIN [STATUS]; FILES [STATUS|FORMAT CONFIRM|LIST slot|STAT name|DELETE name]; SOUND [PATTERNS|PLAY id|STOP]; GNSS [PAUSE 2000..10000|RESUME]; PRESSURE; MOTION; COMPANION [QUERY]; "
            );
            let _ = write!(
                output,
                "CMD id HELP|INFO|STATUS|POWER [STATUS|SHUTDOWN [AFTER 0..30000]|SLEEP|WAKE]|POSITION|INPUT|BATTERY|TIME|SETTINGS|BRIGHTNESS n|TIMEZONE minutes|IDLE seconds level|SAVE|ACTIVITY|WIFI [SCAN|NETWORKS|CONFIG WPA2/WPA3 ssid_hex password_hex|CONNECT|DISCONNECT|FORGET]|BLE [SCAN|PEERS|SELECT HRS/CSC name_hex/- addr_le_hex/- (SDK)|CONNECT|DISCONNECT|FORGET|ECHO]|ANT [SCAN seconds|STOP|DEVICES|CONNECT type number transmission|DISCONNECT type|CHANNEL type|READ|CAPABILITIES|OPERATION id|SEND type number transmission generation hex16]|RADAR [SENSORS] (SDK)|PROFILE [FTP watts|LTHR bpm|OVERWRITE] (SDK)|STORAGE|DISPLAY rgb565hex|RESTART|TEST n"
            );
        }
        Command::Info => {
            #[cfg(feature = "cycling")]
            let recording = sdk.recording();
            #[cfg(not(feature = "cycling"))]
            let recording = false;
            crate::logging::metadata(recording);
            let _ = write!(
                output,
                "board={} slot={} commit={} harness={} cycling={} logging=INFO recording={} protocol=1 max_line=256 log_slots=8 log_bytes=384",
                diagnostics.board(),
                diagnostics.slot(),
                option_env!("CYCLING_BUILD_COMMIT").unwrap_or("unknown"),
                cfg!(feature = "debug-harness"),
                cfg!(feature = "cycling"),
                recording
            );
        }
        Command::Status => {
            let _ = write!(
                output,
                "synthetic_events={} routed_events={} foreground={:?} press_frame_ms={} press_frame_max_ms={} press_frames={} ",
                system.synthetic_events,
                system.routed_events,
                system.foreground,
                system.press_frame_ms,
                system.press_frame_max_ms,
                system.press_frames
            );
            let (frame_us, wait_us, frame_max_us) =
                crate::device::drivers::display::TIMING.snapshot();
            let _ = write!(
                output,
                "display_frame_us={frame_us} display_wait_us={wait_us} display_frame_max_us={frame_max_us} stack_untouched={} ",
                crate::device::stack::untouched()
            );
            let logs = crate::logging::stats();
            let _ = write!(
                output,
                "uptime_ms={} reset={} crash={} heap_free={} heap_min_sampled={} psram_free={} input_events={} display_submissions={} display_ms={} display_max_ms={} storage_ops={} storage_max_ms={} terminal_rejected={} log_attempts={} log_total_us={} log_max_us={} log_lost={} log_depth={} log_buffer_bytes={} usb_poll_max_us={}",
                now,
                system.reset.name(),
                system.crash.name(),
                diagnostics.heap_free(),
                system.heap_min_sampled,
                diagnostics.external_free(),
                system.input_events,
                system.display_submissions,
                system.display_ms,
                system.display_max_ms,
                system.operations,
                system.storage_max_ms,
                terminal.rejected(),
                logs.0,
                logs.1,
                logs.2,
                logs.3,
                logs.4,
                logs.5,
                terminal.tx_max_us()
            );
        }

        Command::Input | Command::Battery => {
            if let Some(s) = input.snapshot(now) {
                let _ = write!(
                    output,
                    "touch_available={} touch_errors={} companion_valid={} bad_crc={} uart_errors={} input_lost={} buttons={:?} battery={:?} power={:?}",
                    s.touch_available,
                    s.touch_errors,
                    s.companion_valid,
                    s.companion_bad_crc,
                    s.uart_errors,
                    s.input_lost,
                    s.button_counts,
                    s.battery,
                    s.power
                );
            } else {
                status = "UNAVAILABLE";
            }
        }
        Command::Time => {
            let t = firmware_services::network_time::snapshot(
                now,
                system.settings.timezone_minutes,
                network.online(),
            );
            let _ = write!(output, "{:?}", t);
        }

        Command::Harness(_)
        | Command::Position
        | Command::Settings
        | Command::SetBrightness(_)
        | Command::SetTimezone(_)
        | Command::SetIdle(..)
        | Command::Save
        | Command::Display(_)
        | Command::Activity => unreachable!("shared shell command"),
        Command::Wifi => {
            let _ = write!(
                output,
                "state={} online={} stats={:?} saved={} operation={} sequence={} error={}",
                network.state().code(),
                network.online(),
                network.stats(),
                system.settings.wifi.is_some(),
                network.control().operation.name(),
                network.control().sequence,
                network.control().error_name()
            );
        }
        Command::WifiReconnect => {
            status = firmware_console::protocol::request_status(
                network.request(device_api::connectivity::WifiOperation::Connect),
            );
        }
        Command::WifiScan => {
            status = firmware_console::protocol::request_status(
                network.request(device_api::connectivity::WifiOperation::Scan),
            )
        }
        Command::WifiDisconnect => {
            status = firmware_console::protocol::request_status(
                network.request(device_api::connectivity::WifiOperation::Disconnect),
            )
        }
        Command::WifiConfigure(_) | Command::WifiForget => {
            let configuration = if let Command::WifiConfigure(value) = request.command {
                Some(value)
            } else {
                None
            };
            if network.control().operation == device_api::connectivity::Operation::Pending {
                status = "UNAVAILABLE";
            } else {
                let previous = system.settings;
                system.settings.wifi = configuration;
                if !system.save_connectivity() {
                    system.settings = previous;
                    status = "FAILED";
                } else {
                    status = firmware_console::protocol::request_status(network.request(
                        device_api::connectivity::WifiOperation::Configure(configuration),
                    ));
                }
            }
        }
        Command::WifiNetworks => {
            for (index, network) in network.discoveries().iter().enumerate() {
                if let Some(network) = network {
                    let _ = write!(output, "index={} ssid_hex=", index);
                    for b in network.ssid.bytes() {
                        let _ = write!(output, "{:02x}", b);
                    }
                    let _ = write!(output, " rssi={} ", network.rssi);
                }
            }
        }
        Command::Ble => {
            let s = ble.snapshot();
            let _ = write!(
                output,
                "link={} connections={} disconnections={} notifications={} dropped={} scan_reports={} saved={} profile={} operation={} sequence={} error={}",
                s.link.name(),
                s.connections,
                s.disconnections,
                s.notifications,
                s.dropped,
                s.scan_reports,
                system.settings.ble.is_some(),
                system.settings.ble_profile,
                ble.control().operation.name(),
                ble.control().sequence,
                ble.control().error_name()
            );
        }
        Command::BleReconnect => {
            status = firmware_console::protocol::request_status(
                ble.request(device_api::ble_transport::Operation::Connect),
            );
        }
        Command::BleScan => {
            status = firmware_console::protocol::request_status(
                ble.request(device_api::ble_transport::Operation::Scan),
            )
        }
        Command::BleDisconnect => {
            status = firmware_console::protocol::request_status(
                ble.request(device_api::ble_transport::Operation::Disconnect),
            )
        }
        Command::BleEcho => {
            status = firmware_console::protocol::request_status(
                ble.request(device_api::ble_transport::Operation::Echo),
            )
        }
        Command::BleForget => {
            if ble.control().operation == device_api::connectivity::Operation::Pending {
                status = "UNAVAILABLE";
            } else {
                let previous = system.settings;
                system.settings.ble = None;
                system.settings.ble_profile = 0;
                if !system.save_connectivity() {
                    system.settings = previous;
                    status = "FAILED";
                } else {
                    #[cfg(feature = "cycling")]
                    sdk.select_profile(
                        vana::sensors::ble_profile::Profile::Echo,
                        ble.snapshot().connections,
                    );
                    status = firmware_console::protocol::request_status(
                        ble.request(device_api::ble_transport::Operation::Select(None)),
                    );
                }
            }
        }
        Command::BleSelect(_, _) => {
            #[cfg(not(feature = "cycling"))]
            {
                status = "UNSUPPORTED";
            }
            #[cfg(feature = "cycling")]
            {
                if ble.control().operation == device_api::connectivity::Operation::Pending {
                    status = "UNAVAILABLE";
                } else {
                    let (selection, profile) = if let Command::BleSelect(peer, profile) =
                        request.command
                    {
                        let profile = vana::sensors::ble_profile::Profile::from_u8(profile);
                        (
                            vana::sensors::ble::selection(profile, peer.name.bytes(), peer.address),
                            profile,
                        )
                    } else {
                        (None, vana::sensors::ble_profile::Profile::Echo)
                    };
                    let previous = system.settings;
                    system.settings.ble = selection;
                    system.settings.ble_profile = profile as u8;
                    if !system.save_connectivity() {
                        system.settings = previous;
                        status = "FAILED";
                    } else {
                        sdk.select_profile(profile, ble.snapshot().connections);
                        status = firmware_console::protocol::request_status(
                            ble.request(device_api::ble_transport::Operation::Select(selection)),
                        );
                    }
                }
            }
        }
        Command::BlePeers => {
            for (index, peer) in ble.discoveries().iter().enumerate() {
                if let Some(peer) = peer {
                    let _ = write!(output, "index={} name_hex=", index);
                    for b in peer.name.bytes() {
                        let _ = write!(output, "{:02x}", b);
                    }
                    let _ = write!(output, " address_le=");
                    for b in peer.address {
                        let _ = write!(output, "{:02x}", b);
                    }
                    let _ = write!(output, " random={} rssi={} ", peer.random, peer.rssi);
                }
            }
        }
        Command::Storage => {
            let _ = write!(output, "{:?}", system.data_storage().geometry());
        }

        Command::Restart => {
            terminal.schedule_reboot(now, false);
            status = "ACCEPTED";
        }
        Command::Test(n) => {
            #[cfg(feature = "debug-harness")]
            {
                match n {
                    0..=4 => {
                        diagnostics.set_fault(n);
                        status = "ACCEPTED";
                    }
                    20 => {
                        log::warn!(target:"diagnostic","controlled executor stall ms=6000");
                        diagnostics.stall();
                        log::info!(target:"diagnostic","executor stall ended");
                        status = "ACCEPTED";
                    }
                    11 => {
                        terminal.schedule_reboot(now, true);
                        status = "ACCEPTED";
                    }
                    10 => {
                        for _ in 0..64 {
                            log::warn!(target:"diagnostic","controlled log saturation");
                        }
                        status = "ACCEPTED";
                    }
                    _ => status = "UNSUPPORTED",
                }
            }
            #[cfg(not(feature = "debug-harness"))]
            {
                let _ = n;
                status = "UNSUPPORTED";
            }
        }
    }
    terminal.reply(request.id, status, output.as_str(), now);
    terminal.command_completed();
}

/// Serve the binary volume transfer session on the terminal's USB until the
/// host ends it or stays silent for `IDLE_MS`. The app does not run meanwhile;
/// `VOLUME TRANSFER` refused while a ride records.
pub async fn transfer(usb: &mut device::drivers::usb::Usb, bulk: &mut device::board::Bulk) {
    use device_api::console::Console;
    use firmware_console::transfer::{self, HEADER, MAX_PAYLOAD, Session};
    use firmware_services::checksum::crc32;
    // The request and reply buffers (8 KiB) live in external RAM for the
    // session only: held across awaits they would sit in the console task's
    // static future and take internal RAM from the main stack.
    struct Buffers(*mut u8);
    const LAYOUT: core::alloc::Layout =
        match core::alloc::Layout::from_size_align(2 * MAX_PAYLOAD, 4) {
            Ok(layout) => layout,
            Err(_) => panic!("transfer buffer layout"),
        };
    impl Drop for Buffers {
        fn drop(&mut self) {
            // SAFETY: the block came from `allocate_external` with `LAYOUT`,
            // is freed only here, and no slice of it outlives `self`.
            unsafe { device::drivers::psram::deallocate_external(self.0, LAYOUT) };
        }
    }
    // SAFETY: `LAYOUT` has a non-zero size.
    let pointer = unsafe { device::drivers::psram::allocate_external(LAYOUT) };
    if pointer.is_null() {
        // No external RAM: the host times out and reports the session failed.
        return;
    }
    let buffers = Buffers(pointer);
    // SAFETY: the allocator returned `LAYOUT.size()` writable bytes, zeroed
    // here before any reference exists; `buffers` frees them only after
    // `payload` and `out` (declared later, dropped first) are gone.
    let (payload, out) = unsafe {
        core::ptr::write_bytes(buffers.0, 0, LAYOUT.size());
        (
            &mut *buffers.0.cast::<[u8; MAX_PAYLOAD]>(),
            &mut *buffers.0.add(MAX_PAYLOAD).cast::<[u8; MAX_PAYLOAD]>(),
        )
    };
    let mut session = Session::default();
    let mut header = [0u8; HEADER];
    let mut last = embassy_time::Instant::now();
    let idle = embassy_time::Duration::from_millis(transfer::IDLE_MS);
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
            while !usb.tx_ready() {
                if embassy_time::Instant::now() >= deadline {
                    return None;
                }
                embassy_time::Timer::after_micros(50).await;
            }
        }
        Some(())
    }
    while !session.ended() {
        // Find the request magic, so stray bytes cannot become a request.
        let mut found = 0;
        while found < 4 {
            let Some(byte) = usb.read() else {
                if last.elapsed() >= idle {
                    return;
                }
                embassy_time::Timer::after_micros(50).await;
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
        let reply = if crc32(body) != request.payload_crc {
            transfer::Reply {
                status: transfer::status::INVALID,
                arg: 0,
                count: 0,
                length: 0,
            }
        } else {
            session.handle(&request, body, bulk, out)
        };
        let data = &out[..reply.length];
        let head = transfer::encode_reply(&request, &reply, data);
        let deadline = embassy_time::Instant::now() + embassy_time::Duration::from_secs(5);
        if transmit(usb, &head, deadline).await.is_none()
            || transmit(usb, data, deadline).await.is_none()
        {
            return;
        }
    }
}
