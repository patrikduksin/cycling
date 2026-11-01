//! The handles VANA reads, frozen once a pass and recorded into a trace.
//!
//! Composition binds each handle at the start of a pass. Every read during
//! the pass returns the value sampled then, so VANA sees one value of each
//! observation per pass however often it asks. A sampled value is recorded
//! only when it differs from what a replay would present: the last recorded
//! value, aged to this pass. Takes and requests pass through to the device
//! and are recorded as they happen, in call order.
use super::{AntRadio, Encoder, Event, Full, ServerRequest};
use core::cell::RefCell;
use device_api::ant::{self, Ant, AntOperation};
use device_api::ble_transport::{self, Ble};
use device_api::bulk::{self, ReadWrite};
use device_api::input::{Controls, Edge, InputObservation, InputSnapshot, InputSource};
use device_api::observation::{Availability, Error};
use device_api::positioning::{self, Positioning};
use device_api::power::{self, PowerControl};
use device_api::server::{self, AccessPoint, AccessPointConfig, AccessPointState, Reach, Server};
use device_api::sound::{self, Sound};

/// Where a pass's events go.
pub trait Record {
    fn record(&mut self, event: &Event<'_>);
}

/// Where sealed event blocks go, in order.
pub trait Sink {
    /// Take a sealed block; false when there is no room for it, and it is
    /// lost.
    fn block(&mut self, bytes: &[u8]) -> bool;
}

/// Encodes events into blocks and hands each full block to its sink. The
/// header goes ahead of them, written by whoever writes the trace; see
/// [`super::encode_header`].
pub struct Recorder<B, S> {
    encoder: Encoder<B>,
    sink: S,
    lost: Lost,
}

/// Blocks the sink refused since it last took one.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Lost {
    blocks: u32,
    bytes: u32,
}

impl<B: AsMut<[u8]> + AsRef<[u8]>, S: Sink> Recorder<B, S> {
    /// `buf` holds one block, of 64 to [`super::MAX_BLOCK`] bytes.
    pub fn new(buf: B, sink: S) -> Self {
        Self {
            encoder: Encoder::new(buf),
            sink,
            lost: Lost::default(),
        }
    }

    /// The block being filled, framed. It stays open.
    pub fn open_block(&mut self) -> &[u8] {
        self.encoder.sealed()
    }

    /// Hand the block being filled to the sink now, so that a reader of the
    /// sink is at most this old; nothing when it holds no event.
    pub fn flush(&mut self) {
        if !self.encoder.is_empty() {
            self.seal();
        }
    }

    pub fn sink(&self) -> &S {
        &self.sink
    }

    pub fn sink_mut(&mut self) -> &mut S {
        &mut self.sink
    }

    fn push(&mut self, event: &Event<'_>) -> Result<(), Full> {
        let result = self.encoder.push(event);
        if result != Err(Full::Block) {
            return result;
        }
        self.seal();
        self.encoder.push(event)
    }

    /// Hand the open block to the sink and open the next one, which says
    /// first what the sink refused since it last took a block.
    fn seal(&mut self) {
        let block = self.encoder.sealed();
        let len = u32::try_from(block.len()).unwrap_or(u32::MAX);
        if self.sink.block(block) {
            self.lost = Lost::default();
        } else {
            self.lost.blocks = self.lost.blocks.saturating_add(1);
            self.lost.bytes = self.lost.bytes.saturating_add(len);
        }
        self.encoder.restart();
        if self.lost != Lost::default() {
            let Lost { blocks, bytes } = self.lost;
            let _ = self.encoder.push(&Event::Lost { blocks, bytes });
        }
    }
}

impl<B: AsMut<[u8]> + AsRef<[u8]>, S: Sink> Record for Recorder<B, S> {
    fn record(&mut self, event: &Event<'_>) {
        if self.push(event) == Err(Full::Record) {
            let _ = self.push(&Event::TooLarge(event.tag()));
        }
    }
}

