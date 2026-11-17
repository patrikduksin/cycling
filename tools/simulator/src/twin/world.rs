//! The recorded world a strict replay runs in: the trace's world records,
//! read a pass at a time, answering every server of the twin.
//!
//! A segment is what one pass recorded, or one terminal command, or the
//! boot before the first pass. Its calls and observations go to a
//! [`script::Pass`], its media records wait in order for the twin's own
//! accesses, and the few inputs that are no capability's (network time, the
//! power going away) are kept beside them.
//!
//! A call the trace holds no answer for has left the recorded world. The
//! world notes why and unwinds with [`Left`], which ends the replay at that
//! pass: there is no value it could hand VANA instead.
use super::media::{Change, Outcome, TwinMedium};
use super::script::{self, Named, Record};
use crate::replay::{Exclusion, Faithful, Kind, Mode, Next, Report};
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
use firmware_services::trace::media::{Access, Content};
use firmware_services::trace::replayed::Replayed;
use firmware_services::trace::wire::{Reader, Wire, Writer};
use firmware_services::trace::{self, End, Event, Events, Medium, ServerRequest, StorageError};
use firmware_shell::preferences::{Settings, Source};
use firmware_shell::storage::{Error as SettingsError, Loaded, SettingsRestore};
use std::collections::{BTreeMap, VecDeque};

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

struct Pending {
    method: MethodId,
    span: Span,
    bytes: Option<Vec<u8>>,
    digest: bool,
    outside: bool,
    fault: Option<StorageError>,
    /// Units of a shared record the twin's accesses have covered.
    met: u32,
}

enum Met {
    /// The trace holds no record of this access.
    Unlisted,
    Recorded {
        digest: bool,
    },
    Outside,
    Fault(StorageError, Option<Vec<u8>>),
}

pub struct Recorded {
    trace: Vec<u8>,
    cursor: trace::Cursor,
    segment: Option<script::Pass>,
    view: script::View,
    pass: Pass,
    open: Option<Open>,
    media: BTreeMap<u8, TwinMedium>,
    pending: VecDeque<Pending>,
    /// The TERRAIN pack's data, which the trace holds only digests of.
    pack: Option<Pack>,
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
    terrain_reads: u32,
    outside: u32,
    settings: Option<String>,
    lost: Option<u64>,
    /// The last record of each request that repeats: what a call the device
    /// left unrecorded gave.
    repeated: BTreeMap<MethodId, Vec<u8>>,
}

impl Recorded {
    /// The world `trace` recorded, with its boot segment loaded.
    pub fn open(trace: Vec<u8>, pack: Option<Pack>) -> Result<Self, trace::Error> {
        let (header, events) = trace::decode(&trace)?;
        let terrain = header.terrain;
        let redacted = crate::trace::world::opened(&trace).redacted;
        let cursor = events.cursor();
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
            media: BTreeMap::new(),
            pending: VecDeque::new(),
            pack,
            terrain,
            redacted,
            token: None,
            network_time: Latest::default(),
            powered_off: false,
            forgotten: Vec::new(),
            redacting: false,
            report: Report::new(Mode::Strict),
            stopped: false,
            parted: None,
            terrain_reads: 0,
            outside: 0,
            settings: None,
            lost: None,
            repeated: BTreeMap::new(),
        };
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
            self.media.remove(&(*medium as u8));
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
                Some(Event::Lost { .. } | Event::TooLarge { .. }) => self.lost = Some(now_ms),
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
                Some(Declared::Media(_)) => self.await_media(method, &body, now_ms),
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

    fn await_media(&mut self, method: MethodId, body: &[u8], now_ms: u64) {
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
        let bytes = match access.content {
            Content::Inline(_) => {
                let len = access.span.count as usize * access.span.medium.unit();
                let mut bytes = vec![0u8; len];
                match access.expand(&mut bytes) {
                    Some(expanded) => bytes.truncate(expanded),
                    None => {
                        return self.note(
                            Kind::Unreplayable,
                            format!("the bytes of a {} record do not decode", Named(method)),
                        );
                    }
                }
                Some(bytes)
            }
            _ => None,
        };
        self.outside += u32::from(access.outside);
        self.pending.push_back(Pending {
            method,
            span: access.span,
            bytes,
            digest: matches!(access.content, Content::Digest(_)),
            outside: access.outside,
            fault: access.fault,
            met: 0,
        });
    }

