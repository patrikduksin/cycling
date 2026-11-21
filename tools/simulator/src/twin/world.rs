//! The recorded world a replay runs in: the trace's world records,
//! read a pass at a time, answering every server of the twin.
//!
//! A segment is what one pass recorded, or one terminal command, or the
//! boot before the first pass. Its calls and observations go to a
//! [`script::Pass`], its media are the [`Model`] the trace's index gives,
//! and the few inputs that are no capability's (network time, the
//! power going away) are kept beside them.
//!
//! A call the trace holds no answer for has left the recorded world. The
//! world notes why and unwinds with [`Left`], which ends the replay at that
//! pass: there is no value it could hand VANA instead. Unless it models:
//! then it hands VANA the nearest value the trace holds, labels the stop,
//! and every later pass is on that branch.
use super::index::{Index, Model};
use super::media::Change;
use super::script::{self, Named, Record};
use crate::replay::{Divergence, Exclusion, Faithful, Kind, Mode, Next, Options, Report};
use crate::trace::card::Pack;
use device_api::capability::{Keys, Kind as Declared};
use device_api::connectivity::{Text, WifiOperation, WifiRestore};
use device_api::input::Edge;
use device_api::journal::Latest;
use device_api::journal::{Aging, Get, MethodId, Pass, Put, Script, Stored};
use device_api::observation::Error;
use device_api::server::{AccessPointConfig, AccessPointRestore, Request, ServerRestore};
use device_api::storage::Region;
use firmware_services::storage::Record as Saved;
use firmware_services::trace::determined::Span;
use firmware_services::trace::media::{Access, Volumes};
use firmware_services::trace::replayed::Replayed;
use firmware_services::trace::wire::{Reader, Wire, Writer};
use firmware_services::trace::{self, End, Event, Events, Medium, ServerRequest, StorageError};
use firmware_shell::preferences::{Settings, Source};
use firmware_shell::storage::{Error as SettingsError, Loaded, SettingsRestore};
use std::collections::BTreeMap;

type Records = BTreeMap<MethodId, Vec<(u64, Vec<u8>)>>;

/// The payload a replay unwinds with when it leaves the recorded world.
pub struct Left;

/// The settings capability's id: its records are the semantic guarantee.
pub const SETTINGS: u8 = firmware_shell::storage::CAPABILITY_SETTINGS.id;

struct Open {
    method: MethodId,
    args: Vec<u8>,
    /// The record's bytes after the arguments, how far they are read, and
    /// the pass they were recorded in.
    answer: Option<(Vec<u8>, usize, u64)>,
}

pub struct Recorded {
    trace: Vec<u8>,
    cursor: trace::Cursor,
    segment: Option<script::Pass>,
    view: script::View,
    pass: Pass,
    open: Option<Open>,
    media: Model,
    terrain: trace::origin::Found<trace::origin::Terrain>,
    redacted: bool,
    /// The twin's open phone session, which a redacted recording's
    /// placeholder for its own session's token stands for.
    pub token: Option<Text<32>>,
    network_time: Latest<firmware_services::network_time::Snapshot>,
    powered_off: bool,
    forgotten: Vec<Medium>,
    /// The recording began replacing secrets after the segment loaded.
    redacting: bool,
    report: Report,
    stopped: bool,
    parted: Option<(u64, String)>,
    settings: Option<String>,
    lost: Option<u64>,
    /// The last record of each request that repeats: what a call the device
    /// left unrecorded gave.
    repeated: BTreeMap<MethodId, Vec<u8>>,
    model: bool,
    recorded: Option<Records>,
}

