//! A simulated ANT companion and the ANT+ sensors around it.
//!
//! The production `Channels` state machine owns link, discovery and packet state,
//! as on C606. This module plays the companion: it answers requests after a
//! transit delay, reports discoveries and link events, and broadcasts sensor
//! pages encoded from the simulated world. Admission mirrors the C606 adapter,
//! including its scan ownership window, so VANA sees the same `Busy` replies.
use crate::world::{Side, World};
use device_api::ant::{
    Admission, Ant, AntOperation, CHANNEL_CAPACITY, CONNECT_TIMEOUT_MS, Capabilities, Discovery,
    Error, Event, Identity, LinkState, Packet, Request, SCAN_STOP_TIMEOUT_MS, ScanSnapshot,
    ScanState, Snapshot,
};
use device_api::observation::Availability;
use firmware_services::ant::Channels;

pub mod device_type {
    pub const POWER: u8 = 11;
    pub const FITNESS_EQUIPMENT: u8 = 17;
    pub const SHIFTING: u8 = 34;
    pub const BIKE_LIGHTS: u8 = 35;
    pub const RADAR: u8 = 40;
    pub const HEART_RATE: u8 = 120;
    pub const SPEED_CADENCE: u8 = 121;
    pub const CADENCE: u8 = 122;
    pub const SPEED: u8 = 123;
    pub const SHIFTING_DI2: u8 = 128;
}

/// Types the stock companion routes.
const SUPPORTED_TYPES: &[u8] = &[11, 17, 34, 35, 40, 120, 121, 122, 123, 128];
/// UART transit plus companion processing for one request.
const COMPANION_MS: u64 = 40;
/// Time an open channel takes to acquire a sensor that is in range.
const ACQUIRE_MS: u64 = 700;
/// Wheel circumference VANA assumes.
const WHEEL_MM: f64 = 2136.0;

/// One ANT+ sensor in radio range of the device, or not.
pub struct AntSensor {
    pub identity: Identity,
    pub rssi: i8,
    /// In range and transmitting. Out-of-range sensors are neither discovered nor heard.
    pub present: bool,
    /// Keeps transmitting but every page is lost, as in heavy interference.
    pub silent: bool,
    /// Delay after a scan starts before the companion reports this sensor.
    pub discovery_ms: u64,
    model: Model,
    next_page_ms: u64,
}

#[derive(Default)]
struct Model {
    messages: u32,
    beats: f64,
    beat_time: u16,
    previous_beat_time: u16,
    power_events: u8,
    accumulated_power: u16,
    wheel_revolutions: f64,
    wheel_time: u16,
    crank_revolutions: f64,
    crank_time: u16,
    last_ms: Option<u64>,
}

impl AntSensor {
    pub fn new(device_type: u8, device_number: u16) -> Self {
        Self {
            identity: Identity {
                device_type,
                device_number,
                transmission_type: if device_type == device_type::RADAR {
                    5
                } else {
                    1
                },
            },
            rssi: -55,
            present: true,
            silent: false,
            discovery_ms: 400 + u64::from(device_number % 7) * 250,
            model: Model::default(),
            next_page_ms: 0,
        }
    }

    /// Message period from the ANT+ profiles, in milliseconds.
    fn period_ms(&self) -> u64 {
        match self.identity.device_type {
            device_type::HEART_RATE => 246,
            device_type::SPEED_CADENCE | device_type::SPEED | device_type::CADENCE => 248,
            _ => 250,
        }
    }

