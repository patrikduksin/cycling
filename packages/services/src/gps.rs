//! Bounded NMEA position state for the C606 receiver.

const LINE: usize = 512;
use device_api::gps::*;

/// A reading less than this far behind the clock is the same clock read late
/// (sentence latency, a 10 ms poll). Further behind may be a correction the
/// receiver made, such as learning the leap seconds.
const RESYNC_MS: u64 = 1_000;
/// Readings, each at least `CONFIRM_GAP_MS` after the last, that must all be
/// `RESYNC_MS` behind before the clock is stepped back. A stalled executor
/// drains old sentences within a few 10 ms polls, which looks like a
/// correction until the next on-time one; a real correction lasts.
const CONFIRM: u8 = 3;
const CONFIRM_GAP_MS: u64 = 500;
/// RMC two-digit years read as 2026..=2079. Earlier years are a GPS week
/// rollover (2006/2007) or a stale receiver RTC. 80 and later is the 1980 GPS
/// epoch default (`060180`) or a textbook sentence (`230394`), not 2080 or 2094.
const PLAUSIBLE_YEARS: core::ops::Range<u32> = 26..80;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Epoch {
    utc: [u8; 6],
    nanos: u32,
}

/// The line buffer and transport counters. Decoding lives in `Decoder`, a
/// separate field, so a complete line is decoded in place instead of copied.
#[derive(Clone)]
pub struct Parser {
    line: [u8; LINE],
    len: usize,
    discarding: bool,
    bytes: u32,
    parse_errors: u32,
    overflows: u32,
    line_overflows: u32,
    decoder: Decoder,
}

#[derive(Clone)]
struct Decoder {
    valid: u32,
    checksum_errors: u32,
    last_sentence: Option<u64>,
    last_fix: Option<u64>,
    current_fix: bool,
    latitude_e7: i32,
    longitude_e7: i32,
    satellites: Option<u8>,
    satellites_at: Option<u64>,
    satellites_epoch: Option<Epoch>,
    /// HDOP in tenths and the epoch it describes (ms of the UTC day).
    hdop: Option<(u16, u32)>,
    /// Ground speed in mm/s and the epoch it describes (ms of the UTC day).
    speed: Option<(u32, u32)>,
    utc: Option<[u8; 6]>,
    current_epoch: Option<Epoch>,
    identity: Identity,
    clock: Option<Clock>,
}

/// Receiver UTC at uptime zero, in ms: the latest `fix utc - uptime its RMC
/// was read at` over dated fixes. Each value is a lower bound, because a
/// sentence is read after its epoch. Uptime drifts from the receiver's clock
/// by about a second after many hours awake, which is one accepted step back.
#[derive(Clone, Copy, Debug)]
struct Clock {
    at_zero_ms: u64,
    /// Readings in a row at least `RESYNC_MS` behind, `CONFIRM_GAP_MS` apart.
    behind: u8,
    behind_at: u64,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            line: [0; LINE],
            len: 0,
            discarding: false,
            bytes: 0,
            parse_errors: 0,
            overflows: 0,
            line_overflows: 0,
            decoder: Decoder {
                valid: 0,
                checksum_errors: 0,
                last_sentence: None,
                last_fix: None,
                current_fix: false,
                latitude_e7: 0,
                longitude_e7: 0,
                satellites: None,
                satellites_at: None,
                satellites_epoch: None,
                hdop: None,
                speed: None,
                utc: None,
                current_epoch: None,
                identity: Identity::Unknown,
                clock: None,
            },
        }
    }
}

impl Parser {
    pub fn overflow(&mut self, count: u32) {
        if count != 0 {
            self.data_loss();
        }
        self.overflows = self.overflows.saturating_add(count);
    }
    /// Explicit stream control invalidates a previously fresh fix immediately.
    /// It also forgets the clock: sleep or a receiver power cycle may have
    /// stopped uptime.
    pub fn control_boundary(&mut self) {
        self.data_loss();
        self.decoder.current_fix = false;
        self.decoder.last_fix = None;
        self.decoder.satellites = None;
        self.decoder.satellites_at = None;
        self.decoder.satellites_epoch = None;
        self.decoder.hdop = None;
        self.decoder.speed = None;
        self.decoder.current_epoch = None;
        self.decoder.clock = None;
    }
    pub fn data_loss(&mut self) {
        self.len = 0;
        self.discarding = true;
    }
    pub fn push(&mut self, byte: u8, now: u64) {
        self.bytes = self.bytes.saturating_add(1);
        if byte == b'$' {
            self.len = 1;
            self.line[0] = byte;
            self.discarding = false;
            return;
        }
        if self.discarding || self.len == 0 {
            return;
        }
        if byte == b'\r' {
            return;
        }
        if byte == b'\n' {
            let len = self.len;
            self.len = 0;
            if !self.decoder.parse_line(&self.line[..len], now) {
                self.parse_errors = self.parse_errors.saturating_add(1);
            }
            return;
        }
        if self.len == LINE {
            self.line_overflows = self.line_overflows.saturating_add(1);
            self.len = 0;
            self.discarding = true;
            return;
        }
        self.line[self.len] = byte;
        self.len += 1;
    }

