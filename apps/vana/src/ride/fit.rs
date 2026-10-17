//! A recorded ride as a FIT activity file, streamed slot by slot.
//!
//! The journal keeps samples, pauses and the end of each ride; this module
//! turns them into the messages Strava, Garmin Connect and other FIT readers
//! expect from a cycling computer: `file_id`, `device_info`, timer events for
//! the start, every pause and resume and the end, one `record` per sample,
//! then one `lap`, the `session` and the `activity`.
//!
//! The FIT header carries the file's size, so a file is written in two passes
//! over the same slots: a sizing pass into a [`Counter`], whose [`Plan`] gives
//! the size and the ride's clock, then the real one. Each pass holds only
//! running totals, so a ride of any length streams through small buffers.
//!
//! Time: a sample's UTC is `utc - active` of the running stretch it belongs
//! to plus its active time, as the host GPX export dates samples. A stretch
//! without UTC (the clock synchronized later, or never) keeps the previous
//! stretch's offset, so its pause counts as no time; samples before the
//! ride's first UTC take that first offset. A ride with no UTC at all needs a
//! start time from the caller. Timestamps never go backwards.
//!
//! What the journal does not have stays out: there is no altitude (pressure
//! is not recorded), no laps (they live in RAM only) and no distance without
//! wheel speed, so readers fall back to GPS for distance and their own
//! elevation data. Totals use the ride page's rules (`Totals`).

use crate::ride::log::{self, Entry, Kind, Sample, Slot};
use crate::ride::totals::Totals;
use firmware_services::fit::{
    self, BaseType, Counter, Field, Message, Sink, Value, Writer, semicircles,
};

/// Product name in `file_id` and `device_info`.
const PRODUCT_NAME: &str = "VANA C606";
/// Garmin's manufacturer id for development and unregistered devices.
const MANUFACTURER_DEVELOPMENT: u16 = 255;
const PRODUCT: u16 = 606;
/// Software version times 100, from the crate version.
const SOFTWARE_VERSION: u16 = software_version();

const fn software_version() -> u16 {
    let version = env!("CARGO_PKG_VERSION").as_bytes();
    let mut index = 0;
    let mut parts = [0u16; 3];
    let mut part = 0;
    while index < version.len() && part < 3 {
        let byte = version[index];
        if byte == b'.' {
            part += 1;
        } else if byte.is_ascii_digit() {
            parts[part] = parts[part] * 10 + (byte - b'0') as u16;
        } else {
            break;
        }
        index += 1;
    }
    parts[0] * 100 + parts[1] * 10 + parts[2]
}

// Profile enums used below.
const FILE_ACTIVITY: u8 = 4;
const EVENT_TIMER: u8 = 0;
const EVENT_LAP: u8 = 9;
const EVENT_SESSION: u8 = 8;
const EVENT_ACTIVITY: u8 = 26;
const EVENT_TYPE_START: u8 = 0;
const EVENT_TYPE_STOP: u8 = 1;
const EVENT_TYPE_STOP_ALL: u8 = 4;
const SPORT_CYCLING: u8 = 2;
const SUB_SPORT_GENERIC: u8 = 0;
const LAP_TRIGGER_SESSION_END: u8 = 7;
const SESSION_TRIGGER_ACTIVITY_END: u8 = 0;
const ACTIVITY_MANUAL: u8 = 0;

const NAME: u8 = 16;

const FILE_ID: Message = Message {
    local: 0,
    global: 0,
    fields: &[
        Field::new(0, BaseType::Enum),
        Field::new(1, BaseType::Uint16),
        Field::new(2, BaseType::Uint16),
        Field::new(3, BaseType::Uint32z),
        Field::new(4, BaseType::Uint32),
        Field::string(8, NAME),
    ],
};

const DEVICE_INFO: Message = Message {
    local: 1,
    global: 23,
    fields: &[
        Field::new(253, BaseType::Uint32),
        Field::new(0, BaseType::Uint8),
        Field::new(2, BaseType::Uint16),
        Field::new(4, BaseType::Uint16),
        Field::new(5, BaseType::Uint16),
        Field::string(27, NAME),
    ],
};

const EVENT: Message = Message {
    local: 2,
    global: 21,
    fields: &[
        Field::new(253, BaseType::Uint32),
        Field::new(0, BaseType::Enum),
        Field::new(1, BaseType::Enum),
    ],
};

