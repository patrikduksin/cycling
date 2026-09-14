//! Single bounded USB owner for ordinary commands and JSON logs.
use core::fmt::{self, Write};
use cycling_os::terminal_protocol::{Command, Lines, Request};
use serde::Serialize;

const BYTES: usize = 1536;
struct Text {
    bytes: [u8; 1024],
    len: usize,
}
impl Text {
    fn new() -> Self {
        Self {
            bytes: [0; 1024],
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
#[derive(Serialize)]
struct Reply<'a> {
    r#type: &'static str,
    id: u32,
    status: &'static str,
    ms: u64,
    data: &'a str,
}
pub trait Diagnostics {
    fn board(&self) -> &'static str;
    fn heap_free(&self) -> usize;
    fn external_free(&self) -> usize;
    fn restart(&self, panic: bool) -> !;
    fn set_fault(&self, fault: u8);
    fn stall(&self);
}
pub struct Terminal<T> {
    usb: T,
    lines: Lines,
    current: [u8; BYTES],
    length: usize,
    offset: usize,
    reply: [u8; BYTES],
    reply_len: usize,
    pub rejected: u32,
    pub sent: u32,
    tx_max_us: u64,
    reboot: Option<(u64, bool)>,
}
impl<T: cycling_os::capabilities::Console> Terminal<T> {
    pub fn new(usb: T) -> Self {
        Self {
            usb,
            lines: Lines::default(),
            current: [0; BYTES],
            length: 0,
            offset: 0,
            reply: [0; BYTES],
            reply_len: 0,
            rejected: 0,
            sent: 0,
            tx_max_us: 0,
            reboot: None,
        }
    }
    fn reply(&mut self, id: u32, status: &'static str, data: &str, now: u64) {
        let reply = Reply {
            r#type: "reply",
            id,
            status,
            ms: now,
            data,
        };
        match serde_json_core::to_slice(&reply, &mut self.reply[..BYTES - 1]) {
            Ok(n) => {
                self.reply[n] = b'\n';
                self.reply_len = n + 1;
            }
            Err(_) => {
                self.rejected = self.rejected.saturating_add(1);
                let fallback = Reply {
                    r#type: "reply",
                    id,
                    status: "ENCODING_FAILED",
                    ms: now,
                    data: "",
                };
                let n = serde_json_core::to_slice(&fallback, &mut self.reply[..BYTES - 1]).unwrap();
                self.reply[n] = b'\n';
                self.reply_len = n + 1;
            }
        }
    }
    /// Whole records cannot interleave. Replies win between records; each poll
    /// offers at most 64 bytes and never waits on a disconnected host.
    pub fn pump(&mut self) {
        let started = embassy_time::Instant::now();
        if self.offset == self.length {
            if self.reply_len > 0 {
                self.length = self.reply_len;
                self.current[..self.length].copy_from_slice(&self.reply[..self.length]);
                self.reply_len = 0;
            } else if let Some(record) = crate::logging::take() {
                self.length = record.as_bytes().len();
                self.current[..self.length].copy_from_slice(record.as_bytes());
            } else {
                return;
            }
            self.offset = 0;
        }
        for _ in 0..64 {
            if self.offset == self.length {
                break;
            }
            if !self.usb.write(self.current[self.offset]) {
                break;
            }
            self.offset += 1;
        }
        self.usb.flush();
        self.tx_max_us = self.tx_max_us.max(started.elapsed().as_micros());
    }
    pub fn finish_reboot(&self, now: u64, diagnostics: &impl Diagnostics) {
        if let Some((at, panic)) = self.reboot {
            if now >= at + 1000
                || (now >= at + 100 && self.reply_len == 0 && self.offset == self.length)
            {
                diagnostics.restart(panic);
            }
        }
    }
    pub fn request(&mut self, now: u64) -> Option<Request> {
        if self.reply_len != 0 {
            return None;
        }
        for _ in 0..64 {
            let Some(byte) = self.usb.read() else {
                break;
            };
            if let Some(result) = self.lines.push(byte) {
                return match result {
                    Ok(request) => Some(request),
                    Err(error) => {
                        self.rejected = self.rejected.saturating_add(1);
                        self.reply(
                            0,
                            "INVALID",
                            match error {
                                cycling_os::terminal_protocol::Error::Invalid => {
                                    "malformed command"
                                }
                                cycling_os::terminal_protocol::Error::Overlong => {
                                    "line exceeds 256 bytes"
                                }
                            },
                            now,
                        );
                        None
                    }
                };
            }
        }
        None
    }
}

#[allow(clippy::too_many_arguments)]
pub fn execute<
    D: cycling_os::capabilities::Display,
    I: cycling_os::capabilities::InputSource,
    P: cycling_os::capabilities::Power,
    B: cycling_os::storage::OwnedFlash,
>(
    terminal: &mut Terminal<impl cycling_os::capabilities::Console>,
    request: Request,
    system: &mut cycling_os::shell::Shell<D, I, P, B>,
    now: u64,
    ant: &mut impl cycling_os::capabilities::Ant,
    ble: &mut impl cycling_os::capabilities::Ble,
    position: &mut (impl cycling_os::capabilities::Positioning + cycling_os::position_control::Control),
    bulk: &mut impl cycling_os::bulk::Read,
    sound: &mut impl cycling_os::sound::Sound,
    sensors: &mut impl cycling_os::capabilities::Sensors,
    network: &mut impl cycling_os::capabilities::Network,
    input: &impl cycling_os::capabilities::InputObservation,
    power: &mut impl cycling_os::power::Control,
    diagnostics: &impl Diagnostics,
    #[cfg(feature = "cycling")] sdk: &mut crate::sdk_runtime::Runtime,
) {
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
                | Command::Restart
                | Command::Help
        )
    {
        terminal.reply(request.id, "BUSY", "power transition", now);
        return;
    }
    if let Some(status) =
        cycling_os::shell::commands::execute(request.command, system, now, &mut output)
    {
        if matches!(
            request.command,
            Command::Harness(cycling_os::harness::Command {
                operation: cycling_os::harness::Operation::Caps,
                ..
            })
        ) {
            let _ = write!(
                output,
                " ordinary=HELP,INFO,STATUS,POWER,POSITION,INPUT,BATTERY,TIME,SETTINGS,BRIGHTNESS,TIMEZONE,IDLE,SAVE,ACTIVITY,WIFI,BLE,ANT,MMC,SOUND,GNSS,PRESSURE,MOTION,COMPANION,STORAGE,DISPLAY,RESTART,TEST"
            );
            #[cfg(feature = "cycling")]
            {
                let _ = write!(output, ",RIDE,EXPORT,RADAR,FOUNDATION");
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
                    (Some(cycling_os::power::Operation::Shutdown), delay_ms)
                }
                _ => unreachable!(),
            };
            if let Some(operation) = operation {
                #[cfg(feature = "cycling")]
                if sdk.recording() {
                    terminal.reply(request.id, "BUSY", "active work", now);
                    return;
                }
                status = cycling_os::terminal_protocol::request_status(
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
        Command::Peripheral(command) => {
            status = cycling_os::peripheral_commands::execute(
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
                &mut system.store.data(),
                now,
                &mut output,
                ant,
            );
        }
        Command::Ant => {
            let _ = write!(output, "scanning={} ", ant.scanning());
            for channel in ant.channels(now).iter().flatten() {
                if let Some(peer) = channel.selected {
                    let _ = write!(
                        output,
                        "type={} link={} packets={} dropped={} stale={}; ",
                        peer.device_type,
                        channel.link.name(),
                        channel.packets,
                        channel.dropped_packets,
                        channel.stale
                    );
                }
            }
        }
        Command::AntChannel(kind) => {
            if let Some(channel) = ant.channel(kind, now) {
                let _ = write!(output, "{:?}", channel);
            } else {
                status = "UNAVAILABLE";
            }
        }
        Command::AntDevices => {
            for device in ant.discoveries().iter().flatten() {
                let p = device.identity;
                let _ = write!(
                    output,
                    "type={} number={} transmission={} rssi={} age_ms={}; ",
                    p.device_type,
                    p.device_number,
                    p.transmission_type,
                    device.rssi,
                    now.saturating_sub(device.seen_ms)
                );
            }
        }
        Command::AntRead => {
            #[cfg(feature = "cycling")]
            {
                status = "SDK_OWNS_QUEUE";
            }
            #[cfg(not(feature = "cycling"))]
            if let Some(packet) = ant.take_packet() {
                let _ = write!(output, "{:?}", packet);
            } else {
                status = "EMPTY";
            }
        }
        Command::AntScan(seconds) => {
            status = ant.request(
                cycling_os::capabilities::AntOperation::Scan(u32::from(seconds) * 1000),
                now,
            );
        }
        Command::AntStop => {
            status = ant.request(cycling_os::capabilities::AntOperation::StopScan, now);
        }
        Command::AntConnect(peer) => {
            status = ant.request(cycling_os::capabilities::AntOperation::Connect(peer), now);
        }
        Command::AntDisconnect(kind) => {
            status = ant.request(
                cycling_os::capabilities::AntOperation::Disconnect(kind),
                now,
            );
        }
        Command::Help => {
            let _ = write!(
                output,
                "MMC [READ sector offset length|CLOCK hz|RECOVER]; SOUND [PATTERNS|PLAY id|STOP]; GNSS [PAUSE 2000..10000|RESUME]; PRESSURE; MOTION; COMPANION [QUERY]; "
            );
            let _ = write!(
                output,
                "CMD id HELP|INFO|STATUS|POWER [STATUS|SHUTDOWN [AFTER 0..30000]|SLEEP|WAKE]|POSITION|INPUT|BATTERY|TIME|SETTINGS|BRIGHTNESS n|TIMEZONE minutes|IDLE seconds level|SAVE|ACTIVITY|WIFI [SCAN|NETWORKS|CONFIG WPA2/WPA3 ssid_hex password_hex|CONNECT|DISCONNECT|FORGET]|BLE [SCAN|PEERS|SELECT HRS/CSC name_hex/- addr_le_hex/- (SDK)|CONNECT|DISCONNECT|FORGET|ECHO]|ANT [SCAN seconds|STOP|DEVICES|CONNECT type number transmission|DISCONNECT type|CHANNEL type|READ]|RADAR SDK|FOUNDATION SCREEN/MENU [300..600]|FOUNDATION LOG MANUAL/START/SAMPLED 300..600/STOP/STATUS/INFO (SDK)|STORAGE|DISPLAY rgb565hex|RESTART|TEST n"
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
                "board={} commit={} harness={} cycling={} logging=INFO recording={} protocol=1 max_line=256 log_slots=8 log_bytes=384",
                diagnostics.board(),
                option_env!("CYCLING_BUILD_COMMIT").unwrap_or("unknown"),
                cfg!(feature = "debug-harness"),
                cfg!(feature = "cycling"),
                recording
            );
        }
        Command::Status => {
            let _ = write!(
                output,
                "synthetic_events={} routed_events={} foreground={:?} ",
                system.synthetic_events, system.routed_events, system.foreground
            );
            let logs = crate::logging::stats();
            let _ = write!(
                output,
                "uptime_ms={} reset={} crash={} heap_free={} heap_min_sampled={} psram_free={} input_events={} display_submissions={} display_max_ms={} storage_ops={} storage_max_ms={} terminal_rejected={} log_attempts={} log_total_us={} log_max_us={} log_lost={} log_depth={} log_buffer_bytes={} usb_poll_max_us={}",
                now,
                system.reset.name(),
                system.crash.name(),
                diagnostics.heap_free(),
                system.heap_min_sampled,
                diagnostics.external_free(),
                system.input_events,
                system.display_submissions,
                system.display_max_ms,
                system.operations,
                system.storage_max_ms,
                terminal.rejected,
                logs.0,
                logs.1,
                logs.2,
                logs.3,
                logs.4,
                logs.5,
                terminal.tx_max_us
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
            let t = cycling_os::network_time::snapshot(
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
                network.state(),
                network.online(),
                network.stats(),
                system.settings.wifi.is_some(),
                network.control().operation.name(),
                network.control().sequence,
                network.control().error
            );
        }
        Command::WifiReconnect => {
            status = cycling_os::terminal_protocol::request_status(
                network.request(cycling_os::connectivity::WifiOperation::Connect),
            );
        }
        Command::WifiScan => {
            status = cycling_os::terminal_protocol::request_status(
                network.request(cycling_os::connectivity::WifiOperation::Scan),
            )
        }
        Command::WifiDisconnect => {
            status = cycling_os::terminal_protocol::request_status(
                network.request(cycling_os::connectivity::WifiOperation::Disconnect),
            )
        }
        Command::WifiConfigure(_) | Command::WifiForget => {
            let configuration = if let Command::WifiConfigure(value) = request.command {
                Some(value)
            } else {
                None
            };
            if network.control().operation == cycling_os::connectivity::Operation::Pending {
                status = "UNAVAILABLE";
            } else {
                let previous = system.settings;
                system.settings.wifi = configuration;
                if !system.save_connectivity() {
                    system.settings = previous;
                    status = "FAILED";
                } else {
                    status = cycling_os::terminal_protocol::request_status(network.request(
                        cycling_os::connectivity::WifiOperation::Configure(configuration),
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
                ble.control().error
            );
        }
        Command::BleReconnect => {
            status = cycling_os::terminal_protocol::request_status(
                ble.request(cycling_os::ble_transport::Operation::Connect),
            );
        }
        Command::BleScan => {
            status = cycling_os::terminal_protocol::request_status(
                ble.request(cycling_os::ble_transport::Operation::Scan),
            )
        }
        Command::BleDisconnect => {
            status = cycling_os::terminal_protocol::request_status(
                ble.request(cycling_os::ble_transport::Operation::Disconnect),
            )
        }
        Command::BleEcho => {
            status = cycling_os::terminal_protocol::request_status(
                ble.request(cycling_os::ble_transport::Operation::Echo),
            )
        }
        Command::BleForget => {
            if ble.control().operation == cycling_os::connectivity::Operation::Pending {
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
                        cycling_os::sdk::ble_sensor::Profile::Echo,
                        ble.snapshot().connections,
                    );
                    status = cycling_os::terminal_protocol::request_status(
                        ble.request(cycling_os::ble_transport::Operation::Select(None)),
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
                if ble.control().operation == cycling_os::connectivity::Operation::Pending {
                    status = "UNAVAILABLE";
                } else {
                    let (selection, profile) =
                        if let Command::BleSelect(peer, profile) = request.command {
                            let profile = cycling_os::sdk::ble_sensor::Profile::from_u8(profile);
                            (
                                cycling_os::sdk::ble::selection(
                                    profile,
                                    peer.name.bytes(),
                                    peer.address,
                                ),
                                profile,
                            )
                        } else {
                            (None, cycling_os::sdk::ble_sensor::Profile::Echo)
                        };
                    let previous = system.settings;
                    system.settings.ble = selection;
                    system.settings.ble_profile = profile as u8;
                    if !system.save_connectivity() {
                        system.settings = previous;
                        status = "FAILED";
                    } else {
                        sdk.select_profile(profile, ble.snapshot().connections);
                        status = cycling_os::terminal_protocol::request_status(
                            ble.request(cycling_os::ble_transport::Operation::Select(selection)),
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
            let _ = write!(output, "{:?}", system.store.data().geometry());
        }

        Command::Restart => {
            terminal.reboot = Some((now, false));
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
                        terminal.reboot = Some((now, true));
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
    terminal.sent = terminal.sent.saturating_add(1);
}
