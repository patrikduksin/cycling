//! The look-ahead as a bounded state machine.
//!
//! Feed fixes with [`Lookahead::fix`]; call [`Lookahead::step`] often (the
//! device calls it once per main-loop pass, outside rendering). Each step
//! does one thing: reads the header, loads one chunk of a tile, matches the
//! newest fix, or predicts the road ahead. Matching and prediction restart
//! from scratch when they need a tile that is not cached, so no partial
//! state survives between steps; they are cheap enough for that.
use crate::cache::Cache;
use crate::geo::{self, Point};
use crate::matcher::{Matched, Matcher};
use crate::pack::{Error, HEADER_BYTES, Header, Source, TileKey};
pub use crate::predict::{End, MAX_SAMPLES, SAMPLE_M};
use crate::predict::{Need, Prediction, predict};

/// Movement that sets a new heading: under it, GPS noise dominates.
const HEADING_MOVE_M: f32 = 15.0;
/// Fixes kept for the heading: 16 s, enough at walking pace.
const HISTORY: usize = 16;
/// A heading this old (no movement) is forgotten.
const HEADING_MAX_AGE_MS: u64 = 120_000;
/// Predict at most this often, and on every new match.
const PREDICT_EVERY_MS: u64 = 2_000;
/// Give up on a pack that fails to read this many times in a row.
const READ_FAILURES: u8 = 8;
/// Tile loads one match or prediction may ask for before it gives up: more
/// than the cache could hold means its tiles evict each other.
const fn reload_limit(pages: usize) -> u16 {
    (2 * pages + 16) as u16
}

/// A GPS position, micro-degrees, at a monotonic time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fix {
    pub lat: i32,
    pub lon: i32,
    pub at_ms: u64,
}

impl Fix {
    /// From the 1e-7 degree integers the GPS reports.
    pub fn from_e7(lat_e7: i32, lon_e7: i32, at_ms: u64) -> Self {
        Self {
            lat: lat_e7 / 10,
            lon: lon_e7 / 10,
            at_ms,
        }
    }
}

/// The road ahead, as of one fix.
#[derive(Clone, Debug, PartialEq)]
pub struct Ahead {
    /// Increments with every new result.
    pub generation: u32,
    /// The fix this was computed from.
    pub at_ms: u64,
    pub matched: Option<Matched>,
    /// Map elevation at the rider, decimetres.
    pub start_z_dm: i32,
    /// Elevation every `SAMPLE_M` from the rider, relative to `start_z_dm`.
    pub samples: [i16; MAX_SAMPLES],
    pub len: usize,
    pub end: End,
    pub end_m: f32,
    /// 0..100: the match and every decision along the way.
    pub confidence: u8,
}

impl Default for Ahead {
    fn default() -> Self {
        Self {
            generation: 0,
            at_ms: 0,
            matched: None,
            start_z_dm: 0,
            samples: [0; MAX_SAMPLES],
            len: 0,
            end: End::Uncertain,
            end_m: 0.0,
            confidence: 0,
        }
    }
}

impl Ahead {
    /// The rider is on a mapped road and its profile is known.
    pub fn on_road(&self) -> bool {
        self.matched.is_some() && self.len > 1
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing to do until the next fix.
    Idle,
    /// More work queued; call again.
    Working,
    /// No readable pack: the caller falls back to map-free behaviour.
    NoPack,
    /// The newest fix is outside the pack's grid.
    OffMap,
}

#[derive(Clone, Copy, Debug)]
enum Pack {
    Unread,
    Ready(Header),
    Failed,
}

pub struct Lookahead<'a> {
    cache: Cache<'a>,
    pack: Pack,
    failures: u8,
    matcher: Matcher,
    fix: Option<Fix>,
    match_due: bool,
    predict_due: bool,
    last_predict_ms: Option<u64>,
    need: Option<TileKey>,
    /// Tiles requested by the current match or prediction attempt.
    reloads: u16,
    history: [Point; HISTORY],
    history_len: usize,
    heading: Option<(f32, u64)>,
    horizon_m: f32,
    ahead: Ahead,
    /// Diagnostics: steps that did work, and the longest one's kind.
    pub steps: u32,
}

