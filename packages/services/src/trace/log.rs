//! The world log of a session: the journal every recorder of a composition
//! writes to.
//!
//! A [`Log`] numbers the passes, turns each call a recorder reports into an
//! [`Event::Call`], records media as [`super::media`] says and replaces
//! secrets as [`super::redact`] says. It keeps counting passes while its
//! recorder is off, so the recorders freeze what VANA reads either way.
use super::determined::Span;
use super::media::{self, Class, FlashError, Memory, Place, Run, Volumes};
use super::recorder::{Record, Recorder, Sink};
use super::wire::{Wire, Writer};
use super::{AntCapabilities, Call, Cost, Event, Medium, ServerRequest, StorageError, redact};
use core::ops::DerefMut;
use device_api::connectivity::{Text, WifiConfig, WifiOperation, WifiRedact};
use device_api::journal::{Journal, Media, MethodId, Pass, Put};
use device_api::observation::Error;
use device_api::server::{AccessPointConfig, AccessPointRedact, Request, ServerRedact};
use device_api::storage::Region;

/// A button or touch report: its block is sealed as soon as it is
/// recorded, so a reader of the spool sees the press at once.
const EDGE: MethodId = MethodId {
    capability: 7,
    operation: 1,
};

/// The open session's phone token, to tell it from another in a request.
type Token = heapless::Vec<u8, 32>;

pub struct Log<B, S, M> {
    pub recorder: Option<Recorder<B, S>>,
    memory: M,
    pass: Pass,
    open: Option<Open>,
    volumes: Volumes,
    /// Secrets are replaced as they are recorded; with the open session's
    /// token, when there is one.
    redacting: Option<Token>,
    cost: Cost,
    held: Option<Held>,
    /// The working memory the open trace recorded.
    session: Option<super::origin::Memory>,
}

/// The record of a repeated request, written only if another record
/// follows before it is released.
struct Held {
    method: MethodId,
    body: heapless::Vec<u8, HELD>,
}

/// The longest record body that can be held back; a longer one is written.
const HELD: usize = 16;

#[derive(Clone, Copy)]
struct Open {
    method: MethodId,
    len: usize,
    overflow: bool,
}

impl Open {
    fn new(method: MethodId) -> Self {
        Self {
            method,
            len: 0,
            overflow: false,
        }
    }
}