impl Recorded {
    /// The world `trace` recorded, with its boot segment loaded.
    pub fn open(
        trace: Vec<u8>,
        pack: Option<Pack>,
        options: Options,
    ) -> Result<Self, super::index::Error> {
        let (header, events) = trace::decode(&trace)?;
        let terrain = header.terrain;
        let redacted = crate::trace::world::opened(&trace).redacted;
        let cursor = events.cursor();
        let mut sector = [0u8; device_api::bulk::SECTOR_SIZE];
        let volumes = Volumes::read(&mut crate::trace::card::of(&trace).host(), &mut sector);
        let check_digests = options.mode == Mode::Faithful;
        let media = Model::new(Index::of(&trace, volumes)?, pack, check_digests);
        let mut world = Self {
            trace,
            cursor,
            segment: None,
            view: script::View::default(),
            pass: Pass {
                number: 0,
                now_ms: 0,
            },
            open: None,
            media,
            terrain,
            redacted,
            token: None,
            network_time: Latest::default(),
            powered_off: false,
            forgotten: Vec::new(),
            redacting: false,
            report: Report::new(options.mode),
            stopped: false,
            parted: None,
            settings: None,
            lost: None,
            repeated: BTreeMap::new(),
            model: options.model,
            recorded: None,
        };
        if let Some(conflict) = world.media.conflict() {
            let why = format!("a recording bug: {conflict}");
            world.note(Kind::Unreplayable, why);
        }
        world.load(0);
        Ok(world)
    }

    pub fn now(&self) -> u64 {
        self.pass.now_ms
    }

    pub fn redacted(&self) -> bool {
        self.redacted
    }

    /// The TERRAIN pack the recorded session found.
    pub fn terrain(&self) -> trace::origin::Found<trace::origin::Terrain> {
        self.terrain
    }

    /// Whether the replay ended, with its trace or before it.
    pub fn stopped(&self) -> bool {
        self.stopped
    }

    /// The device's power went away in this pass.
    pub fn powered_off(&self) -> bool {
        self.powered_off
    }

    pub fn network_time(&self) -> firmware_services::network_time::Snapshot {
        self.network_time.at(self.pass.now_ms).unwrap_or_default()
    }

    /// The media a host wrote unseen after the segment that just ran. The
    /// twin knows nothing of them from here on, and the session's own log
    /// forgets them too.
    pub fn forget(&mut self) -> Vec<Medium> {
        let forgotten = core::mem::take(&mut self.forgotten);
        for medium in &forgotten {
            self.media.forget(*medium);
        }
        forgotten
    }

    /// Whether the recording began replacing secrets after the segment
    /// that just ran: the twin does from here on, as its session must.
    pub fn take_redacting(&mut self) -> bool {
        let redacting = core::mem::take(&mut self.redacting);
        self.redacted |= redacting;
        redacting
    }

    /// Something read an input the trace holds no value for, and went on
    /// with a stand-in.
    pub fn unrecorded(&mut self, input: &str) {
        self.note(Kind::Unrecorded, format!("unrecorded input {input}"));
    }

    fn note(&mut self, kind: Kind, detail: String) {
        self.report.now = self.pass.now_ms;
        self.report.note(kind, 1, || detail);
    }

    /// End the segment that ran and load the next one.
    pub fn advance(&mut self) -> Next {
        if self.stopped {
            return Next::End;
        }
        self.close();
        if self.stopped {
            return Next::End;
        }
        let mut events = Events::resume(&self.trace, self.cursor);
        let (now_ms, next) = match events.next() {
            Some(Event::Pass { now_ms }) => (now_ms, Next::Pass),
            Some(Event::Command(line)) => (self.pass.now_ms, Next::Command(line.into())),
            _ => {
                if events.end() == Some(End::Corrupt) {
                    self.note(
                        Kind::Truncated,
                        "the trace is damaged after this pass; the rest does not read".into(),
                    );
                }
                self.stopped = true;
                return Next::End;
            }
        };
        self.cursor = events.cursor();
        self.pass.number += 1;
        self.load(now_ms);
        match self.stopped {
            true => Next::End,
            false => next,
        }
    }

