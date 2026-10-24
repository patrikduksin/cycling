//! A recorded ride as a FIT activity file, streamed slot by slot.
//!
//! The journal keeps samples, pauses and the end of each ride; this module
//! turns them into the messages Strava, Garmin Connect and other FIT readers
//! expect from a cycling computer: `file_id`, `device_info`, timer events for
//! the start, every pause and resume and the end, one `record` per sample,
//! one `lap` as each lap ends, then the `session` and the `activity`.
//!
//! Laps: a sample marked as the first of a lap (`log::LapStart`) closes the
//! lap before it, which ends at the record before, with the mark's trigger:
//! manual, distance, or for a workout step time or distance (FIT has no
//! workout-step trigger). The last lap ends with the session. A ride recorded
//! without lap marks is one lap. Lap totals follow the session's rules, and
//! the last lap's timer takes what the earlier ones left of the ride's, so
//! the laps add up to the session.
//!
//! The FIT header carries the file's size, so a file is written in two passes
//! over the same slots: a sizing pass into a [`Counter`], whose [`Plan`] gives
//! the size and the ride's clock, then the real one. Each pass holds only
//! running totals, so a ride of any length streams through small buffers.
//!
//! Time: a sample's UTC is `utc - active` of the running stretch it belongs
//! to plus its active time, as the host GPX export dates samples. A stretch
//! starts at the start, each resume and each unreadable slot (which may have
//! hidden a pause). A stretch whose clock arrives part way is dated from it
//! back to its start: the sizing pass notes those offsets for the writing
//! pass. A stretch without any UTC keeps the previous stretch's offset, so
//! its pause counts as no time. A ride with no UTC at all needs a start time
//! from the caller. Timestamps never go backwards.
//!
//! Altitude is the barometric one the journal keeps per sample (smoothed,
//! standard atmosphere, 0.2 m steps), written as `altitude` and
//! `enhanced_altitude`; samples without it leave both invalid in the single
//! record definition. Total ascent and descent follow those altitudes with
//! the ride page's rules (`Totals`), and are left out of a ride without any.
//! Rides recorded before altitude was kept have none.
//!
//! What the journal does not have stays out: no distance without a
//! recorded speed (wheel or GPS), so readers fall back to GPS for distance.
//! Other totals use the ride page's rules too (`Totals`, and `Lap` for each
//! lap; a lap's ascent and descent are the session's over its time).

use crate::ride::laps::Lap;
use crate::ride::log::{self, Entry, Kind, LapStart, Sample, Slot};
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
const LAP_TRIGGER_MANUAL: u8 = 0;
const LAP_TRIGGER_TIME: u8 = 1;
const LAP_TRIGGER_DISTANCE: u8 = 2;
const LAP_TRIGGER_SESSION_END: u8 = 7;
const SESSION_TRIGGER_ACTIVITY_END: u8 = 0;
const ACTIVITY_MANUAL: u8 = 0;

const fn lap_trigger(start: LapStart) -> Option<u8> {
    match start {
        LapStart::Ride => None,
        LapStart::Manual => Some(LAP_TRIGGER_MANUAL),
        LapStart::Distance | LapStart::DistanceStep => Some(LAP_TRIGGER_DISTANCE),
        LapStart::Step => Some(LAP_TRIGGER_TIME),
    }
}

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
        Field::new(2, BaseType::Uint16),
        Field::new(78, BaseType::Uint32),
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
        Field::new(21, BaseType::Uint16),
        Field::new(22, BaseType::Uint16),
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
        Field::new(22, BaseType::Uint16),
        Field::new(23, BaseType::Uint16),
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

const LAP_RECORD: usize = 58;

/// Largest output of one [`Export::slot`] call: a resume, four records each
/// closing a lap, and a pause. Sinks that drain between calls need at least
/// this much room.
pub const MAX_SLOT_OUTPUT: usize = 2 * 7 + log::SAMPLES_PER_BATCH * (31 + LAP_RECORD);
/// Output of [`Export::start`]: header, definitions, `file_id` and `device_info`.
pub const MAX_START_OUTPUT: usize = 384;
/// Output of [`Export::finish`]: the closing event, lap, session, activity and CRC.
pub const MAX_FINISH_OUTPUT: usize = 192;