    /// Integrate the world since the previous page and encode the next one.
    /// Sensor clocks follow world time, so they keep running across device restarts.
    fn page(&mut self, world: &World) -> [u8; 8] {
        let now = world.time_ms;
        let model = &mut self.model;
        let elapsed = now.saturating_sub(model.last_ms.unwrap_or(now)) as f64 / 1000.0;
        model.last_ms = Some(now);
        model.messages = model.messages.wrapping_add(1);
        let ticks = |seconds: f64| ((seconds * 1024.0) as u64 & 0xffff) as u16;
        let seconds = now as f64 / 1000.0;
        match self.identity.device_type {
            device_type::HEART_RATE => {
                let before = model.beats;
                model.beats += world.heart_bpm / 60.0 * elapsed;
                if model.beats.floor() > before.floor() && world.heart_bpm > 0.0 {
                    // The beat happened when the fractional part crossed zero.
                    let since = model.beats.fract() / (world.heart_bpm / 60.0);
                    model.previous_beat_time = model.beat_time;
                    model.beat_time = ticks(seconds - since);
                }
                let toggle = if (model.messages / 4) % 2 == 1 {
                    0x80
                } else {
                    0
                };
                let [previous_lo, previous_hi] = model.previous_beat_time.to_le_bytes();
                let [beat_lo, beat_hi] = model.beat_time.to_le_bytes();
                [
                    0x04 | toggle,
                    0xff,
                    previous_lo,
                    previous_hi,
                    beat_lo,
                    beat_hi,
                    (model.beats as u64 & 0xff) as u8,
                    world.heart_bpm.round().clamp(0.0, 255.0) as u8,
                ]
            }
            device_type::POWER => {
                let watts = world.power_w.round().clamp(0.0, 65_534.0) as u16;
                model.power_events = model.power_events.wrapping_add(1);
                model.accumulated_power = model.accumulated_power.wrapping_add(watts);
                let cadence = if world.cadence_rpm > 0.0 {
                    world.cadence_rpm.round().clamp(0.0, 254.0) as u8
                } else {
                    0xff
                };
                let [accumulated_lo, accumulated_hi] = model.accumulated_power.to_le_bytes();
                let [watts_lo, watts_hi] = watts.to_le_bytes();
                [
                    0x10,
                    model.power_events,
                    0xff,
                    cadence,
                    accumulated_lo,
                    accumulated_hi,
                    watts_lo,
                    watts_hi,
                ]
            }
            device_type::SPEED | device_type::SPEED_CADENCE | device_type::CADENCE => {
                let wheel_hz = world.speed_kmh / 3.6 * 1000.0 / WHEEL_MM;
                if advance_revolutions(&mut model.wheel_revolutions, wheel_hz, elapsed) {
                    let since = model.wheel_revolutions.fract() / wheel_hz;
                    model.wheel_time = ticks(seconds - since);
                }
                let crank_hz = world.cadence_rpm / 60.0;
                if advance_revolutions(&mut model.crank_revolutions, crank_hz, elapsed) {
                    let since = model.crank_revolutions.fract() / crank_hz;
                    model.crank_time = ticks(seconds - since);
                }
                let wheel = (model.wheel_revolutions as u64 & 0xffff) as u16;
                let crank = (model.crank_revolutions as u64 & 0xffff) as u16;
                let (time, count) = if self.identity.device_type == device_type::CADENCE {
                    (model.crank_time, crank)
                } else {
                    (model.wheel_time, wheel)
                };
                let [crank_time_lo, crank_time_hi] = model.crank_time.to_le_bytes();
                let [crank_lo, crank_hi] = crank.to_le_bytes();
                let [time_lo, time_hi] = time.to_le_bytes();
                let [count_lo, count_hi] = count.to_le_bytes();
                if self.identity.device_type == device_type::SPEED_CADENCE {
                    [
                        crank_time_lo,
                        crank_time_hi,
                        crank_lo,
                        crank_hi,
                        time_lo,
                        time_hi,
                        count_lo,
                        count_hi,
                    ]
                } else {
                    let toggle = if (model.messages / 4) % 2 == 1 {
                        0x80
                    } else {
                        0
                    };
                    [
                        toggle, 0xff, 0xff, 0xff, time_lo, time_hi, count_lo, count_hi,
                    ]
                }
            }
            device_type::RADAR => radar_page(world, model.messages),
            _ => [0; 8],
        }
    }
}

/// Count whole revolutions; returns whether a new one completed.
fn advance_revolutions(count: &mut f64, hz: f64, seconds: f64) -> bool {
    let before = *count;
    *count += hz.max(0.0) * seconds;
    hz > 0.0 && count.floor() > before.floor()
}

