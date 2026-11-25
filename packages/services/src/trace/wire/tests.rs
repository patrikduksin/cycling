use super::*;
use device_api::ble_transport;
use device_api::bulk;
use device_api::connectivity::{self, Text};
use device_api::display;
use device_api::gps;
use device_api::input::{Controls, Fixture, Fixtures, MAX_BUTTONS, Role, Side};
use device_api::observation::Availability;
use device_api::position_control;
use device_api::power;
use device_api::sensors;
use device_api::storage;
use std::string::String;
use std::vec::Vec;

const NOW: u64 = 90_000;

fn encoded<'a>(value: &impl Wire<'a>) -> Vec<u8> {
    let mut out = std::vec![0u8; 512];
    let mut w = Writer::new(&mut out, NOW);
    value.put(&mut w);
    assert!(!w.overflow);
    let len = w.len;
    out.truncate(len);
    out
}

fn round_trip<T: for<'a> Wire<'a>>(value: &T) -> T {
    let bytes = encoded(value);
    let mut r = Reader::new(&bytes, NOW);
    let back = T::take(&mut r);
    assert!(r.ok() && r.at == bytes.len(), "{bytes:?} read back whole");
    assert_eq!(encoded(&back), bytes);
    back
}

fn same<T: for<'a> Wire<'a> + PartialEq + core::fmt::Debug>(value: T) {
    assert_eq!(round_trip(&value), value);
}

fn text<const N: usize>(value: &str) -> Text<N> {
    Text::new(value.as_bytes()).unwrap()
}

fn excerpt(at: Option<u32>, head: &[u8], before: &[u8], after: &[u8]) -> gps::Excerpt {
    let mut excerpt = gps::Excerpt {
        at,
        head_len: head.len() as u8,
        before_len: before.len() as u8,
        after_len: after.len() as u8,
        ..Default::default()
    };
    excerpt.head[..head.len()].copy_from_slice(head);
    excerpt.before[..before.len()].copy_from_slice(before);
    excerpt.after[..after.len()].copy_from_slice(after);
    excerpt
}

#[test]
fn records_keep_their_fields() {
    let fields: Vec<String> = device::RECORD_FIELDS
        .iter()
        .chain(crate::trace::RECORD_FIELDS)
        .map(|(record, fields)| std::format!("{record}: {}", fields.join(" ")))
        .collect();
    let pinned = "\
Point: x y
Edge: sequence received_ms input
ant::Identity: device_type device_number transmission_type
ant::Packet: identity data received_ms generation loss_count
ant::Snapshot: scanning link selected generation packets dropped_packets discovery_overflows transport_losses tx_failures age_ms stale
ant::Discovery: identity rssi seen_ms
ant::ScanSnapshot: state generation discovery_overflows
ant::Target: identity generation
ant::SendObservation: id target stage
ble_transport::Snapshot: link connections disconnections notifications dropped scan_reports
ble_transport::BatteryLevel: connection raw received_ms
device_api::gps::Snapshot: state identity latitude_e7 longitude_e7 satellites hdop_tenths speed_mm_s epoch_ms utc utc_ms age_ms bytes valid_sentences checksum_errors parse_errors overflows line_overflows long_proprietary uart_errors binary_bytes non_nmea_frames
positioning::Snapshot: gps transport sequence received_at_ms fault_at_ms
sensors::Pressure: pressure_centi_pa temperature_centi_c
sensors::Motion: axes
sensors::Identity: fields
sensors::Snapshot: pressure motion identity reports invalid_reports losses
sensors::SensorSet: barometer light
Fixture: id role edge at
InputSnapshot: battery power button_counts touch_available touch_errors companion_valid companion_bad_crc uart_errors input_lost
Reach: access_point station
sound::Snapshot: state pattern requested_ms submitted_ms until_ms
power::Status: sequence operation state at_ms prepare_at_ms failure ready
position_control::Snapshot: state at_ms resume_at_ms generation
gps::Diagnostics: overflow non_nmea gap
bulk::Info: sectors sector_size clock_hz bus_width reads failures last_read_us
bulk::OwnedInfo: start_sector sectors sector_size
display::Geometry: width height
storage::Geometry: capacity program_size erase_size durable
ble_transport::Selection: name address service characteristic
ble_transport::Discovery: name address random rssi
connectivity::ControlStatus: sequence operation error
connectivity::NetworkDiscovery: ssid rssi secure
connectivity::WifiStats: associations probe_successes probe_failures fault
power::WakeSources: button usb timer
power::Capabilities: shutdown sleep sleep_timer_ms wake shutdown_wake sleep_wake
sound::Pattern: id frequency_hz nominal_ms guard_ms
crate::network_time::Snapshot: unix_seconds millis local_minutes age_ms status
crate::storage::Record: sequence length
ServerRequest<'a>: id method path query session gzip body
AntCapabilities<'a>: supported_types connection_capacity one_peer_per_type concurrent_scan acknowledged_send radio_delivery_feedback burst max_pending_sends
FrameTime: frames mean_us
Cost: evictions high_water
Health: simulated heap_free heap_min psram_free frame_max_us pass_max_us gps_sentences gps_overflows gps_uart_errors companion_errors card_write_max_us card_recoveries spool_used spool_size wifi wifi_rssi ble_rssi
Consumers: cpu_mhz backlight frames gnss wifi access_point ble ant ant_scanning ant_channels slept_ms
Activity: simulated window_ms wakes awake_us main_us radio_us idle_us switches ticks hooks_us consumers";
    assert_eq!(
        fields.join("\n"),
        pinned,
        "a record grew or changed: give new fields a new tag"
    );
}