/// Stretches whose clock arrives part way that a plan remembers; later
/// ones keep the previous offset until their clock arrives.
pub const LATE_STRETCHES: usize = 8;

/// What the sizing pass learned, for the pass that writes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// Bytes of the complete file.
    pub size: u32,
    /// UTC minus active time of the ride's first dated sample.
    pub offset_ms: Option<u64>,
    pub samples: u32,
    /// `lap` messages of the file.
    pub laps: u16,
    /// Recorded time of the ride.
    pub active_ms: u64,
    /// Stretch number and offset of stretches dated part way.
    late: [(u32, u64); LATE_STRETCHES],
    late_count: usize,
    /// Lowest and highest offset any stretch dated itself with.
    offsets: Option<(u64, u64)>,
}

impl Plan {
    /// The offset the writing pass dates samples with: the ride's own, or
    /// one that puts its start at `start_utc_ms`.
    pub fn clock(&self, start_utc_ms: Option<u64>) -> Option<u64> {
        self.offset_ms.or(start_utc_ms)
    }

    fn late_offset(&self, stretch: u32) -> Option<u64> {
        self.late[..self.late_count]
            .iter()
            .find(|(number, _)| *number == stretch)
            .map(|(_, offset)| *offset)
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
    /// Running stretches so far, and whether this one began without UTC.
    stretch: u32,
    stretch_untimed: bool,
    /// Offsets of stretches dated part way: the plan's, then those found.
    plan: Option<Plan>,
    late: [(u32, u64); LATE_STRETCHES],
    late_count: usize,
    offsets: Option<(u64, u64)>,
    /// The latest sample, whose readings hold until the next one or a pause.
    last_sample: Option<Sample>,
    /// A start or resume waiting for the next sample to date it.
    pending_start: Option<u64>,
    paused: bool,
    start_ts: Option<u32>,
    last_ts: u32,
    previous_active_ms: Option<u64>,
    active_ms: u64,
    samples: u32,
    first_position: Option<(i32, i32)>,
    distance_mm: Option<u64>,
    totals: Totals,
    lap: Lap,
    lap_start_ts: Option<u32>,
    lap_first_position: Option<(i32, i32)>,
    lap_last_position: Option<(i32, i32)>,
    laps: u16,
    laps_timer_ms: u64,
    /// The ride's ascent and descent when the lap being written began.
    lap_climbed_from_dm: (u32, u32),
    /// The ride's distance when the current lap began.
    lap_distance_from_mm: u64,
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
            stretch: 0,
            stretch_untimed: false,
            plan: plan.copied(),
            late: [(0, 0); LATE_STRETCHES],
            late_count: 0,
            offsets: None,
            last_sample: None,
            pending_start: None,
            paused: false,
            start_ts: None,
            last_ts: 0,
            previous_active_ms: None,
            active_ms: 0,
            samples: 0,
            first_position: None,
            distance_mm: None,
            totals: Totals::default(),
            lap: Lap::default(),
            lap_start_ts: None,
            lap_first_position: None,
            lap_last_position: None,
            laps: 0,
            laps_timer_ms: 0,
            lap_climbed_from_dm: (0, 0),
            lap_distance_from_mm: 0,
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

    /// A new running stretch: date it from the plan when the sizing pass
    /// found its clock part way, else from its first UTC sample.
    fn new_stretch(&mut self) {
        self.stretch += 1;
        self.stretch_dated = false;
        self.stretch_untimed = false;
        if let Some(offset) = self.plan.and_then(|plan| plan.late_offset(self.stretch)) {
            self.offset_ms = offset;
            self.stretch_dated = true;
        }
    }

    /// Account the readings of the last sample up to `active_ms`, as the
    /// live totals do for the time between a sample and a pause or the end.
    fn settle(&mut self, active_ms: u64) {
        let (Some(previous), Some(last)) = (self.previous_active_ms, self.last_sample) else {
            return;
        };
        let elapsed = active_ms.saturating_sub(previous);
        if elapsed == 0 {
            return;
        }
        let step = last
            .speed_mm_s
            .map(|speed| u64::from(speed) * elapsed / 1000);
        if let Some(step) = step {
            self.distance_mm = Some(self.distance_mm.unwrap_or(0) + step);
        }
        let held = Sample {
            speed_mm_s: last.speed_mm_s,
            power_watts: last.power_watts,
            ..Sample::default()
        };
        self.totals.add(elapsed, step.unwrap_or(0), &held, None);
        self.lap.add(elapsed, step.unwrap_or(0), &held);
        self.previous_active_ms = Some(active_ms);
    }

    /// Feed one slot. Returns [`Flow::End`] once the ride's terminal record,
    /// or the next ride's start, arrives.
    pub fn slot(&mut self, slot: &Slot, out: &mut impl Sink) -> Flow {
        match log::decode(slot) {
            Some(entry) => self.entry(&entry, out),
            None if self.ended => Flow::End,
            None => {
                // An unreadable slot may have been a pause and resume:
                // take the clock again from the next dated sample.
                if self.started {
                    self.new_stretch();
                }
                Flow::More
            }
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
                self.new_stretch();
                self.pending_start = Some(entry.active_ms);
            }
            _ if !self.started => {}
            Kind::Samples => {
                for sample in &entry.samples[..usize::from(entry.count)] {
                    self.sample(sample, out);
                }
            }
            Kind::Pause => {
                if !self.paused && self.pending_start.is_none() {
                    self.settle(entry.active_ms);
                }
                self.active_ms = self.active_ms.max(entry.active_ms);
                // A pause right after a resume, with no sample between, adds nothing.
                if self.pending_start.take().is_none() && !self.paused {
                    self.event(out, entry.active_ms, EVENT_TYPE_STOP_ALL);
                }
                self.paused = true;
            }
            Kind::Resume => {
                self.paused = false;
                self.last_sample = None;
                self.new_stretch();
                self.pending_start = Some(entry.active_ms);
            }
            Kind::Finish | Kind::Recovered | Kind::Full => {
                if !self.paused && self.pending_start.is_none() {
                    self.settle(entry.active_ms);
                }
                self.active_ms = self.active_ms.max(entry.active_ms);
                self.ended = true;
                return Flow::End;
            }
        }
        Flow::More
    }