    pub fn snapshot(&self, now: u64) -> Snapshot {
        let age = self.decoder.last_fix.map(|at| now.saturating_sub(at));
        let state = if self.decoder.last_sentence.is_none() {
            FixState::NoData
        } else if self.decoder.current_fix {
            if age.unwrap_or(u64::MAX) <= STALE_MS {
                FixState::Fresh
            } else {
                FixState::Stale
            }
        } else {
            FixState::NoFix
        };
        let expose = matches!(state, FixState::Fresh | FixState::Stale);
        let epoch_ms = self
            .decoder
            .current_epoch
            .filter(|_| expose)
            .map(Epoch::ms_of_day);
        let of_fix = |epoch: u32| epoch_ms == Some(epoch);
        Snapshot {
            state,
            identity: self.decoder.identity,
            latitude_e7: expose.then_some(self.decoder.latitude_e7),
            longitude_e7: expose.then_some(self.decoder.longitude_e7),
            satellites: self
                .decoder
                .satellites_at
                .filter(|at| now.saturating_sub(*at) <= STALE_MS)
                .filter(|_| !expose || self.decoder.satellites_epoch == self.decoder.current_epoch)
                .and(self.decoder.satellites),
            hdop_tenths: self
                .decoder
                .hdop
                .filter(|(_, epoch)| of_fix(*epoch))
                .map(|(hdop, _)| hdop),
            speed_mm_s: self
                .decoder
                .speed
                .filter(|(_, epoch)| state == FixState::Fresh && of_fix(*epoch))
                .map(|(speed, _)| speed),
            epoch_ms,
            utc: self.decoder.utc,
            utc_ms: self.decoder.clock.map(|clock| clock.at_zero_ms + now),
            age_ms: age,
            bytes: self.bytes,
            valid_sentences: self.decoder.valid,
            checksum_errors: self.decoder.checksum_errors,
            parse_errors: self.parse_errors,
            overflows: self.overflows,
            line_overflows: self.line_overflows,
            uart_errors: 0,
        }
    }
}

impl Decoder {
    fn parse_line(&mut self, line: &[u8], now: u64) -> bool {
        let len = line.len();
        let Some(star) = line.iter().position(|&b| b == b'*') else {
            return false;
        };
        if star + 3 != len {
            return false;
        }
        let Some(wanted) = hex(line[star + 1], line[star + 2]) else {
            return false;
        };
        let actual = line[1..star].iter().fold(0, |sum, &b| sum ^ b);
        if actual != wanted {
            self.checksum_errors = self.checksum_errors.saturating_add(1);
            return true;
        }
        if line[1..star].starts_with(b"PAIR020,") || line[1..star].starts_with(b"PAIR001,020") {
            self.observe_identity(Identity::Pair);
        } else if line[1..star].starts_with(b"PDTINFO,") {
            self.observe_identity(Identity::Pdt);
        }
        let mut fields = line[1..star].split(|&b| b == b',');
        let Some(kind) = fields.next() else {
            return false;
        };
        if kind.len() != 5
            || !matches!(&kind[2..], b"GGA" | b"RMC" | b"VTG")
            || !matches!(&kind[..2], b"GN" | b"GP" | b"GL" | b"GA" | b"GB" | b"BD")
        {
            self.valid = self.valid.saturating_add(1);
            return true;
        }
        let mut values = [&[][..]; 16];
        let mut n = 0;
        for field in fields {
            if n == values.len() {
                return false;
            }
            values[n] = field;
            n += 1;
        }
        let parsed = match &kind[2..] {
            b"GGA" => {
                let parsed = self.gga(&values[..n], now);
                if parsed {
                    self.gga_hdop(&values[..n]);
                }
                parsed
            }
            b"RMC" => {
                let parsed = self.rmc(&values[..n], now);
                if parsed {
                    self.rmc_speed(&values[..n]);
                    self.rmc_clock(&values[..n], now);
                }
                parsed
            }
            _ => self.vtg(&values[..n]),
        };
        if parsed {
            self.valid = self.valid.saturating_add(1);
            self.last_sentence = Some(now);
        }
        parsed
    }

    fn gga(&mut self, f: &[&[u8]], now: u64) -> bool {
        if f.len() < 7 {
            return false;
        }
        let Some(quality) = decimal_u8(f[5]) else {
            return false;
        };
        let epoch = parse_epoch(f[0]);
        self.satellites = decimal_u8(f[6]);
        self.satellites_at = self.satellites.map(|_| now);
        self.satellites_epoch = self.satellites.and(epoch);
        if !matches!(quality, 1 | 2 | 4 | 5) {
            self.current_fix = false;
            return true;
        }
        let Some(epoch) = epoch else {
            return false;
        };
        if self.set_fix(f[1], f[2], f[3], f[4], now) {
            self.utc = Some(epoch.utc);
            self.current_epoch = Some(epoch);
            true
        } else {
            false
        }
    }

    fn rmc(&mut self, f: &[&[u8]], now: u64) -> bool {
        if f.len() < 9 {
            return false;
        }
        let mode = f.get(11).copied().unwrap_or(b"");
        if f[1] != b"A" || (!mode.is_empty() && !matches!(mode, b"A" | b"D" | b"R" | b"F")) {
            self.current_fix = false;
            return true;
        }
        let Some(epoch) = parse_epoch(f[0]) else {
            return false;
        };
        let Ok(date) = <[u8; 6]>::try_from(f[8]) else {
            return false;
        };
        if !date.iter().all(u8::is_ascii_digit) {
            return false;
        }
        if self.set_fix(f[2], f[3], f[4], f[5], now) {
            self.utc = Some(epoch.utc);
            self.current_epoch = Some(epoch);
            true
        } else {
            false
        }
    }

    /// VTG carries no time: its speed belongs to the fix it follows, so a VTG
    /// sent ahead of its epoch's position never reaches a reader.
    fn vtg(&mut self, f: &[&[u8]]) -> bool {
        if f.len() < 8 {
            return false;
        }
        let mode = f.get(8).copied().unwrap_or(b"");
        if mode == b"N" || !self.current_fix {
            return true;
        }
        let speed = match decimal_milli(f[6]) {
            Some(kmh_milli) => kmh_mm_s(kmh_milli),
            None => decimal_milli(f[4]).and_then(knots_mm_s),
        };
        if speed.is_some() {
            self.ground_speed(speed);
        }
        true
    }

    /// The speed of an RMC that just set the fix. Kept apart from `rmc` and
    /// out of line: in `rmc` it overflows the Xtensa register scavenger
    /// ("Cannot scavenge register without an emergency spill slot").
    #[inline(never)]
    fn rmc_speed(&mut self, f: &[&[u8]]) {
        if self.current_fix && parse_epoch(f[0]) == self.current_epoch {
            self.ground_speed(decimal_milli(f[6]).and_then(knots_mm_s));
        }
    }

