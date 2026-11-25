use super::{Reader, Wire, Writer, codes, record};
use device_api::ant;
use device_api::ble_transport;
use device_api::bulk;
use device_api::connectivity::{self, Text};
use device_api::display;
use device_api::gps;
use device_api::input::{
    Controls, Edge, Fixture, Fixtures, Input, InputSnapshot, Point, Role, Side,
};
use device_api::observation::{Availability, Observation};
use device_api::position_control;
use device_api::positioning;
use device_api::power;
use device_api::sensors;
use device_api::server::{AccessPointState, Listen, Method, PhoneLink, Reach};
use device_api::sound;
use device_api::storage;

impl<'a, T: Wire<'a>> Wire<'a> for Observation<T> {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Unavailable => w.byte(0),
            Self::Fresh { value, received_ms } => {
                w.byte(1);
                value.put(w);
                w.time(*received_ms);
            }
            Self::Stale { value, received_ms } => {
                w.byte(2);
                value.put(w);
                w.time(*received_ms);
            }
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        match r.byte() {
            0 => Self::Unavailable,
            kind @ (1 | 2) => {
                let value = T::take(r);
                let received_ms = r.time();
                if kind == 1 {
                    Self::Fresh { value, received_ms }
                } else {
                    Self::Stale { value, received_ms }
                }
            }
            _ => {
                r.fail();
                Self::Unavailable
            }
        }
    }
}

codes! {
    Availability { Unsupported = 0, Initializing = 1, Unconfigured = 2, Ready = 3, Failed = 4 }
    Role { Act = 0, Back = 1, Move = 2 }
    Side { Left = 0, Right = 1, Top = 2, Bottom = 3 }
    ant::LinkState {
        Idle = 0, Connecting = 1, Connected = 2, Disconnecting = 3, Disconnected = 4,
        TimedOut = 5, TransportLost = 6,
    }
    ant::ScanState { Idle = 0, Starting = 1, Active = 2, Stopping = 3, Uncertain = 4 }
    ant::Error {
        Busy = 0, InvalidIdentity = 1, InvalidDuration = 2, InvalidState = 3, UnsupportedType = 4,
        Capacity = 5, Unavailable = 6, Disconnected = 7, StaleGeneration = 8, Uncertain = 9,
    }
    ble_transport::Link {
        Off = 0, Advertising = 1, Scanning = 2, Connecting = 3, Connected = 4, Retrying = 5,
        Failed = 6,
    }
    positioning::Transport { Unavailable = 0, Receiving = 1, Failed = 2, Silent = 3 }
    device_api::gps::FixState { NoData = 0, NoFix = 1, Fresh = 2, Stale = 3 }
    device_api::gps::Identity { Unknown = 0, Pair = 1, Pdt = 2, Conflicting = 3 }
    sensors::Startup {
        Unsupported = 0, NotSubmitted = 1, Probing = 2, Pending = 3, Waiting = 4, Charging = 5,
        WakePending = 6, AlreadyRunning = 7, RunningDegraded = 8, Ready = 9, Uncertain = 10,
        TimedOut = 11, Unavailable = 12, PowerTransition = 13,
    }
    device_api::observation::Error { Unsupported = 0, Unavailable = 1, Failed = 2, Invalid = 3 }
    Method { Get = 0, Head = 1, Delete = 2, Put = 3, Post = 4, Options = 5 }
    Listen { Off = 0, Network = 1, Bluetooth = 2 }
    PhoneLink { Off = 0, Advertising = 1, Connected = 2, Failed = 3, Unsupported = 4 }
    sound::State {
        Idle = 0, Queued = 1, Submitted = 2, Elapsed = 3, StopQueued = 4, StopSubmitted = 5,
        Failed = 6,
    }
    power::Operation { Shutdown = 0, Sleep = 1, Wake = 2 }
    power::State {
        Initializing = 0, Idle = 1, Charging = 2, Requested = 3, Quiescing = 4, Submitted = 5,
        Sleeping = 6, Recovering = 7, Completed = 8, Failed = 9, Uncertain = 10,
    }
    power::Failure {
        Busy = 0, Storage = 1, Display = 2, Wifi = 3, Ble = 4, Ant = 5, Gnss = 6, Sound = 7,
        Companion = 8, Sleep = 9, PreparationTimeout = 10, Submission = 11,
        CompletionUnobservable = 12, Recovery = 13,
    }
    device_api::connectivity::WifiState {
        Unconfigured = 0, Connecting = 1, Associated = 2, AddressReady = 3, Verified = 4,
        Retrying = 5, Failed = 6,
    }
    sensors::IdentityQuery {
        Idle = 0, Queued = 1, Waiting = 2, ReplyObserved = 3, Timeout = 4, Failed = 5,
    }
    position_control::State {
        Receiving = 0, PauseQueued = 1, PauseSubmitted = 2, SilenceObserved = 3, ResumeQueued = 4,
        ResumeSubmitted = 5, Failed = 6,
    }
    bulk::Error { Unsupported = 0, Unavailable = 1, Range = 2, Failed = 3 }
    display::Format { Rgb565 = 0 }
    connectivity::Operation { Idle = 0, Pending = 1, Completed = 2, Failed = 3 }
    connectivity::ControlError {
        Unconfigured = 0, ScanFailed = 1, ScanTimeout = 2, PeerNotFound = 3, ConnectFailed = 4,
        ConnectTimeout = 5, ConnectionEnded = 6, GattFailed = 7, DisconnectFailed = 8,
        DisconnectTimeout = 9, ConfigurationFailed = 10, LinkLost = 11, RunnerStopped = 12,
        WrongPassword = 13, NetworkNotFound = 14,
    }
    power::Support { Unsupported = 0, Unknown = 1, Supported = 2 }
    storage::Region { Configuration = 0, Data = 1, Crash = 2, Intent = 4, Screens = 5 }
}