    fn sample(&mut self, sample: &Sample, out: &mut impl Sink) {
        match sample
            .utc_ms
            .and_then(|utc| utc.checked_sub(sample.active_ms))
        {
            Some(offset) if !self.stretch_dated => {
                self.offset_ms = offset;
                self.stretch_dated = true;
                self.first_offset_ms.get_or_insert(offset);
                self.offsets = Some(self.offsets.map_or((offset, offset), |(low, high)| {
                    (low.min(offset), high.max(offset))
                }));
                if self.stretch_untimed && self.late_count < LATE_STRETCHES {
                    self.late[self.late_count] = (self.stretch, offset);
                    self.late_count += 1;
                }
            }
            Some(offset) => {
                self.first_offset_ms.get_or_insert(offset);
            }
            None if !self.stretch_dated => self.stretch_untimed = true,
            None => {}
        }
        if let Some(trigger) = sample.lap.and_then(lap_trigger)
            && self.samples > 0
        {
            self.write_lap(out, trigger, self.lap.time_ms);
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
        self.last_sample = Some(*sample);
        self.active_ms = self.active_ms.max(sample.active_ms);
        let step = sample
            .speed_mm_s
            .map(|speed| u64::from(speed) * elapsed / 1000);
        if let Some(step) = step {
            self.distance_mm = Some(self.distance_mm.unwrap_or(0) + step);
        }
        self.totals.add(elapsed, step.unwrap_or(0), sample, None);
        if let Some(altitude) = sample.altitude {
            self.totals.follow_altitude_dm(altitude.dm());
        }
        self.lap.add(elapsed, step.unwrap_or(0), sample);
        let position = sample.location_e7.filter(|(lat, lon)| {
            (-900_000_000..=900_000_000).contains(lat)
                && (-1_800_000_000..=1_800_000_000).contains(lon)
        });
        if position.is_some() {
            self.first_position = self.first_position.or(position);
            self.lap_first_position = self.lap_first_position.or(position);
            self.lap_last_position = position;
        }
        self.samples += 1;
        let altitude = sample.altitude.map(log::Altitude::fit);
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
                Value::U8(
                    sample
                        .cadence_tenths
                        .map(|v| (v.saturating_add(5) / 10).min(254) as u8),
                ),
                Value::U16(sample.power_watts.filter(|w| *w != u16::MAX)),
                Value::I16(
                    sample
                        .gradient_tenths
                        .map(|g| g.saturating_mul(10))
                        .filter(|g| *g != i16::MAX),
                ),
                Value::U16(altitude),
                Value::U32(altitude.map(u32::from)),
            ],
        );
    }

    fn write_lap(&mut self, out: &mut impl Sink, trigger: u8, timer_ms: u64) {
        let end = self.last_ts;
        let started = self
            .lap_start_ts
            .or(self.start_ts)
            .or_else(|| self.stamp(0));
        let ts = started.map(|started| end.max(started));
        let start = started.unwrap_or(end).min(end);
        let lap = self.lap;
        self.laps_timer_ms = self.laps_timer_ms.saturating_add(timer_ms);
        let elapsed_ms = u64::from(end - start) * 1000;
        let timer = Some(timer_ms.min(u64::from(u32::MAX - 1)) as u32);
        let elapsed = Some(elapsed_ms.max(timer_ms).min(u64::from(u32::MAX - 1)) as u32);
        let distance_mm = self.distance_mm.map(|_| lap.distance_mm);
        let centimetres = |mm: u64| (mm / 10).min(u64::from(u32::MAX - 1)) as u32;
        // Differences of the rounded running total, so the laps add up to the session.
        let distance = self
            .distance_mm
            .map(|total| centimetres(total).saturating_sub(centimetres(self.lap_distance_from_mm)));
        let speed = |v: Option<u32>| v.map(|v| v.min(u32::from(u16::MAX - 1)) as u16);
        let byte = |v: Option<u16>| v.map(|v| v.min(254) as u8);
        let lat = |p: Option<(i32, i32)>| p.map(|(lat, _)| semicircles(lat));
        let lon = |p: Option<(i32, i32)>| p.map(|(_, lon)| semicircles(lon));
        let (first, last) = (self.lap_first_position, self.lap_last_position);
        let totals = &self.totals;
        let (ascent_dm, descent_dm) = (totals.ascent_dm(), totals.descent_dm());
        let metres = |dm: u32| ((dm + 5) / 10).min(u32::from(u16::MAX - 1)) as u16;
        let (from_ascent, from_descent) = self.lap_climbed_from_dm;
        // Differences of the rounded running totals, so the laps add up to the session.
        let climbed = |to: u32, from: u32| metres(to).saturating_sub(metres(from));
        let ascent = totals
            .altitude_dm()
            .map(|_| climbed(ascent_dm, from_ascent));
        let descent = totals
            .altitude_dm()
            .map(|_| climbed(descent_dm, from_descent));
        self.writer.record(
            out,
            &LAP,
            &[
                Value::U32(ts),
                Value::U16(Some(self.laps)),
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
                Value::U16(speed(distance_mm.and(lap.avg_speed_mm_s()))),
                Value::U16(speed(lap.max_speed_mm_s())),
                Value::U8(byte(lap.avg_heart_bpm())),
                Value::U8(byte(lap.max_heart_bpm())),
                Value::U8(byte(lap.avg_cadence_rpm())),
                Value::U16(lap.avg_power()),
                Value::U16(lap.max_power()),
                Value::U16(ascent),
                Value::U16(descent),
                Value::Enum(Some(trigger)),
                Value::Enum(Some(SPORT_CYCLING)),
            ],
        );
        self.laps = self.laps.saturating_add(1);
        self.lap = Lap::default();
        self.lap_start_ts = ts;
        self.lap_first_position = None;
        self.lap_last_position = None;
        self.lap_climbed_from_dm = (ascent_dm, descent_dm);
        self.lap_distance_from_mm = self.distance_mm.unwrap_or(0);
    }

    /// Close the ride: the last timer stop, then the last lap, session,
    /// activity and the file CRC. Returns what the sizing pass learned.
    pub fn finish(mut self, out: &mut impl Sink) -> Plan {
        if self.pending_start.is_none() && !self.paused && self.started {
            self.event(out, self.active_ms, EVENT_TYPE_STOP_ALL);
        }
        // The last lap takes what the others left, so the laps add up to the session.
        let rest_ms = self.active_ms.saturating_sub(self.laps_timer_ms);
        self.write_lap(out, LAP_TRIGGER_SESSION_END, rest_ms);
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
        let metres = |dm: u32| ((dm + 5) / 10).min(u32::from(u16::MAX - 1)) as u16;
        let ascent = totals.altitude_dm().map(|_| metres(totals.ascent_dm()));
        let descent = totals.altitude_dm().map(|_| metres(totals.descent_dm()));
        let first = self.first_position;
        let lat = |p: Option<(i32, i32)>| p.map(|(lat, _)| semicircles(lat));
        let lon = |p: Option<(i32, i32)>| p.map(|(_, lon)| semicircles(lon));
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
                Value::U16(ascent),
                Value::U16(descent),
                Value::U16(Some(0)),
                Value::U16(Some(self.laps)),
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
            laps: self.laps,
            active_ms: self.active_ms,
            late: self.late,
            late_count: self.late_count,
            offsets: self.offsets,
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
    /// The given start puts the ride outside the dates FIT can hold
    /// (1990 to 2126), as a start in seconds instead of milliseconds would.
    BadClock,
}

