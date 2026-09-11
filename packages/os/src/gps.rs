//! Bounded NMEA position state for the C606 receiver.

const LINE: usize = 512;
pub const STALE_MS: u64 = 3_000;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Epoch {
    utc: [u8; 6],
    nanos: u32,
}

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
    pub utc: Option<[u8; 6]>,
    pub age_ms: Option<u64>,
    pub bytes: u32,
    pub valid_sentences: u32,
    pub checksum_errors: u32,
    pub parse_errors: u32,
    pub overflows: u32,
    pub line_overflows: u32,
    pub uart_errors: u32,
}

impl Default for Snapshot {
    fn default() -> Self {
        Self {
            state: FixState::NoData,
            identity: Identity::Unknown,
            latitude_e7: None,
            longitude_e7: None,
            satellites: None,
            utc: None,
            age_ms: None,
            bytes: 0,
            valid_sentences: 0,
            checksum_errors: 0,
            parse_errors: 0,
            overflows: 0,
            line_overflows: 0,
            uart_errors: 0,
        }
    }
}

pub struct Parser {
    line: [u8; LINE],
    len: usize,
    discarding: bool,
    bytes: u32,
    valid: u32,
    checksum_errors: u32,
    parse_errors: u32,
    overflows: u32,
    line_overflows: u32,
    last_sentence: Option<u64>,
    last_fix: Option<u64>,
    current_fix: bool,
    latitude_e7: i32,
    longitude_e7: i32,
    satellites: Option<u8>,
    satellites_at: Option<u64>,
    satellites_epoch: Option<Epoch>,
    utc: Option<[u8; 6]>,
    current_epoch: Option<Epoch>,
    identity: Identity,
}

impl Default for Parser {
    fn default() -> Self {
        Self {
            line: [0; LINE],
            len: 0,
            discarding: false,
            bytes: 0,
            valid: 0,
            checksum_errors: 0,
            parse_errors: 0,
            overflows: 0,
            line_overflows: 0,
            last_sentence: None,
            last_fix: None,
            current_fix: false,
            latitude_e7: 0,
            longitude_e7: 0,
            satellites: None,
            satellites_at: None,
            satellites_epoch: None,
            utc: None,
            current_epoch: None,
            identity: Identity::Unknown,
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
            if !self.parse_line(len, now) {
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

    fn parse_line(&mut self, len: usize, now: u64) -> bool {
        let line = self.line;
        let Some(star) = line[..len].iter().position(|&b| b == b'*') else {
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
            || !matches!(&kind[2..], b"GGA" | b"RMC")
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
        let parsed = if &kind[2..] == b"GGA" {
            self.gga(&values[..n], now)
        } else {
            self.rmc(&values[..n], now)
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
        if f[8].len() != 6 || !f[8].iter().all(u8::is_ascii_digit) {
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

    pub fn snapshot(&self, now: u64) -> Snapshot {
        let age = self.last_fix.map(|at| now.saturating_sub(at));
        let state = if self.last_sentence.is_none() {
            FixState::NoData
        } else if self.current_fix {
            if age.unwrap_or(u64::MAX) <= STALE_MS {
                FixState::Fresh
            } else {
                FixState::Stale
            }
        } else {
            FixState::NoFix
        };
        let expose = matches!(state, FixState::Fresh | FixState::Stale);
        Snapshot {
            state,
            identity: self.identity,
            latitude_e7: expose.then_some(self.latitude_e7),
            longitude_e7: expose.then_some(self.longitude_e7),
            satellites: self
                .satellites_at
                .filter(|at| now.saturating_sub(*at) <= STALE_MS)
                .filter(|_| expose && self.satellites_epoch == self.current_epoch)
                .and(self.satellites),
            utc: self.utc,
            age_ms: age,
            bytes: self.bytes,
            valid_sentences: self.valid,
            checksum_errors: self.checksum_errors,
            parse_errors: self.parse_errors,
            overflows: self.overflows,
            line_overflows: self.line_overflows,
            uart_errors: 0,
        }
    }

    fn observe_identity(&mut self, identity: Identity) {
        self.identity = match (self.identity, identity) {
            (Identity::Unknown, identity) | (identity, Identity::Unknown) => identity,
            (left, right) if left == right => left,
            _ => Identity::Conflicting,
        };
    }
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
        assert_eq!(p.snapshot(3011).state, FixState::Stale);
        feed(&mut p, "$GPRMC,0*00\n", 20);
        assert_eq!(p.checksum_errors, 1)
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
    fn recognizes_only_checksum_valid_identity_protocol_responses() {
        let mut parser = Parser::default();
        body(&mut parser, b"PAIR001,020,0", 1);
        assert_eq!(parser.snapshot(1).identity, Identity::Pair);
        feed(&mut parser, "$PDTINFO,spoofed*00\n", 2);
        assert_eq!(parser.snapshot(2).identity, Identity::Pair);
        body(&mut parser, b"PDTINFO,model", 3);
        assert_eq!(parser.snapshot(3).identity, Identity::Conflicting);
    }
}
