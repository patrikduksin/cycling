//! The handles VANA reads, frozen once a pass and recorded into a trace.
//!
//! Composition binds each handle at the start of a pass. Every read during
//! the pass returns the value sampled then, so VANA sees one value of each
//! observation per pass however often it asks. A sampled value is recorded
//! only when it differs from what a replay would present: the last recorded
//! value, aged to this pass. Takes and requests pass through to the device
//! and are recorded as they happen, in call order.
use super::{AntRadio, Encoder, Event, Full, Header, encode_header};
use core::cell::RefCell;
use device_api::ant::{self, Ant, AntOperation};
use device_api::ble_transport::{self, Ble};
use device_api::input::{Controls, Edge, InputObservation, InputSnapshot, InputSource};
use device_api::observation::{Availability, Error};
use device_api::positioning::{self, Positioning};

/// Where a pass's events go.
pub trait Record {
    fn record(&mut self, event: &Event<'_>);
}

/// Where sealed blocks go, in order. The first is the header.
pub trait Sink {
    fn block(&mut self, bytes: &[u8]);
}

/// Encodes events into blocks and hands each full block to its sink.
pub struct Recorder<B, S> {
    encoder: Encoder<B>,
    sink: S,
    refused: u32,
}

impl<B: AsMut<[u8]> + AsRef<[u8]>, S: Sink> Recorder<B, S> {
    /// Writes the header block first; `None` when it does not fit `buf`.
    pub fn new(header: &Header<'_>, mut buf: B, mut sink: S) -> Option<Self> {
        let len = encode_header(header, buf.as_mut())?;
        sink.block(&buf.as_ref()[..len]);
        Some(Self {
            encoder: Encoder::new(buf),
            sink,
            refused: 0,
        })
    }

    /// The block being filled, framed. It stays open.
    pub fn open_block(&mut self) -> &[u8] {
        self.encoder.sealed()
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    /// Events larger than a whole block, which were left out.
    pub fn refused(&self) -> u32 {
        self.refused
    }
}

impl<B: AsMut<[u8]> + AsRef<[u8]>, S: Sink> Record for Recorder<B, S> {
    fn record(&mut self, event: &Event<'_>) {
        let mut result = self.encoder.push(event);
        if result == Err(Full::Block) {
            self.sink.block(self.encoder.sealed());
            self.encoder.restart();
            result = self.encoder.push(event);
        }
        if result.is_err() {
            self.refused = self.refused.saturating_add(1);
        }
    }
}

/// How a value read at one pass reads `ms` later, for the parts its producer
/// computes from the time of the read. Most values do not age.
pub trait Aging: Copy + PartialEq {
    fn aged(self, _ms: u64) -> Self {
        self
    }
}

impl Aging for bool {}
impl Aging for Availability {}
impl Aging for device_api::sensors::Startup {}
impl Aging for device_api::sensors::Snapshot {}
impl Aging for InputSnapshot {}
impl Aging for ble_transport::Snapshot {}
impl Aging for ant::Capabilities {}
impl Aging for ant::ScanSnapshot {}
impl Aging for [Option<ant::Discovery>; ant::DISCOVERY_CAPACITY] {}

impl Aging for ant::Snapshot {
    fn aged(self, ms: u64) -> Self {
        Self {
            age_ms: self.age_ms.map(|age| age.saturating_add(ms)),
            ..self
        }
    }
}

impl Aging for positioning::Snapshot {
    fn aged(mut self, ms: u64) -> Self {
        self.gps.age_ms = self.gps.age_ms.map(|age| age.saturating_add(ms));
        self.gps.utc_ms = self.gps.utc_ms.map(|utc| utc.saturating_add(ms));
        self
    }
}

impl Aging for crate::network_time::Snapshot {
    /// UTC advances with uptime, as the clock owner computes it.
    fn aged(self, ms: u64) -> Self {
        let Some(seconds) = self.unix_seconds else {
            return self;
        };
        let total_ms = seconds * 1_000 + u64::from(self.millis) + ms;
        let current = total_ms / 1_000;
        let age_ms = self.age_ms.map(|age| age.saturating_add(ms));
        Self {
            unix_seconds: Some(current),
            millis: (total_ms % 1_000) as u16,
            local_minutes: self.local_minutes.map(|minutes| {
                ((u64::from(minutes) + current / 60 - seconds / 60) % (24 * 60)) as u16
            }),
            age_ms,
            status: if age_ms.is_some_and(|age| age >= crate::network_time::STALE_AFTER_MS) {
                crate::network_time::Status::Stale
            } else {
                self.status
            },
        }
    }
}

impl<T: Aging> Aging for Option<T> {
    fn aged(self, ms: u64) -> Self {
        self.map(|value| value.aged(ms))
    }
}

impl<A: Aging, B: Aging> Aging for (A, B) {
    fn aged(self, ms: u64) -> Self {
        (self.0.aged(ms), self.1.aged(ms))
    }
}

/// The last recorded value of an observation and the pass that read it.
#[derive(Clone, Copy, Debug)]
pub struct Latest<T> {
    recorded: Option<(T, u64)>,
}

impl<T> Default for Latest<T> {
    fn default() -> Self {
        Self { recorded: None }
    }
}

impl<T: Aging> Latest<T> {
    /// Whether `value`, read at `now`, differs from the recorded value aged
    /// to `now`. A different value becomes the recorded one.
    pub fn changed(&mut self, value: T, now: u64) -> bool {
        if self.at(now) == Some(value) {
            return false;
        }
        self.recorded = Some((value, now));
        true
    }

