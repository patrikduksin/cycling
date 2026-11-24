//! Portable SNTP validation and the device clock: a UTC anchor advanced from
//! monotonic time, fed by SNTP, the receiver and a set time, and kept across
//! resets as RTC ticks.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const NTP_UNIX_EPOCH: u32 = 2_208_988_800;
pub const STALE_AFTER_MS: u64 = 6 * 60 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timestamp {
    pub unix_seconds: u32,
    pub millis: u16,
    pub stratum: u8,
}

impl Timestamp {
    pub fn unix_ms(self) -> u64 {
        u64::from(self.unix_seconds) * 1_000 + u64::from(self.millis)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ParseError {
    Length,
    Source,
    Leap,
    Version,
    Mode,
    RateLimited,
    Denied,
    Stratum,
    Origin,
    Timestamp,
}

pub fn parse_response(
    packet: &[u8],
    expected_origin: &[u8; 8],
    expected_source: bool,
) -> Result<Timestamp, ParseError> {
    if packet.len() < 48 {
        return Err(ParseError::Length);
    }
    if !expected_source {
        return Err(ParseError::Source);
    }
    let leap = packet[0] >> 6;
    let version = (packet[0] >> 3) & 7;
    let mode = packet[0] & 7;
    if !(3..=4).contains(&version) {
        return Err(ParseError::Version);
    }
    if mode != 4 {
        return Err(ParseError::Mode);
    }
    if packet[24..32] != *expected_origin {
        return Err(ParseError::Origin);
    }
    if packet[1] == 0 {
        return Err(match &packet[12..16] {
            b"RATE" => ParseError::RateLimited,
            b"DENY" | b"RSTR" => ParseError::Denied,
            _ => ParseError::Stratum,
        });
    }
    if packet[1] > 15 {
        return Err(ParseError::Stratum);
    }
    if leap == 3 {
        return Err(ParseError::Leap);
    }
    if packet[32..40] == [0; 8] || packet[40..48] == [0; 8] {
        return Err(ParseError::Timestamp);
    }
    let ntp_seconds = u32::from_be_bytes(packet[40..44].try_into().unwrap());
    if ntp_seconds < NTP_UNIX_EPOCH {
        return Err(ParseError::Timestamp);
    }
    let fraction = u32::from_be_bytes(packet[44..48].try_into().unwrap());
    Ok(Timestamp {
        unix_seconds: ntp_seconds - NTP_UNIX_EPOCH,
        millis: ((u64::from(fraction) * 1_000) >> 32) as u16,
        stratum: packet[1],
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Status {
    Unavailable,
    Syncing,
    Fresh,
    Offline,
    Stale,
}

impl Status {
    pub fn name(self) -> &'static str {
        match self {
            Self::Unavailable => "unavailable",
            Self::Syncing => "syncing",
            Self::Fresh => "fresh",
            Self::Offline => "offline",
            Self::Stale => "stale",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Snapshot {
    pub unix_seconds: Option<u64>,
    pub millis: u16,
    pub local_minutes: Option<u16>,
    pub age_ms: Option<u64>,
    pub status: Status,
}

/// A snapshot read `ms` later: its clock ran on, and it went stale with age.
impl device_api::journal::Aging for Snapshot {
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
            status: if age_ms.is_some_and(|age| age >= STALE_AFTER_MS) {
                Status::Stale
            } else {
                self.status
            },
        }
    }
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            unix_seconds: None,
            millis: 0,
            local_minutes: None,
            age_ms: None,
            status: Status::Unavailable,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Source {
    /// A host or the phone said what time it is.
    Set,
    Gnss,
    Sntp,
}

impl Source {
    pub fn name(self) -> &'static str {
        match self {
            Self::Set => "set",
            Self::Gnss => "gnss",
            Self::Sntp => "sntp",
        }
    }

    const fn code(self) -> u32 {
        match self {
            Self::Set => 1,
            Self::Gnss => 2,
            Self::Sntp => 3,
        }
    }

    const fn from_code(code: u32) -> Option<Self> {
        match code {
            1 => Some(Self::Set),
            2 => Some(Self::Gnss),
            3 => Some(Self::Sntp),
            _ => None,
        }
    }
}

/// UTC as a source gave it, the uptime it was true at, and how long ago the
/// source gave it then: a resumed anchor is older than this boot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Anchor {
    pub unix_ms: u64,
    pub uptime_ms: u64,
    pub age_ms: u64,
    pub source: Source,
}

/// UTC from 2026 up to 2100, in ms: a set time or a kept copy outside it
/// is a wrong clock or corrupt memory.
pub const PLAUSIBLE_MS: core::ops::Range<u64> = 1_767_225_600_000..4_102_444_800_000;

impl Anchor {
    pub const fn new(source: Source, unix_ms: u64, uptime_ms: u64) -> Self {
        Self {
            unix_ms,
            uptime_ms,
            age_ms: 0,
            source,
        }
    }

    /// The same clock, anchored at `now_ms` of uptime.
    pub fn at(self, now_ms: u64) -> Self {
        let elapsed_ms = now_ms.saturating_sub(self.uptime_ms);
        Self {
            unix_ms: self.unix_ms.saturating_add(elapsed_ms),
            uptime_ms: now_ms,
            age_ms: self.age_ms.saturating_add(elapsed_ms),
            source: self.source,
        }
    }

    pub fn snapshot(self, now_ms: u64, timezone_minutes: i16, online: bool) -> Snapshot {
        let now = self.at(now_ms);
        let current_seconds = now.unix_ms / 1_000;
        let local_minutes = ((current_seconds / 60) as i64 + i64::from(timezone_minutes))
            .rem_euclid(24 * 60) as u16;
        Snapshot {
            unix_seconds: Some(current_seconds),
            millis: (now.unix_ms % 1_000) as u16,
            local_minutes: Some(local_minutes),
            age_ms: Some(now.age_ms),
            status: if now.age_ms >= STALE_AFTER_MS {
                Status::Stale
            } else if !online {
                Status::Offline
            } else {
                Status::Fresh
            },
        }
    }
}

/// How far a receiver's time may differ from the anchor it gave before it
/// replaces it: its time is whole NMEA seconds stamped on arrival, so it
/// wanders by a fraction of a second from one sentence to the next.
const RECEIVER_TOLERANCE_MS: u64 = 1_000;

/// The anchor to keep once `offer` arrives. A set time and SNTP always win:
/// someone asked for the one, and the other is the best clock there is. A
/// receiver's time replaces a set time, a stale anchor, or its own when it
/// moved; it never overrules fresh SNTP.
pub fn offered(current: Option<Anchor>, offer: Anchor) -> Option<Anchor> {
    let takes = match (current, offer.source) {
        (None, _) | (_, Source::Set | Source::Sntp) => true,
        (Some(held), Source::Gnss) => {
            let held = held.at(offer.uptime_ms);
            match held.source {
                Source::Set => true,
                _ if held.age_ms >= STALE_AFTER_MS => true,
                Source::Sntp => false,
                Source::Gnss => held.unix_ms.abs_diff(offer.unix_ms) > RECEIVER_TOLERANCE_MS,
            }
        }
    };
    if takes { Some(offer) } else { current }
}

/// The RTC timer's ticks as microseconds, for a calibrated slow-clock
/// `period` in microseconds with 19 fractional bits. Split so the product
/// cannot overflow however long the timer has run.
pub fn ticks_to_us(ticks: u64, period: u32) -> u64 {
    const FRACTION: u32 = 19;
    const MASK: u64 = (1 << FRACTION) - 1;
    let period = u64::from(period);
    (ticks >> FRACTION) * period + (((ticks & MASK) * period) >> FRACTION)
}

/// An anchor as RTC memory keeps it across a reset: UTC and its age at a
/// reading of the RTC timer, which counts from power-on through resets and
/// sleep. Kept as ticks, not microseconds, because each boot calibrates the
/// slow clock again.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Retained {
    pub unix_ms: u64,
    pub age_ms: u64,
    pub source: Source,
    pub rtc_ticks: u64,
}

pub const RETAINED_WORDS: usize = 8;
const RETAINED_MAGIC: u32 = 0x434c_4b31;

fn checksum(words: &[u32]) -> u32 {
    words.iter().fold(0x811c_9dc5, |hash, word| {
        (hash ^ word).wrapping_mul(0x0100_0193)
    })
}

impl Retained {
    /// The words to write; the last is the checksum, written last.
    pub fn encode(self) -> [u32; RETAINED_WORDS] {
        let mut words = [
            RETAINED_MAGIC,
            self.unix_ms as u32,
            (self.unix_ms >> 32) as u32,
            self.rtc_ticks as u32,
            (self.rtc_ticks >> 32) as u32,
            self.age_ms.min(u64::from(u32::MAX)) as u32,
            self.source.code(),
            0,
        ];
        words[RETAINED_WORDS - 1] = checksum(&words[..RETAINED_WORDS - 1]);
        words
    }

    /// None for memory a power-on left, or a record torn by a reset.
    pub fn decode(words: &[u32; RETAINED_WORDS]) -> Option<Self> {
        if words[0] != RETAINED_MAGIC
            || words[RETAINED_WORDS - 1] != checksum(&words[..RETAINED_WORDS - 1])
        {
            return None;
        }
        let unix_ms = u64::from(words[1]) | u64::from(words[2]) << 32;
        if !PLAUSIBLE_MS.contains(&unix_ms) {
            return None;
        }
        Some(Self {
            unix_ms,
            rtc_ticks: u64::from(words[3]) | u64::from(words[4]) << 32,
            age_ms: u64::from(words[5]),
            source: Source::from_code(words[6])?,
        })
    }

    /// The anchor at `now_ms` of this boot's uptime, the RTC timer reading
    /// `rtc_ticks` at a slow-clock `period`. None when the timer is behind
    /// the record: power-on or an RTC-module reset restarted it.
    pub fn resume(self, rtc_ticks: u64, period: u32, now_ms: u64) -> Option<Anchor> {
        let elapsed_ms = ticks_to_us(rtc_ticks.checked_sub(self.rtc_ticks)?, period) / 1_000;
        Some(Anchor {
            unix_ms: self.unix_ms.saturating_add(elapsed_ms),
            uptime_ms: now_ms,
            age_ms: self.age_ms.saturating_add(elapsed_ms),
            source: self.source,
        })
    }
}

static SEQUENCE: AtomicU32 = AtomicU32::new(0);
static UNIX_LOW: AtomicU32 = AtomicU32::new(0);
static UNIX_HIGH: AtomicU32 = AtomicU32::new(0);
static UPTIME_LOW: AtomicU32 = AtomicU32::new(0);
static UPTIME_HIGH: AtomicU32 = AtomicU32::new(0);
static AGE_MS: AtomicU32 = AtomicU32::new(0);
static SOURCE: AtomicU32 = AtomicU32::new(0);
static SYNCING: AtomicBool = AtomicBool::new(false);

pub fn set_syncing(syncing: bool) {
    SYNCING.store(syncing, Ordering::Relaxed);
}

/// Changes with every anchor write, for an owner that copies the anchor out.
pub fn generation() -> u32 {
    SEQUENCE.load(Ordering::Acquire)
}

fn store(anchor: Anchor) {
    SEQUENCE.fetch_add(1, Ordering::AcqRel);
    UNIX_LOW.store(anchor.unix_ms as u32, Ordering::Relaxed);
    UNIX_HIGH.store((anchor.unix_ms >> 32) as u32, Ordering::Relaxed);
    UPTIME_LOW.store(anchor.uptime_ms as u32, Ordering::Relaxed);
    UPTIME_HIGH.store((anchor.uptime_ms >> 32) as u32, Ordering::Relaxed);
    AGE_MS.store(
        anchor.age_ms.min(u64::from(u32::MAX)) as u32,
        Ordering::Relaxed,
    );
    SOURCE.store(anchor.source.code(), Ordering::Relaxed);
    SEQUENCE.fetch_add(1, Ordering::Release);
}

fn held() -> Option<Anchor> {
    match clock() {
        Clock::Anchored(anchor) => Some(anchor),
        Clock::Unsynced { .. } => None,
    }
}

pub fn offer(time: Anchor) {
    let current = held();
    if let Some(next) = offered(current, time)
        && Some(next) != current
    {
        store(next);
    }
    if time.source == Source::Sntp {
        SYNCING.store(false, Ordering::Relaxed);
    }
}

/// The anchor a reset kept in RTC memory, before any source has spoken.
pub fn resume(anchor: Anchor) {
    if held().is_none() {
        store(anchor);
    }
}

/// Carry the anchor across a device interval that stopped monotonic time:
/// the RTC kept counting, so UTC moves on by the time uptime missed. Call
/// once before readmitting consumers.
pub fn sleep_elapsed(rtc_elapsed_us: u64, uptime_elapsed_us: u64) {
    let Some(anchor) = held() else {
        return;
    };
    let missed_ms = rtc_elapsed_us.saturating_sub(uptime_elapsed_us) / 1_000;
    if missed_ms > 0 {
        store(Anchor {
            unix_ms: anchor.unix_ms.saturating_add(missed_ms),
            age_ms: anchor.age_ms.saturating_add(missed_ms),
            ..anchor
        });
    }
}

pub fn snapshot(now_ms: u64, timezone_minutes: i16, online: bool) -> Snapshot {
    clock().snapshot(now_ms, timezone_minutes, online)
}

pub fn clock() -> Clock {
    let (unix_low, unix_high, uptime_low, uptime_high, age_ms, source) = loop {
        let before = SEQUENCE.load(Ordering::Acquire);
        if before & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let values = (
            UNIX_LOW.load(Ordering::Relaxed),
            UNIX_HIGH.load(Ordering::Relaxed),
            UPTIME_LOW.load(Ordering::Relaxed),
            UPTIME_HIGH.load(Ordering::Relaxed),
            AGE_MS.load(Ordering::Relaxed),
            SOURCE.load(Ordering::Relaxed),
        );
        if before == SEQUENCE.load(Ordering::Acquire) {
            break values;
        }
    };
    let Some(source) = Source::from_code(source) else {
        return Clock::Unsynced {
            syncing: SYNCING.load(Ordering::Relaxed),
        };
    };
    Clock::Anchored(Anchor {
        unix_ms: u64::from(unix_low) | u64::from(unix_high) << 32,
        uptime_ms: u64::from(uptime_low) | u64::from(uptime_high) << 32,
        age_ms: u64::from(age_ms),
        source,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Clock {
    /// Never given a time since power-on.
    Unsynced {
        syncing: bool,
    },
    Anchored(Anchor),
}

impl Clock {
    pub fn snapshot(self, now_ms: u64, timezone_minutes: i16, online: bool) -> Snapshot {
        match self {
            Self::Unsynced { syncing } => Snapshot::unsynced(syncing),
            Self::Anchored(anchor) => anchor.snapshot(now_ms, timezone_minutes, online),
        }
    }

    pub fn source(self) -> Option<Source> {
        match self {
            Self::Unsynced { .. } => None,
            Self::Anchored(anchor) => Some(anchor.source),
        }
    }
}

impl Snapshot {
    pub fn unsynced(syncing: bool) -> Self {
        Self {
            status: if syncing {
                Status::Syncing
            } else {
                Status::Unavailable
            },
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    static CLOCK_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());

    fn response(origin: [u8; 8]) -> [u8; 48] {
        let mut packet = [0; 48];
        packet[0] = 0x24;
        packet[1] = 2;
        packet[24..32].copy_from_slice(&origin);
        packet[32..36].copy_from_slice(&(NTP_UNIX_EPOCH + 99).to_be_bytes());
        packet[40..44].copy_from_slice(&(NTP_UNIX_EPOCH + 100).to_be_bytes());
        packet[44..48].copy_from_slice(&0x8000_0000u32.to_be_bytes());
        packet
    }

    #[test]
    fn validates_server_response_and_fraction() {
        let origin = [1, 2, 3, 4, 5, 6, 7, 8];
        assert_eq!(
            parse_response(&response(origin), &origin, true),
            Ok(Timestamp {
                unix_seconds: 100,
                millis: 500,
                stratum: 2,
            })
        );
    }

    #[test]
    fn rejects_malformed_wrong_source_rate_limit_and_unsynchronized() {
        let origin = [1; 8];
        for (mut packet, source, error) in [
            (response(origin), false, ParseError::Source),
            (
                {
                    let mut p = response(origin);
                    p[0] = 0x23;
                    p
                },
                true,
                ParseError::Mode,
            ),
            (
                {
                    let mut p = response(origin);
                    p[0] |= 0xc0;
                    p
                },
                true,
                ParseError::Leap,
            ),
            (
                {
                    let mut p = response(origin);
                    p[1] = 0;
                    p
                },
                true,
                ParseError::Stratum,
            ),
            (
                {
                    let mut p = response(origin);
                    p[24] ^= 1;
                    p
                },
                true,
                ParseError::Origin,
            ),
            (
                {
                    let mut p = response(origin);
                    p[40..48].fill(0);
                    p
                },
                true,
                ParseError::Timestamp,
            ),
        ] {
            assert_eq!(parse_response(&packet, &origin, source), Err(error));
            packet.fill(0);
        }
        assert_eq!(
            parse_response(&[0; 47], &origin, true),
            Err(ParseError::Length)
        );
        let mut rate = response(origin);
        rate[0] |= 0xc0;
        rate[1] = 0;
        rate[12..16].copy_from_slice(b"RATE");
        assert_eq!(
            parse_response(&rate, &origin, true),
            Err(ParseError::RateLimited)
        );
        rate[12..16].copy_from_slice(b"DENY");
        assert_eq!(
            parse_response(&rate, &origin, true),
            Err(ParseError::Denied)
        );
    }

    #[test]
    fn advances_utc_offline_and_marks_old_anchor_stale() {
        let _guard = CLOCK_TEST.lock().unwrap();
        offer(Anchor::new(Source::Sntp, 1_700_000_000_250, 9_900));
        let fresh = snapshot(11_150, -180, true);
        assert_eq!(fresh.unix_seconds, Some(1_700_000_001));
        assert_eq!(fresh.millis, 500);
        assert_eq!(fresh.status, Status::Fresh);
        assert_eq!(snapshot(11_150, 0, false).status, Status::Offline);
        assert_eq!(
            snapshot(9_900 + STALE_AFTER_MS, 0, false).status,
            Status::Stale
        );
        assert_eq!(snapshot(9_899, 0, true).age_ms, Some(0));
    }

    #[test]
    fn a_sleep_moves_utc_on_by_the_time_uptime_missed() {
        let _guard = CLOCK_TEST.lock().unwrap();
        offer(Anchor::new(Source::Sntp, 1_700_000_000_250, 1_000));
        sleep_elapsed(10_000_000, 500_000);
        let clock = snapshot(1_000, 0, true);
        assert_eq!(clock.unix_seconds, Some(1_700_000_009));
        assert_eq!(clock.millis, 750);
        assert_eq!(clock.age_ms, Some(9_500));
        assert_eq!(clock.status, Status::Fresh);
    }

    #[test]
    fn a_sleep_that_uptime_kept_up_with_leaves_the_anchor() {
        let _guard = CLOCK_TEST.lock().unwrap();
        offer(Anchor::new(Source::Sntp, 100_000, 5_000));
        for (rtc, uptime) in [(0, 0), (500, 500), (400, 500)] {
            sleep_elapsed(rtc, uptime);
            let clock = snapshot(6_000, 0, true);
            assert_eq!(clock.unix_seconds, Some(101));
            assert_eq!(clock.age_ms, Some(1_000));
        }
    }

    #[test]
    fn a_set_time_yields_to_the_receiver_and_sntp_but_not_back() {
        let set = Anchor::new(Source::Set, 1_000_000, 0);
        let gnss = Anchor::new(Source::Gnss, 2_000_000, 1_000);
        let sntp = Anchor::new(Source::Sntp, 3_000_000, 2_000);
        assert_eq!(offered(None, set), Some(set));
        assert_eq!(
            offered(Some(set), gnss),
            Some(gnss),
            "receiver over a set time"
        );
        assert_eq!(
            offered(Some(gnss), sntp),
            Some(sntp),
            "SNTP over the receiver"
        );
        let later = Anchor::new(Source::Gnss, 4_000_000, 3_000);
        assert_eq!(
            offered(Some(sntp), later),
            Some(sntp),
            "the receiver never overrules SNTP"
        );
        assert_eq!(offered(Some(sntp), set), Some(set), "someone asked");
    }

    #[test]
    fn the_receiver_replaces_its_own_anchor_only_when_it_moved() {
        let gnss = Anchor::new(Source::Gnss, 2_000_000, 1_000);
        let agrees = Anchor::new(Source::Gnss, 2_060_400, 61_000);
        assert_eq!(offered(Some(gnss), agrees), Some(gnss), "no write each fix");
        let a_week_on = Anchor::new(Source::Gnss, 2_000_000 + 7 * 86_400_000, 61_000);
        assert_eq!(offered(Some(gnss), a_week_on), Some(a_week_on));
    }

    #[test]
    fn the_receiver_replaces_a_stale_anchor() {
        let sntp = Anchor::new(Source::Sntp, 3_000_000, 0);
        let fix = Anchor::new(Source::Gnss, 9_000_000, STALE_AFTER_MS);
        assert_eq!(offered(Some(sntp), fix), Some(fix));
        let resumed = Anchor {
            age_ms: STALE_AFTER_MS,
            ..Anchor::new(Source::Gnss, 3_000_000, 0)
        };
        let fix = Anchor::new(Source::Gnss, 3_000_100, 100);
        assert_eq!(
            offered(Some(resumed), fix),
            Some(fix),
            "age before the reset counts"
        );
    }

    #[test]
    fn the_global_clock_applies_the_same_precedence() {
        let _guard = CLOCK_TEST.lock().unwrap();
        offer(Anchor::new(Source::Set, 1_000_000, 0));
        offer(Anchor::new(Source::Gnss, 5_000_000, 0));
        assert_eq!(clock().source(), Some(Source::Gnss));
        assert_eq!(snapshot(0, 0, true).unix_seconds, Some(5_000));
        let before = generation();
        offer(Anchor::new(Source::Gnss, 5_001_200, 1_000));
        assert_eq!(generation(), before, "an ignored fix writes nothing");
        resume(Anchor::new(Source::Set, 1, 0));
        assert_eq!(
            clock().source(),
            Some(Source::Gnss),
            "a source beat the reset's copy"
        );
    }

    #[test]
    fn rtc_ticks_convert_at_the_calibrated_period_without_overflow() {
        let period = (7.352_941 * f64::from(1 << 19)) as u32;
        assert_eq!(ticks_to_us(136_000, period) / 1_000, 999);
        let years: u64 = 32_768 * 10 * 365 * 86_400;
        assert!(years.checked_mul(16_000_000).is_none());
        assert_eq!(
            ticks_to_us(years, 16_000_000),
            10 * 365 * 86_400 * 1_000_000
        );
    }

    #[test]
    fn a_retained_anchor_resumes_with_the_rtc_time_since_it_was_kept() {
        let period = 8 << 19;
        let kept = Retained {
            unix_ms: 1_793_521_800_000,
            age_ms: 60_000,
            source: Source::Sntp,
            rtc_ticks: 1_000_000,
        };
        let decoded = Retained::decode(&kept.encode()).unwrap();
        assert_eq!(decoded, kept);
        let anchor = decoded.resume(1_250_000, period, 700).unwrap();
        assert_eq!(
            anchor,
            Anchor {
                unix_ms: 1_793_521_802_000,
                uptime_ms: 700,
                age_ms: 62_000,
                source: Source::Sntp,
            }
        );
        assert_eq!(
            anchor.snapshot(1_700, 0, true).unix_seconds,
            Some(1_793_521_803)
        );
    }

    #[test]
    fn the_largest_anchor_saturates_instead_of_overflowing() {
        let end = Anchor {
            age_ms: u64::MAX,
            ..Anchor::new(Source::Gnss, u64::MAX, 0)
        };
        assert_eq!(end.at(1).unix_ms, u64::MAX);
        assert_eq!(end.snapshot(1, 0, true).age_ms, Some(u64::MAX));
    }

    #[test]
    fn a_power_loss_leaves_nothing_to_resume() {
        let kept = Retained {
            unix_ms: 1_793_521_800_000,
            age_ms: 0,
            source: Source::Set,
            rtc_ticks: 5_000_000,
        };
        assert_eq!(
            kept.resume(4_999_999, 8 << 19, 0),
            None,
            "the timer restarted"
        );
        let words = kept.encode();
        for index in 0..RETAINED_WORDS {
            let mut torn = words;
            torn[index] ^= 1 << 7;
            assert_eq!(Retained::decode(&torn), None, "word {index}");
        }
        assert_eq!(Retained::decode(&[0; RETAINED_WORDS]), None);
        let mut dropped = words;
        dropped[RETAINED_WORDS - 1] = 0;
        assert_eq!(
            Retained::decode(&dropped),
            None,
            "a dropped copy stays dropped"
        );
        let corrupt = Retained {
            unix_ms: u64::MAX - 1,
            ..kept
        };
        assert_eq!(
            Retained::decode(&corrupt.encode()),
            None,
            "nothing to resume into a boot loop"
        );
    }
}
