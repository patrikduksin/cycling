pub mod ant;
pub mod peripherals;
pub mod shell;
pub mod volumes;

use crate::protocol::{Command, request_status};
use core::fmt::Write;
use device_api::ant::Ant;
use device_api::ble_transport::{self, Ble};
use device_api::bulk::ReadWrite;
use device_api::connectivity::{Wifi, WifiOperation};
use device_api::display::Display;
use device_api::input::{InputObservation, InputSource};
use device_api::power::{Power, PowerControl};
use device_api::sensors::Startup;
use device_api::storage::OwnedFlash;
use firmware_services::network_time;
use firmware_shell::harness;
use firmware_shell::shell::Shell;
use vana::app::Runtime;

pub struct Capabilities<'a> {
    pub ant: &'a mut dyn Ant,
    pub ble: &'a mut dyn Ble,
    pub wifi: &'a mut dyn Wifi,
    pub power: &'a mut dyn PowerControl,
    pub input: &'a dyn InputObservation,
}

pub struct Identity {
    pub board: &'static str,
    /// Application slot that booted: `A`, `B` or `unknown`.
    pub slot: &'static str,
    pub commit: &'static str,
}

pub trait Card {
    /// The card as VANA's storage reaches it, recorded into the trace.
    fn media(&mut self) -> impl ReadWrite + '_;
    /// The card as a STORE command reaches it, unrecorded: the trace does
    /// not see its own store's files, and a replay answers from its own.
    fn store(&mut self) -> impl ReadWrite + '_;
    fn trace_in_store(&self) -> bool;
    /// ` trace=…` for `STORE STATUS`.
    fn trace_status(&self, out: &mut dyn Write);
    /// After an admitted volume command other than `VOLUME TRANSFER`, which
    /// may have changed the card.
    fn after_volume(&mut self, now: u64);
    /// Make the trace whole on the card before a power operation.
    fn sync_trace(&mut self, now: u64);
}

/// What only the composition answers: its card, its terminal, its clock, and
/// the hardware the twin does not model.
pub trait Board<System> {
    /// The board's own commands, for `HARNESS CAPS`: each starts with `,`.
    const HARDWARE: &'static str;
    /// Why work is not admitted yet, for a `BUSY` reply.
    fn startup(&self) -> Startup;
    fn card(&mut self) -> impl Card + '_;
    /// Hand the terminal to a binary transfer session once the reply is out.
    fn transfer(&mut self) -> &'static str;
    fn restart(&mut self, now: u64);
    fn identity(&mut self, recording: bool) -> Identity;
    /// The board's own `STATUS` fields, each after a space.
    fn status(&self, out: &mut dyn Write);
    fn clock(&self) -> network_time::Clock;
    /// The commands [`Self::HARDWARE`] lists, plus `CRASH` and `CRASH CLEAR`.
    fn hardware(
        &mut self,
        system: &mut System,
        command: Command,
        now: u64,
        out: &mut dyn Write,
    ) -> &'static str;
}