    fn load(&mut self, now_ms: u64) {
        self.pass.now_ms = now_ms;
        self.powered_off = false;
        let mut records = Vec::new();
        let mut events = Events::resume(&self.trace, self.cursor);
        let mut loaded = Vec::new();
        let mut repeats = Vec::new();
        loop {
            let before = events.cursor();
            match events.next() {
                None => break,
                Some(Event::Pass { .. } | Event::Command(_)) => {
                    events = Events::resume(&self.trace, before);
                    break;
                }
                Some(Event::Call(call)) => loaded.push((call.method, call.body.to_vec())),
                Some(Event::Repeated { method, count }) => {
                    repeats.push((loaded.len(), method, count))
                }
                Some(Event::Forget(medium)) => self.forgotten.push(medium),
                Some(Event::PoweredOff) => self.powered_off = true,
                Some(Event::Session { redacted: true, .. }) => self.redacting = !self.redacted,
                Some(Event::NetworkTime(snapshot)) => self.network_time.set(snapshot, now_ms),
                Some(event) if event.ends_replay() => self.lost = Some(now_ms),
                Some(_) => {}
            }
        }
        // Blocks the recording lost are missing just before the event that
        // says so, which opens the next block it kept: this segment's end
        // may be among them.
        let mut after = Events::resume(&self.trace, events.cursor());
        let cut = after
            .by_ref()
            .find(|event| !matches!(event, Event::Pass { .. }))
            .is_some_and(|event| matches!(event, Event::Lost { .. }));
        if cut {
            self.lost = Some(now_ms);
        }
        self.cursor = events.cursor();
        if let Some(pass_ms) = self.lost {
            self.part(
                Kind::Lost,
                format!("the recording lost world records at {pass_ms} ms; the replay ends there"),
            );
            return;
        }
        let mut repeats = repeats.into_iter().peekable();
        for (index, (method, body)) in loaded.into_iter().enumerate() {
            while let Some((_, method, count)) = repeats.next_if(|(at, ..)| *at <= index) {
                records.push(Record::Repeated { method, count });
            }
            match script::declared(method).map(|(_, declared)| declared.kind) {
                Some(Declared::Media(_)) => self.check_media(method, &body, now_ms),
                Some(Declared::Observe(Keys::Unkeyed | Keys::All(_))) => {
                    records.push(Record::Observed { method, body })
                }
                Some(
                    Declared::Observe(Keys::Asked)
                    | Declared::Take { .. }
                    | Declared::Request { .. }
                    | Declared::Sink { .. }
                    | Declared::Command,
                ) => records.push(Record::Called { method, body }),
                Some(Declared::Derived | Declared::Unrecorded(_)) | None => self.note(
                    Kind::Unreplayable,
                    format!("a record of {}, which no capability records", Named(method)),
                ),
            }
        }
        records.extend(repeats.map(|(_, method, count)| Record::Repeated { method, count }));
        let view = core::mem::take(&mut self.view);
        self.segment = Some(script::Pass::new(now_ms, view, records));
    }

    fn check_media(&mut self, method: MethodId, body: &[u8], now_ms: u64) {
        let call = trace::Call {
            method,
            now_ms,
            body,
        };
        let Some(access) = Access::of(&call) else {
            return self.note(
                Kind::Unreplayable,
                format!("a media record of {} that does not decode", Named(method)),
            );
        };
        let len = access.span.count as usize * access.span.medium.unit();
        if access.expand(&mut vec![0u8; len]).is_none()
            && matches!(
                access.content,
                firmware_services::trace::media::Content::Inline(_)
            )
        {
            self.note(
                Kind::Unreplayable,
                format!("the bytes of a {} record do not decode", Named(method)),
            );
        }
    }

    fn close(&mut self) {
        self.media.end_pass();
        let Some(segment) = self.segment.take() else {
            return;
        };
        let (view, left) = segment.finish();
        self.view = view;
        if left.is_empty() {
            return;
        }
        let stop = script::Stop::Unconsumed(left);
        match self.model {
            true => self.branch(Kind::NotRequested, stop.to_string()),
            false => self.part(Kind::LeftOver, stop.to_string()),
        }
    }

