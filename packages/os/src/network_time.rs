//! Portable SNTP validation and a UTC anchor advanced from monotonic time.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

const NTP_UNIX_EPOCH: u32 = 2_208_988_800;
pub const STALE_AFTER_MS: u64 = 6 * 60 * 60 * 1_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Timestamp {
    pub unix_seconds: u32,
    pub millis: u16,
    pub stratum: u8,
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

static SEQUENCE: AtomicU32 = AtomicU32::new(0);
static UTC_SECONDS: AtomicU32 = AtomicU32::new(0);
static UTC_MILLIS: AtomicU32 = AtomicU32::new(0);
static MONO_SECONDS: AtomicU32 = AtomicU32::new(0);
static MONO_MILLIS: AtomicU32 = AtomicU32::new(0);
static AVAILABLE: AtomicBool = AtomicBool::new(false);
static SYNCING: AtomicBool = AtomicBool::new(false);

pub fn set_syncing(syncing: bool) {
    SYNCING.store(syncing, Ordering::Relaxed);
}

pub fn update(timestamp: Timestamp, now_ms: u64) {
    SEQUENCE.fetch_add(1, Ordering::AcqRel);
    UTC_SECONDS.store(timestamp.unix_seconds, Ordering::Relaxed);
    UTC_MILLIS.store(u32::from(timestamp.millis), Ordering::Relaxed);
    MONO_SECONDS.store((now_ms / 1_000) as u32, Ordering::Relaxed);
    MONO_MILLIS.store((now_ms % 1_000) as u32, Ordering::Relaxed);
    AVAILABLE.store(true, Ordering::Relaxed);
    SEQUENCE.fetch_add(1, Ordering::Release);
    SYNCING.store(false, Ordering::Relaxed);
}

pub fn snapshot(now_ms: u64, timezone_minutes: i16, online: bool) -> Snapshot {
    let (utc_seconds, utc_millis, mono_seconds, mono_millis, available) = loop {
        let before = SEQUENCE.load(Ordering::Acquire);
        if before & 1 != 0 {
            core::hint::spin_loop();
            continue;
        }
        let values = (
            UTC_SECONDS.load(Ordering::Relaxed),
            UTC_MILLIS.load(Ordering::Relaxed),
            MONO_SECONDS.load(Ordering::Relaxed),
            MONO_MILLIS.load(Ordering::Relaxed),
            AVAILABLE.load(Ordering::Relaxed),
        );
        if before == SEQUENCE.load(Ordering::Acquire) {
            break values;
        }
    };
    if !available {
        return Snapshot {
            unix_seconds: None,
            millis: 0,
            local_minutes: None,
            age_ms: None,
            status: if SYNCING.load(Ordering::Relaxed) {
                Status::Syncing
            } else {
                Status::Unavailable
            },
        };
    }
    let now_seconds = (now_ms / 1_000) as u32;
    let seconds_delta = now_seconds.wrapping_sub(mono_seconds);
    let elapsed_ms = if seconds_delta > i32::MAX as u32 {
        0
    } else {
        let mut elapsed = u64::from(seconds_delta) * 1_000;
        let remainder = (now_ms % 1_000) as i32 - mono_millis as i32;
        if remainder < 0 {
            elapsed = elapsed.saturating_sub((-remainder) as u64);
        } else {
            elapsed += remainder as u64;
        }
        elapsed
    };
    let total_ms = u64::from(utc_seconds) * 1_000 + u64::from(utc_millis) + elapsed_ms;
    let current_seconds = total_ms / 1_000;
    let local_minutes =
        ((current_seconds / 60) as i64 + i64::from(timezone_minutes)).rem_euclid(24 * 60) as u16;
    Snapshot {
        unix_seconds: Some(current_seconds),
        millis: (total_ms % 1_000) as u16,
        local_minutes: Some(local_minutes),
        age_ms: Some(elapsed_ms),
        status: if elapsed_ms >= STALE_AFTER_MS {
            Status::Stale
        } else if !online {
            Status::Offline
        } else {
            Status::Fresh
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
        update(
            Timestamp {
                unix_seconds: 1_700_000_000,
                millis: 250,
                stratum: 2,
            },
            9_900,
        );
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
}