impl<B, S, M> Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    pub fn new(recorder: Option<Recorder<B, S>>, memory: M) -> Self {
        Self {
            recorder,
            memory,
            pass: Pass {
                number: 0,
                now_ms: 0,
            },
            open: None,
            volumes: Volumes::default(),
            redacting: None,
            cost: Cost::default(),
            held: None,
            session: None,
        }
    }

    /// A pass begins at `now`. A `quiet` one is recorded only when something
    /// else is recorded in it.
    pub fn begin_pass(&mut self, now: u64, quiet: bool) {
        self.flush_run();
        self.pass = Pass {
            number: self.pass.number.wrapping_add(1),
            now_ms: now,
        };
        self.memory.determined.next_pass();
        let Some(recorder) = &mut self.recorder else {
            return;
        };
        if quiet {
            recorder.quiet_pass(now);
        } else {
            recorder.record(&Event::Pass { now_ms: now });
        }
    }

    /// A terminal command begins between two passes. What it reads is
    /// sampled anew, and is recorded after its [`Event::Command`].
    pub fn begin_command(&mut self) {
        self.flush_run();
        self.pass.number = self.pass.number.wrapping_add(1);
    }

    /// One of the trace's own records.
    pub fn record(&mut self, event: &Event<'_>) {
        self.flush_run();
        self.recorder.record(event);
    }

    /// The trace opens: record the working memory composition gave VANA,
    /// and whether secrets are replaced. Call it before anything else is
    /// recorded.
    pub fn open(&mut self, memory: super::origin::Memory) {
        self.session = Some(memory);
        self.record(&Event::Session {
            memory,
            redacted: self.redacts(),
        });
    }

    /// Replace secrets from now on, as [`super::redact`] says.
    pub fn redact(&mut self) {
        if self.redacts() {
            return;
        }
        self.redacting = Some(Token::new());
        if let Some(memory) = self.session {
            self.record(&Event::Session {
                memory,
                redacted: true,
            });
        }
    }

    /// Whether secrets are replaced as they are recorded.
    pub fn redacts(&self) -> bool {
        self.redacting.is_some()
    }

    /// The open phone session's token, when this log redacts: call it each
    /// pass before the server is sampled.
    pub fn session_token(&mut self, token: Option<&[u8]>) {
        if let Some(kept) = &mut self.redacting {
            *kept = token
                .and_then(|token| Token::from_slice(token).ok())
                .unwrap_or_default();
        }
    }

    /// Which card sectors are TERRAIN and STORE, once the volume table reads.
    pub fn volumes(&mut self, volumes: Volumes) {
        self.volumes = volumes;
    }

    /// A host wrote `medium` unseen: nothing of it is determined.
    pub fn forget(&mut self, medium: Medium) {
        self.flush_run();
        self.memory.determined.forget(medium);
        self.recorder.record(&Event::Forget(medium));
    }

    /// Record what the determined set cost so far, when it changed.
    pub fn cost(&mut self) {
        let determined = &self.memory.determined;
        let cost = Cost {
            evictions: determined.evictions(),
            high_water: determined.high_water(),
        };
        if cost != self.cost {
            self.cost = cost;
            self.record(&Event::Cost(cost));
        }
    }

    /// Stop recording: the passes still count.
    pub fn off(&mut self) {
        self.recorder = None;
        self.memory.run = None;
    }

    fn write(&mut self, put: impl FnOnce(&mut Writer)) {
        let Some(open) = &mut self.open else {
            return;
        };
        let mut w = Writer::new(&mut self.memory.scratch[open.len..], self.pass.now_ms);
        put(&mut w);
        open.len += w.len;
        open.overflow |= w.overflow;
    }

    /// Flush the held request before the media run to preserve recording order.
    fn flush_run(&mut self) {
        if let Some(held) = self.held.take() {
            self.recorder.record(&Event::Call(Call {
                method: held.method,
                now_ms: self.pass.now_ms,
                body: &held.body,
            }));
        }
        let Some(run) = self.memory.run.take() else {
            return;
        };
        let each = run.each.filter(|&each| each != run.span.count);
        let flags = media::DIGEST | each.map_or(0, |_| media::EACH);
        self.media_record(run.method, flags, run.span, |w| {
            w.bytes(&run.crc.to_le_bytes());
            if let Some(each) = each {
                each.put(w);
            }
        });
    }

    fn media_record(
        &mut self,
        method: MethodId,
        flags: u8,
        span: Span,
        rest: impl FnOnce(&mut Writer),
    ) {
        self.open = Some(Open::new(method));
        self.write(|w| {
            media::put_span(w, flags, span);
            rest(w);
        });
        self.close();
        if flags & media::FAULT != 0
            && let Some(recorder) = &mut self.recorder
        {
            recorder.flush();
        }
    }

    fn close(&mut self) {
        let Some(open) = self.open.take() else {
            return;
        };
        let Some(recorder) = &mut self.recorder else {
            return;
        };
        if open.overflow {
            recorder.record(&Event::TooLarge {
                capability: open.method.capability,
                operation: open.method.operation,
            });
            return;
        }
        recorder.record(&Event::Call(Call {
            method: open.method,
            now_ms: self.pass.now_ms,
            body: &self.memory.scratch[..open.len],
        }));
        if open.method == EDGE {
            recorder.flush();
        }
    }

    /// `each`: the count of every read the run joins; `None` for writes.
    fn run(&mut self, method: MethodId, span: Span, bytes: &[u8], each: Option<u32>) {
        let crc = self.memory.crc;
        if let Some(run) = &mut self.memory.run
            && run.extend(method, span, bytes, crc)
        {
            return;
        }
        self.flush_run();
        self.memory.run = Some(Run {
            method,
            span,
            crc: crc(0, bytes),
            each,
        });
    }

    fn outside(&mut self, method: MethodId, span: Span, fault: Option<StorageError>) {
        self.flush_run();
        let flags = media::OUTSIDE | fault.map_or(0, |_| media::FAULT);
        self.media_record(method, flags, span, |w| {
            if let Some(fault) = fault {
                fault.put(w);
            }
        });
    }

    fn inline(
        &mut self,
        method: MethodId,
        place: Place,
        bytes: &[u8],
        fault: Option<StorageError>,
    ) {
        self.flush_run();
        let mut flags = media::INLINE | fault.map_or(0, |_| media::FAULT);
        let units = (media::PART / place.unit).max(1);
        let mut start = place.span.start;
        for part in bytes.chunks(units * place.unit) {
            let span = Span {
                start,
                count: (part.len() / place.unit) as u32,
                ..place.span
            };
            start = start.saturating_add(span.count);
            self.media_record(method, flags, span, |w| {
                media::elide(w, part);
                if let Some(fault) = fault {
                    fault.put(w);
                }
            });
            flags |= media::CONTINUES;
        }
    }

    fn read(&mut self, method: MethodId, place: Place, bytes: &[u8], fault: Option<StorageError>) {
        if self.recorder.is_none() {
            return;
        }
        match (place.class, fault) {
            (Class::Outside, _) => self.outside(method, place.span, fault),
            (_, Some(_)) => self.inline(method, place, bytes, fault),
            (Class::DigestOnly, None) => {
                self.run(method, place.span, bytes, Some(place.span.count));
            }
            (Class::Inline, None) if self.memory.determined.covers(place.span) => {
                self.run(method, place.span, bytes, Some(place.span.count));
            }
            (Class::Inline, None) => {
                self.inline(method, place, bytes, None);
                self.memory.determined.insert(place.span);
            }
            (Class::Volatile, None) => self.inline(method, place, bytes, None),
        }
    }

    fn wrote(&mut self, method: MethodId, place: Place, bytes: &[u8], fault: Option<StorageError>) {
        if self.recorder.is_none() {
            return;
        }
        match (place.class, fault) {
            (Class::Outside, _) => self.outside(method, place.span, fault),
            (_, Some(fault)) => {
                let attempted = (self.memory.crc)(0, bytes);
                self.failed(method, place.span, Some(attempted), fault);
            }
            (class, None) => {
                self.run(method, place.span, bytes, None);
                let determined = &mut self.memory.determined;
                if class == Class::Inline {
                    if !place.nor {
                        determined.insert(place.span);
                    } else if !determined.covers(place.span) {
                        determined.remove(place.span);
                    }
                }
            }
        }
    }

    fn erased(&mut self, method: MethodId, place: Place, fault: Option<StorageError>) {
        if self.recorder.is_none() {
            return;
        }
        match (place.class, fault) {
            (Class::Outside, _) => self.outside(method, place.span, fault),
            (_, Some(fault)) => self.failed(method, place.span, None, fault),
            (class, None) => {
                self.flush_run();
                self.media_record(method, 0, place.span, |_| {});
                if class == Class::Inline {
                    self.memory.determined.insert(place.span);
                }
            }
        }
    }

    fn failed(
        &mut self,
        method: MethodId,
        span: Span,
        attempted: Option<u32>,
        fault: StorageError,
    ) {
        self.flush_run();
        self.memory.determined.remove(span);
        let flags = media::FAULT | attempted.map_or(0, |_| media::DIGEST);
        self.media_record(method, flags, span, |w| {
            if let Some(crc) = attempted {
                w.bytes(&crc.to_le_bytes());
            }
            fault.put(w);
        });
    }
}