    /// Where the replay parted from its recording so far, and what it
    /// proved by recording `replayed`: the check runs over the passes it
    /// has run.
    pub fn report(&self, replayed: &[u8]) -> Report {
        let mut report = self.report.clone();
        let running = !self.stopped || self.parted.is_some();
        let checked = super::check::rerecorded(&self.trace, replayed, self.redacted, running);
        if let Some((pass_ms, difference)) = &checked.difference {
            if (report.first.as_ref()).is_none_or(|first| *pass_ms <= first.at_ms) {
                report.first = None;
            }
            let kind = match checked.frame {
                true => Kind::Seen,
                false => Kind::Rerecorded,
            };
            report.now = *pass_ms;
            report.note(kind, 1, || difference.clone());
        }
        report.note(
            Kind::Redacted,
            checked.redacted_frames as usize,
            String::new,
        );
        let exclusions = [
            (checked.redacted_frames > 0)
                .then_some(Exclusion::RedactedFrames(checked.redacted_frames)),
            (checked.refused_frames > 0)
                .then_some(Exclusion::RefusedFrames(checked.refused_frames)),
            self.settings.clone().map(Exclusion::SettingsJournal),
            (self.media.terrain_reads > 0)
                .then_some(Exclusion::TerrainByDigest(self.media.terrain_reads)),
            (self.media.outside > 0).then_some(Exclusion::Outside(self.media.outside)),
            self.lost.map(|pass_ms| Exclusion::Lost { pass_ms }),
        ];
        let ended = match (&checked.difference, &self.parted) {
            (Some(difference), Some(parted)) if difference.0 > parted.0 => Some(parted.clone()),
            (Some(difference), _) => Some(difference.clone()),
            (None, parted) => parted.clone(),
        };
        report.faithful = Some(Faithful {
            physical: checked.physical,
            semantic: checked.semantic,
            ended,
            exclusions: exclusions.into_iter().flatten().collect(),
        });
        report
    }

    /// Open a window: the next divergence becomes the report's first.
    /// Returns the counts before the window.
    pub fn open_window(&mut self) -> BTreeMap<Kind, u32> {
        self.report.first = None;
        self.report.counts.clone()
    }

    /// Why the replay ended before its trace, when it did.
    pub fn parted(&self) -> Option<&str> {
        self.parted.as_ref().map(|(_, why)| why.as_str())
    }

    fn part(&mut self, kind: Kind, why: String) {
        self.parted.get_or_insert((self.pass.now_ms, why.clone()));
        if matches!(kind, Kind::Unrecorded | Kind::LeftOver | Kind::Panicked) {
            self.report.stopped.get_or_insert(Divergence {
                at_ms: self.pass.now_ms,
                kind,
                detail: why.clone(),
            });
        }
        self.note(kind, why);
        self.stopped = true;
    }

    fn stop(&mut self, why: String) -> ! {
        let at = crate::trace::timeline::session_clock(self.pass.now_ms);
        self.part(
            Kind::Unrecorded,
            format!("left the recorded world at {at}: {why}"),
        );
        std::panic::resume_unwind(Box::new(Left))
    }

    /// VANA panicked or broke an invariant in the step that ran.
    pub fn panicked(&mut self, why: &str) {
        self.part(Kind::Panicked, format!("VANA panicked: {why}"));
    }

    fn branch(&mut self, kind: Kind, why: String) {
        let at = crate::trace::timeline::session_clock(self.pass.now_ms);
        let why = format!("{} at {at}: {why}", kind.name());
        self.report.branched.get_or_insert(Divergence {
            at_ms: self.pass.now_ms,
            kind,
            detail: why.clone(),
        });
        self.note(kind, why);
    }

    fn nearest(&mut self, method: MethodId, prefix: &[u8]) -> Option<(u64, Vec<u8>)> {
        let trace = &self.trace;
        let recorded = self.recorded.get_or_insert_with(|| {
            let mut recorded = Records::new();
            let Ok((_, events)) = trace::decode(trace) else {
                return recorded;
            };
            let mut now_ms = 0;
            for event in events {
                match event {
                    Event::Pass { now_ms: at } => now_ms = at,
                    Event::Call(call) => (recorded.entry(call.method).or_default())
                        .push((now_ms, call.body.to_vec())),
                    _ => {}
                }
            }
            recorded
        });
        let records = recorded.get(&method)?;
        let now = self.pass.now_ms;
        let fits = |(_, body): &&(u64, Vec<u8>)| body.starts_with(prefix);
        let before = records.iter().filter(|(at, _)| *at <= now).rev().find(fits);
        let after = || records.iter().filter(|(at, _)| *at > now).find(fits);
        before.or_else(after).cloned()
    }

    fn segment(&mut self) -> &mut script::Pass {
        let ended = "the replay ended: its devices answer nothing after the trace";
        self.segment.as_mut().expect(ended)
    }

    fn opened(&mut self) -> &mut Open {
        self.open.as_mut().expect("a call is open")
    }

