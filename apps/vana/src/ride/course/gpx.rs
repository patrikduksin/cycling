//! A streaming GPX reader for courses: bytes in, in pieces of any size, and
//! the points of the file's first track of two points or more out, into a
//! `Course`, or of its first such route when it has no such track.
//!
//! It is not an XML parser. It knows tags, attributes, comments, CDATA and
//! declarations well enough to find `trk` and `rte` elements, their `trkpt`
//! or `rtept` points with `lat` and `lon` attributes, and the first `name`
//! outside a point. Every other element is skipped. A track's segments join
//! into one course. A track replaces a route already read once it has two
//! points. Its state is a few dozen bytes whatever the input: tag and
//! attribute names longer than any it looks for are kept only as "long", and
//! coordinates are read digit by digit, so hostile input costs time, never
//! memory.
//!
//! Coordinates are decimal degrees, `-33.4512345`, read exactly to 1e-7
//! degree with integers (later digits are dropped). A point whose latitude
//! or longitude is missing, not a decimal or out of range refuses the whole
//! file: it is damaged, and following part of it could lead anywhere.
use super::{Builder, Course, Follow, Point};

/// Why a file gives no course.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    /// A point of the track or route has no real latitude and longitude.
    Coordinates,
    /// Fewer than two points in a track or route.
    Empty,
}

/// The longest tag or attribute name kept; `trkpt` is 5 bytes, `![CDATA[`
/// 8, and a namespace prefix may come before them.
const TAG: usize = 16;
const ATTRIBUTE: usize = 8;
/// The longest entity name read past, as in `&thetasym;`.
const ENTITY: u8 = 8;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Text,
    /// After `<`.
    Open,
    TagName,
    /// Inside a tag, between attributes.
    Tag,
    AttributeName,
    /// After an attribute's name, before `=`.
    Equals,
    /// After `=`, before the quote.
    Quote,
    /// Inside a value that ends at this quote byte.
    Value(u8),
    /// After `/` inside a tag.
    Slash,
    /// Inside `<!--`: how much of `-->` has matched.
    Comment(u8),
    /// Inside `<![CDATA[`: how much of `]]>` has matched.
    Cdata(u8),
    /// Inside any other `<!` or `<?`, which ends at `>`.
    Declaration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Route {
    /// No track or route of two points or more read yet.
    Looking,
    /// Inside a `trk` (true) or `rte` (false) that may become the course,
    /// with this many points so far; a route of two points or more was
    /// read before it, if `route_kept`.
    Inside {
        track: bool,
        points: u32,
        route_kept: bool,
    },
    /// The course is this track or route; only a track replaces a route.
    Kept { track: bool },
}

/// A short name, or "long" once it outgrew the buffer.
#[derive(Clone, Copy, Debug)]
struct Word<const N: usize> {
    bytes: [u8; N],
    len: u8,
    long: bool,
}

impl<const N: usize> Word<N> {
    const fn new() -> Self {
        Self {
            bytes: [0; N],
            len: 0,
            long: false,
        }
    }

    fn push(&mut self, byte: u8) {
        if usize::from(self.len) < N {
            self.bytes[usize::from(self.len)] = byte;
            self.len += 1;
        } else {
            self.long = true;
        }
    }

    fn get(&self) -> &[u8] {
        if self.long {
            b""
        } else {
            &self.bytes[..usize::from(self.len)]
        }
    }

    /// The name without a namespace prefix.
    fn local(&self) -> &[u8] {
        let name = self.get();
        match name.iter().rposition(|&b| b == b':') {
            Some(at) => &name[at + 1..],
            None => name,
        }
    }
}

/// Decimal degrees, read a byte at a time into 1e-7 degrees.
#[derive(Clone, Copy, Debug)]
struct Decimal {
    negative: bool,
    /// A sign or digit was read.
    started: bool,
    /// Space after the number: anything more but space is invalid.
    ended: bool,
    point: bool,
    digits: u8,
    /// Whole degrees, without leading zeros.
    whole: u32,
    whole_digits: u8,
    fraction: u32,
    fraction_digits: u8,
    invalid: bool,
}