#[test]
fn input_controls_round_trip_with_their_fixtures() {
    let fixture = |id, role| Fixture {
        id,
        role,
        edge: Side::Bottom,
        at: 1000,
    };
    for buttons in [
        &[][..],
        &[fixture(4, None)],
        &[
            fixture(0, Some(Role::Back)),
            Fixture {
                edge: Side::Top,
                at: 0,
                ..fixture(1, Some(Role::Move))
            },
            fixture(2, Some(Role::Act)),
        ],
        &[fixture(7, Some(Role::Act)); MAX_BUTTONS],
    ] {
        same(Controls {
            touch: Availability::Ready,
            buttons: Fixtures::new(buttons),
        });
    }
    let mut r = Reader::new(&[3, MAX_BUTTONS as u8 + 1], NOW);
    Controls::take(&mut r);
    assert!(!r.ok(), "more fixtures than a device may have are refused");
}

#[test]
fn position_control_round_trips() {
    same(position_control::Snapshot {
        state: position_control::State::ResumeQueued,
        at_ms: NOW - 1_500,
        resume_at_ms: Some(NOW + 30_000),
        generation: 7,
    });
    same(position_control::Snapshot {
        state: position_control::State::Failed,
        at_ms: 0,
        resume_at_ms: None,
        generation: 0,
    });
    same(Some(gps::Diagnostics {
        overflow: excerpt(Some(4_000), b"GPGSV,3,1,12", b"1234,5", b"\xff\r\n"),
        non_nmea: excerpt(None, b"", b"", b""),
        gap: excerpt(Some(u32::MAX), &[b'x'; 32], &[0; 8], &[0xff; 8]),
    }));
}

#[test]
fn bulk_media_answers_round_trip() {
    same::<Result<bulk::Info, bulk::Error>>(Ok(bulk::Info {
        sectors: 62_333_952,
        sector_size: 512,
        clock_hz: 40_000_000,
        bus_width: 4,
        reads: 1_234,
        failures: 2,
        last_read_us: 950,
    }));
    same::<Result<bulk::OwnedInfo, bulk::Error>>(Ok(bulk::OwnedInfo {
        start_sector: 2_048,
        sectors: 1 << 20,
        sector_size: 512,
    }));
    for error in [
        bulk::Error::Unsupported,
        bulk::Error::Unavailable,
        bulk::Error::Range,
        bulk::Error::Failed,
    ] {
        same::<Result<(), bulk::Error>>(Err(error));
        same::<Result<bulk::Info, bulk::Error>>(Err(error));
    }
    same::<Result<(), bulk::Error>>(Ok(()));
}

#[test]
fn sensor_identity_queries_round_trip() {
    for query in [
        sensors::IdentityQuery::Idle,
        sensors::IdentityQuery::Queued,
        sensors::IdentityQuery::Waiting,
        sensors::IdentityQuery::ReplyObserved,
        sensors::IdentityQuery::Timeout,
        sensors::IdentityQuery::Failed,
    ] {
        same(query);
    }
}

#[test]
fn display_geometry_format_and_rows_round_trip() {
    same(display::Geometry {
        width: 240,
        height: 320,
    });
    same(display::Format::Rgb565);
    for rows in [
        display::Rows::NONE,
        display::Rows::all(320),
        display::Rows::band(0) | display::Rows::band(127),
        display::Rows(u128::MAX),
    ] {
        same(rows);
    }
    assert_eq!(
        encoded(&display::Rows::band(2)),
        [4, 0],
        "one band near the top costs two bytes"
    );
}