/// How a value read at one pass reads `ms` later, for the parts its producer
/// computes from the time of the read. Most values do not age.
pub trait Aging: Clone + PartialEq {
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
impl Aging for usize {}
impl Aging for AccessPointState {}
impl Aging for Reach {}
impl Aging for server::Request {}
impl Aging for sound::Snapshot {}
impl Aging for power::Status {}

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
    fn aged(self, ms: u64) -> Self {
        let Some(seconds) = self.unix_seconds else {
            return self;
        };
        let total_ms = seconds
            .saturating_mul(1_000)
            .saturating_add(u64::from(self.millis))
            .saturating_add(ms);
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
        if self.at(now).as_ref() == Some(&value) {
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
        let (value, at) = self.recorded.as_ref()?;
        Some(value.clone().aged(now.saturating_sub(*at)))
    }

    /// The value, recorded as `event` when it changed.
    pub fn observe<R: Record + ?Sized>(
        &mut self,
        value: T,
        now: u64,
        record: &RefCell<R>,
        event: impl FnOnce(T) -> Event<'static>,
    ) -> T {
        if self.changed(value.clone(), now) {
            record.borrow_mut().record(&event(value.clone()));
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

    #[inline(never)]
    fn sample<A: Ant + ?Sized, R: Record + ?Sized>(
        &mut self,
        inner: &A,
        now: u64,
        record: &RefCell<R>,
    ) {
        let view = &mut self.view;
        view.availability = inner.availability();
        view.capabilities = inner.capabilities();
        view.scanning = inner.scanning();
        view.scan = inner.scan();
        view.discoveries = inner.discoveries();
        view.channels = inner.channels(now);
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
            .record(&Event::AntRequest { operation, result });
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

/// The bulk medium, recording each entropy value it gives. Sectors pass
/// through: a replay starts from the media as it stood.
pub struct RecordedBulk<'a, B: ?Sized, R: ?Sized> {
    inner: &'a mut B,
    record: &'a RefCell<R>,
}

impl<'a, B: ?Sized, R: ?Sized> RecordedBulk<'a, B, R> {
    pub fn new(inner: &'a mut B, record: &'a RefCell<R>) -> Self {
        Self { inner, record }
    }
}

impl<B: ReadWrite + ?Sized, R: Record + ?Sized> ReadWrite for RecordedBulk<'_, B, R> {
    fn owned_info(&mut self) -> Result<bulk::OwnedInfo, bulk::Error> {
        self.inner.owned_info()
    }
    fn owned_read(
        &mut self,
        sector: u64,
        output: &mut [u8; bulk::SECTOR_SIZE],
    ) -> Result<(), bulk::Error> {
        self.inner.owned_read(sector, output)
    }
    fn owned_write(
        &mut self,
        sector: u64,
        input: &[u8; bulk::SECTOR_SIZE],
    ) -> Result<(), bulk::Error> {
        self.inner.owned_write(sector, input)
    }
    fn entropy(&mut self) -> u64 {
        let value = self.inner.entropy();
        self.record.borrow_mut().record(&Event::Entropy(value));
        value
    }
}

/// The phone access point as VANA sees it across passes.
#[derive(Default)]
pub struct FrozenAccessPoint {
    latest: Latest<(AccessPointState, Reach)>,
    view: (AccessPointState, Reach),
    redacting: bool,
}

impl FrozenAccessPoint {
    /// Record each entropy word as [`super::redact::ENTROPY`] from now on.
    pub fn redact(&mut self) {
        self.redacting = true;
    }

    /// Sample `inner` for this pass and record what changed.
    pub fn bind<'a, A: AccessPoint + ?Sized, R: Record + ?Sized>(
        &'a mut self,
        inner: &'a mut A,
        now: u64,
        record: &'a RefCell<R>,
    ) -> PassAccessPoint<'a, A, R> {
        let mut pass = PassAccessPoint {
            frozen: self,
            inner,
            now,
            record,
        };
        pass.sample();
        pass
    }
}

/// The access point for one pass. VANA's start and stop are recorded with
/// their answers, and what they changed is sampled right after; each
/// entropy word is recorded as it is given.
pub struct PassAccessPoint<'a, A: ?Sized, R: ?Sized> {
    frozen: &'a mut FrozenAccessPoint,
    inner: &'a mut A,
    now: u64,
    record: &'a RefCell<R>,
}