    /// Record `value` as read at `at`, as a replay does with a decoded event.
    pub fn set(&mut self, value: T, at: u64) {
        self.recorded = Some((value, at));
    }

    /// The recorded value as it reads at `now`.
    pub fn at(&self, now: u64) -> Option<T> {
        self.recorded
            .map(|(value, at)| value.aged(now.saturating_sub(at)))
    }

    /// The value, recorded as `event` when it changed.
    pub fn observe<R: Record + ?Sized>(
        &mut self,
        value: T,
        now: u64,
        record: &RefCell<R>,
        event: impl FnOnce(T) -> Event<'static>,
    ) -> T {
        if self.changed(value, now) {
            record.borrow_mut().record(&event(value));
        }
        value
    }
}

#[derive(Clone, Copy)]
struct AntView {
    availability: Availability,
    capabilities: ant::Capabilities,
    scanning: bool,
    scan: ant::ScanSnapshot,
    discoveries: [Option<ant::Discovery>; ant::DISCOVERY_CAPACITY],
    channels: [Option<ant::Snapshot>; ant::CHANNEL_CAPACITY],
}

/// The ANT radio as VANA sees it across passes.
pub struct FrozenAnt {
    radio: Latest<((Availability, ant::Capabilities), (bool, ant::ScanSnapshot))>,
    discoveries: Latest<[Option<ant::Discovery>; ant::DISCOVERY_CAPACITY]>,
    channels: [Latest<Option<ant::Snapshot>>; ant::CHANNEL_CAPACITY],
    view: AntView,
}

impl Default for FrozenAnt {
    fn default() -> Self {
        Self {
            radio: Latest::default(),
            discoveries: Latest::default(),
            channels: [Latest::default(); ant::CHANNEL_CAPACITY],
            view: AntView {
                availability: Availability::Unsupported,
                capabilities: ant::Capabilities::UNAVAILABLE,
                scanning: false,
                scan: ant::ScanSnapshot {
                    state: ant::ScanState::Idle,
                    generation: 0,
                    discovery_overflows: 0,
                },
                discoveries: [None; ant::DISCOVERY_CAPACITY],
                channels: [None; ant::CHANNEL_CAPACITY],
            },
        }
    }
}

impl FrozenAnt {
    /// Sample `inner` for this pass and record what changed.
    pub fn bind<'a, A: Ant + ?Sized, R: Record + ?Sized>(
        &'a mut self,
        inner: &'a mut A,
        now: u64,
        record: &'a RefCell<R>,
    ) -> PassAnt<'a, A, R> {
        self.sample(inner, now, record);
        PassAnt {
            frozen: self,
            inner,
            now,
            record,
        }
    }