/// Decide, after the sizing pass, whether and how the writing pass runs.
pub fn clock_for(plan: &Plan, start_utc_ms: Option<u64>) -> Result<Option<u64>, Refusal> {
    if plan.samples == 0 {
        return Err(Refusal::Empty);
    }
    let clock = plan.clock(start_utc_ms).ok_or(Refusal::NoClock)?;
    // Every sample is dated by the clock or a stretch's own offset, plus an
    // active time no later than the ride's.
    let (low, high) = plan.offsets.map_or((clock, clock), |(low, high)| {
        (low.min(clock), high.max(clock))
    });
    let end = high.checked_add(plan.active_ms).ok_or(Refusal::BadClock)?;
    if fit::timestamp(low).is_none() || fit::timestamp(end).is_none() {
        return Err(Refusal::BadClock);
    }
    Ok(Some(clock))
}

/// Both passes over a restartable slot source, for hosts and tests: the file
/// of the ride whose START is the first slot.
pub fn write_all<S: core::borrow::Borrow<Slot>>(
    slots: impl Iterator<Item = S> + Clone,
    ride_id: u32,
    start_utc_ms: Option<u64>,
    out: &mut impl Sink,
) -> Result<Plan, Refusal> {
    let mut counter = Counter::default();
    let mut sizing = Export::start(ride_id, None, None, &mut counter);
    for slot in slots.clone() {
        if sizing.slot(slot.borrow(), &mut counter) == Flow::End {
            break;
        }
    }
    let plan = sizing.finish(&mut counter);
    let clock = clock_for(&plan, start_utc_ms)?;
    let mut export = Export::start(ride_id, Some(&plan), clock, out);
    for slot in slots {
        if export.slot(slot.borrow(), out) == Flow::End {
            break;
        }
    }
    let written = export.finish(out);
    debug_assert_eq!(written.size, plan.size);
    Ok(written)
}

#[cfg(test)]
mod tests;