#[test]
fn ble_requests_control_and_discoveries_round_trip() {
    let selection = ble_transport::Selection {
        name: text("HRM-Pro"),
        address: Some([1, 2, 3, 4, 5, 6]),
        service: 0x180d,
        characteristic: 0x2a37,
    };
    same(selection);
    use ble_transport::Operation;
    assert!(matches!(round_trip(&Operation::Scan), Operation::Scan));
    assert!(matches!(
        round_trip(&Operation::Select(Some(selection))),
        Operation::Select(Some(back)) if back == selection
    ));
    assert!(matches!(
        round_trip(&Operation::Select(None)),
        Operation::Select(None)
    ));
    assert!(matches!(
        round_trip(&Operation::Connect),
        Operation::Connect
    ));
    assert!(matches!(
        round_trip(&Operation::Disconnect),
        Operation::Disconnect
    ));
    for error in connectivity::ControlError::ALL {
        same(connectivity::ControlStatus {
            sequence: 9,
            operation: connectivity::Operation::Failed,
            error: Some(error),
        });
    }
    same(connectivity::ControlStatus::new());
    let found = ble_transport::Discovery {
        name: text("KICKR"),
        address: [6, 5, 4, 3, 2, 1],
        random: true,
        rssi: -71,
    };
    let back = round_trip(&[Some(found), None, None, None, None, None, None, Some(found)]);
    assert_eq!(
        back[7].map(|d| (d.name, d.address, d.random, d.rssi)),
        Some((found.name, found.address, true, -71))
    );
}

#[test]
fn wifi_observations_round_trip() {
    same(connectivity::WifiStats {
        associations: 3,
        probe_successes: 41,
        probe_failures: 2,
        fault: 7,
    });
    let found = connectivity::NetworkDiscovery {
        ssid: text("home"),
        rssi: -64,
        secure: true,
    };
    let back = round_trip(&[None, Some(found), None, None, None, None, None, None]);
    assert_eq!(
        back[1].map(|d| (d.ssid, d.rssi, d.secure)),
        Some((found.ssid, -64, true))
    );
    for state in [
        connectivity::WifiState::Unconfigured,
        connectivity::WifiState::Verified,
        connectivity::WifiState::Failed,
    ] {
        same(state);
    }
}

#[test]
fn power_capabilities_and_battery_round_trip() {
    same(power::Capabilities::UNSUPPORTED);
    same(power::Capabilities {
        shutdown: power::Support::Supported,
        sleep: power::Support::Unknown,
        sleep_timer_ms: Some(60_000),
        wake: power::Support::Supported,
        shutdown_wake: power::WakeSources {
            button: power::Support::Supported,
            usb: power::Support::Unknown,
            timer: power::Support::Unsupported,
        },
        sleep_wake: power::WakeSources {
            button: power::Support::Supported,
            usb: power::Support::Supported,
            timer: power::Support::Supported,
        },
    });
    same(Some((87u8, 4_012u16, NOW - 2_000)));
    same(None::<(u8, u16, u64)>);
}

#[test]
fn sound_patterns_round_trip() {
    same(device_api::sound::Pattern {
        id: 3,
        frequency_hz: 2_700,
        nominal_ms: 80,
        guard_ms: 20,
    });
}

#[test]
fn sizes_round_trip() {
    same(usize::MAX);
    same(0usize);
}

#[test]
fn flash_regions_round_trip_and_bulk_is_no_region() {
    for region in [
        storage::Region::Configuration,
        storage::Region::Data,
        storage::Region::Crash,
        storage::Region::Intent,
        storage::Region::Screens,
    ] {
        same(region);
    }
    same(storage::Geometry {
        capacity: 0x10_0000,
        program_size: 4,
        erase_size: 4_096,
        durable: true,
    });
    let mut r = Reader::new(&[3], NOW);
    storage::Region::take(&mut r);
    assert!(!r.ok());
}

#[test]
fn text_longer_than_its_room_is_refused() {
    let mut r = Reader::new(&[3, b'a', b'b', b'c'], NOW);
    Text::<2>::take(&mut r);
    assert!(!r.ok());
    same(text::<32>(""));
}

trait Encoded {}
impl<T: for<'a> Wire<'a>> Encoded for T {}
impl Encoded for device_api::ant::Capabilities where
    for<'a> crate::trace::AntCapabilities<'a>: Wire<'a>
{
}
// Each secret below overlaps the blanket impl, so a `Wire` impl for it fails
// to compile (E0119).
impl Encoded for device_api::server::AccessPointConfig {}
impl Encoded for Option<device_api::server::Request> {}
impl Encoded for device_api::connectivity::WifiOperation {}

trait EachEncoded {}
impl EachEncoded for () {}
impl<T: Encoded, Rest: EachEncoded> EachEncoded for (T, Rest) {}

trait EveryCapability {}
impl EveryCapability for () {}
impl<Types: EachEncoded, Rest: EveryCapability> EveryCapability for (Types, Rest) {}

#[test]
fn every_recorded_type_has_a_wire_encoding_or_a_named_exception() {
    fn encoded<T: EveryCapability>() {}
    encoded::<device_api::capability::Recorded>();
}