/// Bike radar page A (vehicles 1–4), or page B when more vehicles exist.
fn radar_page(world: &World, message: u32) -> [u8; 8] {
    let second = world.vehicles.len() > 4 && message % 2 == 1;
    let start = if second { 4 } else { 0 };
    let mut page = [if second { 0x31 } else { 0x30 }, 0, 0, 0, 0, 0, 0, 0];
    let mut ranges = 0u32;
    for (slot, vehicle) in world.vehicles.iter().skip(start).take(4).enumerate() {
        page[1] |= (if vehicle.fast { 2 } else { 1 }) << (2 * slot);
        page[2] |= (match vehicle.side {
            Side::Behind => 0,
            Side::Right => 1,
            Side::Left => 2,
        }) << (2 * slot);
        let range = (vehicle.range_m / 3.125).round().clamp(0.0, 63.0) as u32;
        ranges |= range << (6 * slot);
        let speed = (vehicle.closing_kmh / 3.6 / 3.04).round().clamp(0.0, 15.0) as u8;
        page[6 + slot / 2] |= speed << (4 * (slot % 2));
    }
    page[3..6].copy_from_slice(&ranges.to_le_bytes()[..3]);
    page
}

/// Local scan deadlines release the discovery UI, not ownership of the radio.
/// Mirrors the C606 adapter's untagged scan-end handling.
#[derive(Clone, Copy)]
enum ScanOwnership {
    Idle,
    Active(u64),
    Stopping(u64, u8),
    Uncertain,
}

impl ScanOwnership {
    fn tick(&mut self, now: u64) {
        if matches!(*self, Self::Active(deadline) | Self::Stopping(deadline, _) if now >= deadline)
        {
            *self = Self::Uncertain;
        }
    }

    fn ended(&mut self, now: u64) {
        match *self {
            Self::Active(deadline)
                if now >= deadline.saturating_sub(SCAN_STOP_TIMEOUT_MS) && now < deadline =>
            {
                *self = Self::Idle
            }
            Self::Stopping(deadline, remaining) if now < deadline => {
                if remaining > 1 {
                    *self = Self::Stopping(deadline, remaining - 1);
                } else if now >= deadline.saturating_sub(SCAN_STOP_TIMEOUT_MS) {
                    *self = Self::Idle;
                }
            }
            _ => {}
        }
    }

    fn idle(self) -> bool {
        matches!(self, Self::Idle)
    }
}

/// A channel the companion opened for one identity.
struct Open {
    identity: Identity,
    opened_ms: u64,
    connected: bool,
}

/// Companion-side scan: discovery runs until its own timer, even after a stop.
struct CompanionScan {
    started_ms: u64,
    natural_end_ms: u64,
    discovering: bool,
    reported: Vec<Identity>,
}

pub struct SimulatedAnt {
    pub availability: Availability,
    pub sensors: Vec<AntSensor>,
    /// Requests the companion fails to submit, as on UART loss.
    pub fail_requests: bool,
    state: Channels,
    pending: Option<(Request, u64)>,
    scan: ScanOwnership,
    companion_scan: Option<CompanionScan>,
    open: Vec<Open>,
    events: Vec<(u64, Event)>,
    /// Every request the companion received, for scenario reports.
    pub log: Vec<(u64, Request)>,
}

impl Default for SimulatedAnt {
    fn default() -> Self {
        Self::new(Availability::Ready)
    }
}

impl SimulatedAnt {
    pub fn new(availability: Availability) -> Self {
        Self {
            availability,
            sensors: Vec::new(),
            fail_requests: false,
            state: Channels::new(),
            pending: None,
            scan: ScanOwnership::Idle,
            companion_scan: None,
            open: Vec::new(),
            events: Vec::new(),
            log: Vec::new(),
        }
    }

    pub fn sensor(&mut self, device_type: u8, device_number: u16) -> &mut AntSensor {
        let index = match self.sensors.iter().position(|s| {
            s.identity.device_type == device_type && s.identity.device_number == device_number
        }) {
            Some(index) => index,
            None => {
                self.sensors
                    .push(AntSensor::new(device_type, device_number));
                self.sensors.len() - 1
            }
        };
        &mut self.sensors[index]
    }

    /// A device restart: the companion powers down with it and forgets every
    /// channel, while the sensors keep transmitting.
    pub fn restarted(&mut self) {
        let sensors = core::mem::take(&mut self.sensors);
        *self = Self::new(self.availability);
        self.sensors = sensors;
    }