impl<'a> Lookahead<'a> {
    /// A look-ahead caching tiles in `arena` and following the road for
    /// `horizon_m` (at most 10 km).
    pub fn new(arena: &'a mut [u8], horizon_m: f32) -> Self {
        Self {
            cache: Cache::new(arena),
            pack: Pack::Unread,
            failures: 0,
            matcher: Matcher::default(),
            fix: None,
            match_due: false,
            predict_due: false,
            last_predict_ms: None,
            need: None,
            reloads: 0,
            history: [Point::default(); HISTORY],
            history_len: 0,
            heading: None,
            horizon_m: horizon_m.min((MAX_SAMPLES - 1) as f32 * SAMPLE_M),
            ahead: Ahead::default(),
            steps: 0,
        }
    }

    pub fn ahead(&self) -> &Ahead {
        &self.ahead
    }

    pub fn header(&self) -> Option<&Header> {
        match &self.pack {
            Pack::Ready(header) => Some(header),
            _ => None,
        }
    }

    /// Tiles loaded since start.
    pub fn loads(&self) -> u32 {
        self.cache.loads
    }

    /// Forget the pack and every cached tile, for example after the volume
    /// was rewritten.
    pub fn reset(&mut self) {
        self.cache.clear();
        self.pack = Pack::Unread;
        self.failures = 0;
        self.matcher.reset();
        self.need = None;
        self.reloads = 0;
        self.last_predict_ms = None;
        self.predict_due = false;
        // The fix still stands: match it again against the new pack.
        self.match_due = self.fix.is_some();
        self.ahead = Ahead {
            generation: self.ahead.generation.wrapping_add(1),
            ..Ahead::default()
        };
    }

    /// Bearing of travel from recent fixes, if the rider moved recently.
    pub fn heading(&self) -> Option<f32> {
        self.heading.map(|(bearing, _)| bearing)
    }

    /// A new fix. Fixes that do not change do nothing.
    pub fn fix(&mut self, fix: Fix) {
        if self
            .fix
            .is_some_and(|f| f.lat == fix.lat && f.lon == fix.lon)
        {
            return;
        }
        // Heading: from the newest earlier fix at least HEADING_MOVE_M away,
        // so each fix updates it and GPS noise is a small part of the span.
        let here = Point {
            lat: fix.lat,
            lon: fix.lon,
        };
        let lon_um = lon_scale_um(fix.lat);
        let back = self.history[..self.history_len]
            .iter()
            .rev()
            .find_map(|old| {
                let (east, north) = here.metres_from(*old, lon_um);
                (geo::length(east, north) >= HEADING_MOVE_M).then_some((east, north))
            });
        if let Some((east, north)) = back {
            self.heading = Some((geo::bearing(east, north), fix.at_ms));
        }
        if self.history_len == HISTORY {
            self.history.copy_within(1.., 0);
            self.history_len -= 1;
        }
        self.history[self.history_len] = here;
        self.history_len += 1;
        if self
            .heading
            .is_some_and(|(_, at)| fix.at_ms.saturating_sub(at) > HEADING_MAX_AGE_MS)
        {
            self.heading = None;
        }
        self.fix = Some(fix);
        self.match_due = true;
    }

    /// The fix stream stopped (no fix or stale): forget the road.
    pub fn lost(&mut self) {
        self.fix = None;
        self.history_len = 0;
        self.heading = None;
        self.match_due = false;
        self.predict_due = false;
        self.matcher.reset();
        if self.ahead.matched.is_some() {
            self.ahead = Ahead {
                generation: self.ahead.generation.wrapping_add(1),
                ..Ahead::default()
            };
        }
    }