    fn write(&mut self, put: impl FnOnce(&mut Writer)) {
        let mut scratch = [0u8; 4096];
        let mut w = Writer::new(&mut scratch, self.pass.now_ms);
        put(&mut w);
        let len = w.written().expect("a call's arguments fit the scratch");
        self.opened().args.extend_from_slice(&scratch[..len]);
    }

    fn read<T>(&mut self, take: impl FnOnce(&mut Reader<'_>) -> T) -> T {
        let open = self.opened();
        let method = open.method;
        let (answer, at, now_ms) = open.answer.as_mut().expect("the call was answered");
        let mut r = Reader::new(&answer[*at..], *now_ms);
        let value = take(&mut r);
        match r.read() {
            Some(read) => {
                *at += read;
                value
            }
            None => self.stop(format!("the record of {} does not decode", Named(method))),
        }
    }

    /// The replayed build's own pages of `medium` that hold a byte the
    /// twin knows.
    pub fn pages(&mut self, medium: Medium) -> Vec<(u64, [u8; super::media::PAGE])> {
        self.media.pages(medium)
    }

    /// The replayed build's own bytes of `medium`, as far as `out` reaches.
    pub fn contents(&mut self, medium: Medium, out: &mut [u8]) {
        self.media.contents(medium, out);
    }

    /// Whether the trace holds an observation of `method` under `key`, at
    /// any time.
    pub fn ever_observed(&mut self, method: MethodId, key: &[u8]) -> bool {
        self.nearest(method, key).is_some()
    }

    /// The latest observation of `method`, when the trace holds one: for a
    /// device that keeps its own value otherwise.
    pub fn observation<T: Replayed + Aging>(&mut self, method: MethodId, key: &[u8]) -> Option<T> {
        let now = self.pass.now_ms;
        let seen = self.segment.as_mut()?.observe(method, key)?;
        let mut r = Reader::new(&seen.value, seen.at_ms);
        let value = T::replayed(&mut r);
        r.read().map(|_| value.aged(now.saturating_sub(seen.at_ms)))
    }

    /// The answer a sink's attempt got, when the device recorded one: its
    /// refusal.
    pub fn refused(
        &mut self,
        method: MethodId,
        attempt: u8,
        args: impl FnOnce(&mut Writer),
    ) -> Option<Error> {
        self.begin(method);
        Put::put(self, &attempt);
        self.write(args);
        let refusal = match self.answered() {
            true => Get::<Result<(), Error>>::get(self).err(),
            false => None,
        };
        self.end();
        refusal
    }

    /// The next input edge the pass took.
    pub fn edge(&mut self, method: MethodId) -> Option<Edge> {
        self.begin(method);
        let edge = self.answered().then(|| self.get());
        self.end();
        edge
    }

    pub fn media_read(
        &mut self,
        method: MethodId,
        medium: Medium,
        start: u32,
        out: &mut [u8],
    ) -> Result<(), StorageError> {
        let count = (out.len() as u64 / medium.unit() as u64) as u32;
        let span = Span {
            medium,
            start,
            count,
        };
        match self.media.read(method, span, out) {
            Ok(read) => read,
            Err(leave @ super::index::Leave::Unknown { .. }) if self.model => {
                self.branch(Kind::Unrecorded, leave.to_string());
                self.media.assume(method, span, out);
                Ok(())
            }
            Err(leave) => self.stop(leave.to_string()),
        }
    }

    pub fn media_write(
        &mut self,
        method: MethodId,
        medium: Medium,
        start: u32,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        let count = (bytes.len() as u64 / medium.unit() as u64) as u32;
        let change = match medium {
            Medium::Bulk => Change::Write(bytes),
            _ => Change::Program(bytes),
        };
        self.mutate(method, medium, start, count, change)
    }

    pub fn media_erase(
        &mut self,
        method: MethodId,
        medium: Medium,
        start: u32,
        length: usize,
    ) -> Result<(), StorageError> {
        let len = length as u64;
        let count = (len / medium.unit() as u64) as u32;
        self.mutate(method, medium, start, count, Change::Erase { len })
    }

    fn mutate(
        &mut self,
        method: MethodId,
        medium: Medium,
        start: u32,
        count: u32,
        change: Change<'_>,
    ) -> Result<(), StorageError> {
        let span = Span {
            medium,
            start,
            count,
        };
        self.media.mutate(method, span, change)
    }
}

impl Script for Recorded {
    fn pass(&self) -> Pass {
        self.pass
    }