impl<A: AccessPoint + ?Sized, R: Record + ?Sized> PassAccessPoint<'_, A, R> {
    fn sample(&mut self) {
        let view = (self.inner.state(), self.inner.reach());
        self.frozen.view =
            self.frozen
                .latest
                .observe(view, self.now, self.record, |(state, reach)| {
                    Event::AccessPoint { state, reach }
                });
    }

    fn answered(&mut self, start: bool, result: Result<(), Error>) -> Result<(), Error> {
        self.record
            .borrow_mut()
            .record(&Event::AccessPointRequest { start, result });
        self.sample();
        result
    }
}

impl<A: AccessPoint + ?Sized, R: Record + ?Sized> AccessPoint for PassAccessPoint<'_, A, R> {
    fn start(&mut self, config: AccessPointConfig) -> Result<(), Error> {
        let result = self.inner.start(config);
        self.answered(true, result)
    }
    fn stop(&mut self) -> Result<(), Error> {
        let result = self.inner.stop();
        self.answered(false, result)
    }
    fn state(&self) -> AccessPointState {
        self.frozen.view.0
    }
    fn reach(&self) -> Reach {
        self.frozen.view.1
    }
    fn entropy(&mut self) -> u32 {
        let value = self.inner.entropy();
        let recorded = match self.frozen.redacting {
            true => super::redact::ENTROPY,
            false => value.into(),
        };
        self.record.borrow_mut().record(&Event::Entropy(recorded));
        value
    }
}

/// Body bytes a server read records in one event, so that a read of a whole
/// transport buffer splits into records that each fit a block.
pub const READ_PART: usize = 1024;

/// The phone server as VANA sees it across passes: the request it holds and
/// its room for the response.
#[derive(Default)]
pub struct FrozenServer {
    latest: Latest<(Option<server::Request>, usize)>,
    view: (Option<server::Request>, usize),
    redacting: Option<heapless::Vec<u8, 32>>,
}

impl FrozenServer {
    /// Record session tokens redacted from now on, given the open session's
    /// `token` (see [`super::redact`]); call it each pass before binding.
    pub fn redact(&mut self, token: Option<&[u8]>) {
        let token = token.and_then(|token| heapless::Vec::from_slice(token).ok());
        self.redacting = Some(token.unwrap_or_default());
    }

    /// Sample `inner` for this pass and record what changed.
    pub fn bind<'a, S: Server + ?Sized, R: Record + ?Sized>(
        &'a mut self,
        inner: &'a mut S,
        now: u64,
        record: &'a RefCell<R>,
    ) -> PassServer<'a, S, R> {
        let mut pass = PassServer {
            frozen: self,
            inner,
            now,
            record,
        };
        pass.sample(None);
        pass
    }
}

/// The server for one pass. Reads and writes are recorded with what they
/// moved; a listen or finish is recorded only when it changed what the
/// server holds, just before the change.
pub struct PassServer<'a, S: ?Sized, R: ?Sized> {
    frozen: &'a mut FrozenServer,
    inner: &'a mut S,
    now: u64,
    record: &'a RefCell<R>,
}