    /// The receiver clock from an RMC that just set the fix. Date and time come
    /// from this one sentence, so GGA's new-day time never meets yesterday's
    /// date. Out of line for the same reason.
    #[inline(never)]
    fn rmc_clock(&mut self, f: &[&[u8]], now: u64) {
        if !self.current_fix {
            return;
        }
        let Some(epoch) = self.current_epoch else {
            return;
        };
        let Ok(date) = <[u8; 6]>::try_from(f[8]) else {
            return;
        };
        let Some(observed) = dated_ms(date, epoch).and_then(|ms| ms.checked_sub(now)) else {
            return;
        };
        match &mut self.clock {
            Some(clock) => clock.read(observed, now),
            clock => *clock = Some(Clock::new(observed)),
        }
    }

    /// The HDOP of a GGA, for its epoch. Out of line for the same reason.
    #[inline(never)]
    fn gga_hdop(&mut self, f: &[&[u8]]) {
        self.hdop = f
            .get(7)
            .and_then(|hdop| decimal_milli(hdop))
            .zip(parse_epoch(f[0]))
            .map(|(milli, epoch)| {
                let tenths = (milli / 100).min(u32::from(u16::MAX)) as u16;
                (tenths, epoch.ms_of_day())
            });
    }

    /// The ground speed of the current fix's epoch.
    fn ground_speed(&mut self, speed_mm_s: Option<u32>) {
        self.speed = speed_mm_s
            .zip(self.current_epoch)
            .map(|(speed, epoch)| (speed, epoch.ms_of_day()));
    }

    fn set_fix(&mut self, lat: &[u8], ns: &[u8], lon: &[u8], ew: &[u8], now: u64) -> bool {
        let Some(lat) = coordinate(lat, ns, 2, 90, b'N', b'S') else {
            return false;
        };
        let Some(lon) = coordinate(lon, ew, 3, 180, b'E', b'W') else {
            return false;
        };
        self.latitude_e7 = lat;
        self.longitude_e7 = lon;
        self.current_fix = true;
        self.last_fix = Some(now);
        true
    }

    fn observe_identity(&mut self, identity: Identity) {
        self.identity = match (self.identity, identity) {
            (Identity::Unknown, identity) | (identity, Identity::Unknown) => identity,
            (left, right) if left == right => left,
            _ => Identity::Conflicting,
        };
    }
}

impl Epoch {
    fn ms_of_day(self) -> u32 {
        let digit = |i: usize| u32::from(self.utc[i] - b'0');
        let hours = digit(0) * 10 + digit(1);
        let minutes = digit(2) * 10 + digit(3);
        let seconds = digit(4) * 10 + digit(5);
        ((hours * 60 + minutes) * 60 + seconds) * 1000 + self.nanos / 1_000_000
    }
}

impl Clock {
    fn new(observed: u64) -> Self {
        Self {
            at_zero_ms: observed,
            behind: 0,
            behind_at: 0,
        }
    }

    /// One more reading, taken at uptime `now`. A later reading (less latency,
    /// or uptime running slow) moves the clock forward. One less than
    /// `RESYNC_MS` behind is ignored, so jitter never steps it back. One
    /// further behind is a correction only once `CONFIRM` of them arrive
    /// `CONFIRM_GAP_MS` apart in a row, and then the latest is taken.
    fn read(&mut self, observed: u64, now: u64) {
        let behind = self.at_zero_ms.saturating_sub(observed);
        if behind < RESYNC_MS {
            self.at_zero_ms = self.at_zero_ms.max(observed);
            self.behind = 0;
        } else {
            if self.behind == 0 || now >= self.behind_at + CONFIRM_GAP_MS {
                self.behind += 1;
                self.behind_at = now;
            }
            if self.behind >= CONFIRM {
                self.at_zero_ms = observed;
                self.behind = 0;
            }
        }
    }
}

/// Unix ms of an RMC epoch, fraction included, when `ddmmyy` is a real day in
/// `PLAUSIBLE_YEARS`.
fn dated_ms(date: [u8; 6], epoch: Epoch) -> Option<u64> {
    if !PLAUSIBLE_YEARS.contains(&digit_pair(&date[4..6])?) {
        return None;
    }
    let seconds = u64::try_from(unix_seconds(date, epoch.utc)?).ok()?;
    Some(seconds * 1000 + u64::from(epoch.nanos / 1_000_000))
}

fn digit_pair(bytes: &[u8]) -> Option<u32> {
    let [tens, ones] = bytes else { return None };
    (tens.is_ascii_digit() && ones.is_ascii_digit())
        .then(|| u32::from(tens - b'0') * 10 + u32::from(ones - b'0'))
}

/// Days from 1970-01-01 to a proleptic Gregorian date.
pub fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let of_era = year - era * 400;
    let month = i64::from(month);
    let of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let of_era_days = of_era * 365 + of_era / 4 - of_era / 100 + of_year;
    era * 146_097 + of_era_days - 719_468
}

/// Unix seconds from a receiver's ASCII `ddmmyy` date and `hhmmss` time, for a
/// day that exists in 2000..=2099.
pub fn unix_seconds(date: [u8; 6], time: [u8; 6]) -> Option<i64> {
    let (day, month, year) = (
        digit_pair(&date[0..2])?,
        digit_pair(&date[2..4])?,
        digit_pair(&date[4..6])?,
    );
    let (hour, minute, second) = (
        digit_pair(&time[0..2])?,
        digit_pair(&time[2..4])?,
        digit_pair(&time[4..6])?,
    );
    let year = 2000 + i64::from(year);
    if !(1..=12).contains(&month)
        || day == 0
        || day > month_days(year, month)
        || hour > 23
        || minute > 59
    {
        return None;
    }
    let days = days_from_civil(year, month, day);
    Some(days * 86_400 + i64::from(hour * 3600 + minute * 60 + second.min(60)))
}