    /// The companion restarted or its UART failed: every channel is lost.
    pub fn transport_loss(&mut self, now: u64) {
        self.pending = None;
        self.open.clear();
        self.companion_scan = None;
        self.events.clear();
        self.state.transport_loss(now);
        if !self.scan.idle() {
            self.scan = ScanOwnership::Uncertain;
        }
    }

    /// Advance the companion and radio by one simulation step.
    pub fn tick(&mut self, world: &World, now: u64) {
        if self.availability != Availability::Ready {
            return;
        }
        self.state.tick(now);
        self.scan.tick(now);
        if let Some((request, at)) = self.pending
            && now >= at
        {
            self.pending = None;
            if self.fail_requests {
                self.state.request_failed(request, now);
                if matches!(request, Request::Scan { .. } | Request::StopScan) {
                    self.scan = ScanOwnership::Uncertain;
                }
            } else {
                self.log.push((now, request));
                self.companion(request, now);
            }
        }
        self.discover(now);
        self.link(now);
        self.broadcast(world, now);
        self.deliver(now);
    }

    fn companion(&mut self, request: Request, now: u64) {
        match request {
            Request::Scan { duration_ms } => {
                let seconds = u64::from(duration_ms.div_ceil(1000).max(5));
                self.companion_scan = Some(CompanionScan {
                    started_ms: now,
                    natural_end_ms: now + (seconds + 1) * 1000 + 200 - COMPANION_MS,
                    discovering: true,
                    reported: Vec::new(),
                });
            }
            Request::StopScan => {
                if let Some(scan) = &mut self.companion_scan {
                    scan.discovering = false;
                }
                self.events.push((now + COMPANION_MS, Event::ScanEnded));
            }
            Request::Connect { identity } => {
                self.open
                    .retain(|o| o.identity.device_type != identity.device_type);
                self.open.push(Open {
                    identity,
                    opened_ms: now,
                    connected: false,
                });
            }
            Request::Disconnect { identity } => {
                self.open
                    .retain(|o| o.identity.device_type != identity.device_type);
                self.events
                    .push((now + COMPANION_MS, Event::Disconnected(identity)));
            }
        }
    }

    fn discover(&mut self, now: u64) {
        let Some(scan) = &mut self.companion_scan else {
            return;
        };
        if now >= scan.natural_end_ms {
            self.companion_scan = None;
            self.events.push((now, Event::ScanEnded));
            return;
        }
        if !scan.discovering {
            return;
        }
        for sensor in &self.sensors {
            if sensor.present
                && !scan.reported.contains(&sensor.identity)
                && now >= scan.started_ms + sensor.discovery_ms
            {
                scan.reported.push(sensor.identity);
                self.events.push((
                    now,
                    Event::Discovery {
                        identity: sensor.identity,
                        rssi: sensor.rssi,
                    },
                ));
            }
        }
    }

    fn link(&mut self, now: u64) {
        let sensors = &self.sensors;
        let events = &mut self.events;
        self.open.retain_mut(|open| {
            if open.connected {
                return true;
            }
            let found = sensors
                .iter()
                .any(|s| s.identity == open.identity && s.present && !s.silent);
            if found && now >= open.opened_ms + ACQUIRE_MS {
                open.connected = true;
                events.push((now, Event::Connected(open.identity)));
                return true;
            }
            // The companion's search timeout closes the channel just before
            // the host's own connect deadline.
            if now >= open.opened_ms + CONNECT_TIMEOUT_MS - 500 {
                events.push((now, Event::Timeout(open.identity)));
                return false;
            }
            true
        });
    }

    fn broadcast(&mut self, world: &World, now: u64) {
        for sensor in &mut self.sensors {
            if world.time_ms < sensor.next_page_ms {
                continue;
            }
            sensor.next_page_ms = world.time_ms + sensor.period_ms();
            let data = sensor.page(world);
            let heard = sensor.present
                && !sensor.silent
                && self
                    .open
                    .iter()
                    .any(|o| o.connected && o.identity == sensor.identity);
            if heard {
                self.events.push((
                    now,
                    Event::Data {
                        device_type: sensor.identity.device_type,
                        data,
                    },
                ));
            }
        }
    }