    /// The segment ended: what it left unanswered parts the replay from
    /// the recording, and the bytes it carried and the twin never read
    /// stay known.
    fn close(&mut self) {
        let Some(segment) = self.segment.take() else {
            return;
        };
        for pending in core::mem::take(&mut self.pending) {
            if let (Some(bytes), None) = (&pending.bytes, pending.fault) {
                let at = u64::from(pending.span.start) * pending.span.medium.unit() as u64;
                self.medium(pending.span.medium).install(at, bytes);
            }
        }
        match segment.end() {
            Ok(view) => self.view = view,
            Err(stop) => self.part(Kind::LeftOver, stop.to_string()),
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
            (self.terrain_reads > 0).then_some(Exclusion::TerrainByDigest(self.terrain_reads)),
            (self.outside > 0).then_some(Exclusion::Outside(self.outside)),
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
        self.note(kind, why);
        self.stopped = true;
    }

    fn stop(&mut self, why: String) -> ! {
        self.part(Kind::Unrecorded, format!("left the recorded world: {why}"));
        std::panic::resume_unwind(Box::new(Left))
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

    fn medium(&mut self, medium: Medium) -> &mut TwinMedium {
        self.media.entry(medium as u8).or_default()
    }

    /// What the trace holds of the twin's access to `count` units at
    /// `start`: the first record waiting that covers its start. A record
    /// of bytes is installed, with the parts that follow it.
    fn meet(&mut self, method: MethodId, medium: Medium, start: u32, count: u32) -> Met {
        let concerns = |pending: &Pending| {
            let from = pending.span.start.saturating_add(pending.met);
            let end = u64::from(pending.span.start) + u64::from(pending.span.count);
            pending.method == method
                && pending.span.medium == medium
                && start >= from
                && u64::from(start) < end
        };
        let Some(index) = self.pending.iter().position(concerns) else {
            return Met::Unlisted;
        };
        let pending = &mut self.pending[index];
        let (digest, outside, fault) = (pending.digest, pending.outside, pending.fault);
        if pending.bytes.is_none() {
            pending.met = start - pending.span.start + count;
            if pending.met >= pending.span.count {
                self.pending.remove(index);
            }
            return match (fault, outside) {
                (Some(fault), _) => Met::Fault(fault, None),
                (None, true) => Met::Outside,
                (None, false) => Met::Recorded { digest },
            };
        }
        let mut carried = Vec::new();
        let end = u64::from(start) + u64::from(count);
        let mut next = None;
        while let Some(part) = self.pending.get(index)
            && part.method == method
            && part.span.medium == medium
            && u64::from(part.span.start) < end
            && next.is_none_or(|next| part.span.start == next)
            && let Some(bytes) = &part.bytes
        {
            next = Some(part.span.start + part.span.count);
            match fault {
                Some(_) => carried.extend_from_slice(bytes),
                None => {
                    let (from, bytes) = (
                        u64::from(part.span.start) * medium.unit() as u64,
                        bytes.clone(),
                    );
                    self.medium(medium).install(from, &bytes);
                }
            }
            self.pending.remove(index);
        }
        match fault {
            Some(fault) => Met::Fault(fault, Some(carried)),
            None => Met::Recorded { digest: false },
        }
    }

    pub fn media_read(
        &mut self,
        method: MethodId,
        medium: Medium,
        start: u32,
        out: &mut [u8],
    ) -> Result<(), StorageError> {
        let count = (out.len() as u64 / medium.unit() as u64) as u32;
        let met = self.meet(method, medium, start, count);
        let place = || format!("{medium:?} {start}+{count}");
        let digest = match met {
            Met::Fault(fault, left) => {
                if let Some(left) = left {
                    let len = left.len().min(out.len());
                    out[..len].copy_from_slice(&left[..len]);
                }
                return Err(fault);
            }
            Met::Outside => self.stop(format!("outside {}", place())),
            Met::Recorded { digest } => digest,
            Met::Unlisted => false,
        };
        let from = u64::from(start) * medium.unit() as u64;
        if self.medium(medium).read(from, out).is_ok() {
            return Ok(());
        }
        let packed = match (&self.pack, medium, digest) {
            (Some(pack), Medium::Bulk, true) => out
                .chunks_mut(device_api::bulk::SECTOR_SIZE)
                .enumerate()
                .try_for_each(|(index, sector)| {
                    let bytes = pack.sector(u64::from(start) + index as u64)?;
                    sector.copy_from_slice(&bytes[..sector.len()]);
                    Some(())
                })
                .is_some(),
            _ => false,
        };
        if !packed {
            self.stop(format!("unknown media {}", place()))
        }
        self.terrain_reads += 1;
        Ok(())
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
        let from = u64::from(start) * medium.unit() as u64;
        match self.meet(method, medium, start, count) {
            Met::Fault(fault, _) => {
                self.medium(medium).mutate(from, change, Outcome::Failed);
                Err(fault)
            }
            Met::Outside => Ok(()),
            Met::Recorded { .. } | Met::Unlisted => {
                self.medium(medium).mutate(from, change, Outcome::Done);
                Ok(())
            }
        }
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
        if let Err(stop) = self.segment().answer(method, &args) {
            let why = stop.to_string();
            let why = why
                .strip_prefix("left the recorded world: ")
                .unwrap_or(&why);
            self.stop(why.into())
        }
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
        match self.segment().observe(method, &key) {
            Some(seen) => {
                self.opened().answer = Some((seen.value, 0, seen.at_ms));
                seen.at_ms
            }
            None => self.stop(format!("{} was never observed", Named(method))),
        }
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