    /// One bounded unit of work.
    pub fn step(&mut self, source: &mut impl Source) -> Status {
        let header = match self.pack {
            Pack::Failed => return Status::NoPack,
            Pack::Ready(header) => header,
            Pack::Unread => {
                let mut sector = [0u8; HEADER_BYTES];
                return match source
                    .read(0, &mut sector)
                    .and_then(|_| Header::parse(&sector))
                {
                    Ok(header) => {
                        self.pack = Pack::Ready(header);
                        self.failures = 0;
                        Status::Working
                    }
                    Err(Error::Media) if self.failures + 1 < READ_FAILURES => {
                        self.failures += 1;
                        Status::Working
                    }
                    Err(_) => {
                        self.pack = Pack::Failed;
                        Status::NoPack
                    }
                };
            }
        };
        self.steps = self.steps.wrapping_add(1);
        if let Some(key) = self.need {
            match self.cache.load(source, &header, key) {
                Ok(true) => {
                    self.need = None;
                    self.failures = 0;
                }
                Ok(false) => {}
                Err(_) => {
                    self.failures = self.failures.saturating_add(1);
                    if self.failures >= READ_FAILURES {
                        self.pack = Pack::Failed;
                        return Status::NoPack;
                    }
                }
            }
            return Status::Working;
        }
        let Some(fix) = self.fix else {
            return Status::Idle;
        };
        let point = Point {
            lat: fix.lat,
            lon: fix.lon,
        };
        if header.key(fix.lat, fix.lon).is_none() {
            if self.ahead.matched.is_some() {
                self.matcher.reset();
                self.publish(fix, None, Prediction::default(), End::MapEdge);
            }
            self.match_due = false;
            return Status::OffMap;
        }
        if self.match_due {
            match self
                .matcher
                .update(&self.cache, &header, point, self.heading())
            {
                Err(key) => {
                    if self.reload(key) {
                        return Status::Working;
                    }
                    // The tiles around the fix do not fit together.
                    self.match_due = false;
                    return Status::Idle;
                }
                Ok(matched) => {
                    self.reloads = 0;
                    self.match_due = false;
                    let changed = matched.map(|m| (m.tile, m.edge))
                        != self.ahead.matched.map(|m| (m.tile, m.edge));
                    let due = self
                        .last_predict_ms
                        .is_none_or(|at| fix.at_ms.saturating_sub(at) >= PREDICT_EVERY_MS);
                    if matched.is_none() {
                        if self.ahead.matched.is_some() {
                            self.publish(fix, None, Prediction::default(), End::Uncertain);
                        }
                    } else if changed || due {
                        self.predict_due = true;
                    }
                    return Status::Working;
                }
            }
        }
        if self.predict_due {
            let Some(matched) = self.matcher.current() else {
                self.predict_due = false;
                return Status::Idle;
            };
            let mut samples = [0i16; MAX_SAMPLES];
            match predict(&self.cache, &header, &matched, self.horizon_m, &mut samples) {
                Err(Need::Match) => {
                    self.matcher.reset();
                    self.match_due = true;
                    return Status::Working;
                }
                Err(Need::Tile(key)) => {
                    if self.reload(key) {
                        return Status::Working;
                    }
                    // The road ahead's tiles evict each other.
                    self.predict_due = false;
                    self.last_predict_ms = Some(fix.at_ms);
                    let memory = Prediction {
                        end: End::Memory,
                        ..Prediction::default()
                    };
                    self.publish(fix, Some(matched), memory, End::Memory);
                    return Status::Working;
                }
                Ok(prediction) => {
                    self.reloads = 0;
                    self.predict_due = false;
                    self.last_predict_ms = Some(fix.at_ms);
                    self.ahead.samples = samples;
                    self.publish(fix, Some(matched), prediction, prediction.end);
                    return Status::Working;
                }
            }
        }
        Status::Idle
    }

    /// Ask for `key`, unless this attempt already asked for more tiles than
    /// the cache holds; then the attempt is abandoned.
    fn reload(&mut self, key: TileKey) -> bool {
        self.reloads += 1;
        if self.reloads > reload_limit(self.cache.capacity()) {
            self.reloads = 0;
            return false;
        }
        self.need = Some(key);
        true
    }

    fn publish(&mut self, fix: Fix, matched: Option<Matched>, prediction: Prediction, end: End) {
        self.ahead.generation = self.ahead.generation.wrapping_add(1);
        self.ahead.at_ms = fix.at_ms;
        self.ahead.matched = matched;
        self.ahead.start_z_dm = matched.map_or(0, |m| m.z_dm);
        self.ahead.len = if matched.is_some() { prediction.len } else { 0 };
        self.ahead.end = end;
        self.ahead.end_m = prediction.end_m;
        self.ahead.confidence = matched.map_or(0, |_| prediction.confidence);
    }
}

/// Micrometres per micro-degree of longitude at `lat` micro-degrees.
fn lon_scale_um(lat: i32) -> u32 {
    (geo::LAT_UM * libm::cosf((lat as f32 * 1e-6).to_radians())) as u32
}