record! {
    Point { x, y }
    Edge { sequence, received_ms: time, input }
    ant::Identity { device_type, device_number, transmission_type }
    ant::Packet { identity, data, received_ms: time, generation, loss_count }
    ant::Snapshot {
        scanning, link, selected, generation, packets, dropped_packets, discovery_overflows,
        transport_losses, tx_failures, age_ms, stale,
    }
    ant::Discovery { identity, rssi, seen_ms: time }
    ant::ScanSnapshot { state, generation, discovery_overflows }
    ant::Target { identity, generation }
    ant::SendObservation { id, target, stage }
    ble_transport::Snapshot { link, connections, disconnections, notifications, dropped, scan_reports }
    ble_transport::BatteryLevel { connection, raw, received_ms: time }
    device_api::gps::Snapshot {
        state, identity, latitude_e7, longitude_e7, satellites, hdop_tenths, speed_mm_s,
        epoch_ms, utc, utc_ms, age_ms, bytes, valid_sentences, checksum_errors, parse_errors,
        overflows, line_overflows, long_proprietary, uart_errors, binary_bytes, non_nmea_frames,
    }
    positioning::Snapshot { gps, transport, sequence, received_at_ms: times, fault_at_ms: times }
    sensors::Pressure { pressure_centi_pa, temperature_centi_c }
    sensors::Motion { axes }
    sensors::Identity { fields }
    sensors::Snapshot { pressure, motion, identity, reports, invalid_reports, losses }
    sensors::SensorSet { barometer, light }
    Fixture { id, role, edge, at }
    InputSnapshot {
        battery, power, button_counts, touch_available, touch_errors, companion_valid,
        companion_bad_crc, uart_errors, input_lost,
    }
    Reach { access_point, station }
    sound::Snapshot { state, pattern, requested_ms: time, submitted_ms: times, until_ms: time }
    power::Status { sequence, operation, state, at_ms: time, prepare_at_ms: times, failure, ready }
    position_control::Snapshot { state, at_ms: time, resume_at_ms: times, generation }
    gps::Diagnostics { overflow, non_nmea, gap }
    bulk::Info { sectors, sector_size, clock_hz, bus_width, reads, failures, last_read_us }
    bulk::OwnedInfo { start_sector, sectors, sector_size }
    display::Geometry { width, height }
    storage::Geometry { capacity, program_size, erase_size, durable }
    ble_transport::Selection { name, address, service, characteristic }
    ble_transport::Discovery { name, address, random, rssi }
    connectivity::ControlStatus { sequence, operation, error }
    connectivity::NetworkDiscovery { ssid, rssi, secure }
    connectivity::WifiStats { associations, probe_successes, probe_failures, fault }
    power::WakeSources { button, usb, timer }
    power::Capabilities { shutdown, sleep, sleep_timer_ms, wake, shutdown_wake, sleep_wake }
    sound::Pattern { id, frequency_hz, nominal_ms, guard_ms }
}