impl<S: Server + ?Sized, R: Record + ?Sized> PassServer<'_, S, R> {
    fn sample(&mut self, cause: Option<Event<'_>>) {
        let view = (self.inner.request(), self.inner.room());
        if !self.frozen.latest.changed(view.clone(), self.now) {
            return;
        }
        self.frozen.view = view;
        let mut record = self.record.borrow_mut();
        if let Some(cause) = cause {
            record.record(&cause);
        }
        let (request, room) = &self.frozen.view;
        let room = u32::try_from(*room).unwrap_or(u32::MAX);
        let Some(request) = request else {
            record.record(&Event::Server {
                request: None,
                room,
            });
            return;
        };
        let mut recorded = ServerRequest::from(request);
        let redacted = self.frozen.redacting.as_ref().map(|token| {
            let token = (!token.is_empty()).then_some(token.as_slice());
            (
                super::redact::session::<32>(&request.session, token),
                super::redact::query::<96>(&request.query, token),
            )
        });
        if let Some((session, query)) = &redacted {
            (recorded.session, recorded.query) = (session, query);
        }
        record.record(&Event::Server {
            request: Some(recorded),
            room,
        });
    }
}

impl<S: Server + ?Sized, R: Record + ?Sized> Server for PassServer<'_, S, R> {
    fn listen(&mut self, on: bool) {
        self.inner.listen(on);
        self.sample(Some(Event::ServerListen(on)));
    }
    fn request(&self) -> Option<server::Request> {
        self.frozen.view.0.clone()
    }
    fn read(&mut self, id: u32, out: &mut [u8]) -> usize {
        let n = self.inner.read(id, out);
        let read = &out[..n.min(out.len())];
        let mut parts = read.chunks(READ_PART).peekable();
        let mut record = self.record.borrow_mut();
        if parts.peek().is_none() {
            record.record(&Event::ServerRead {
                id,
                part: &[],
                more: false,
            });
        }
        while let Some(part) = parts.next() {
            let more = parts.peek().is_some();
            record.record(&Event::ServerRead { id, part, more });
        }
        drop(record);
        self.sample(None);
        n
    }
    fn room(&self) -> usize {
        self.frozen.view.1
    }
    fn write(&mut self, id: u32, bytes: &[u8]) -> usize {
        let taken = self.inner.write(id, bytes);
        self.record.borrow_mut().record(&Event::ServerWrite {
            id,
            taken: u32::try_from(taken).unwrap_or(u32::MAX),
        });
        self.sample(None);
        taken
    }
    fn finish(&mut self, id: u32) {
        self.inner.finish(id);
        self.sample(Some(Event::ServerFinish(id)));
    }
}

/// The buzzer as VANA sees it across passes, and the patterns it accepted.
pub struct FrozenSound {
    latest: Latest<sound::Snapshot>,
    view: sound::Snapshot,
    started: heapless::Vec<u8, 4>,
}

impl Default for FrozenSound {
    fn default() -> Self {
        Self {
            latest: Latest::default(),
            view: sound::Snapshot {
                state: sound::State::Idle,
                pattern: None,
                requested_ms: 0,
                submitted_ms: None,
                until_ms: 0,
            },
            started: heapless::Vec::new(),
        }
    }
}

impl FrozenSound {
    /// Sample `inner` for this pass and record what changed.
    pub fn bind<'a, S: Sound + ?Sized, R: Record + ?Sized>(
        &'a mut self,
        inner: &'a mut S,
        now: u64,
        record: &'a RefCell<R>,
    ) -> PassSound<'a, S, R> {
        let mut pass = PassSound {
            frozen: self,
            inner,
            now,
            record,
        };
        pass.sample();
        pass
    }

    /// The patterns the buzzer accepted since the last call, in order.
    pub fn take_started(&mut self) -> heapless::Vec<u8, 4> {
        core::mem::take(&mut self.started)
    }
}

/// The buzzer for one pass. The pattern list is the build's own and passes
/// through.
pub struct PassSound<'a, S: ?Sized, R: ?Sized> {
    frozen: &'a mut FrozenSound,
    inner: &'a mut S,
    now: u64,
    record: &'a RefCell<R>,
}