    fn begin(&mut self, method: MethodId) {
        self.open = Some(Open {
            method,
            args: Vec::new(),
            answer: None,
        });
    }

    fn answer(&mut self) {
        if self.answered() {
            return;
        }
        let open = self.opened();
        let (method, args) = (open.method, open.args.clone());
        let Err(stop) = self.segment().answer(method, &args) else {
            return;
        };
        let why = stop.to_string();
        let why = why
            .strip_prefix("left the recorded world: ")
            .unwrap_or(&why)
            .to_string();
        let script::Stop::Unanswered(identity) = stop else {
            self.stop(why)
        };
        if !self.model {
            self.stop(why)
        }
        let Some((at_ms, body)) = self.nearest(method, &args) else {
            self.stop(format!("no model for {identity}"))
        };
        self.branch(Kind::Modelled, why);
        let answer = body[args.len()..].to_vec();
        self.opened().answer = Some((answer, 0, at_ms));
    }

    fn answered(&mut self) -> bool {
        let open = self.opened();
        let (method, args) = (open.method, open.args.clone());
        let now_ms = self.pass.now_ms;
        let repeats = matches!(
            script::declared(method).map(|(_, declared)| declared.kind),
            Some(Declared::Request { repeats: true, .. })
        );
        let answer = match self.segment().answer(method, &args) {
            Ok(answer) => {
                if repeats {
                    self.repeated.insert(method, [&args[..], &answer].concat());
                }
                Some(answer)
            }
            Err(_) if repeats => (self.repeated.get(&method))
                .and_then(|last| last.strip_prefix(&args[..]))
                .map(<[u8]>::to_vec),
            Err(_) => None,
        };
        let answered = answer.is_some();
        self.opened().answer = answer.map(|answer| (answer, 0, now_ms));
        answered
    }

    fn observed(&mut self) -> u64 {
        let open = self.opened();
        let (method, key) = (open.method, open.args.clone());
        if let Some(seen) = self.segment().observe(method, &key) {
            self.opened().answer = Some((seen.value, 0, seen.at_ms));
            return seen.at_ms;
        }
        let why = format!("{} was never observed", Named(method));
        let latest = match self.model {
            true => self.nearest(method, &[]),
            false => None,
        };
        let Some((at_ms, body)) = latest else {
            self.stop(why)
        };
        self.branch(Kind::Unrecorded, why);
        let value = body.get(key.len()..).unwrap_or_default().to_vec();
        self.opened().answer = Some((value, 0, at_ms));
        at_ms
    }

    fn fill(&mut self, out: &mut [u8]) -> usize {
        self.read(|r| {
            let bytes = <&[u8]>::take(r);
            let len = bytes.len().min(out.len());
            out[..len].copy_from_slice(&bytes[..len]);
            len
        })
    }

    fn end(&mut self) {
        self.open = None;
    }
}

impl<T: for<'a> Wire<'a>> Put<T> for Recorded {
    fn put(&mut self, value: &T) {
        self.write(|w| value.put(w));
    }
}

impl Put<[u8]> for Recorded {
    fn put(&mut self, value: &[u8]) {
        self.write(|w| value.put(w));
    }
}

impl<T: Replayed> Get<T> for Recorded {
    fn get(&mut self) -> T {
        self.read(T::replayed)
    }
}

impl Stored<u64, Result<(), device_api::bulk::Error>> for Recorded {
    fn read(
        &mut self,
        method: MethodId,
        sector: u64,
        out: &mut [u8],
    ) -> Result<(), device_api::bulk::Error> {
        let Ok(start) = u32::try_from(sector) else {
            self.stop(format!("outside Bulk {sector}+1"))
        };
        self.media_read(method, Medium::Bulk, start, out)
            .map_err(StorageError::bulk)
    }