const ORDINARY: &str = "HELP,INFO,STATUS,POWER,POSITION,INPUT,BATTERY,TIME,SETTINGS,BRIGHTNESS,TIMEZONE,IDLE,SAVE,ACTIVITY,WIFI,BLE,ANT,VOLUME,TERRAIN,FILES,STORE,STORAGE,DISPLAY,RESTART";
const DOMAIN: &str = ",RIDE,EXPORT,RADAR,PROFILE,WORKOUT,PHONE";
const HELP: &str = "MMC OWNED TEST relative_sector expected_crc_hex fill_hex; MMC [READ sector offset length|CLOCK hz|RECOVER]; MMC OWNED [STATUS|READ sector offset length]; FORMAT [CONFIRM] (SDK); VOLUME [STATUS|INIT|TRANSFER]; TERRAIN [STATUS]; FILES [STATUS|FORMAT CONFIRM|LIST slot|STAT name|DELETE name]; STORE [STATUS|FORMAT CONFIRM|LIST index]; SOUND [PATTERNS|PLAY id|STOP]; GNSS [PAUSE 2000..10000|RESUME]; PRESSURE; MOTION; COMPANION [QUERY]; CMD id HELP|INFO|STATUS|POWER [STATUS|SHUTDOWN [AFTER 0..30000]|SLEEP|WAKE]|POSITION|INPUT|BATTERY|TIME|SETTINGS|BRIGHTNESS n|AUTO|TIMEZONE minutes|IDLE seconds level|SAVE|ACTIVITY|WIFI [SCAN|NETWORKS|CONFIG WPA2/WPA3 ssid_hex password_hex|CONNECT|DISCONNECT|FORGET]|BLE [SCAN|PEERS|SELECT HRS/CSC name_hex/- addr_le_hex/- (SDK)|CONNECT|DISCONNECT|FORGET|ECHO]|ANT [SCAN seconds|STOP|DEVICES|CONNECT type number transmission|DISCONNECT type|CHANNEL type|READ|CAPABILITIES|OPERATION id|SEND type number transmission generation hex16]|RADAR [SENSORS] (SDK)|PROFILE [FTP watts|LTHR bpm] (SDK)|PHONE [START [HOME]|STOP] (SDK)|STORAGE|FITTED|DISPLAY rgb565hex|RESTART|CRASH [CLEAR]|TEST n";