    fn sample<A: Ant + ?Sized, R: Record + ?Sized>(
        &mut self,
        inner: &A,
        now: u64,
        record: &RefCell<R>,
    ) {
        let view = AntView {
            availability: inner.availability(),
            capabilities: inner.capabilities(),
            scanning: inner.scanning(),
            scan: inner.scan(),
            discoveries: inner.discoveries(),
            channels: inner.channels(now),
        };
        let radio = (
            (view.availability, view.capabilities),
            (view.scanning, view.scan),
        );
        let mut r = record.borrow_mut();
        if self.radio.changed(radio, now) {
            r.record(&Event::AntRadio(AntRadio {
                availability: view.availability,
                capabilities: view.capabilities.into(),
                scanning: view.scanning,
                scan: view.scan,
            }));
        }
        if self.discoveries.changed(view.discoveries, now) {
            r.record(&Event::AntDiscoveries(view.discoveries));
        }
        for (channel, (latest, snapshot)) in self.channels.iter_mut().zip(view.channels).enumerate()
        {
            if latest.changed(snapshot, now) {
                r.record(&Event::AntLink {
                    channel: channel as u8,
                    snapshot,
                });
            }
        }
        drop(r);
        self.view = view;
    }
}

/// The ANT radio for one pass. VANA's own request is the one thing that
/// changes it mid-pass: what changed is sampled and recorded right after the
/// request's answer, so the rest of the pass sees the request taken.
pub struct PassAnt<'a, A: ?Sized, R: ?Sized> {
    frozen: &'a mut FrozenAnt,
    inner: &'a mut A,
    now: u64,
    record: &'a RefCell<R>,
}

impl<A: Ant + ?Sized, R: Record + ?Sized> Ant for PassAnt<'_, A, R> {
    fn scanning(&self) -> bool {
        self.frozen.view.scanning
    }
    fn discoveries(&self) -> [Option<ant::Discovery>; 8] {
        self.frozen.view.discoveries
    }
    fn request(
        &mut self,
        operation: AntOperation,
        now_ms: u64,
    ) -> Result<ant::Admission, ant::Error> {
        let result = self.inner.request(operation, now_ms);
        self.record
            .borrow_mut()
            .record(&Event::AntAdmission(result));
        self.frozen.sample(self.inner, self.now, self.record);
        result
    }
    fn capabilities(&self) -> ant::Capabilities {
        self.frozen.view.capabilities
    }
    fn scan(&self) -> ant::ScanSnapshot {
        self.frozen.view.scan
    }
    fn send_status(&self, id: ant::OperationId) -> Option<ant::SendObservation> {
        let status = self.inner.send_status(id);
        self.record.borrow_mut().record(&Event::AntSend(status));
        status
    }
    /// The diagnostic stream is the terminal's, not VANA's: not recorded.
    fn take_diagnostic_packet(&mut self) -> Option<ant::Packet> {
        self.inner.take_diagnostic_packet()
    }
    fn availability(&self) -> Availability {
        self.frozen.view.availability
    }
    fn channels(&self, _now_ms: u64) -> [Option<ant::Snapshot>; ant::CHANNEL_CAPACITY] {
        self.frozen.view.channels
    }
    fn take_packet(&mut self) -> Option<ant::Packet> {
        let packet = self.inner.take_packet()?;
        self.record.borrow_mut().record(&Event::AntPacket(packet));
        Some(packet)
    }
}

/// The BLE link as VANA sees it across passes.
pub struct FrozenBle {
    link: Latest<(Availability, ble_transport::Snapshot)>,
    view: (Availability, ble_transport::Snapshot),
}

impl Default for FrozenBle {
    fn default() -> Self {
        Self {
            link: Latest::default(),
            view: (
                Availability::Unsupported,
                ble_transport::Snapshot::default(),
            ),
        }
    }
}

impl FrozenBle {
    /// Sample `inner` for this pass and record what changed.
    pub fn bind<'a, B: Ble + ?Sized, R: Record + ?Sized>(
        &'a mut self,
        inner: &'a mut B,
        now: u64,
        record: &'a RefCell<R>,
    ) -> PassBle<'a, B, R> {
        self.view = self.link.observe(
            (inner.availability(), inner.snapshot()),
            now,
            record,
            |(availability, snapshot)| Event::BleLink {
                availability,
                snapshot,
            },
        );
        PassBle {
            view: self.view,
            inner,
            record,
        }
    }

    /// The link as this pass sampled it.
    pub fn snapshot(&self) -> ble_transport::Snapshot {
        self.view.1
    }
}

/// The BLE link for one pass. VANA reads neither discoveries nor control
/// status, so those and the requests pass through unrecorded.
pub struct PassBle<'a, B: ?Sized, R: ?Sized> {
    view: (Availability, ble_transport::Snapshot),
    inner: &'a mut B,
    record: &'a RefCell<R>,
}

