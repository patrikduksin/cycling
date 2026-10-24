//! Position fixes and receiver observations.
pub const STALE_MS: u64 = 3_000;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FixState {
    NoData,
    NoFix,
    Fresh,
    Stale,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Identity {
    Unknown,
    Pair,
    Pdt,
    Conflicting,
}
impl Identity {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Pair => "pair",
            Self::Pdt => "pdt",
            Self::Conflicting => "conflicting",
        }
    }
}
impl FixState {
    pub const fn name(self) -> &'static str {
        match self {
            Self::NoData => "no_data",
            Self::NoFix => "no_fix",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub state: FixState,
    pub identity: Identity,
    pub latitude_e7: Option<i32>,
    pub longitude_e7: Option<i32>,
    pub satellites: Option<u8>,
    /// Horizontal dilution of precision of the exposed fix, in tenths, when
    /// the receiver reported it for that fix's epoch.
    pub hdop_tenths: Option<u16>,
    /// Receiver ground speed (Doppler) for the exposed fix's epoch, in mm/s.
    /// Absent when the receiver sends no speed for that epoch.
    pub speed_mm_s: Option<u32>,
    /// UTC time of day of the exposed fix's epoch, in ms. Fixes of the same
    /// epoch from different sentences share it.
    pub epoch_ms: Option<u32>,
    /// UTC time of day of the last fix (GGA or RMC) as ASCII `hhmmss`: a display
    /// and night fallback for receivers that never date a fix. It has no date;
    /// for UTC use `utc_ms`.
    pub utc: Option<[u8; 6]>,
    /// UTC now in ms since the Unix epoch, by the receiver: read from its dated
    /// RMC fixes and carried on by uptime, so it keeps going through a lost
    /// fix. It steps back only after a correction of a second or more is
    /// confirmed by three readings half a second apart, so a stall that
    /// drains within half a second cannot step it back; uptime drift can,
    /// once, after many hours awake. None until a plausible dated fix since
    /// the last control boundary.
    pub utc_ms: Option<u64>,
    pub age_ms: Option<u64>,
    pub bytes: u32,
    pub valid_sentences: u32,
    pub checksum_errors: u32,
    pub parse_errors: u32,
    pub overflows: u32,
    /// Standard sentences that ran past the parser's line limit without an
    /// ending. NMEA caps them at 82 bytes, so this is a receiver or framing fault.
    pub line_overflows: u32,
    /// Proprietary (`$P...`) lines past the line limit. Vendors may send long
    /// ones, so the parser skips them without counting a fault.
    pub long_proprietary: u32,
    pub uart_errors: u32,
    /// Bytes outside printable ASCII, CR and LF, wherever they arrive: the
    /// volume of the receiver's non-NMEA (binary) output.
    pub binary_bytes: u32,
    /// Candidate lines that a non-NMEA byte ended before their line ending. A
    /// `$` inside binary output starts one; this is expected, not a fault.
    pub non_nmea_frames: u32,
}

/// A bounded copy of the stream around one framing event, to capture what the
/// receiver sends. `at` is the event byte's index in the received stream.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Excerpt {
    pub at: Option<u32>,
    /// The first bytes of the candidate line after its `$`, or of the gap.
    pub head: [u8; 32],
    pub head_len: u8,
    /// The bytes just before the event byte.
    pub before: [u8; 8],
    pub before_len: u8,
    /// The event byte and the bytes after it.
    pub after: [u8; 8],
    pub after_len: u8,
}
impl Excerpt {
    pub fn head(&self) -> &[u8] {
        &self.head[..usize::from(self.head_len)]
    }
    pub fn before(&self) -> &[u8] {
        &self.before[..usize::from(self.before_len)]
    }
    pub fn after(&self) -> &[u8] {
        &self.after[..usize::from(self.after_len)]
    }
}

/// The latest excerpt of each framing event the parser recovers from.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Diagnostics {
    /// The last printable line that overflowed: its head, and the bytes
    /// either side of the first byte that did not fit.
    pub overflow: Excerpt,
    /// The last candidate line a non-NMEA byte ended, around that byte.
    pub non_nmea: Excerpt,
    /// The last bytes after a line ending that did not start a sentence: the
    /// head of whatever the receiver sends between sentences.
    pub gap: Excerpt,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: FixState::NoData,
            identity: Identity::Unknown,
            latitude_e7: None,
            longitude_e7: None,
            satellites: None,
            hdop_tenths: None,
            speed_mm_s: None,
            epoch_ms: None,
            utc: None,
            utc_ms: None,
            age_ms: None,
            bytes: 0,
            valid_sentences: 0,
            checksum_errors: 0,
            parse_errors: 0,
            overflows: 0,
            line_overflows: 0,
            long_proprietary: 0,
            uart_errors: 0,
            binary_bytes: 0,
            non_nmea_frames: 0,
        }
    }
}