impl Wire<'_> for sensors::Light {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        Self(u16::take(r))
    }
}

impl Wire<'_> for Input {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Touch(point) => {
                w.byte(0);
                point.put(w);
            }
            Self::Release => w.byte(1),
            Self::Cancel => w.byte(2),
            Self::Button { id, code } => {
                w.byte(3);
                id.put(w);
                code.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Touch(Point::take(r)),
            1 => Self::Release,
            2 => Self::Cancel,
            3 => Self::Button {
                id: u8::take(r),
                code: u16::take(r),
            },
            _ => {
                r.fail();
                Self::Cancel
            }
        }
    }
}

impl Wire<'_> for ant::OperationId {
    fn put(&self, w: &mut Writer) {
        self.0.put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        Self(u32::take(r))
    }
}

impl Wire<'_> for ant::AntOperation {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Scan(duration_ms) => {
                w.byte(0);
                duration_ms.put(w);
            }
            Self::StopScan => w.byte(1),
            Self::Connect(identity) => {
                w.byte(2);
                identity.put(w);
            }
            Self::Disconnect(device_type) => {
                w.byte(3);
                device_type.put(w);
            }
            Self::Send { target, data } => {
                w.byte(4);
                target.put(w);
                data.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Scan(u32::take(r)),
            1 => Self::StopScan,
            2 => Self::Connect(ant::Identity::take(r)),
            3 => Self::Disconnect(u8::take(r)),
            4 => Self::Send {
                target: ant::Target::take(r),
                data: <[u8; 8]>::take(r),
            },
            _ => {
                r.fail();
                Self::StopScan
            }
        }
    }
}

impl Wire<'_> for ant::SendStage {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Queued => w.byte(0),
            Self::UartSubmitted => w.byte(1),
            Self::BridgeReplied { accepted } => {
                w.byte(2);
                accepted.put(w);
            }
            Self::Uncertain => w.byte(3),
            Self::Cancelled => w.byte(4),
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Queued,
            1 => Self::UartSubmitted,
            2 => Self::BridgeReplied {
                accepted: bool::take(r),
            },
            3 => Self::Uncertain,
            4 => Self::Cancelled,
            _ => {
                r.fail();
                Self::Uncertain
            }
        }
    }
}

impl Wire<'_> for ant::Admission {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Accepted => w.byte(0),
            Self::SendQueued(id) => {
                w.byte(1);
                id.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Accepted,
            1 => Self::SendQueued(ant::OperationId::take(r)),
            _ => {
                r.fail();
                Self::Accepted
            }
        }
    }
}

impl<'a, T: Wire<'a>, E: Wire<'a>> Wire<'a> for Result<T, E> {
    fn put(&self, w: &mut Writer) {
        match self {
            Ok(value) => {
                w.byte(0);
                value.put(w);
            }
            Err(error) => {
                w.byte(1);
                error.put(w);
            }
        }
    }
    fn take(r: &mut Reader<'a>) -> Self {
        match r.byte() {
            0 => Ok(Wire::take(r)),
            1 => Err(Wire::take(r)),
            _ => {
                r.fail();
                Err(Wire::take(r))
            }
        }
    }
}