const RECORD: Message = Message {
    local: 3,
    global: 20,
    fields: &[
        Field::new(253, BaseType::Uint32),
        Field::new(0, BaseType::Sint32),
        Field::new(1, BaseType::Sint32),
        Field::new(5, BaseType::Uint32),
        Field::new(6, BaseType::Uint16),
        Field::new(3, BaseType::Uint8),
        Field::new(4, BaseType::Uint8),
        Field::new(7, BaseType::Uint16),
        Field::new(9, BaseType::Sint16),
    ],
};

const LAP: Message = Message {
    local: 4,
    global: 19,
    fields: &[
        Field::new(253, BaseType::Uint32),
        Field::new(254, BaseType::Uint16),
        Field::new(0, BaseType::Enum),
        Field::new(1, BaseType::Enum),
        Field::new(2, BaseType::Uint32),
        Field::new(3, BaseType::Sint32),
        Field::new(4, BaseType::Sint32),
        Field::new(5, BaseType::Sint32),
        Field::new(6, BaseType::Sint32),
        Field::new(7, BaseType::Uint32),
        Field::new(8, BaseType::Uint32),
        Field::new(9, BaseType::Uint32),
        Field::new(13, BaseType::Uint16),
        Field::new(14, BaseType::Uint16),
        Field::new(15, BaseType::Uint8),
        Field::new(16, BaseType::Uint8),
        Field::new(17, BaseType::Uint8),
        Field::new(19, BaseType::Uint16),
        Field::new(20, BaseType::Uint16),
        Field::new(24, BaseType::Enum),
        Field::new(25, BaseType::Enum),
    ],
};

const SESSION: Message = Message {
    local: 5,
    global: 18,
    fields: &[
        Field::new(253, BaseType::Uint32),
        Field::new(254, BaseType::Uint16),
        Field::new(0, BaseType::Enum),
        Field::new(1, BaseType::Enum),
        Field::new(2, BaseType::Uint32),
        Field::new(3, BaseType::Sint32),
        Field::new(4, BaseType::Sint32),
        Field::new(5, BaseType::Enum),
        Field::new(6, BaseType::Enum),
        Field::new(7, BaseType::Uint32),
        Field::new(8, BaseType::Uint32),
        Field::new(9, BaseType::Uint32),
        Field::new(14, BaseType::Uint16),
        Field::new(15, BaseType::Uint16),
        Field::new(16, BaseType::Uint8),
        Field::new(17, BaseType::Uint8),
        Field::new(18, BaseType::Uint8),
        Field::new(20, BaseType::Uint16),
        Field::new(21, BaseType::Uint16),
        Field::new(25, BaseType::Uint16),
        Field::new(26, BaseType::Uint16),
        Field::new(28, BaseType::Enum),
        Field::new(34, BaseType::Uint16),
    ],
};

const ACTIVITY: Message = Message {
    local: 6,
    global: 34,
    fields: &[
        Field::new(253, BaseType::Uint32),
        Field::new(0, BaseType::Uint32),
        Field::new(1, BaseType::Uint16),
        Field::new(2, BaseType::Enum),
        Field::new(3, BaseType::Enum),
        Field::new(4, BaseType::Enum),
    ],
};

const DEFINITIONS: [&Message; 7] = [
    &FILE_ID,
    &DEVICE_INFO,
    &EVENT,
    &RECORD,
    &LAP,
    &SESSION,
    &ACTIVITY,
];

/// Largest output of one [`Export::slot`] call: a resume, four records and
/// a pause. Sinks that drain between calls need at least this much room.
pub const MAX_SLOT_OUTPUT: usize = 2 * 7 + log::SAMPLES_PER_BATCH * 25;
/// Output of [`Export::start`]: header, definitions, `file_id` and `device_info`.
pub const MAX_START_OUTPUT: usize = 384;
/// Output of [`Export::finish`]: the closing event, lap, session, activity and CRC.
pub const MAX_FINISH_OUTPUT: usize = 192;

/// What the sizing pass learned, for the pass that writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Bytes of the complete file.
    pub size: u32,
    /// UTC minus active time of the ride's first dated sample.
    pub offset_ms: Option<u64>,
    pub samples: u32,
}

