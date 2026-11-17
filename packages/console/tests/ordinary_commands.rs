use device_api::ble_transport::{self, Ble};
use device_api::connectivity::{ControlStatus, NetworkDiscovery, Operation, Wifi, WifiOperation};
use device_api::observation::{Availability, Error};
use device_api::storage::{Geometry, Region};
use firmware_console::commands::{Board, Capabilities, Card, Identity, execute};
use firmware_services::testing::Media;
use firmware_shell::shell::Shell;
use std::string::String;
use std::vec::Vec;

const SECTOR: usize = 4096;
const C606_SECTORS: u64 = 5_777_407;
const SECTORS: usize = 64;

struct Panel;
impl device_api::display::Display for Panel {
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn geometry(&self) -> device_api::display::Geometry {
        device_api::display::Geometry {
            width: 32,
            height: 24,
        }
    }
    fn format(&self) -> device_api::display::Format {
        device_api::display::Format::Rgb565
    }
    fn submit(
        &mut self,
        rows: device_api::display::Rows,
        mut row: impl FnMut(usize, &mut [u16]),
    ) -> Result<(), Error> {
        for y in rows.runs(24).flatten() {
            row(y, &mut [0; 32]);
        }
        Ok(())
    }
    fn check(&mut self, _: impl FnMut(usize, &mut [u16])) {}
}

struct Keys;
impl device_api::input::InputSource for Keys {
    fn controls(&self) -> device_api::input::Controls {
        device_api::input::Controls {
            touch: Availability::Unsupported,
            buttons: &[device_api::input::Button::TopLeft],
        }
    }
    fn take_edge(&mut self) -> Option<device_api::input::Edge> {
        None
    }
}

struct Backlight;
impl device_api::power::Power for Backlight {
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn battery(&self) -> Option<(u8, u16, u64)> {
        None
    }
    fn brightness(&mut self, _: u8) -> Result<(), Error> {
        Ok(())
    }
}

struct Flash(Vec<u8>);
impl device_api::storage::OwnedFlash for Flash {
    type Error = ();
    fn availability(&self, _: Region) -> Availability {
        Availability::Ready
    }
    fn geometry(&self, region: Region) -> Geometry {
        Geometry {
            capacity: if region == Region::Configuration {
                self.0.len()
            } else {
                0
            },
            program_size: 4,
            erase_size: SECTOR,
        }
    }
    fn read(&mut self, region: Region, offset: usize, output: &mut [u8]) -> Result<(), ()> {
        let bytes = self.bytes(region, offset, output.len())?;
        output.copy_from_slice(bytes);
        Ok(())
    }
    fn program(&mut self, region: Region, offset: usize, input: &[u8]) -> Result<(), ()> {
        for (byte, value) in self
            .bytes(region, offset, input.len())?
            .iter_mut()
            .zip(input)
        {
            *byte &= value;
        }
        Ok(())
    }
    fn erase(&mut self, region: Region, offset: usize, length: usize) -> Result<(), ()> {
        self.bytes(region, offset, length)?.fill(0xff);
        Ok(())
    }
}
impl Flash {
    fn bytes(&mut self, region: Region, offset: usize, length: usize) -> Result<&mut [u8], ()> {
        if region != Region::Configuration {
            return Err(());
        }
        self.0.get_mut(offset..offset + length).ok_or(())
    }
}

type System = Shell<Panel, Keys, Backlight, Flash>;

fn card() -> Media {
    Media::new(C606_SECTORS)
}

struct CardMedia<'a>(&'a mut Media);
impl device_api::bulk::ReadWrite for CardMedia<'_> {
    fn owned_info(&mut self) -> Result<device_api::bulk::OwnedInfo, device_api::bulk::Error> {
        self.0.owned_info()
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; device_api::bulk::SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        self.0.owned_read(sector, output)
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; device_api::bulk::SECTOR_SIZE],
    ) -> Result<(), device_api::bulk::Error> {
        self.0.owned_write(sector, input)
    }
    fn entropy(&mut self) -> u64 {
        self.0.entropy()
    }
}