    fn deliver(&mut self, now: u64) {
        let mut index = 0;
        while index < self.events.len() {
            if self.events[index].0 > now {
                index += 1;
                continue;
            }
            let (_, event) = self.events.remove(index);
            if event == Event::ScanEnded {
                self.scan.ended(now);
            }
            self.state.receive(event, now);
        }
    }
}

impl Ant for SimulatedAnt {
    fn availability(&self) -> Availability {
        self.availability
    }

    fn capabilities(&self) -> Capabilities {
        if self.availability != Availability::Ready {
            return Capabilities::UNAVAILABLE;
        }
        Capabilities {
            supported_types: SUPPORTED_TYPES,
            connection_capacity: CHANNEL_CAPACITY as u8,
            one_peer_per_type: true,
            concurrent_scan: true,
            acknowledged_send: false,
            radio_delivery_feedback: false,
            burst: false,
            max_pending_sends: 0,
        }
    }

    fn scan(&self) -> ScanSnapshot {
        let mut snapshot = self.state.scan();
        if matches!(self.scan, ScanOwnership::Uncertain) {
            snapshot.state = ScanState::Uncertain;
        } else if !self.scan.idle() && snapshot.state == ScanState::Idle {
            snapshot.state = ScanState::Stopping;
        }
        snapshot
    }

    fn scanning(&self) -> bool {
        self.state.scanning()
    }

    fn discoveries(&self) -> [Option<Discovery>; 8] {
        *self.state.discoveries()
    }

    fn request(&mut self, operation: AntOperation, now: u64) -> Result<Admission, Error> {
        if self.availability != Availability::Ready {
            return Err(Error::Unavailable);
        }
        self.scan.tick(now);
        let starts = matches!(
            operation,
            AntOperation::Scan(_) | AntOperation::Connect(_) | AntOperation::Send { .. }
        );
        if self.pending.is_some() || starts && !self.scan.idle() {
            return Err(Error::Busy);
        }
        let result = match operation {
            AntOperation::Scan(ms) => {
                if ms == 0 || ms > 60_000 {
                    return Err(Error::InvalidDuration);
                }
                self.state.begin_scan(now, ms)
            }
            AntOperation::StopScan => self.state.stop_scan(now),
            AntOperation::Connect(peer) => {
                if !SUPPORTED_TYPES.contains(&peer.device_type) {
                    return Err(Error::UnsupportedType);
                }
                self.state.connect(peer, now)
            }
            AntOperation::Disconnect(kind) => {
                self.state.disconnect(kind, now).ok_or(Error::Disconnected)
            }
            AntOperation::Send { .. } => return Err(Error::Unavailable),
        };
        match result {
            Ok(request) => {
                match request {
                    Request::Scan { duration_ms } => {
                        self.scan = ScanOwnership::Active(
                            now + u64::from(duration_ms.div_ceil(1000).max(5) + 1) * 1000
                                + SCAN_STOP_TIMEOUT_MS,
                        );
                    }
                    Request::StopScan => {
                        let ScanOwnership::Active(deadline) = self.scan else {
                            return Err(Error::Busy);
                        };
                        self.scan = ScanOwnership::Stopping(deadline, 2);
                    }
                    _ => {}
                }
                self.pending = Some((request, now + COMPANION_MS));
                Ok(Admission::Accepted)
            }
            Err(Error::Busy) => Err(Error::InvalidState),
            Err(error) => Err(error),
        }
    }

    fn channels(&self, now: u64) -> [Option<Snapshot>; CHANNEL_CAPACITY] {
        self.state.snapshots(now)
    }

    fn take_packet(&mut self) -> Option<Packet> {
        self.state.pop_packet()
    }

    fn take_diagnostic_packet(&mut self) -> Option<Packet> {
        self.state.pop_diagnostic_packet()
    }
}

/// Link state of the channel selecting `device_type`, for assertions.
pub fn link(ant: &SimulatedAnt, device_type: u8, now: u64) -> Option<LinkState> {
    ant.channel(device_type, now).map(|s| s.link)
}