impl Wire<'_> for AccessPointState {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Off => w.byte(0),
            Self::Starting => w.byte(1),
            Self::Up { clients } => {
                w.byte(2);
                clients.put(w);
            }
            Self::Stopping => w.byte(3),
            Self::Failed => w.byte(4),
            Self::Unsupported => w.byte(5),
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Off,
            1 => Self::Starting,
            2 => Self::Up {
                clients: u8::take(r),
            },
            3 => Self::Stopping,
            4 => Self::Failed,
            5 => Self::Unsupported,
            _ => {
                r.fail();
                Self::Failed
            }
        }
    }
}

impl Wire<'_> for ble_transport::Packet {
    fn put(&self, w: &mut Writer) {
        self.connection.put(w);
        self.sequence.put(w);
        self.dropped.put(w);
        w.time(self.received_ms);
        self.length.put(w);
        w.bytes(&self.bytes[..usize::from(self.length).min(self.bytes.len())]);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        let connection = u32::take(r);
        let sequence = u32::take(r);
        let dropped = u32::take(r);
        let received_ms = r.time();
        let length = u8::take(r);
        let mut bytes = [0; 60];
        let data = r.bytes(usize::from(length).min(bytes.len()));
        bytes[..data.len()].copy_from_slice(data);
        Self {
            connection,
            sequence,
            dropped,
            received_ms,
            bytes,
            length,
        }
    }
}

impl<const N: usize> Wire<'_> for Text<N> {
    fn put(&self, w: &mut Writer) {
        self.bytes().put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        let bytes = <&[u8]>::take(r);
        Self::new(bytes).unwrap_or_else(|| {
            r.fail();
            Self::empty()
        })
    }
}

impl Wire<'_> for ble_transport::Operation {
    fn put(&self, w: &mut Writer) {
        match self {
            Self::Scan => w.byte(0),
            Self::Select(selection) => {
                w.byte(1);
                selection.put(w);
            }
            Self::Connect => w.byte(2),
            Self::Disconnect => w.byte(3),
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        match r.byte() {
            0 => Self::Scan,
            1 => Self::Select(Wire::take(r)),
            2 => Self::Connect,
            3 => Self::Disconnect,
            _ => {
                r.fail();
                Self::Scan
            }
        }
    }
}

impl Wire<'_> for gps::Excerpt {
    fn put(&self, w: &mut Writer) {
        self.at.put(w);
        self.head().put(w);
        self.before().put(w);
        self.after().put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        fn part<const N: usize>(r: &mut Reader<'_>) -> ([u8; N], u8) {
            let mut out = [0; N];
            let bytes = <&[u8]>::take(r);
            match out.get_mut(..bytes.len()) {
                Some(slot) => slot.copy_from_slice(bytes),
                None => r.fail(),
            }
            (out, bytes.len().min(N) as u8)
        }
        let at = Wire::take(r);
        let (head, head_len) = part(r);
        let (before, before_len) = part(r);
        let (after, after_len) = part(r);
        Self {
            at,
            head,
            head_len,
            before,
            before_len,
            after,
            after_len,
        }
    }
}

impl Wire<'_> for display::Rows {
    fn put(&self, w: &mut Writer) {
        (self.0 as u64, (self.0 >> 64) as u64).put(w);
    }
    fn take(r: &mut Reader<'_>) -> Self {
        let (low, high) = <(u64, u64)>::take(r);
        Self(u128::from(high) << 64 | u128::from(low))
    }
}

impl Wire<'_> for Controls {
    fn put(&self, w: &mut Writer) {
        self.touch.put(w);
        let buttons = self.buttons.as_slice();
        w.byte(buttons.len() as u8);
        for fixture in buttons {
            fixture.put(w);
        }
    }
    fn take(r: &mut Reader<'_>) -> Self {
        let touch = Wire::take(r);
        let len = usize::from(r.byte());
        if len > device_api::input::MAX_BUTTONS {
            r.fail();
            return Self {
                touch,
                buttons: Fixtures::NONE,
            };
        }
        let mut list = [Fixture {
            id: 0,
            role: None,
            edge: Side::Left,
            at: 0,
        }; device_api::input::MAX_BUTTONS];
        for fixture in &mut list[..len] {
            *fixture = Fixture::take(r);
        }
        Self {
            touch,
            buttons: Fixtures::new(&list[..len]),
        }
    }
}