impl<B, S, M> Record for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn record(&mut self, event: &Event<'_>) {
        Log::record(self, event);
    }
}

impl<B, S, M> Journal for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn pass(&self) -> Pass {
        self.pass
    }

    fn begin(&mut self, method: MethodId) {
        self.flush_run();
        self.open = Some(Open::new(method));
    }

    fn end(&mut self) {
        self.close();
    }

    fn hold(&mut self) {
        let held = self.open.filter(|open| !open.overflow).and_then(|open| {
            let body = heapless::Vec::from_slice(&self.memory.scratch[..open.len]).ok()?;
            Some(Held {
                method: open.method,
                body,
            })
        });
        match held {
            Some(held) => {
                self.open = None;
                self.held = Some(held);
            }
            None => self.close(),
        }
    }

    fn release(&mut self) -> bool {
        self.held.take().is_some()
    }

    fn repeated(&mut self, method: MethodId, count: u8) {
        self.record(&Event::Repeated { method, count });
    }
}

impl<B, S, M, T> Put<T> for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
    T: for<'a> Wire<'a>,
{
    fn put(&mut self, value: &T) {
        self.write(|w| value.put(w));
    }
}

impl<B, S, M> Put<[u8]> for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn put(&mut self, value: &[u8]) {
        self.write(|w| value.put(w));
    }
}

impl<B, S, M> Put<device_api::ant::Capabilities> for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn put(&mut self, value: &device_api::ant::Capabilities) {
        self.write(|w| AntCapabilities::from(*value).put(w));
    }
}