fn month_days(year: i64, month: u32) -> u32 {
    match month {
        2 if year % 4 == 0 => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Speeds above 999 knots or 999 km/h are not ground speeds.
const MAX_SPEED_MILLI: u32 = 999_000;

/// 1 knot is 1852 m/h: knots/1000 * 1_852_000 mm / 3600 s.
fn knots_mm_s(knots_milli: u32) -> Option<u32> {
    (knots_milli <= MAX_SPEED_MILLI).then(|| knots_milli * 1852 / 3600)
}

fn kmh_mm_s(kmh_milli: u32) -> Option<u32> {
    (kmh_milli <= MAX_SPEED_MILLI).then(|| kmh_milli * 10 / 36)
}

/// A non-negative decimal such as `12.345`, in thousandths. Digits past the
/// third decimal are checked and dropped; at most six whole digits.
#[inline(never)]
fn decimal_milli(bytes: &[u8]) -> Option<u32> {
    let (whole, fraction) = match bytes.iter().position(|&b| b == b'.') {
        Some(dot) => (&bytes[..dot], &bytes[dot + 1..]),
        None => (bytes, &[][..]),
    };
    if whole.is_empty() && fraction.is_empty()
        || whole.len() > 6
        || !whole.iter().chain(fraction).all(u8::is_ascii_digit)
    {
        return None;
    }
    let mut value = whole
        .iter()
        .fold(0u32, |n, &b| n * 10 + u32::from(b - b'0'));
    for i in 0..3 {
        value = value * 10 + fraction.get(i).map_or(0, |&b| u32::from(b - b'0'));
    }
    Some(value)
}

fn hex(a: u8, b: u8) -> Option<u8> {
    Some(nibble(a)? * 16 + nibble(b)?)
}
fn nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'A'..=b'F' => Some(b - b'A' + 10),
        b'a'..=b'f' => Some(b - b'a' + 10),
        _ => None,
    }
}
fn decimal_u8(bytes: &[u8]) -> Option<u8> {
    if bytes.is_empty() {
        return None;
    }
    bytes.iter().try_fold(0u8, |v, &b| {
        v.checked_mul(10)?
            .checked_add(b.checked_sub(b'0').filter(|d| *d < 10)?)
    })
}
fn parse_epoch(v: &[u8]) -> Option<Epoch> {
    if v.len() < 6
        || !v[..6].iter().all(u8::is_ascii_digit)
        || (v.len() > 6
            && (v[6] != b'.'
                || v.len() == 7
                || v[7..].len() > 9
                || !v[7..].iter().all(u8::is_ascii_digit)))
    {
        return None;
    }
    let out: [u8; 6] = v[..6].try_into().ok()?;
    let h = (out[0] - b'0') * 10 + out[1] - b'0';
    let m = (out[2] - b'0') * 10 + out[3] - b'0';
    let s = (out[4] - b'0') * 10 + out[5] - b'0';
    if h >= 24 || m >= 60 || s > 60 {
        return None;
    }
    let mut nanos = 0u32;
    if v.len() > 7 {
        for &digit in &v[7..] {
            nanos = nanos * 10 + u32::from(digit - b'0');
        }
        for _ in v[7..].len()..9 {
            nanos *= 10;
        }
    }
    Some(Epoch { utc: out, nanos })
}
fn coordinate(
    v: &[u8],
    hemi: &[u8],
    degrees: usize,
    bound: u32,
    positive: u8,
    negative: u8,
) -> Option<i32> {
    let dot = v.iter().position(|&b| b == b'.')?;
    if dot != degrees + 2
        || v.len() <= dot + 1
        || v[dot + 1..].len() > 7
        || !v[..dot].iter().all(u8::is_ascii_digit)
        || !v[dot + 1..].iter().all(u8::is_ascii_digit)
    {
        return None;
    }
    let deg = v[..degrees].iter().try_fold(0u32, |n, &b| {
        n.checked_mul(10)?.checked_add(u32::from(b - b'0'))
    })?;
    let min_whole = u32::from(v[degrees] - b'0') * 10 + u32::from(v[degrees + 1] - b'0');
    if min_whole >= 60
        || deg > bound
        || (deg == bound && (min_whole != 0 || v[dot + 1..].iter().any(|&b| b != b'0')))
    {
        return None;
    }
    let mut frac = 0u32;
    for &b in &v[dot + 1..] {
        frac = frac * 10 + u32::from(b - b'0');
    }
    for _ in v[dot + 1..].len()..7 {
        frac *= 10;
    }
    let minutes_e7 = u64::from(min_whole) * 10_000_000 + u64::from(frac);
    let value = i64::from(deg) * 10_000_000 + (minutes_e7 / 60) as i64;
    let sign = match hemi {
        [value] if *value == positive => 1,
        [value] if *value == negative => -1,
        _ => return None,
    };
    i32::try_from(value * sign).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::format;
    fn feed(p: &mut Parser, s: &str, now: u64) {
        for b in s.bytes() {
            p.push(b, now)
        }
    }
    fn body(p: &mut Parser, value: &[u8], now: u64) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        let sum = value.iter().fold(0, |sum, &byte| sum ^ byte);
        p.push(b'$', now);
        for &byte in value {
            p.push(byte, now);
        }
        p.push(b'*', now);
        p.push(HEX[(sum >> 4) as usize], now);
        p.push(HEX[(sum & 15) as usize], now);
        p.push(b'\n', now);
    }
    #[test]
    fn parses_fix_no_fix_stale_and_rejects_checksum() {
        let mut p = Parser::default();
        feed(&mut p, "$GNGGA,075848.086,,,,,0,0,,,M,,M,,*5E\r\n", 1);
        assert_eq!(p.snapshot(1).state, FixState::NoFix);
        feed(
            &mut p,
            "$GPRMC,075848.00,A,3321.8814,S,07030.9348,W,0,0,110926,,,A*5E\r\n",
            10,
        );
        assert_eq!(p.snapshot(10).state, FixState::Fresh);
        assert_eq!(p.snapshot(10).latitude_e7, Some(-333646900));
        assert_eq!(p.snapshot(10).utc_ms, Some(1_789_113_528_000));
        assert_eq!(p.snapshot(3011).state, FixState::Stale);
        feed(&mut p, "$GPRMC,0*00\n", 20);
        assert_eq!(p.decoder.checksum_errors, 1)
    }

    #[test]
    fn axis_hemispheres_and_atomic_fix_fields_are_strict() {
        assert!(coordinate(b"3321.8814", b"E", 2, 90, b'N', b'S').is_none());
        assert!(coordinate(b"07030.9348", b"N", 3, 180, b'E', b'W').is_none());
        let mut parser = Parser::default();
        body(
            &mut parser,
            b"GPRMC,010203.00,A,3321.8814,S,07030.9348,W,0,0,110926,,,A",
            1,
        );
        let first = parser.snapshot(1);
        body(
            &mut parser,
            b"GPRMC,111213.00,A,9961.0,N,07030.9348,W,0,0,110926,,,A",
            2,
        );
        assert_eq!(parser.snapshot(2).utc, first.utc);
        body(&mut parser, b"GNGGA,,,,,,0,0,,,M,,M,,", 3);
        assert_eq!(parser.snapshot(3).state, FixState::NoFix);
    }

    #[test]
    fn partial_input_and_loss_resynchronize_at_fresh_sentence() {
        let valid = b"GNGGA,075848.086,,,,,0,0,,,M,,M,,";
        for split in 0..valid.len() {
            let mut parser = Parser::default();
            const HEX: &[u8; 16] = b"0123456789ABCDEF";
            let sum = valid.iter().fold(0, |sum, &byte| sum ^ byte);
            parser.push(b'$', 1);
            for &byte in &valid[..split] {
                parser.push(byte, 1);
            }
            for &byte in &valid[split..] {
                parser.push(byte, 2);
            }
            parser.push(b'*', 2);
            parser.push(HEX[(sum >> 4) as usize], 2);
            parser.push(HEX[(sum & 15) as usize], 2);
            parser.push(b'\n', 2);
            assert_eq!(parser.snapshot(2).state, FixState::NoFix);
        }
        let mut parser = Parser::default();
        for &byte in b"$GNGGA,broken" {
            parser.push(byte, 1);
        }
        parser.data_loss();
        body(&mut parser, valid, 2);
        assert_eq!(parser.snapshot(2).state, FixState::NoFix);
    }

    #[test]
    fn exposes_satellites_only_for_the_coordinate_epoch() {
        let mut parser = Parser::default();
        body(&mut parser, b"GNGGA,010201.00,,,,,0,0,,,M,,M,,", 0);
        assert_eq!(parser.snapshot(0).state, FixState::NoFix);
        assert_eq!(parser.snapshot(0).satellites, Some(0));
        assert_eq!(parser.snapshot(STALE_MS + 1).satellites, None);
        body(
            &mut parser,
            b"GNGGA,010202.00,3321.8814,S,07030.9348,W,1,09,,,M,,M,,",
            1,
        );
        assert_eq!(parser.snapshot(1).satellites, Some(9));
        body(
            &mut parser,
            b"GPRMC,010203.00,A,3321.8814,S,07030.9348,W,0,0,110926,,,A",
            2,
        );
        assert_eq!(parser.snapshot(2).satellites, None);
        body(
            &mut parser,
            b"GNGGA,010203.50,3321.8814,S,07030.9348,W,1,09,,,M,,M,,",
            3,
        );
        assert_eq!(parser.snapshot(3).satellites, Some(9));
        body(
            &mut parser,
            b"GPRMC,010203.00,A,3321.8814,S,07030.9348,W,0,0,110926,,,A",
            4,
        );
        assert_eq!(parser.snapshot(4).satellites, None);
        body(
            &mut parser,
            b"GNGGA,010203.000,3321.8814,S,07030.9348,W,1,09,,,M,,M,,",
            5,
        );
        assert_eq!(parser.snapshot(5).satellites, Some(9));
        body(
            &mut parser,
            b"GPRMC,010204.00,A,3321.8814,S,07030.9348,W,0,0,110926,,,A",
            6,
        );
        assert_eq!(parser.snapshot(6).satellites, None);
    }

    #[test]
    fn ground_speed_and_hdop_belong_to_the_fix_epoch() {
        let mut parser = Parser::default();
        body(
            &mut parser,
            b"GNGGA,010203.00,3321.8814,S,07030.9348,W,1,09,1.25,545.4,M,,M,,",
            0,
        );
        let gga = parser.snapshot(0);
        assert_eq!(gga.hdop_tenths, Some(12));
        assert_eq!(gga.speed_mm_s, None);
        assert_eq!(gga.epoch_ms, Some(3_723_000));
        // 10 knots over ground in the same epoch.
        body(
            &mut parser,
            b"GNRMC,010203.00,A,3321.8814,S,07030.9348,W,10.00,90.0,110926,,,A",
            5,
        );
        let rmc = parser.snapshot(5);
        assert_eq!((rmc.speed_mm_s, rmc.hdop_tenths), (Some(5144), Some(12)));
        // The next epoch's position arrives before its speed: no old speed.
        body(
            &mut parser,
            b"GNGGA,010203.50,3321.8814,S,07030.9348,W,1,09,0.9,545.4,M,,M,,",
            500,
        );
        let next = parser.snapshot(500);
        assert_eq!((next.speed_mm_s, next.epoch_ms), (None, Some(3_723_500)));
        // VTG after the position carries that epoch's speed, km/h preferred.
        body(&mut parser, b"GNVTG,90.0,T,,M,10.00,N,18.000,K,A", 501);
        assert_eq!(parser.snapshot(501).speed_mm_s, Some(5000));
        body(&mut parser, b"GNVTG,90.0,T,,M,1.0,N,,K,A", 502);
        assert_eq!(parser.snapshot(502).speed_mm_s, Some(514));
        // An invalid VTG says nothing; a stale fix exposes no speed.
        body(&mut parser, b"GNVTG,,T,,M,,N,99.0,K,N", 503);
        assert_eq!(parser.snapshot(503).speed_mm_s, Some(514));
        assert_eq!(parser.snapshot(503 + STALE_MS).speed_mm_s, None);
        assert_eq!(parser.snapshot(503 + STALE_MS).epoch_ms, Some(3_723_500));
        // RMC without a speed field leaves its epoch without one.
        body(
            &mut parser,
            b"GNRMC,010204.00,A,3321.8814,S,07030.9348,W,,,110926,,,A",
            1000,
        );
        assert_eq!(parser.snapshot(1000).speed_mm_s, None);
        assert_eq!(parser.snapshot(1000).hdop_tenths, None);
        // No fix: nothing is exposed.
        body(&mut parser, b"GNGGA,010205.00,,,,,0,00,99.9,,M,,M,,", 2000);
        let lost = parser.snapshot(2000);
        assert_eq!(
            (lost.speed_mm_s, lost.hdop_tenths, lost.epoch_ms),
            (None, None, None)
        );
    }

    const FIX: &str = "3321.8814,S,07030.9348,W";
    fn rmc(p: &mut Parser, time: &str, date: &str, now: u64) {
        let line = format!("GNRMC,{time},A,{FIX},0,0,{date},,,A");
        body(p, line.as_bytes(), now);
    }
    fn gga(p: &mut Parser, time: &str, now: u64) {
        let line = format!("GNGGA,{time},{FIX},1,09,1.0,545.4,M,,M,,");
        body(p, line.as_bytes(), now);
    }
    /// Unix ms of 14 October 2026 at `hhmmss`.
    fn oct14(time: &[u8; 6]) -> u64 {
        unix_seconds(*b"141026", *time).unwrap() as u64 * 1000
    }

    #[test]
    fn rmc_dates_the_clock_and_gga_never_does() {
        let mut p = Parser::default();
        gga(&mut p, "100000.00", 4_000);
        assert_eq!(p.snapshot(4_000).utc_ms, None, "GGA has no date");
        body(&mut p, b"GNRMC,100000.00,V,,,,,,,141026,,,N", 4_500);
        assert_eq!(p.snapshot(4_500).utc_ms, None, "no fix, no clock");
        rmc(&mut p, "100000.00", "141026", 5_000);
        assert_eq!(p.snapshot(6_000).utc_ms, Some(oct14(b"100001")));
    }

    #[test]
    fn midnight_gga_before_rmc_keeps_the_day() {
        let mut p = Parser::default();
        rmc(&mut p, "235959.50", "141026", 1_000);
        gga(&mut p, "000000.50", 2_000);
        let midnight = unix_seconds(*b"151026", *b"000000").unwrap() as u64 * 1000;
        assert_eq!(p.snapshot(2_000).utc_ms, Some(midnight + 500));
        rmc(&mut p, "000000.50", "151026", 2_000);
        assert_eq!(p.snapshot(2_000).utc_ms, Some(midnight + 500));
    }

    #[test]
    fn latency_jitter_never_steps_back() {
        let mut p = Parser::default();
        let mut last = 0;
        for (second, late) in [(0u64, 300), (1, 120), (2, 450), (3, 120)] {
            let read_at = second * 1000 + late;
            let time = format!("1000{second:02}.00");
            rmc(&mut p, &time, "141026", read_at);
            let utc = p.snapshot(read_at).utc_ms.unwrap();
            assert!(utc >= last, "step back at second {second}: {utc} < {last}");
            last = utc;
        }
        assert_eq!(last, oct14(b"100000") - 120 + 3_120);
    }

    #[test]
    fn a_second_or_more_behind_is_a_correction_once_confirmed() {
        let after = |times: [&str; 3]| {
            let mut p = Parser::default();
            rmc(&mut p, "100020.00", "141026", 20_000);
            for (time, read_at) in times.iter().zip([40_000, 41_000, 42_000]) {
                rmc(&mut p, time, "141026", read_at);
            }
            p.snapshot(42_000).utc_ms.unwrap() - 42_000
        };
        let uptime_zero = oct14(b"100000");
        assert_eq!(
            after(["100040.50", "100041.50", "100042.50"]),
            uptime_zero + 500,
            "ahead is taken at once"
        );
        assert_eq!(
            after(["100039.50", "100040.50", "100041.50"]),
            uptime_zero,
            "under a second behind is ignored"
        );
        assert_eq!(
            after(["100000.00", "100001.00", "100002.00"]),
            uptime_zero - 40_000,
            "20 s behind, three times, is taken"
        );
        assert_eq!(
            after(["100039.00", "100040.00", "100041.00"]),
            uptime_zero - 1_000,
            "1 s behind, three times, is taken"
        );
        assert_eq!(
            after(["100000.00", "100001.00", "100042.00"]),
            uptime_zero,
            "an on-time reading resets the count"
        );
        assert_eq!(
            after(["100000.00", "100041.50", "100002.00"]),
            uptime_zero + 500,
            "so does one ahead"
        );
    }

    #[test]
    fn a_stall_drained_late_never_steps_the_clock_back() {
        let mut p = Parser::default();
        let mut last = 0;
        let mut check = |p: &Parser, now: u64| {
            let utc = p.snapshot(now).utc_ms.unwrap();
            assert!(utc >= last, "step back at {now}: {utc} < {last}");
            last = utc;
        };
        for second in 0..5u64 {
            let now = second * 1000 + 100;
            rmc(&mut p, &format!("1000{second:02}.00"), "141026", now);
            check(&p, now);
        }
        // The executor stalls for 3 s and then drains the buffered RMCs over
        // consecutive 10 ms polls, each one late by its age.
        check(&p, 8_100);
        for second in 5..9u64 {
            let now = 8_100 + (second - 5) * 10;
            rmc(&mut p, &format!("1000{second:02}.00"), "141026", now);
            check(&p, now);
        }
        for second in 9..12u64 {
            let now = second * 1000 + 100;
            rmc(&mut p, &format!("1000{second:02}.00"), "141026", now);
            check(&p, now);
        }
        assert_eq!(last, oct14(b"100011"), "ends on time");
    }

    #[test]
    fn a_repeated_reading_changes_nothing() {
        let mut p = Parser::default();
        rmc(&mut p, "100000.00", "141026", 5_000);
        let once = p.snapshot(5_000).utc_ms;
        rmc(&mut p, "100000.00", "141026", 5_000);
        assert_eq!(p.snapshot(5_000).utc_ms, once);
    }

    #[test]
    fn implausible_dates_never_set_the_clock() {
        for date in [
            "230394", "060180", "140406", "310226", "290227", "000126", "151326",
        ] {
            let mut p = Parser::default();
            rmc(&mut p, "100000.00", date, 5_000);
            assert_eq!(p.snapshot(5_000).state, FixState::Fresh, "{date}");
            assert_eq!(p.snapshot(5_000).utc_ms, None, "{date}");
        }
        let mut p = Parser::default();
        rmc(&mut p, "100000.00", "290228", 5_000);
        assert!(p.snapshot(5_000).utc_ms.is_some(), "29 Feb 2028 exists");
    }

    #[test]
    fn the_clock_outlives_the_fix_but_not_a_boundary() {
        let mut p = Parser::default();
        rmc(&mut p, "100000.00", "141026", 1_000);
        let at = |p: &Parser, now| p.snapshot(now).utc_ms;
        body(&mut p, b"GNRMC,100001.00,V,,,,,,,141026,,,N", 2_000);
        assert_eq!(p.snapshot(2_000).state, FixState::NoFix);
        assert_eq!(at(&p, 10_000), Some(oct14(b"100009")));
        body(&mut p, b"GNGGA,100002.00,,,,,0,00,,,M,,M,,", 3_000);
        assert_eq!(at(&p, 10_000), Some(oct14(b"100009")));
        p.overflow(1);
        p.data_loss();
        assert_eq!(at(&p, 10_000), Some(oct14(b"100009")));
        p.control_boundary();
        assert_eq!(at(&p, 10_000), None);
        rmc(&mut p, "100020.00", "141026", 11_000);
        assert_eq!(at(&p, 11_000), Some(oct14(b"100020")));
    }

    #[test]
    fn calendar_days_are_real() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(unix_seconds(*b"141026", *b"231000"), Some(1_792_019_400));
        assert_eq!(unix_seconds(*b"141326", *b"231000"), None);
        assert_eq!(unix_seconds(*b"14a026", *b"231000"), None);
        assert_eq!(unix_seconds(*b"000126", *b"231000"), None);
        assert_eq!(unix_seconds(*b"310426", *b"231000"), None);
        assert!(unix_seconds(*b"290228", *b"231000").is_some());
        assert_eq!(unix_seconds(*b"290227", *b"231000"), None);
        assert!(
            unix_seconds(*b"290200", *b"231000").is_some(),
            "2000 is a leap year"
        );
    }

    #[test]
    fn decimals_are_strict_and_bounded() {
        assert_eq!(decimal_milli(b"12.3456"), Some(12_345));
        assert_eq!(decimal_milli(b"7"), Some(7_000));
        assert_eq!(decimal_milli(b".5"), Some(500));
        assert_eq!(decimal_milli(b"999999.9"), Some(999_999_900));
        for bad in [&b""[..], b".", b"1.2.3", b"-1", b"1e3", b"1234567"] {
            assert_eq!(decimal_milli(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn recognizes_only_checksum_valid_identity_protocol_responses() {
        let mut parser = Parser::default();
        body(&mut parser, b"PAIR001,020,0", 1);
        assert_eq!(parser.snapshot(1).identity, Identity::Pair);
        feed(&mut parser, "$PDTINFO,spoofed*00\n", 2);
        assert_eq!(parser.snapshot(2).identity, Identity::Pair);
        body(&mut parser, b"PDTINFO,model", 3);
        assert_eq!(parser.snapshot(3).identity, Identity::Conflicting);
    }

    const NMEA: &[u8] = b"$$,,,**\r\n\n0123456789.ABCDEFGNPRSWLMTV-";

    /// Bytes a noisy UART delivers: mostly NMEA's alphabet, sometimes any byte.
    fn noise(rng: &mut crate::testing::Rng, length: usize) -> std::vec::Vec<u8> {
        (0..length)
            .map(|_| match rng.below(4) {
                0 => rng.next() as u8,
                _ => NMEA[rng.below(NMEA.len() as u64) as usize],
            })
            .collect()
    }

    /// A checksum-valid sentence of plausible fields in any order and count.
    fn sentence(rng: &mut crate::testing::Rng) -> std::vec::Vec<u8> {
        const KINDS: &[&[u8]] = &[b"GNGGA", b"GPRMC", b"GNVTG", b"GLGSV", b"PAIR020", b"GPGGA"];
        const FIELDS: &[&[u8]] = &[
            b"",
            b"0",
            b"1",
            b"A",
            b"V",
            b"N",
            b"S",
            b"E",
            b"W",
            b"K",
            b"M",
            b"T",
            b"D",
            b"075848.086",
            b"235959.99",
            b"3321.8814",
            b"9000.0000",
            b"07030.9348",
            b"18000.0",
            b"110926",
            b"999999",
            b"12.5",
            b"99.9",
            b"-1",
            b"4294967296",
        ];
        let mut value = KINDS[rng.below(KINDS.len() as u64) as usize].to_vec();
        for _ in 0..rng.below(20) {
            value.push(b',');
            value.extend_from_slice(FIELDS[rng.below(FIELDS.len() as u64) as usize]);
        }
        value
    }

    /// Lines the parser completes, as its framing rules define them.
    fn completed_lines(bytes: &[u8]) -> (u32, u32) {
        let (mut lines, mut overlong, mut length) = (0, 0, None);
        for &byte in bytes {
            length = match (byte, length) {
                (b'$', _) => Some(1),
                (_, None) | (b'\r', _) => length,
                (b'\n', Some(_)) => {
                    lines += 1;
                    None
                }
                (_, Some(LINE)) => {
                    overlong += 1;
                    None
                }
                (_, Some(n)) => Some(n + 1),
            };
        }
        (lines, overlong)
    }

    #[test]
    fn arbitrary_input_never_panics_and_every_line_is_counted_once() {
        for seed in 0..1_000 {
            let mut rng = crate::testing::Rng(seed * 2 + 1);
            let mut parser = Parser::default();
            let mut fed = std::vec::Vec::new();
            for _ in 0..rng.below(30) {
                let mut chunk = if rng.below(2) == 0 {
                    let length = rng.below(700) as usize;
                    noise(&mut rng, length)
                } else {
                    let value = sentence(&mut rng);
                    let sum = value.iter().fold(0, |sum, &byte| sum ^ byte);
                    let mut line = std::vec![b'$'];
                    line.extend_from_slice(&value);
                    line.extend_from_slice(std::format!("*{sum:02X}\r\n").as_bytes());
                    line
                };
                if rng.below(4) == 0 && !chunk.is_empty() {
                    let at = rng.below(chunk.len() as u64) as usize;
                    chunk[at] = rng.next() as u8;
                }
                for &byte in &chunk {
                    parser.push(byte, fed.len() as u64);
                    fed.push(byte);
                }
                parser.snapshot(fed.len() as u64 + rng.below(10_000));
            }
            let snapshot = parser.snapshot(fed.len() as u64);
            let (lines, overlong) = completed_lines(&fed);
            assert_eq!(snapshot.bytes as usize, fed.len(), "seed {seed}");
            assert_eq!(
                snapshot.valid_sentences + snapshot.checksum_errors + snapshot.parse_errors,
                lines,
                "seed {seed}"
            );
            assert_eq!(snapshot.line_overflows, overlong, "seed {seed}");
        }
    }

    #[test]
    fn valid_fixes_among_garbage_still_parse() {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for seed in 0..1_000 {
            let mut rng = crate::testing::Rng(seed * 2 + 1);
            let mut parser = Parser::default();
            let length = rng.below(2_000) as usize;
            for &byte in &noise(&mut rng, length) {
                parser.push(byte, 1);
            }
            let degrees = rng.below(90);
            let minutes = rng.below(600_000);
            let value = std::format!(
                "GPRMC,010203.00,A,{degrees:02}{:02}.{:04},S,07030.9348,W,0,0,110926,,,A",
                minutes / 10_000,
                minutes % 10_000
            );
            let sum = value.bytes().fold(0, |sum, byte| sum ^ byte);
            let before = parser.snapshot(2).valid_sentences;
            parser.push(b'$', 2);
            for byte in value.bytes() {
                parser.push(byte, 2);
            }
            for byte in [
                b'*',
                HEX[usize::from(sum >> 4)],
                HEX[usize::from(sum & 15)],
                b'\n',
            ] {
                parser.push(byte, 2);
            }
            let fix = parser.snapshot(2);
            let expected = degrees * 10_000_000 + minutes * 1_000 / 60;
            assert_eq!(fix.state, FixState::Fresh, "seed {seed}");
            assert_eq!(fix.latitude_e7, Some(-(expected as i32)), "seed {seed}");
            assert_eq!(fix.valid_sentences, before + 1, "seed {seed}");
        }
    }

    use std::vec::Vec;
    /// A checksummed sentence with its `$`, `*hh` and the given ending.
    fn checksummed(value: &[u8], ending: &[u8]) -> Vec<u8> {
        let sum = value.iter().fold(0, |sum, &byte| sum ^ byte);
        let mut line =
            std::format!("${}*{sum:02X}", std::str::from_utf8(value).unwrap()).into_bytes();
        line.extend_from_slice(ending);
        line
    }
    const GGA: &[u8] = b"GNGGA,075848.000,,,,,0,00,25.5,,M,,M,,";
    const GSV: &[u8] = b"GPGSV,3,1,12,01,40,083,,02,17,308,,03,07,344,,04,22,228,";
    fn stream(p: &mut Parser, bytes: &[u8]) {
        for &byte in bytes {
            p.push(byte, 1);
        }
    }

    #[test]
    fn a_dollar_inside_a_long_binary_frame_is_not_a_line_overflow() {
        // Issue #134's suspected shape: a `$` early in a frame, then hundreds of
        // bytes with no LF.
        let mut p = Parser::default();
        let mut input = checksummed(GGA, b"\r\n");
        input.extend_from_slice(&[0xBA, 0xCE, 0x00, 0x04, b'$']);
        input.extend((0..1024).map(|i| if i % 5 == 4 { 0x01 } else { b'A' }));
        input.extend(checksummed(GGA, b"\r\n"));
        stream(&mut p, &input);
        let s = p.snapshot(1);
        assert_eq!((s.valid_sentences, s.line_overflows), (2, 0));
    }

    #[test]
    fn carriage_return_alone_ends_a_sentence() {
        for ending in [&b"\r"[..], b"\n", b"\r\n", b"\n\r", b"\r\r\n"] {
            let mut p = Parser::default();
            let mut input = Vec::new();
            for _ in 0..4 {
                input.extend(checksummed(GGA, ending));
                input.extend(checksummed(GSV, ending));
            }
            stream(&mut p, &input);
            let s = p.snapshot(1);
            assert_eq!((s.valid_sentences, s.parse_errors), (8, 0), "{ending:?}");
        }
    }
}