    fn write(
        &mut self,
        method: MethodId,
        sector: u64,
        bytes: &[u8],
    ) -> Result<(), device_api::bulk::Error> {
        let Ok(start) = u32::try_from(sector) else {
            return Ok(());
        };
        self.media_write(method, Medium::Bulk, start, bytes)
            .map_err(StorageError::bulk)
    }

    fn erase(
        &mut self,
        method: MethodId,
        sector: u64,
        length: usize,
    ) -> Result<(), device_api::bulk::Error> {
        let Ok(start) = u32::try_from(sector) else {
            return Ok(());
        };
        self.media_erase(method, Medium::Bulk, start, length)
            .map_err(StorageError::bulk)
    }
}

impl Stored<(Region, usize), Result<(), Error>> for Recorded {
    fn read(&mut self, method: MethodId, at: (Region, usize), out: &mut [u8]) -> Result<(), Error> {
        let Ok(start) = u32::try_from(at.1) else {
            self.stop(format!("outside {:?} {}+{}", at.0, at.1, out.len()))
        };
        self.media_read(method, at.0.into(), start, out)
            .map_err(StorageError::flash)
    }

    fn write(&mut self, method: MethodId, at: (Region, usize), bytes: &[u8]) -> Result<(), Error> {
        let Ok(start) = u32::try_from(at.1) else {
            return Ok(());
        };
        self.media_write(method, at.0.into(), start, bytes)
            .map_err(StorageError::flash)
    }

    fn erase(&mut self, method: MethodId, at: (Region, usize), length: usize) -> Result<(), Error> {
        let Ok(start) = u32::try_from(at.1) else {
            return Ok(());
        };
        self.media_erase(method, at.0.into(), start, length)
            .map_err(StorageError::flash)
    }
}

impl AccessPointRestore for Recorded {
    fn start(&mut self, _: &AccessPointConfig) -> Result<(), Error> {
        self.answer();
        self.get()
    }

    fn entropy(&mut self) -> u32 {
        self.answer();
        Get::<u64>::get(self) as u32
    }
}

impl ServerRestore for Recorded {
    /// A redacted trace holds the open session's token as a placeholder:
    /// the twin's own token stands in for it, and another token stays
    /// another.
    fn request(&mut self) -> Option<Request> {
        self.observed();
        let token = self.token;
        self.read(|r| {
            let recorded = <Option<ServerRequest<'_>> as Wire>::take(r)?;
            let mut request = recorded.to_request();
            if let Some(token) = token
                && trace::redact::is_token(&request.session)
            {
                request.session.clear();
                for byte in token.bytes() {
                    let _ = request.session.push(char::from(*byte));
                }
            }
            Some(request)
        })
    }
}

impl WifiRestore for Recorded {
    fn request(&mut self, operation: &WifiOperation) -> Result<(), Error> {
        let redacted = self.redacted;
        self.write(|w| trace::log::wifi_operation(w, operation, redacted));
        self.answer();
        self.get()
    }
}

impl Recorded {
    fn saved(&mut self, settings: &Settings) -> Result<Saved, SettingsError> {
        let encoded = firmware_shell::storage::recorded::encoded(settings, self.redacted);
        Put::<[u8]>::put(self, &encoded);
        self.answer();
        self.get()
    }
}

impl SettingsRestore for Recorded {
    fn load(&mut self) -> Result<Loaded, SettingsError> {
        self.answer();
        if Get::<u8>::get(self) != 0 {
            let error: SettingsError = self.get();
            self.settings = Some(format!("{error:?}"));
            return Err(error);
        }
        let settings = self.read(|r| {
            let bytes = <&[u8]>::take(r);
            let (settings, source) = firmware_shell::preferences::decode(Some(bytes));
            if source != Source::Current {
                r.fail();
            }
            settings
        });
        let source: Source = self.get();
        if !matches!(source, Source::Current | Source::Missing) {
            self.settings = Some(format!("{source:?}"));
        }
        Ok(Loaded {
            settings,
            source,
            sequence: self.get(),
            length: self.get(),
        })
    }

    fn save(&mut self, settings: &Settings) -> Result<Saved, SettingsError> {
        self.saved(settings)
    }

    fn save_connectivity(&mut self, current: &Settings) -> Result<Saved, SettingsError> {
        self.saved(current)
    }
}

#[cfg(test)]
mod tests;