impl<B, S, M> Media<u64, Result<(), device_api::bulk::Error>> for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn read(
        &mut self,
        method: MethodId,
        sector: u64,
        bytes: &[u8],
        outcome: &Result<(), device_api::bulk::Error>,
    ) {
        let place = Place::card(&self.volumes, sector, bytes.len());
        self.read(
            method,
            place,
            bytes,
            outcome.err().map(StorageError::from_bulk),
        );
    }

    fn wrote(
        &mut self,
        method: MethodId,
        sector: u64,
        bytes: &[u8],
        outcome: &Result<(), device_api::bulk::Error>,
    ) {
        let place = Place::card(&self.volumes, sector, bytes.len());
        self.wrote(
            method,
            place,
            bytes,
            outcome.err().map(StorageError::from_bulk),
        );
    }

    fn erased(
        &mut self,
        method: MethodId,
        sector: u64,
        length: usize,
        outcome: &Result<(), device_api::bulk::Error>,
    ) {
        let place = Place::card(&self.volumes, sector, length);
        self.erased(method, place, outcome.err().map(StorageError::from_bulk));
    }
}

impl<B, S, M, E> Media<(Region, usize), Result<(), E>> for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
    E: FlashError,
{
    fn read(
        &mut self,
        method: MethodId,
        at: (Region, usize),
        bytes: &[u8],
        outcome: &Result<(), E>,
    ) {
        let place = Place::flash(at.0, at.1, bytes.len());
        let fault = outcome.as_ref().err().map(FlashError::storage_error);
        self.read(method, place, bytes, fault);
    }

    fn wrote(
        &mut self,
        method: MethodId,
        at: (Region, usize),
        bytes: &[u8],
        outcome: &Result<(), E>,
    ) {
        let place = Place::flash(at.0, at.1, bytes.len());
        let fault = outcome.as_ref().err().map(FlashError::storage_error);
        self.wrote(method, place, bytes, fault);
    }

    fn erased(
        &mut self,
        method: MethodId,
        at: (Region, usize),
        length: usize,
        outcome: &Result<(), E>,
    ) {
        let place = Place::flash(at.0, at.1, length);
        let fault = outcome.as_ref().err().map(FlashError::storage_error);
        self.erased(method, place, fault);
    }
}

impl<B, S, M> AccessPointRedact for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    /// The network's name and password come from the entropy the trace
    /// holds, so only the answer is recorded.
    fn start(&mut self, _: &AccessPointConfig, value: &Result<(), Error>) {
        self.write(|w| value.put(w));
    }

    fn entropy(&mut self, value: &u32) {
        let recorded = match self.redacting {
            Some(_) => redact::ENTROPY,
            None => u64::from(*value),
        };
        self.write(|w| recorded.put(w));
    }
}

impl<B, S, M> ServerRedact for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn request(&mut self, value: &Option<Request>) {
        let redacted = value
            .as_ref()
            .zip(self.redacting.as_ref())
            .map(|(request, token)| {
                let token = (!token.is_empty()).then_some(token.as_slice());
                (
                    redact::session::<32>(&request.session, token),
                    redact::query::<96>(&request.query, token),
                )
            });
        let recorded = value.as_ref().map(|request| {
            let mut recorded = ServerRequest::from(request);
            if let Some((session, query)) = &redacted {
                (recorded.session, recorded.query) = (session, query);
            }
            recorded
        });
        self.write(|w| recorded.put(w));
    }
}

impl<B, S, M> WifiRedact for Log<B, S, M>
where
    B: AsMut<[u8]> + AsRef<[u8]>,
    S: Sink,
    M: DerefMut<Target = Memory>,
{
    fn request(&mut self, operation: &WifiOperation, value: &Result<(), Error>) {
        let redacting = self.redacting.is_some();
        self.write(|w| {
            wifi_operation(w, operation, redacting);
            value.put(w);
        });
    }
}

/// A Wi-Fi request as the trace records it.
pub fn wifi_operation(w: &mut Writer, operation: &WifiOperation, redacting: bool) {
    match operation {
        WifiOperation::Scan => w.byte(0),
        WifiOperation::Configure(config) => {
            w.byte(1);
            config.map(|config| wifi_config(config, redacting)).put(w);
        }
        WifiOperation::Connect => w.byte(2),
        WifiOperation::Disconnect => w.byte(3),
    }
}

/// A Wi-Fi configuration as the trace records it: its network, the length
/// of its password, and the password itself only when nothing is redacted.
fn wifi_config(config: WifiConfig, redacting: bool) -> (Text<32>, Text<63>, bool) {
    let password = match redacting {
        true => redact::wifi_password(config.password),
        false => config.password,
    };
    (config.ssid, password, config.wpa3)
}

#[cfg(test)]
mod tests;