impl Decimal {
    const fn new() -> Self {
        Self {
            negative: false,
            started: false,
            ended: false,
            point: false,
            digits: 0,
            whole: 0,
            whole_digits: 0,
            fraction: 0,
            fraction_digits: 0,
            invalid: false,
        }
    }

    fn push(&mut self, byte: u8) {
        if self.invalid {
            return;
        }
        match byte {
            b' ' | b'\t' | b'\r' | b'\n' => self.ended |= self.started,
            _ if self.ended => self.invalid = true,
            b'-' | b'+' if !self.started => {
                self.negative = byte == b'-';
                self.started = true;
            }
            b'.' if !self.point => {
                self.point = true;
                self.started = true;
            }
            b'0'..=b'9' => {
                self.started = true;
                self.digits = self.digits.saturating_add(1);
                let digit = u32::from(byte - b'0');
                if self.point {
                    if self.fraction_digits < 7 {
                        self.fraction = self.fraction * 10 + digit;
                        self.fraction_digits += 1;
                    }
                } else if self.whole_digits > 0 || digit > 0 {
                    // Degrees never have four digits.
                    if self.whole_digits == 3 {
                        self.invalid = true;
                    } else {
                        self.whole = self.whole * 10 + digit;
                        self.whole_digits += 1;
                    }
                }
            }
            _ => self.invalid = true,
        }
    }