impl<S: Sound + ?Sized, R: Record + ?Sized> PassSound<'_, S, R> {
    fn sample(&mut self) {
        self.frozen.view = self.frozen.latest.observe(
            self.inner.snapshot(),
            self.now,
            self.record,
            Event::SoundState,
        );
    }

    fn answered(&mut self, play: Option<u8>, result: Result<(), Error>) -> Result<(), Error> {
        self.record
            .borrow_mut()
            .record(&Event::SoundRequest { play, result });
        if let (Some(id), Ok(())) = (play, result) {
            let _ = self.frozen.started.push(id);
        }
        self.sample();
        result
    }
}

impl<S: Sound + ?Sized, R: Record + ?Sized> Sound for PassSound<'_, S, R> {
    fn patterns(&self) -> &'static [sound::Pattern] {
        self.inner.patterns()
    }
    fn snapshot(&self) -> sound::Snapshot {
        self.frozen.view
    }
    fn play(&mut self, id: u8, now: u64) -> Result<(), Error> {
        let result = self.inner.play(id, now);
        self.answered(Some(id), result)
    }
    fn stop(&mut self, now: u64) -> Result<(), Error> {
        let result = self.inner.stop(now);
        self.answered(None, result)
    }
}

/// Power control as composition and VANA see it across passes.
pub struct FrozenPowerControl {
    latest: Latest<power::Status>,
    view: power::Status,
}

impl Default for FrozenPowerControl {
    fn default() -> Self {
        Self {
            latest: Latest::default(),
            view: power::Status::IDLE,
        }
    }
}

impl FrozenPowerControl {
    /// Sample `inner` and record it when it changed; the status this pass
    /// reads until VANA's own request.
    pub fn freeze<P: PowerControl + ?Sized, R: Record + ?Sized>(
        &mut self,
        inner: &P,
        now: u64,
        record: &RefCell<R>,
    ) -> power::Status {
        self.view = self
            .latest
            .observe(inner.status(), now, record, Event::PowerControl);
        self.view
    }

    /// The status as frozen, for VANA's requests this pass.
    pub fn bind<'a, P: PowerControl + ?Sized, R: Record + ?Sized>(
        &'a mut self,
        inner: &'a mut P,
        now: u64,
        record: &'a RefCell<R>,
    ) -> PassPowerControl<'a, P, R> {
        PassPowerControl {
            frozen: self,
            inner,
            now,
            record,
        }
    }
}

/// Power control for one pass. Capabilities pass through: VANA does not
/// read them.
pub struct PassPowerControl<'a, P: ?Sized, R: ?Sized> {
    frozen: &'a mut FrozenPowerControl,
    inner: &'a mut P,
    now: u64,
    record: &'a RefCell<R>,
}

impl<P: PowerControl + ?Sized, R: Record + ?Sized> PassPowerControl<'_, P, R> {
    fn answered(
        &mut self,
        operation: power::Operation,
        delay_ms: u32,
        result: Result<(), Error>,
    ) -> Result<(), Error> {
        self.record.borrow_mut().record(&Event::PowerRequest {
            operation,
            delay_ms,
            result,
        });
        self.frozen.freeze(self.inner, self.now, self.record);
        result
    }
}

impl<P: PowerControl + ?Sized, R: Record + ?Sized> PowerControl for PassPowerControl<'_, P, R> {
    fn capabilities(&self) -> power::Capabilities {
        self.inner.capabilities()
    }
    fn status(&self) -> power::Status {
        self.frozen.view
    }
    fn request(&mut self, operation: power::Operation) -> Result<(), Error> {
        let result = self.inner.request(operation);
        self.answered(operation, 0, result)
    }
    fn request_after(&mut self, operation: power::Operation, delay_ms: u32) -> Result<(), Error> {
        let result = self.inner.request_after(operation, delay_ms);
        self.answered(operation, delay_ms, result)
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