impl<B: Ble + ?Sized, R: Record + ?Sized> Ble for PassBle<'_, B, R> {
    fn request(&mut self, operation: ble_transport::Operation) -> Result<(), Error> {
        self.inner.request(operation)
    }
    fn control(&self) -> device_api::connectivity::ControlStatus {
        self.inner.control()
    }
    fn discoveries(&self) -> [Option<ble_transport::Discovery>; 8] {
        self.inner.discoveries()
    }
    fn availability(&self) -> Availability {
        self.view.0
    }
    fn snapshot(&self) -> ble_transport::Snapshot {
        self.view.1
    }
    fn take_packet(&mut self) -> Option<ble_transport::Packet> {
        let packet = self.inner.take_packet()?;
        self.record.borrow_mut().record(&Event::BlePacket(packet));
        Some(packet)
    }
    fn take_battery(&mut self) -> Option<ble_transport::BatteryLevel> {
        let level = self.inner.take_battery()?;
        self.record.borrow_mut().record(&Event::BleBattery(level));
        Some(level)
    }
    fn reconnect(&mut self) -> Result<(), Error> {
        self.inner.reconnect()
    }
}

/// Positioning as VANA and the shell see it across passes.
#[derive(Default)]
pub struct FrozenPosition {
    latest: Latest<(Availability, Option<positioning::Snapshot>)>,
}

impl FrozenPosition {
    /// Sample `inner` for this pass and record what changed.
    pub fn freeze<P: Positioning + ?Sized, R: Record + ?Sized>(
        &mut self,
        inner: &P,
        now: u64,
        record: &RefCell<R>,
    ) -> PositionView {
        let (availability, snapshot) = self.latest.observe(
            (inner.availability(), inner.snapshot(now)),
            now,
            record,
            |(availability, snapshot)| Event::Position {
                availability,
                snapshot,
            },
        );
        PositionView {
            availability,
            snapshot,
        }
    }
}

/// Positioning for one pass.
#[derive(Clone, Copy, Debug)]
pub struct PositionView {
    pub availability: Availability,
    pub snapshot: Option<positioning::Snapshot>,
}

impl Positioning for PositionView {
    fn availability(&self) -> Availability {
        self.availability
    }
    fn snapshot(&self, _now_ms: u64) -> Option<positioning::Snapshot> {
        self.snapshot
    }
}

/// The battery, power and button counters the companion reports, across passes.
#[derive(Default)]
pub struct FrozenCompanion {
    latest: Latest<Option<InputSnapshot>>,
}

impl FrozenCompanion {
    /// Sample `inner` for this pass and record what changed.
    pub fn freeze<C: InputObservation + ?Sized, R: Record + ?Sized>(
        &mut self,
        inner: &C,
        now: u64,
        record: &RefCell<R>,
    ) -> CompanionView {
        CompanionView(
            self.latest
                .observe(inner.snapshot(now), now, record, Event::Power),
        )
    }
}

/// The companion's report for one pass.
#[derive(Clone, Copy, Debug)]
pub struct CompanionView(pub Option<InputSnapshot>);

impl InputObservation for CompanionView {
    fn snapshot(&self, _now_ms: u64) -> Option<InputSnapshot> {
        self.0
    }
}

/// Physical input edges taken during a pass, kept until composition records
/// them. Holds one shell tick's worth; past that, edges wait in the device
/// queue for the next pass rather than go unrecorded.
pub struct RecordedEdges<I> {
    inner: I,
    taken: heapless::Vec<Edge, 16>,
}

impl<I> RecordedEdges<I> {
    pub fn new(inner: I) -> Self {
        Self {
            inner,
            taken: heapless::Vec::new(),
        }
    }

    /// Record the edges taken since the last call.
    pub fn record<R: Record + ?Sized>(&mut self, record: &mut R) {
        for edge in self.taken.drain(..) {
            record.record(&Event::Input(edge));
        }
    }
}

impl<I: InputSource> InputSource for RecordedEdges<I> {
    fn controls(&self) -> Controls {
        self.inner.controls()
    }
    fn take_edge(&mut self) -> Option<Edge> {
        if self.taken.is_full() {
            return None;
        }
        let edge = self.inner.take_edge()?;
        let _ = self.taken.push(edge);
        Some(edge)
    }
}

#[cfg(test)]
mod tests;