    /// The value in 1e-7 degrees, if it was a decimal.
    fn e7(&self) -> Option<i32> {
        if self.invalid || self.digits == 0 {
            return None;
        }
        let mut fraction = i64::from(self.fraction);
        for _ in self.fraction_digits..7 {
            fraction *= 10;
        }
        let value = i64::from(self.whole) * 10_000_000 + fraction;
        i32::try_from(if self.negative { -value } else { value }).ok()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Gpx {
    state: State,
    tag: Word<TAG>,
    closing: bool,
    attribute: Word<ATTRIBUTE>,
    value: Decimal,
    lat: Option<Decimal>,
    lon: Option<Decimal>,
    route: Route,
    /// Inside a `trkpt`, `rtept` or `wpt`, whose names are not the course's.
    in_point: bool,
    /// Inside the `name` element that names the course.
    naming: bool,
    named: bool,
    /// Bytes read inside `&...;` in a name. A blank, or an entity longer
    /// than `ENTITY`, ends it without its `;`.
    entity: Option<u8>,
    name: Word<{ super::NAME }>,
    refused: Option<Refusal>,
    builder: Builder,
    /// A track's first point, while a route read before it is kept.
    first: Option<Point>,
}

impl Default for Gpx {
    fn default() -> Self {
        Self::new()
    }
}

impl Gpx {
    pub const fn new() -> Self {
        Self {
            state: State::Text,
            tag: Word::new(),
            closing: false,
            attribute: Word::new(),
            value: Decimal::new(),
            lat: None,
            lon: None,
            route: Route::Looking,
            in_point: false,
            naming: false,
            named: false,
            entity: None,
            name: Word::new(),
            refused: None,
            builder: Builder::new(),
            first: None,
        }
    }

    /// Start reading a file into `course`, which is emptied; `follow`
    /// follows it as it is built.
    pub fn start(&mut self, course: &mut Course, follow: &mut impl Follow) {
        *self = Self::new();
        self.builder.start(course, follow);
    }

    /// The file refused already, before its end.
    pub fn refused(&self) -> Option<Refusal> {
        self.refused
    }

    pub fn feed(&mut self, bytes: &[u8], course: &mut Course, follow: &mut impl Follow) {
        for &byte in bytes {
            if self.refused.is_some() {
                return;
            }
            self.step(byte, course, follow);
        }
    }

    /// The file ended: the course is complete, or why there is none. A
    /// refused file leaves `course` empty.
    pub fn finish(&mut self, course: &mut Course, follow: &mut impl Follow) -> Result<(), Refusal> {
        if self.refused.is_none() {
            self.builder.finish(course, follow);
            if course.points().len() < 2 {
                self.refused = Some(Refusal::Empty);
            }
        }
        match self.refused {
            Some(refusal) => {
                course.clear();
                Err(refusal)
            }
            None => {
                course.set_name(self.name.get());
                Ok(())
            }
        }
    }

    fn step(&mut self, byte: u8, course: &mut Course, follow: &mut impl Follow) {
        let space = matches!(byte, b' ' | b'\t' | b'\r' | b'\n');
        self.state = match self.state {
            State::Text => {
                if byte == b'<' {
                    State::Open
                } else {
                    self.name_byte(byte);
                    State::Text
                }
            }
            State::Open => {
                self.tag = Word::new();
                self.closing = byte == b'/';
                self.lat = None;
                self.lon = None;
                match byte {
                    b'/' => State::TagName,
                    b'?' => State::Declaration,
                    _ if space => State::Text,
                    _ => {
                        self.tag.push(byte);
                        State::TagName
                    }
                }
            }
            State::TagName => match byte {
                b'>' => self.end_tag(false, course, follow),
                b'/' => State::Slash,
                _ if space => State::Tag,
                _ => {
                    self.tag.push(byte);
                    match self.tag.get() {
                        b"!--" => State::Comment(0),
                        b"![CDATA[" => State::Cdata(0),
                        [b'!', rest @ ..]
                            if !b"--".starts_with(rest) && !b"[CDATA[".starts_with(rest) =>
                        {
                            State::Declaration
                        }
                        _ => State::TagName,
                    }
                }
            },
            State::Tag => match byte {
                b'>' => self.end_tag(false, course, follow),
                b'/' => State::Slash,
                _ if space => State::Tag,
                _ => {
                    self.attribute = Word::new();
                    self.attribute.push(byte);
                    State::AttributeName
                }
            },
            State::AttributeName => match byte {
                b'=' => State::Quote,
                b'>' => self.end_tag(false, course, follow),
                _ if space => State::Equals,
                _ => {
                    self.attribute.push(byte);
                    State::AttributeName
                }
            },
            State::Equals => match byte {
                b'=' => State::Quote,
                b'>' => self.end_tag(false, course, follow),
                _ if space => State::Equals,
                // An attribute without a value; this byte starts the next.
                _ => {
                    self.attribute = Word::new();
                    self.attribute.push(byte);
                    State::AttributeName
                }
            },
            State::Quote => match byte {
                b'"' | b'\'' => {
                    self.value = Decimal::new();
                    State::Value(byte)
                }
                b'>' => self.end_tag(false, course, follow),
                _ if space => State::Quote,
                _ => State::Tag,
            },
            State::Value(quote) => {
                if byte == quote {
                    match self.attribute.local() {
                        b"lat" => self.lat = Some(self.value),
                        b"lon" => self.lon = Some(self.value),
                        _ => {}
                    }
                    State::Tag
                } else {
                    self.value.push(byte);
                    State::Value(quote)
                }
            }
            State::Slash => match byte {
                b'>' => self.end_tag(true, course, follow),
                _ if space => State::Slash,
                _ => State::Tag,
            },
            State::Comment(matched) => match (byte, matched) {
                (b'-', 0 | 1) => State::Comment(matched + 1),
                (b'-', _) => State::Comment(2),
                (b'>', 2) => State::Text,
                _ => State::Comment(0),
            },
            State::Cdata(matched) => match (byte, matched) {
                (b']', 0 | 1) => State::Cdata(matched + 1),
                (b']', _) => {
                    self.name_byte(b']');
                    State::Cdata(2)
                }
                (b'>', 2) => State::Text,
                _ => {
                    for _ in 0..matched {
                        self.name_byte(b']');
                    }
                    self.name_byte(byte);
                    State::Cdata(0)
                }
            },
            State::Declaration => {
                if byte == b'>' {
                    State::Text
                } else {
                    State::Declaration
                }
            }
        };
    }

    /// A byte of text: part of the course's name while inside it.
    fn name_byte(&mut self, byte: u8) {
        if !self.naming {
            return;
        }
        match (byte, self.entity) {
            (b'&', _) => self.entity = Some(0),
            (b';', Some(_)) => self.entity = None,
            (_, Some(n)) if n < ENTITY && !byte.is_ascii_whitespace() => self.entity = Some(n + 1),
            _ => {
                self.entity = None;
                let byte = byte.to_ascii_uppercase();
                let blank = matches!(byte, b' ' | b'\t' | b'\r' | b'\n');
                if blank && self.name.len == 0 {
                    return;
                }
                // A long name is cut, not dropped.
                if (blank || super::name_byte(byte)) && usize::from(self.name.len) < super::NAME {
                    self.name.push(if blank { b' ' } else { byte });
                }
            }
        }
    }

    fn end_tag(&mut self, empty: bool, course: &mut Course, follow: &mut impl Follow) -> State {
        let tag = self.tag.local();
        if self.closing {
            match (tag, self.route) {
                (
                    b"trk" | b"rte",
                    Route::Inside {
                        track,
                        points,
                        route_kept,
                    },
                ) if (tag == b"trk") == track => {
                    self.route = match (points, route_kept) {
                        (2.., _) => Route::Kept { track },
                        (0 | 1, true) => Route::Kept { track: false },
                        _ => Route::Looking,
                    };
                }
                (b"trkpt" | b"rtept" | b"wpt", _) => self.in_point = false,
                (b"name", _) if self.naming => {
                    self.naming = false;
                    self.named = self.name.len > 0;
                }
                _ => {}
            }
            return State::Text;
        }
        match (tag, self.route) {
            (b"trk", Route::Looking | Route::Kept { track: false }) | (b"rte", Route::Looking) => {
                self.route = Route::Inside {
                    track: tag == b"trk",
                    points: 0,
                    route_kept: self.route == Route::Kept { track: false },
                };
            }
            (
                b"trkpt" | b"rtept",
                Route::Inside {
                    track,
                    points,
                    route_kept,
                },
            ) if (tag == b"trkpt") == track => {
                self.route = Route::Inside {
                    track,
                    points: points.saturating_add(1),
                    route_kept,
                };
                let point = self
                    .lat
                    .and_then(|lat| lat.e7())
                    .zip(self.lon.and_then(|lon| lon.e7()))
                    .and_then(|(lat, lon)| Point::new(lat, lon));
                match (point, points, route_kept) {
                    (None, ..) => self.refused = Some(Refusal::Coordinates),
                    // A route already read stays until the track has two
                    // points; its first waits.
                    (Some(point), 0, true) => self.first = Some(point),
                    (Some(point), 0, false) => {
                        self.builder.start(course, follow);
                        self.builder.push(course, point, follow);
                    }
                    (Some(point), 1, true) => {
                        self.builder.start(course, follow);
                        if let Some(first) = self.first.take() {
                            self.builder.push(course, first, follow);
                        }
                        self.builder.push(course, point, follow);
                    }
                    (Some(point), ..) => self.builder.push(course, point, follow),
                }
                self.in_point = !empty;
            }
            (b"trkpt" | b"rtept" | b"wpt", _) => self.in_point = !empty,
            (b"name", _) if !self.in_point && !self.named && !empty => {
                self.naming = true;
                self.name = Word::new();
            }
            _ => {}
        }
        State::Text
    }
}

#[cfg(test)]
mod tests;