impl Plan {
    /// The offset the writing pass dates samples with: the ride's own, or
    /// one that puts its start at `start_utc_ms`.
    pub fn clock(&self, start_utc_ms: Option<u64>) -> Option<u64> {
        self.offset_ms.or(start_utc_ms)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Flow {
    /// Feed the next slot.
    More,
    /// The ride ended; call [`Export::finish`].
    End,
}

/// One ride being converted. Feed its slots in journal order from its START
/// slot; slots of other rides and invalid slots are skipped.
#[derive(Clone)]
pub struct Export {
    ride_id: u32,
    writer: Writer,
    started: bool,
    ended: bool,
    /// UTC minus active time for the current stretch.
    offset_ms: u64,
    first_offset_ms: Option<u64>,
    /// The current running stretch has fixed its offset from its own UTC.
    stretch_dated: bool,
    /// A start or resume waiting for the next sample to date it.
    pending_start: Option<u64>,
    paused: bool,
    start_ts: Option<u32>,
    last_ts: u32,
    previous_active_ms: Option<u64>,
    active_ms: u64,
    samples: u32,
    first_position: Option<(i32, i32)>,
    last_position: Option<(i32, i32)>,
    distance_mm: Option<u64>,
    totals: Totals,
}

impl Export {
    /// Begin the file. `clock` is the offset from [`Plan::clock`]; the sizing
    /// pass passes `None` and writes placeholder times of the same size.
    pub fn start(
        ride_id: u32,
        plan: Option<&Plan>,
        clock: Option<u64>,
        out: &mut impl Sink,
    ) -> Self {
        let size = plan.map_or(0, |plan| plan.size);
        let data = (size as usize).saturating_sub(fit::HEADER_SIZE + fit::CRC_SIZE) as u32;
        let mut export = Self {
            ride_id,
            writer: Writer::start(out, data),
            started: false,
            ended: false,
            offset_ms: clock.unwrap_or(0),
            first_offset_ms: None,
            stretch_dated: false,
            pending_start: None,
            paused: false,
            start_ts: None,
            last_ts: 0,
            previous_active_ms: None,
            active_ms: 0,
            samples: 0,
            first_position: None,
            last_position: None,
            distance_mm: None,
            totals: Totals::default(),
        };
        for message in DEFINITIONS {
            export.writer.define(out, message);
        }
        let created = export.stamp(0);
        export.writer.record(
            out,
            &FILE_ID,
            &[
                Value::Enum(Some(FILE_ACTIVITY)),
                Value::U16(Some(MANUFACTURER_DEVELOPMENT)),
                Value::U16(Some(PRODUCT)),
                Value::U32z(Some(ride_id)),
                Value::U32(created),
                Value::Str(PRODUCT_NAME),
            ],
        );
        export.writer.record(
            out,
            &DEVICE_INFO,
            &[
                Value::U32(created),
                Value::U8(Some(0)),
                Value::U16(Some(MANUFACTURER_DEVELOPMENT)),
                Value::U16(Some(PRODUCT)),
                Value::U16(Some(SOFTWARE_VERSION)),
                Value::Str(PRODUCT_NAME),
            ],
        );
        export
    }

    /// FIT time of active time `active_ms` in the current stretch.
    fn stamp(&self, active_ms: u64) -> Option<u32> {
        fit::timestamp(self.offset_ms.saturating_add(active_ms))
    }

    /// Monotonic FIT time for an event or record at `active_ms`.
    fn advance(&mut self, active_ms: u64) -> Option<u32> {
        let stamp = self.stamp(active_ms).map(|ts| ts.max(self.last_ts));
        if let Some(ts) = stamp {
            self.last_ts = ts;
            self.start_ts.get_or_insert(ts);
        }
        stamp
    }

    fn event(&mut self, out: &mut impl Sink, active_ms: u64, event_type: u8) {
        let ts = self.advance(active_ms);
        self.writer.record(
            out,
            &EVENT,
            &[
                Value::U32(ts),
                Value::Enum(Some(EVENT_TIMER)),
                Value::Enum(Some(event_type)),
            ],
        );
    }

    /// Feed one slot. Returns [`Flow::End`] once the ride's terminal record,
    /// or the next ride's start, arrives.
    pub fn slot(&mut self, slot: &Slot, out: &mut impl Sink) -> Flow {
        match log::decode(slot) {
            Some(entry) => self.entry(&entry, out),
            None if self.ended => Flow::End,
            None => Flow::More,
        }
    }

    pub fn entry(&mut self, entry: &Entry, out: &mut impl Sink) -> Flow {
        if self.ended {
            return Flow::End;
        }
        if entry.kind == Kind::Start && (self.started || entry.ride_id != self.ride_id) {
            // An interrupted ride ends where the next one starts.
            if self.started {
                self.ended = true;
                return Flow::End;
            }
            return Flow::More;
        }
        if entry.ride_id != self.ride_id {
            return Flow::More;
        }
        match entry.kind {
            Kind::Start => {
                self.started = true;
                self.pending_start = Some(entry.active_ms);
            }
            _ if !self.started => {}
            Kind::Samples => {
                for sample in &entry.samples[..usize::from(entry.count)] {
                    self.sample(sample, out);
                }
            }
            Kind::Pause => {
                self.active_ms = self.active_ms.max(entry.active_ms);
                // A pause right after a resume, with no sample between, adds nothing.
                if self.pending_start.take().is_none() && !self.paused {
                    self.event(out, entry.active_ms, EVENT_TYPE_STOP_ALL);
                }
                self.paused = true;
            }
            Kind::Resume => {
                self.paused = false;
                self.stretch_dated = false;
                self.pending_start = Some(entry.active_ms);
            }
            Kind::Finish | Kind::Recovered | Kind::Full => {
                self.active_ms = self.active_ms.max(entry.active_ms);
                self.ended = true;
                return Flow::End;
            }
        }
        Flow::More
    }

    fn sample(&mut self, sample: &Sample, out: &mut impl Sink) {
        if let Some(utc) = sample.utc_ms
            && !self.stretch_dated
            && let Some(offset) = utc.checked_sub(sample.active_ms)
        {
            self.offset_ms = offset;
            self.stretch_dated = true;
            self.first_offset_ms.get_or_insert(offset);
        }
        if let Some(active) = self.pending_start.take() {
            self.event(out, active, EVENT_TYPE_START);
            self.previous_active_ms = Some(active);
            self.paused = false;
        }
        let elapsed = sample
            .active_ms
            .saturating_sub(self.previous_active_ms.unwrap_or(sample.active_ms));
        self.previous_active_ms = Some(sample.active_ms);
        self.active_ms = self.active_ms.max(sample.active_ms);
        let step = sample
            .speed_mm_s
            .map(|speed| u64::from(speed) * elapsed / 1000);
        if let Some(step) = step {
            self.distance_mm = Some(self.distance_mm.unwrap_or(0) + step);
        }
        self.totals.add(elapsed, step.unwrap_or(0), sample, None);
        let position = sample.location_e7.filter(|(lat, lon)| {
            (-900_000_000..=900_000_000).contains(lat)
                && (-1_800_000_000..=1_800_000_000).contains(lon)
        });
        if position.is_some() {
            self.first_position = self.first_position.or(position);
            self.last_position = position;
        }
        self.samples += 1;
        let ts = self.advance(sample.active_ms);
        self.writer.record(
            out,
            &RECORD,
            &[
                Value::U32(ts),
                Value::I32(position.map(|(lat, _)| semicircles(lat))),
                Value::I32(position.map(|(_, lon)| semicircles(lon))),
                Value::U32(
                    self.distance_mm
                        .map(|mm| (mm / 10).min(u64::from(u32::MAX - 1)) as u32),
                ),
                Value::U16(
                    sample
                        .speed_mm_s
                        .map(|v| v.min(u32::from(u16::MAX - 1)) as u16),
                ),
                Value::U8(sample.heart_bpm.map(|v| v.min(254) as u8)),
                Value::U8(sample.cadence_tenths.map(|v| ((v + 5) / 10).min(254) as u8)),
                Value::U16(sample.power_watts.filter(|w| *w != u16::MAX)),
                Value::I16(
                    sample
                        .gradient_tenths
                        .map(|g| g.saturating_mul(10))
                        .filter(|g| *g != i16::MAX),
                ),
            ],
        );
    }

    /// Close the ride: the last timer stop, then lap, session, activity and
    /// the file CRC. Returns what the sizing pass learned.
    pub fn finish(mut self, out: &mut impl Sink) -> Plan {
        if self.pending_start.is_none() && !self.paused && self.started {
            self.event(out, self.active_ms, EVENT_TYPE_STOP_ALL);
        }
        let end = self.last_ts;
        let start = self.start_ts.unwrap_or(end);
        let started = self.start_ts.or_else(|| self.stamp(0));
        let ts = started.map(|started| end.max(started));
        let elapsed_ms = u64::from(end.saturating_sub(start)) * 1000;
        let timer = Some(self.active_ms.min(u64::from(u32::MAX - 1)) as u32);
        let elapsed = Some(elapsed_ms.max(self.active_ms).min(u64::from(u32::MAX - 1)) as u32);
        let distance = self
            .distance_mm
            .map(|mm| (mm / 10).min(u64::from(u32::MAX - 1)) as u32);
        let totals = &self.totals;
        let speed = |v: Option<u32>| v.map(|v| v.min(u32::from(u16::MAX - 1)) as u16);
        let avg_speed = speed(self.distance_mm.and(totals.avg_speed_mm_s()));
        let max_speed = speed(totals.max_speed_mm_s());
        let byte = |v: Option<u16>| v.map(|v| v.min(254) as u8);
        let (avg_hr, max_hr) = (byte(totals.avg_heart_bpm()), byte(totals.max_heart_bpm()));
        let avg_cadence = byte(totals.avg_cadence_rpm());
        let (avg_power, max_power) = (totals.avg_power(), totals.max_power());
        let first = self.first_position;
        let last = self.last_position;
        let lat = |p: Option<(i32, i32)>| p.map(|(lat, _)| semicircles(lat));
        let lon = |p: Option<(i32, i32)>| p.map(|(_, lon)| semicircles(lon));
        self.writer.record(
            out,
            &LAP,
            &[
                Value::U32(ts),
                Value::U16(Some(0)),
                Value::Enum(Some(EVENT_LAP)),
                Value::Enum(Some(EVENT_TYPE_STOP)),
                Value::U32(started),
                Value::I32(lat(first)),
                Value::I32(lon(first)),
                Value::I32(lat(last)),
                Value::I32(lon(last)),
                Value::U32(elapsed),
                Value::U32(timer),
                Value::U32(distance),
                Value::U16(avg_speed),
                Value::U16(max_speed),
                Value::U8(avg_hr),
                Value::U8(max_hr),
                Value::U8(avg_cadence),
                Value::U16(avg_power),
                Value::U16(max_power),
                Value::Enum(Some(LAP_TRIGGER_SESSION_END)),
                Value::Enum(Some(SPORT_CYCLING)),
            ],
        );
        self.writer.record(
            out,
            &SESSION,
            &[
                Value::U32(ts),
                Value::U16(Some(0)),
                Value::Enum(Some(EVENT_SESSION)),
                Value::Enum(Some(EVENT_TYPE_STOP)),
                Value::U32(started),
                Value::I32(lat(first)),
                Value::I32(lon(first)),
                Value::Enum(Some(SPORT_CYCLING)),
                Value::Enum(Some(SUB_SPORT_GENERIC)),
                Value::U32(elapsed),
                Value::U32(timer),
                Value::U32(distance),
                Value::U16(avg_speed),
                Value::U16(max_speed),
                Value::U8(avg_hr),
                Value::U8(max_hr),
                Value::U8(avg_cadence),
                Value::U16(avg_power),
                Value::U16(max_power),
                Value::U16(Some(0)),
                Value::U16(Some(1)),
                Value::Enum(Some(SESSION_TRIGGER_ACTIVITY_END)),
                Value::U16(totals.normalized_power()),
            ],
        );
        self.writer.record(
            out,
            &ACTIVITY,
            &[
                Value::U32(ts),
                Value::U32(timer),
                Value::U16(Some(1)),
                Value::Enum(Some(ACTIVITY_MANUAL)),
                Value::Enum(Some(EVENT_ACTIVITY)),
                Value::Enum(Some(EVENT_TYPE_STOP)),
            ],
        );
        self.writer.finish(out);
        Plan {
            size: self.writer.written() as u32,
            offset_ms: self.first_offset_ms,
            samples: self.samples,
        }
    }
}

/// Why a ride cannot become a FIT file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// The ride has no UTC and no start time was given.
    NoClock,
    /// The ride has no samples.
    Empty,
}

/// Decide, after the sizing pass, whether and how the writing pass runs.
pub fn clock_for(plan: &Plan, start_utc_ms: Option<u64>) -> Result<Option<u64>, Refusal> {
    if plan.samples == 0 {
        return Err(Refusal::Empty);
    }
    plan.clock(start_utc_ms).map(Some).ok_or(Refusal::NoClock)
}

/// Both passes over slots held in memory, for hosts and tests: the file of
/// the ride whose START is `slots[0]`.
pub fn write_all<'a>(
    slots: impl Iterator<Item = &'a Slot> + Clone,
    ride_id: u32,
    start_utc_ms: Option<u64>,
    out: &mut impl Sink,
) -> Result<Plan, Refusal> {
    let mut counter = Counter::default();
    let mut sizing = Export::start(ride_id, None, None, &mut counter);
    for slot in slots.clone() {
        if sizing.slot(slot, &mut counter) == Flow::End {
            break;
        }
    }
    let plan = sizing.finish(&mut counter);
    let clock = clock_for(&plan, start_utc_ms)?;
    let mut export = Export::start(ride_id, Some(&plan), clock, out);
    for slot in slots {
        if export.slot(slot, out) == Flow::End {
            break;
        }
    }
    let written = export.finish(out);
    debug_assert_eq!(written.size, plan.size);
    Ok(written)
}

#[cfg(test)]
mod tests;