struct NoAnt;
impl device_api::ant::Ant for NoAnt {
    fn capabilities(&self) -> device_api::ant::Capabilities {
        device_api::ant::Capabilities::UNAVAILABLE
    }
    fn scan(&self) -> device_api::ant::ScanSnapshot {
        device_api::ant::ScanSnapshot {
            state: device_api::ant::ScanState::Idle,
            generation: 0,
            discovery_overflows: 0,
        }
    }
    fn send_status(
        &self,
        _: device_api::ant::OperationId,
    ) -> Option<device_api::ant::SendObservation> {
        None
    }
    fn take_diagnostic_packet(&mut self) -> Option<device_api::ant::Packet> {
        None
    }
    fn channel(&self, _: u8, _: u64) -> Option<device_api::ant::Snapshot> {
        None
    }
    fn availability(&self) -> Availability {
        Availability::Unsupported
    }
    fn scanning(&self) -> bool {
        false
    }
    fn discoveries(&self) -> [Option<device_api::ant::Discovery>; 8] {
        [None; 8]
    }
    fn request(
        &mut self,
        _: device_api::ant::AntOperation,
        _: u64,
    ) -> Result<device_api::ant::Admission, device_api::ant::Error> {
        Err(device_api::ant::Error::Unavailable)
    }
    fn channels(
        &self,
        _: u64,
    ) -> [Option<device_api::ant::Snapshot>; device_api::ant::CHANNEL_CAPACITY] {
        [None; device_api::ant::CHANNEL_CAPACITY]
    }
    fn take_packet(&mut self) -> Option<device_api::ant::Packet> {
        None
    }
}

#[derive(Default)]
struct Radio {
    control: ControlStatus,
    selections: Vec<bool>,
}
impl Ble for Radio {
    fn request(&mut self, operation: ble_transport::Operation) -> Result<(), Error> {
        if let ble_transport::Operation::Select(selection) = operation {
            self.selections.push(selection.is_some());
        }
        Ok(())
    }
    fn control(&self) -> ControlStatus {
        self.control
    }
    fn discoveries(&self) -> [Option<ble_transport::Discovery>; 8] {
        [None; 8]
    }
    fn availability(&self) -> Availability {
        Availability::Ready
    }
    fn snapshot(&self) -> ble_transport::Snapshot {
        ble_transport::Snapshot::default()
    }
    fn take_packet(&mut self) -> Option<ble_transport::Packet> {
        None
    }
    fn take_battery(&mut self) -> Option<ble_transport::BatteryLevel> {
        None
    }
}

#[derive(Default)]
struct Station {
    configured: Vec<bool>,
}
impl Wifi for Station {
    fn request(&mut self, operation: WifiOperation) -> Result<(), Error> {
        match operation {
            WifiOperation::Configure(config) => {
                self.configured.push(config.is_some());
                Ok(())
            }
            _ => Err(Error::Unsupported),
        }
    }
    fn control(&self) -> ControlStatus {
        ControlStatus::new()
    }
    fn discoveries(&self) -> [Option<NetworkDiscovery>; 8] {
        [None; 8]
    }
    fn state(&self) -> device_api::connectivity::WifiState {
        device_api::connectivity::WifiState::Unconfigured
    }
    fn online(&self) -> bool {
        false
    }
    fn stats(&self) -> device_api::connectivity::WifiStats {
        Default::default()
    }
}

struct Switch(device_api::power::Status);
impl device_api::power::PowerControl for Switch {
    fn capabilities(&self) -> device_api::power::Capabilities {
        device_api::power::Capabilities::UNSUPPORTED
    }
    fn status(&self) -> device_api::power::Status {
        self.0
    }
    fn request(&mut self, _: device_api::power::Operation) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
    fn request_after(&mut self, _: device_api::power::Operation, _: u32) -> Result<(), Error> {
        Err(Error::Unsupported)
    }
}