/// Answer one ordinary command: its status, with its reply text in `out`.
pub fn execute<D: Display, I: InputSource, P: Power, F: OwnedFlash, B: Board<Shell<D, I, P, F>>>(
    command: Command,
    now: u64,
    devices: Capabilities<'_>,
    system: &mut Shell<D, I, P, F>,
    sdk: &mut Runtime,
    board: &mut B,
    out: &mut impl Write,
) -> &'static str {
    let Capabilities {
        ant,
        ble,
        wifi,
        power,
        input,
    } = devices;
    let readiness = power.status();
    if !crate::protocol::admitted(command, readiness) {
        crate::protocol::write_not_ready(out, readiness.state, board.startup());
        return "BUSY";
    }
    if let Some(status) = shell::execute(command, system, now, out) {
        match command {
            Command::Harness(harness::Command {
                operation: harness::Operation::Caps,
                ..
            }) => {
                let _ = write!(out, " ordinary={ORDINARY}{}{DOMAIN}", B::HARDWARE);
            }
            Command::Display(_) => sdk.suspend_display(),
            _ => {}
        }
        return status;
    }
    let mut status = "OK";
    match command {
        Command::Power(_) | Command::PowerShutdownAfter(_) => {
            let (operation, delay_ms) = match command {
                Command::PowerShutdownAfter(delay_ms) => {
                    (Some(device_api::power::Operation::Shutdown), delay_ms)
                }
                Command::Power(operation) => (operation, 0),
                _ => unreachable!(),
            };
            if let Some(operation) = operation {
                if sdk.recorder_busy() {
                    let _ = out.write_str("active work");
                    return "BUSY";
                }
                if operation != device_api::power::Operation::Wake && sdk.holds_unsynced() {
                    return "UNSYNCED";
                }
                board.card().sync_trace(now);
                status = request_status(power.request_after(operation, delay_ms));
            }
            let _ = write!(
                out,
                "capabilities={:?} status={:?}",
                power.capabilities(),
                power.status()
            );
        }
        Command::Volume(command) => return volume(command, now, sdk, board, out),
        Command::Peripheral(_)
        | Command::Fitted
        | Command::Crash
        | Command::CrashClear
        | Command::Test(_) => return board.hardware(system, command, now, out),
        Command::Domain { bytes, length } => {
            return sdk.command(
                core::str::from_utf8(&bytes[..length]).unwrap_or(""),
                &mut system.data_storage(),
                &mut board.card().media(),
                now,
                out,
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
            status = ant::execute(command, ant, now, out).unwrap_or("UNSUPPORTED");
        }
        Command::Help => {
            let _ = out.write_str(HELP);
        }
        Command::Info => {
            let recording = sdk.recorder_busy();
            let Identity {
                board,
                slot,
                commit,
            } = board.identity(recording);
            let _ = write!(
                out,
                "board={board} slot={slot} commit={commit} logging=INFO recording={recording} protocol=1 max_line=256 log_slots=8 log_bytes=384"
            );
        }
        Command::Status => {
            shell::status(system, now, out);
            if let Some(work) = sdk.map_work_since_attached() {
                let _ = write!(
                    out,
                    "map_fills={} map_fill_max_us={} map_extracts={} map_extract_max_us={} map_cuts={} ",
                    work.fill.runs,
                    work.fill.max_us,
                    work.extract.runs,
                    work.extract.max_us,
                    work.cuts
                );
            }
            let _ = write!(
                out,
                "reset={} crash={} heap_min_sampled={}",
                system.reset.name(),
                system.crash.name(),
                system.heap_min_sampled
            );
            board.status(out);
        }
        Command::Input | Command::Battery => match input.snapshot(now) {
            Some(s) => {
                let _ = write!(
                    out,
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
            }
            None => status = "UNAVAILABLE",
        },
        Command::Time => {
            let time = board
                .clock()
                .snapshot(now, system.settings.timezone_minutes, wifi.online());
            let _ = write!(out, "{time:?}");
        }
        Command::Wifi => {
            let control = wifi.control();
            let _ = write!(
                out,
                "state={} online={} stats={:?} saved={} operation={} sequence={} error={}",
                wifi.state().code(),
                wifi.online(),
                wifi.stats(),
                system.settings.wifi.is_some(),
                control.operation.name(),
                control.sequence,
                control.error_name()
            );
        }
        Command::WifiReconnect => status = request_status(wifi.request(WifiOperation::Connect)),
        Command::WifiScan => status = request_status(wifi.request(WifiOperation::Scan)),
        Command::WifiDisconnect => status = request_status(wifi.request(WifiOperation::Disconnect)),
        Command::WifiConfigure(_) | Command::WifiForget => {
            let configuration = match command {
                Command::WifiConfigure(value) => Some(value),
                _ => None,
            };
            status = match system.save_wifi(wifi.control().operation, configuration) {
                Ok(()) => request_status(wifi.request(WifiOperation::Configure(configuration))),
                Err(refused) => refused,
            };
        }
        Command::WifiNetworks => {
            for (index, network) in wifi.discoveries().iter().enumerate() {
                if let Some(network) = network {
                    let _ = write!(out, "index={index} ssid_hex=");
                    for b in network.ssid.bytes() {
                        let _ = write!(out, "{b:02x}");
                    }
                    let _ = write!(out, " rssi={} ", network.rssi);
                }
            }
        }
        Command::Ble => {
            let s = ble.snapshot();
            let control = ble.control();
            let _ = write!(
                out,
                "link={} connections={} disconnections={} notifications={} dropped={} scan_reports={} saved={} profile={} operation={} sequence={} error={}",
                s.link.name(),
                s.connections,
                s.disconnections,
                s.notifications,
                s.dropped,
                s.scan_reports,
                system.settings.ble.is_some(),
                system.settings.ble_profile,
                control.operation.name(),
                control.sequence,
                control.error_name()
            );
        }
        Command::BleReconnect => {
            status = request_status(ble.request(ble_transport::Operation::Connect))
        }
        Command::BleScan => status = request_status(ble.request(ble_transport::Operation::Scan)),
        Command::BleDisconnect => {
            status = request_status(ble.request(ble_transport::Operation::Disconnect))
        }
        Command::BleEcho => status = request_status(ble.request(ble_transport::Operation::Echo)),
        Command::BleForget | Command::BleSelect(_, _) => {
            let choice = match command {
                Command::BleSelect(peer, profile) => Some((peer, profile)),
                _ => None,
            };
            status = match sdk.select_ble(
                system,
                ble.control().operation,
                ble.snapshot().connections,
                choice,
            ) {
                Ok(selection) => {
                    request_status(ble.request(ble_transport::Operation::Select(selection)))
                }
                Err(refused) => refused,
            };
        }
        Command::BlePeers => {
            for (index, peer) in ble.discoveries().iter().enumerate() {
                if let Some(peer) = peer {
                    let _ = write!(out, "index={index} name_hex=");
                    for b in peer.name.bytes() {
                        let _ = write!(out, "{b:02x}");
                    }
                    let _ = write!(out, " address_le=");
                    for b in peer.address {
                        let _ = write!(out, "{b:02x}");
                    }
                    let _ = write!(out, " random={} rssi={} ", peer.random, peer.rssi);
                }
            }
        }
        Command::Storage => {
            let geometry = system.data_storage().data().geometry();
            let _ = write!(
                out,
                "Geometry {{ capacity: {}, program_size: {}, erase_size: {}, durable: {} }}",
                geometry.capacity, geometry.program_size, geometry.erase_size, geometry.durable
            );
        }
        Command::Restart => {
            if sdk.holds_unsynced() {
                return "UNSYNCED";
            }
            board.restart(now);
            status = "ACCEPTED";
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
    }
    status
}

fn volume<System>(
    command: volumes::Command,
    now: u64,
    sdk: &Runtime,
    board: &mut impl Board<System>,
    out: &mut impl Write,
) -> &'static str {
    let mut card = board.card();
    let refused = if sdk.holds_host_storage() {
        Some("formatting")
    // A transfer pauses the app, and a STORE format erases every file in
    // STORE; refuse both while a ride records. Other volume writes never
    // reach RIDES or STORE.
    } else if matches!(
        command,
        volumes::Command::Transfer | volumes::Command::StoreFormat
    ) && sdk.recorder_busy()
    {
        Some("active work")
    } else if command == volumes::Command::StoreFormat && card.trace_in_store() {
        Some("trace open")
    } else {
        None
    };
    if let Some(why) = refused {
        let _ = out.write_str(why);
        return "BUSY";
    }
    let status = match command {
        volumes::Command::Store
        | volumes::Command::StoreFormat
        | volumes::Command::StoreList(_) => volumes::execute(command, &mut card.store(), out),
        _ => volumes::execute(command, &mut card.media(), out),
    };
    if command == volumes::Command::Store {
        card.trace_status(out);
    }
    if command != volumes::Command::Transfer {
        card.after_volume(now);
    }
    drop(card);
    if command == volumes::Command::Transfer && status == "ACCEPTED" {
        return board.transfer();
    }
    status
}

#[cfg(test)]
mod tests {
    use super::{DOMAIN, ORDINARY};

    const C606_HARDWARE: &str = ",MMC,SOUND,GNSS,PRESSURE,MOTION,COMPANION,FITTED,TEST";
    const PARSED_BUT_NOT_IN_CAPS: &str = ",HARNESS,CRASH,FORMAT";

    #[test]
    fn harness_caps_lists_every_command_the_parser_reads() {
        let source = include_str!("../protocol.rs");
        let parse = &source[source.find("pub fn parse(").unwrap()..];
        let parse = &parse[..parse.find("\n}\n").unwrap()];
        let listed = std::format!("{ORDINARY}{C606_HARDWARE}{DOMAIN}{PARSED_BUT_NOT_IN_CAPS}");
        let verbs = parse
            .lines()
            .filter(|line| line.starts_with("        \"") || line.starts_with("        name @ ("))
            .filter_map(|line| line.split_once("=>"))
            .flat_map(|(verbs, _)| verbs.split('"').skip(1).step_by(2));
        let mut count = 0;
        for verb in verbs {
            assert!(listed.split(',').any(|word| word == verb), "{verb}");
            count += 1;
        }
        assert!(
            count > 40,
            "{count} verbs: the parser no longer reads as expected"
        );
    }
}
