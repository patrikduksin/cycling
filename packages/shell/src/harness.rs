//! Bounded composition-owned test operations. No device capability implements policy.
use core::fmt::Write;
use device_api::display::Geometry;
use device_api::input::Button;
use device_api::input::Controls;
use device_api::input::Input;
use device_api::input::Point;
use device_api::observation::Availability;

pub const EVENTS: usize = 64;
pub const FRAMES: usize = 4;
pub const CHUNK: usize = 4096;
pub const ENCODED_BYTES: usize = 256;
pub const CAPTURE_BYTES: usize = FRAMES * 240 * 320 * 2;
/// Capture storage belongs to composition. Firmware lends its external RAM;
/// hosted backends own a box, so restart/drop never leave dangling slices.
pub enum CaptureBuffer {
    Borrowed(&'static mut [u8]),
    #[cfg(any(test, feature = "std"))]
    Owned(std::boxed::Box<[u8]>),
}
impl core::ops::Deref for CaptureBuffer {
    type Target = [u8];
    fn deref(&self) -> &[u8] {
        match self {
            Self::Borrowed(b) => b,
            #[cfg(any(test, feature = "std"))]
            Self::Owned(b) => b,
        }
    }
}
impl core::ops::DerefMut for CaptureBuffer {
    fn deref_mut(&mut self) -> &mut [u8] {
        match self {
            Self::Borrowed(b) => b,
            #[cfg(any(test, feature = "std"))]
            Self::Owned(b) => b,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Event {
    Button(u8),
    Down(u16, u16),
    Move(u16, u16),
    Up,
    Cancel,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Operation {
    Caps,
    Open {
        nonce: u32,
        lease: u32,
    },
    Ping,
    Close,
    Cancel,
    InputBegin(u32),
    InputAdd {
        sequence: u32,
        offset: u32,
        event: Event,
    },
    InputRun(u32),
    InputStatus,
    CaptureStart {
        count: u8,
        interval: u32,
    },
    CaptureStatus,
    CaptureMeta(u32),
    CaptureRead {
        frame: u32,
        offset: u32,
        length: u16,
    },
    CaptureStop,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Command {
    pub session: u64,
    pub operation: Operation,
}
pub fn parse(words: &mut core::str::SplitAsciiWhitespace<'_>) -> Option<Command> {
    fn number<T: core::str::FromStr>(w: &mut core::str::SplitAsciiWhitespace<'_>) -> Option<T> {
        w.next()?.parse().ok()
    }
    let first = words.next()?;
    if first == "CAPS" || first == "HELP" {
        return Some(Command {
            session: 0,
            operation: Operation::Caps,
        });
    }
    if first == "OPEN" {
        return Some(Command {
            session: 0,
            operation: Operation::Open {
                nonce: number(words)?,
                lease: number(words)?,
            },
        });
    }
    let session = first.parse().ok()?;
    let operation = match words.next()? {
        "PING" => Operation::Ping,
        "CLOSE" => Operation::Close,
        "CANCEL" => Operation::Cancel,
        "INPUT" => match words.next()? {
            "BEGIN" => Operation::InputBegin(number(words)?),
            "RUN" => Operation::InputRun(number(words)?),
            "STATUS" => Operation::InputStatus,
            "ADD" => {
                let sequence = number(words)?;
                let offset = number(words)?;
                let event = match words.next()? {
                    "BUTTON" => Event::Button(number(words)?),
                    "DOWN" => Event::Down(number(words)?, number(words)?),
                    "MOVE" => Event::Move(number(words)?, number(words)?),
                    "UP" => Event::Up,
                    "CANCEL" => Event::Cancel,
                    _ => return None,
                };
                Operation::InputAdd {
                    sequence,
                    offset,
                    event,
                }
            }
            _ => return None,
        },
        "CAPTURE" => match words.next()? {
            "START" => Operation::CaptureStart {
                count: number(words)?,
                interval: number(words)?,
            },
            "STATUS" => Operation::CaptureStatus,
            "META" => Operation::CaptureMeta(number(words)?),
            "READ" => Operation::CaptureRead {
                frame: number(words)?,
                offset: number(words)?,
                length: number(words)?,
            },
            "STOP" => Operation::CaptureStop,
            _ => return None,
        },
        _ => return None,
    };
    Some(Command { session, operation })
}
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &b in bytes {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
        }
    }
    !crc
}
#[derive(Clone, Copy)]
struct Scheduled {
    offset: u32,
    event: Event,
}
#[derive(Clone, Copy, Default)]
pub struct Frame {
    pub id: u32,
    pub started: u64,
    pub ended: u64,
    pub width: usize,
    pub height: usize,
    pub bytes: usize,
    offset: usize,
    pub crc: u32,
    pub complete: bool,
    pub submitted: bool,
}
pub struct State {
    pub boot: u32,
    session: u64,
    generation: u32,
    lease: u32,
    deadline: u64,
    events: [Option<Scheduled>; EVENTS],
    accepted: usize,
    delivered: usize,
    sequence: u32,
    sequence_high: u32,
    input_state: &'static str,
    started: u64,
    ended: u64,
    held: bool,
    planned_held: bool,
    cancel_pending: bool,
    pub cancelled: u32,
    losses: u32,
    max_late: u64,
    pub buffer: Option<CaptureBuffer>,
    frames: [Frame; FRAMES],
    count: usize,
    requested: usize,
    capture_state: &'static str,
    interval: u32,
    next_frame: u64,
    frame_high: u32,
    skipped: u32,
    writing: bool,
    written: usize,
    ordered: bool,
    pub clock: Option<fn() -> u64>,
    pub now: u64,
}
impl Default for State {
    fn default() -> Self {
        Self::new(0)
    }
}
impl State {
    pub fn new(boot: u32) -> Self {
        Self {
            boot,
            session: 0,
            generation: 0,
            lease: 0,
            deadline: 0,
            events: [None; EVENTS],
            accepted: 0,
            delivered: 0,
            sequence: 0,
            sequence_high: 0,
            input_state: "idle",
            started: 0,
            ended: 0,
            held: false,
            planned_held: false,
            cancel_pending: false,
            cancelled: 0,
            losses: 0,
            max_late: 0,
            buffer: None,
            frames: [Frame::default(); FRAMES],
            count: 0,
            requested: 0,
            capture_state: "idle",
            interval: 0,
            next_frame: 0,
            frame_high: 0,
            skipped: 0,
            writing: false,
            written: 0,
            ordered: true,
            clock: None,
            now: 0,
        }
    }
    pub fn time(&self) -> u64 {
        self.clock.map_or(self.now, |clock| clock())
    }
    pub fn stop_input(&mut self, reason: &'static str) {
        if self.held {
            self.cancel_pending = true;
            self.held = false;
            self.cancelled = self.cancelled.saturating_add(1);
        }
        if matches!(self.input_state, "running" | "queued") {
            self.input_state = reason;
            self.ended = self.now;
        }
    }
    pub fn expire(&mut self, now: u64) {
        self.now = now;
        if self.session != 0 && now >= self.deadline {
            self.stop_input("expired");
            self.capture_state = "expired";
            self.session = 0;
        }
    }
    pub fn take_cancel(&mut self) -> bool {
        core::mem::take(&mut self.cancel_pending)
    }
    pub fn next_input(&mut self, now: u64) -> Option<Input> {
        self.expire(now);
        if self.take_cancel() {
            return Some(Input::Cancel);
        }
        if self.input_state != "running" {
            return None;
        }
        let scheduled = self.events[self.delivered]?;
        let due = self.started + u64::from(scheduled.offset);
        if now < due {
            return None;
        }
        self.max_late = self.max_late.max(now - due);
        self.delivered += 1;
        let event = match scheduled.event {
            Event::Button(index) => Input::Button {
                button: button(index)?,
                code: 1,
            },
            Event::Down(x, y) | Event::Move(x, y) => {
                self.held = true;
                Input::Touch(Point { x, y })
            }
            Event::Up => {
                self.held = false;
                Input::Release
            }
            Event::Cancel => {
                self.held = false;
                Input::Cancel
            }
        };
        if self.delivered == self.accepted {
            self.input_state = "completed";
            self.ended = now;
        }
        Some(event)
    }
    pub fn capture_due(&self, now: u64) -> bool {
        self.capture_state == "running" && now >= self.next_frame
    }
    pub fn begin_frame(&mut self, g: Geometry) -> bool {
        let now = self.time();
        if !self.capture_due(now) {
            return false;
        }
        let bytes = g
            .width
            .checked_mul(g.height)
            .and_then(|v| v.checked_mul(2))
            .unwrap_or(usize::MAX);
        let offset: usize = self.frames[..self.count].iter().map(|f| f.bytes).sum();
        if self
            .buffer
            .as_ref()
            .is_none_or(|b| offset > b.len() || bytes > b.len() - offset)
        {
            self.capture_state = "unavailable";
            return false;
        }
        self.skipped = self
            .skipped
            .saturating_add(((now - self.next_frame) / u64::from(self.interval)) as u32);
        self.next_frame = now + u64::from(self.interval);
        self.frame_high = self.frame_high.saturating_add(1);
        self.frames[self.count] = Frame {
            id: self.frame_high,
            started: now,
            width: g.width,
            height: g.height,
            bytes,
            offset,
            ..Frame::default()
        };
        self.writing = true;
        self.written = 0;
        self.ordered = true;
        true
    }
    pub fn pixel(&mut self, x: usize, y: usize, value: u16) {
        if !self.writing {
            return;
        }
        let f = self.frames[self.count];
        let index = y * f.width + x;
        self.ordered &= x < f.width && y < f.height && index == self.written;
        if x < f.width && y < f.height {
            let base = f.offset + index * 2;
            self.buffer.as_mut().unwrap()[base..base + 2].copy_from_slice(&value.to_le_bytes());
        }
        self.written += 1;
    }
    pub fn finish_frame(&mut self, submitted: bool) {
        if !self.writing {
            return;
        }
        self.writing = false;
        let ended = self.time();
        let f = &mut self.frames[self.count];
        f.ended = ended;
        f.submitted = submitted;
        f.complete = self.ordered && self.written == f.width * f.height;
        if f.complete {
            let base = f.offset;
            f.crc = crc32(&self.buffer.as_ref().unwrap()[base..base + f.bytes]);
        }
        self.count += 1;
        if self.count == self.requested {
            self.capture_state = "completed";
        }
    }
    pub fn command(
        &mut self,
        command: Command,
        controls: Controls,
        g: Geometry,
        now: u64,
        out: &mut impl Write,
    ) -> &'static str {
        self.expire(now);
        let op = command.operation;
        let status = if matches!(op, Operation::Caps) {
            "OK"
        } else if let Operation::Open { nonce: _, lease } = op {
            if !(1000..=120000).contains(&lease) {
                "INVALID"
            } else if self.session != 0 {
                "BUSY"
            } else if self.generation == u32::MAX {
                "UNAVAILABLE"
            } else {
                self.generation += 1;
                self.sequence_high = 0;
                self.sequence = 0;
                self.input_state = "idle";
                self.accepted = 0;
                self.delivered = 0;
                self.count = 0;
                self.capture_state = "idle";
                self.session = (u64::from(self.boot) << 32) | u64::from(self.generation);
                self.lease = lease;
                self.deadline = now + u64::from(lease);
                "OK"
            }
        } else if command.session == 0 || command.session != self.session {
            "STALE_SESSION"
        } else {
            self.deadline = now + u64::from(self.lease);
            self.execute(op, controls, g, now, out)
        };
        let _ = write!(out, " boot={} session={} ", self.boot, self.session);
        match op {
            Operation::Caps => {
                let _ = write!(
                    out,
                    "version=1 input=supported capture={} max_events={} max_frames={} chunk_bytes={} encoded_bytes=256 encodings=raw,rle565 capture_bytes={} width={} height={} touch={} buttons=",
                    if self.buffer.is_some() {
                        "supported"
                    } else {
                        "unavailable"
                    },
                    EVENTS,
                    FRAMES,
                    CHUNK,
                    self.buffer.as_ref().map_or(0, |b| b.len()),
                    g.width,
                    g.height,
                    if controls.touch == Availability::Ready {
                        "supported"
                    } else {
                        "unsupported"
                    }
                );
                for b in controls.buttons {
                    let _ = write!(out, "{},", b.index());
                }
                let _ = write!(
                    out,
                    " lease_min_ms=1000 lease_max_ms=120000 max_sequence_ms=30000 min_interval_ms=100 max_interval_ms=10000 rgb=RGB565LE physical_evidence=unsupported"
                );
            }
            Operation::Open { nonce, .. } => {
                let _ = write!(out, "nonce={} lease_ms={}", nonce, self.lease);
            }
            _ => {}
        }
        status
    }
    fn execute(
        &mut self,
        op: Operation,
        controls: Controls,
        g: Geometry,
        now: u64,
        out: &mut impl Write,
    ) -> &'static str {
        match op {
            Operation::Ping => {}
            Operation::Close | Operation::Cancel => {
                self.stop_input("cancelled");
                self.capture_state = "cancelled";
                if op == Operation::Close {
                    self.session = 0;
                }
            }
            Operation::InputBegin(sequence) => {
                if matches!(self.input_state, "running" | "queued") || self.held {
                    return "BUSY";
                }
                if sequence <= self.sequence_high {
                    return "STALE_OPERATION";
                }
                self.sequence_high = sequence;
                self.sequence = sequence;
                self.accepted = 0;
                self.delivered = 0;
                self.events.fill(None);
                self.planned_held = false;
                self.max_late = 0;
                self.losses = 0;
                self.input_state = "queued";
            }
            Operation::InputAdd {
                sequence,
                offset,
                event,
            } => {
                if sequence != self.sequence || self.input_state != "queued" {
                    return "STALE_OPERATION";
                }
                if self.accepted == EVENTS {
                    self.losses += 1;
                    self.stop_input("overflow");
                    return "OVERFLOW";
                }
                if offset > 30000
                    || self.accepted > 0 && offset < self.events[self.accepted - 1].unwrap().offset
                {
                    return "INVALID";
                }
                match event {
                    Event::Button(index) => {
                        if button(index).is_none_or(|b| !controls.buttons.contains(&b)) {
                            return "UNSUPPORTED";
                        }
                    }
                    Event::Down(x, y) | Event::Move(x, y) => {
                        if controls.touch != Availability::Ready {
                            return "UNSUPPORTED";
                        }
                        if usize::from(x) >= g.width || usize::from(y) >= g.height {
                            return "INVALID";
                        }
                        if matches!(event, Event::Down(..)) == self.planned_held {
                            return "INVALID";
                        }
                        self.planned_held = true;
                    }
                    Event::Up => {
                        if !self.planned_held {
                            return "INVALID";
                        }
                        self.planned_held = false;
                    }
                    Event::Cancel => self.planned_held = false,
                }
                self.events[self.accepted] = Some(Scheduled { offset, event });
                self.accepted += 1;
                let _ = write!(
                    out,
                    "sequence={} accepted={} ",
                    self.sequence, self.accepted
                );
                return "ACCEPTED";
            }
            Operation::InputRun(sequence) => {
                if sequence != self.sequence || self.input_state != "queued" {
                    return "STALE_OPERATION";
                }
                if self.accepted == 0 || self.planned_held {
                    return "INVALID";
                }
                self.started = now;
                self.ended = 0;
                self.input_state = "running";
                let _ = write!(out, "sequence={} started_ms={} ", sequence, now);
                return "ACCEPTED";
            }
            Operation::InputStatus => {
                let _ = write!(
                    out,
                    "sequence={} state={} accepted={} delivered={} losses={} cancelled={} started_ms={} ended_ms={} max_late_ms={} synthetic=true held={} ",
                    self.sequence,
                    self.input_state,
                    self.accepted,
                    self.delivered,
                    self.losses,
                    self.cancelled,
                    self.started,
                    self.ended,
                    self.max_late,
                    self.held
                );
            }
            Operation::CaptureStart { count, interval } => {
                if self.frame_high > u32::MAX - u32::from(count) {
                    return "UNAVAILABLE";
                }
                if self.capture_state == "running" {
                    return "BUSY";
                }
                if !(1..=FRAMES as u8).contains(&count)
                    || !(100..=10000).contains(&interval)
                    || u32::from(count - 1) * interval > 30000
                {
                    return "INVALID";
                }
                if self
                    .buffer
                    .as_ref()
                    .is_none_or(|b| g.width * g.height * 2 > b.len() / usize::from(count))
                {
                    return "UNAVAILABLE";
                }
                self.count = 0;
                self.requested = usize::from(count);
                self.interval = interval;
                self.next_frame = now;
                self.skipped = 0;
                self.frames.fill(Frame::default());
                self.capture_state = "running";
                return "ACCEPTED";
            }
            Operation::CaptureStop => {
                if self.capture_state == "running" {
                    self.capture_state = "cancelled";
                }
            }
            Operation::CaptureStatus => {
                let _ = write!(
                    out,
                    "state={} requested={} captured={} skipped={} interval_ms={} frames=",
                    self.capture_state, self.requested, self.count, self.skipped, self.interval
                );
                for f in &self.frames[..self.count] {
                    let _ = write!(out, "{},", f.id);
                }
                let _ = write!(out, " ");
            }
            Operation::CaptureMeta(id) | Operation::CaptureRead { frame: id, .. } => {
                let Some(index) = self.frames[..self.count].iter().position(|f| f.id == id) else {
                    return "STALE_FRAME";
                };
                let f = self.frames[index];
                if let Operation::CaptureRead { offset, length, .. } = op {
                    let offset = offset as usize;
                    let length = usize::from(length);
                    if length == 0
                        || !offset.is_multiple_of(2)
                        || !length.is_multiple_of(2)
                        || length > CHUNK
                        || offset > f.bytes
                        || length > f.bytes - offset
                    {
                        return "INVALID";
                    }
                    if !f.complete {
                        return "INCOMPLETE";
                    }
                    let bytes = &self.buffer.as_ref().unwrap()
                        [f.offset + offset..f.offset + offset + length];
                    let mut encoded = [0; ENCODED_BYTES];
                    let (encoding, decoded_length, encoded_length) =
                        encode_chunk(bytes, &mut encoded);
                    let _ = write!(
                        out,
                        "frame={} offset={} length={} encoding={} crc32={:08x} hex=",
                        id,
                        offset,
                        decoded_length,
                        encoding,
                        crc32(&bytes[..decoded_length])
                    );
                    for b in &encoded[..encoded_length] {
                        let _ = write!(out, "{:02x}", b);
                    }
                    let _ = write!(out, " ");
                } else {
                    let _ = write!(
                        out,
                        "frame={} width={} height={} format=RGB565LE bytes={} crc32={:08x} started_ms={} ended_ms={} complete={} submitted={} physical=false ",
                        f.id,
                        f.width,
                        f.height,
                        f.bytes,
                        f.crc,
                        f.started,
                        f.ended,
                        f.complete,
                        f.submitted
                    );
                }
            }
            Operation::Caps | Operation::Open { .. } => unreachable!(),
        }
        "OK"
    }
}
fn button(index: u8) -> Option<Button> {
    match index {
        0 => Some(Button::TopLeft),
        1 => Some(Button::BottomLeft),
        2 => Some(Button::BottomRight),
        3 => Some(Button::Center),
        _ => None,
    }
}

/// A run is a little-endian u16 pixel count followed by one RGB565LE pixel.
/// Prefer runs only when one bounded payload covers more than a raw payload.
/// The caller has checked that bytes contains 1..=2048 complete pixels.
fn encode_chunk(bytes: &[u8], encoded: &mut [u8; ENCODED_BYTES]) -> (&'static str, usize, usize) {
    let mut decoded = 0;
    let mut length = 0;
    while decoded < bytes.len() && length < ENCODED_BYTES {
        let pixel = [bytes[decoded], bytes[decoded + 1]];
        let start = decoded;
        decoded += 2;
        while decoded < bytes.len() && bytes[decoded..decoded + 2] == pixel {
            decoded += 2;
        }
        let count = ((decoded - start) / 2) as u16;
        encoded[length..length + 2].copy_from_slice(&count.to_le_bytes());
        encoded[length + 2..length + 4].copy_from_slice(&pixel);
        length += 4;
    }
    if decoded > ENCODED_BYTES {
        ("rle565", decoded, length)
    } else {
        let length = bytes.len().min(ENCODED_BYTES);
        encoded[..length].copy_from_slice(&bytes[..length]);
        ("raw", length, length)
    }
}