struct NoInput;
impl device_api::input::InputObservation for NoInput {
    fn snapshot(&self, _: u64) -> Option<device_api::input::InputSnapshot> {
        None
    }
}

struct Traced {
    card: Media,
    trace_open: bool,
}
impl Card for &mut Traced {
    fn media(&mut self) -> impl device_api::bulk::ReadWrite + '_ {
        CardMedia(&mut self.card)
    }
    fn trace_in_store(&self) -> bool {
        self.trace_open
    }
    fn trace_status(&self, out: &mut dyn core::fmt::Write) {
        let _ = out.write_str(" trace=waiting");
    }
    fn after_volume(&mut self, _: u64) {}
    fn sync_trace(&mut self, _: u64) {}
}

struct C606(Traced);
struct Twin(Traced);

macro_rules! board {
    ($name:ident, $hardware:expr, $board:expr, $slot:expr, $transfer:expr, $diagnostic:expr) => {
        impl Board<System> for $name {
            const HARDWARE: &'static str = $hardware;
            fn startup(&self) -> device_api::sensors::Startup {
                device_api::sensors::Startup::Probing
            }
            fn card(&mut self) -> impl Card + '_ {
                &mut self.0
            }
            fn transfer(&mut self) -> &'static str {
                $transfer
            }
            fn restart(&mut self, _: u64) {}
            fn identity(&mut self, _: bool) -> Identity {
                Identity {
                    board: $board,
                    slot: $slot,
                    commit: "unknown",
                }
            }
            fn status(&self, out: &mut dyn core::fmt::Write) {
                let _ = out.write_str($diagnostic);
            }
            fn clock(&self) -> firmware_services::network_time::Clock {
                firmware_services::network_time::Clock::Unsynced { syncing: false }
            }
            fn hardware(
                &mut self,
                _: &mut System,
                command: firmware_console::protocol::Command,
                _: u64,
                out: &mut dyn core::fmt::Write,
            ) -> &'static str {
                match command {
                    firmware_console::protocol::Command::Crash => {
                        let _ = out.write_str("none");
                        "OK"
                    }
                    firmware_console::protocol::Command::CrashClear => "OK",
                    _ if $hardware.is_empty() => "UNSUPPORTED",
                    _ => "OK",
                }
            }
        }
    };
}
board!(
    C606,
    ",MMC,FITTED,TEST",
    "c606",
    "A",
    "ACCEPTED",
    " heap_free=1"
);
board!(Twin, "", "virtual", "unknown", "UNSUPPORTED", "");

struct Device<B> {
    system: System,
    sdk: vana::app::Runtime,
    board: B,
    ble: Radio,
    wifi: Station,
    power: Switch,
}

impl<B: Board<System>> Device<B> {
    fn new(board: B) -> Self {
        let mut system = Shell::new(
            Panel,
            Keys,
            Backlight,
            Flash(vec![0xff; SECTOR * SECTORS]),
            0,
        )
        .expect("the panel is ready");
        let mut sdk = vana::app::Runtime::new(Default::default(), &vana::screens::BASE);
        sdk.service_format(&mut system, &mut card(), 0);
        assert!(!sdk.holds_host_storage(), "the boot read its format intent");
        Self {
            system,
            sdk,
            board,
            ble: Radio::default(),
            wifi: Station::default(),
            power: Switch(device_api::power::Status::IDLE),
        }
    }

    fn command(&mut self, line: &str) -> (&'static str, String) {
        let request = firmware_console::protocol::parse(format!("CMD 1 {line}").as_bytes())
            .unwrap_or_else(|error| panic!("{line}: {error:?}"));
        let mut out = String::new();
        let status = execute(
            request.command,
            1_000,
            Capabilities {
                ant: &mut NoAnt,
                ble: &mut self.ble,
                wifi: &mut self.wifi,
                power: &mut self.power,
                input: &NoInput,
            },
            &mut self.system,
            &mut self.sdk,
            &mut self.board,
            &mut out,
        );
        (status, out)
    }
}

const BOARD_ANSWERS: [&str; 7] = [
    "INFO",
    "STATUS",
    "HARNESS CAPS",
    "VOLUME TRANSFER",
    "MMC",
    "FITTED",
    "TEST 0",
];

const SCRIPT: [&str; 35] = [
    "INFO",
    "HELP",
    "STATUS",
    "HARNESS CAPS",
    "SETTINGS",
    "BRIGHTNESS 40",
    "TIMEZONE 60",
    "SAVE",
    "WIFI",
    "WIFI CONFIG WPA2 54657374 70617373776f7264",
    "WIFI",
    "WIFI SCAN",
    "WIFI FORGET",
    "BLE",
    "BLE SELECT HRS 54657374 -",
    "BLE",
    "BLE SCAN",
    "BLE FORGET",
    "POWER",
    "TIME",
    "INPUT",
    "STORAGE",
    "VOLUME INIT",
    "VOLUME STATUS",
    "STORE STATUS",
    "VOLUME TRANSFER",
    "ANT",
    "RIDE STATUS",
    "PROFILE FTP 250",
    "MMC",
    "FITTED",
    "CRASH",
    "CRASH CLEAR",
    "TEST 0",
    "RESTART",
];

#[test]
fn the_device_and_the_twin_answer_each_line_alike_outside_their_boards() {
    let (mut device, mut twin) = (
        Device::new(C606(Traced {
            card: card(),
            trace_open: false,
        })),
        Device::new(Twin(Traced {
            card: card(),
            trace_open: false,
        })),
    );
    for line in SCRIPT {
        let (on_device, on_twin) = (device.command(line), twin.command(line));
        if BOARD_ANSWERS.contains(&line) {
            assert_ne!(on_device, on_twin, "{line} is the board's");
        } else {
            assert_eq!(on_device, on_twin, "{line}");
        }
    }
    assert_eq!(device.ble.selections, [true, false]);
    assert_eq!(device.wifi.configured, [true, false]);
}

#[test]
fn a_radio_choice_saves_then_asks_the_radio_and_waits_for_a_pending_one() {
    let mut twin = Device::new(Twin(Traced {
        card: card(),
        trace_open: false,
    }));
    assert_eq!(twin.command("BLE SELECT HRS 54657374 -").0, "ACCEPTED");
    assert!(twin.command("BLE").1.contains("saved=true profile=1 "));
    twin.ble.control.operation = Operation::Pending;
    let settings = twin.command("SETTINGS");
    assert_eq!(twin.command("BLE FORGET").0, "UNAVAILABLE");
    assert_eq!(twin.command("SETTINGS"), settings);
    assert!(twin.command("BLE").1.contains("saved=true profile=1 "));
    assert_eq!(twin.ble.selections, [true]);
    assert_eq!(
        twin.command("WIFI CONFIG WPA2 54657374 70617373776f7264").0,
        "ACCEPTED"
    );
    assert!(twin.command("WIFI").1.contains("saved=true"));
}

#[test]
fn work_waits_for_power_and_a_store_format_for_the_trace() {
    let mut twin = Device::new(Twin(Traced {
        card: card(),
        trace_open: true,
    }));
    twin.power.0.ready = false;
    twin.power.0.state = device_api::power::State::Initializing;
    assert_eq!(
        twin.command("SETTINGS"),
        ("BUSY", "starting companion=probing".into())
    );
    assert_eq!(twin.command("INFO").0, "OK");
    twin.power.0 = device_api::power::Status::IDLE;
    assert_eq!(
        twin.command("STORE FORMAT CONFIRM"),
        ("BUSY", "trace open".into())
    );
    let caps = twin.command("HARNESS CAPS").1;
    assert!(caps.contains(",VOLUME,TERRAIN,FILES,STORE,"), "{caps}");
}
