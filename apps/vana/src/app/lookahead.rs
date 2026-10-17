//! The road ahead: GPS fixes into the terrain look-ahead, its results into
//! the climb forecast, and the climb page brought up before a climb starts.
//!
//! Composition builds a `Terrain` around a tile arena, both in a block of its
//! own (PSRAM on the device), and hands it to `Runtime::attach_terrain`. Then
//! it calls `Runtime::look_ahead` once per main-loop pass with the owned MMC
//! region. The pack lives in its `TERRAIN` volume (`docs/terrain.md`); each
//! call does one bounded step. Without a `Terrain` or a ready pack the climb
//! page keeps its map-free view and says "NO MAP".
use super::Runtime;
use crate::ride::climb::Event as ClimbEvent;
use crate::ride::climb_ahead::{ClimbAhead, Forecast};
use crate::screens::workout::Page;
use device_api::bulk::ReadWrite;
use firmware_services::pack::{self, Pack};
use firmware_services::volume::{self, Extent, Sector, TERRAIN, Volume};

/// A fix older than this stops the look-ahead (the rider is in a tunnel or
/// the receiver lost the sky).
const FIX_LOST_MS: u64 = 5_000;
/// The climb page comes up this far before a climb starts.
const ANNOUNCE_MM: u64 = 500_000;
/// The tile cache composition gives `Terrain::new`: 512 KiB of 8 KiB pages,
/// room for the road ahead through the densest city tiles.
pub const TERRAIN_ARENA_BYTES: usize = 64 * terrain::PAGE_BYTES;
/// How often the volume is looked at again for a new or rewritten pack
/// (four sector reads).
const RECHECK_MS: u64 = 10_000;
/// How far ahead the road is predicted, metres.
const HORIZON_M: f32 = 10_000.0;
/// The forecast without a map.
static NO_MAP: Forecast = Forecast::NONE;

/// The ready pack in the TERRAIN volume.
struct Mounted {
    extent: Extent,
    pack: Pack,
}

/// Everything the look-ahead keeps, about 7 KB with the engine and the
/// forecast. Composition places it outside main's stack and statics (PSRAM
/// on the device), which cannot spare that much; `Runtime` holds a reference.
pub struct Terrain {
    engine: terrain::Lookahead<'static>,
    mounted: Option<Mounted>,
    checked_ms: Option<u64>,
    sector: Sector,
    forecast: Forecast,
    /// When the receiver last had a fresh fix.
    last_fix_ms: Option<u64>,
    /// The last position handed to the engine: a receiver reports about once
    /// a second, the main loop runs far more often, and the matcher's
    /// hysteresis counts fixes, so only a new position is a new fix.
    fed: Option<(i32, i32)>,
    /// The map brought up the climb page for the climb under way or close.
    pub(super) announced: bool,
    /// The announced climb has been under way (the rider reached its foot).
    on_climb: bool,
    status: Option<terrain::Status>,
}

impl Terrain {
    /// The look-ahead around a tile cache (`TERRAIN_ARENA_BYTES`).
    pub fn new(arena: &'static mut [u8]) -> Self {
        Self {
            engine: terrain::Lookahead::new(arena, HORIZON_M),
            mounted: None,
            checked_ms: None,
            sector: [0; 512],
            forecast: Forecast::NONE,
            last_fix_ms: None,
            fed: None,
            announced: false,
            on_climb: false,
            status: None,
        }
    }

    /// Start over as if new, keeping the tile arena: no fix, no pack, no
    /// forecast.
    pub fn reset(&mut self) {
        self.engine.lost();
        self.engine.reset();
        self.mounted = None;
        self.checked_ms = None;
        self.forecast.forget();
        self.forecast.map = false;
        self.last_fix_ms = None;
        self.fed = None;
        self.announced = false;
        self.on_climb = false;
        self.status = None;
    }
}

/// Terrain pack bytes through the volume pack.
struct PackSource<'a, M: ReadWrite> {
    volume: Volume<'a, M>,
    pack: &'a Pack,
    sector: &'a mut Sector,
}

impl<M: ReadWrite> terrain::Source for PackSource<'_, M> {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), terrain::Error> {
        match self.pack.read(&mut self.volume, offset, buf, self.sector) {
            Ok(n) if n == buf.len() => Ok(()),
            Ok(_) | Err(pack::Error::Range) => Err(terrain::Error::Range),
            Err(_) => Err(terrain::Error::Media),
        }
    }
}

/// The ready pack in `media`'s TERRAIN volume, if there is one. Its tag
/// (`CHILE-TERRAIN-V1`) is the host's label; the engine checks the terrain
/// header's own magic and version and treats anything else as no pack.
fn find_pack(media: &mut impl ReadWrite, sector: &mut Sector) -> Option<Mounted> {
    let mut terrain = volume::open(media, &TERRAIN, sector).ok()?;
    let extent = terrain.extent();
    let pack = pack::open(&mut terrain, sector).ok()?;
    Some(Mounted { extent, pack })
}

/// The forecast in `terrain`, or the map-free one: borrows only that field,
/// so the frame can hold it while the rest of `Runtime` changes.
pub(super) fn forecast_of<'a>(terrain: &'a Option<&'static mut Terrain>) -> &'a Forecast {
    terrain.as_deref().map_or(&NO_MAP, |state| &state.forecast)
}

impl Runtime {
    /// Give the look-ahead its state and tile cache; without them it never
    /// runs.
    pub fn attach_terrain(&mut self, terrain: &'static mut Terrain) {
        self.lookahead = Some(terrain);
    }

    /// Hand the look-ahead's block back, for a host that reuses it.
    pub fn detach_terrain(&mut self) -> Option<&'static mut Terrain> {
        self.lookahead.take()
    }

    /// One bounded step of the look-ahead against the TERRAIN volume of the
    /// owned region `media`.
    pub fn look_ahead(&mut self, media: &mut impl ReadWrite, now: u64) -> terrain::Status {
        let Some(state) = self.lookahead.as_deref_mut() else {
            return terrain::Status::NoPack;
        };
        let engine = &mut state.engine;
        match self.readings.live.location_e7 {
            Some((lat, lon)) => {
                if state.fed != Some((lat, lon)) {
                    state.fed = Some((lat, lon));
                    engine.fix(terrain::Fix::from_e7(lat, lon, now));
                }
                state.last_fix_ms = Some(now);
            }
            None => {
                if state
                    .last_fix_ms
                    .is_some_and(|at| now.saturating_sub(at) > FIX_LOST_MS)
                {
                    engine.lost();
                    state.last_fix_ms = None;
                    state.fed = None;
                }
            }
        }
        if state
            .checked_ms
            .is_none_or(|at| now.saturating_sub(at) >= RECHECK_MS)
        {
            state.checked_ms = Some(now);
            let found = find_pack(media, &mut state.sector);
            let same = match (&found, &state.mounted) {
                (Some(new), Some(old)) => {
                    new.extent == old.extent && new.pack.header() == old.pack.header()
                }
                (None, None) => true,
                _ => false,
            };
            // A new, rewritten or removed pack starts the look-ahead over, as
            // does a pack the engine gave up on after repeated read errors.
            if !same || (found.is_some() && state.status == Some(terrain::Status::NoPack)) {
                engine.reset();
            }
            state.mounted = found;
        }
        let status = match &state.mounted {
            Some(mounted) => engine.step(&mut PackSource {
                volume: Volume::new(media, mounted.extent),
                pack: &mounted.pack,
                sector: &mut state.sector,
            }),
            None => terrain::Status::NoPack,
        };
        state.status = Some(status);
        state.forecast.map = status != terrain::Status::NoPack;
        if status == terrain::Status::NoPack {
            // No pack, or one the engine gave up on: the last forecast is
            // stale, and the page falls back to the barometer.
            state.forecast.forget();
        } else {
            state.forecast.update(engine.ahead());
        }
        // Every pass, not only on a new forecast: a climb seen before the
        // ride screen was open is announced once it is.
        self.follow_forecast();
        status
    }

    /// The ride screen follows the map too: the climb page comes up shortly
    /// before a climb, and goes back once the map shows no climb under way
    /// or close, unless the ride's own detector still sees one.
    pub(super) fn follow_forecast(&mut self) {
        let Some(state) = self.lookahead.as_deref_mut() else {
            return;
        };
        let forecast = &state.forecast;
        // An unusable forecast (a fork, an unsure match) changes nothing:
        // the page waits for the map to say something again.
        if !forecast.usable() {
            return;
        }
        let near = |c: &ClimbAhead| c.summary.start_mm <= ANNOUNCE_MM;
        let current = forecast.current().is_some();
        let coming = current || forecast.upcoming().is_some_and(|c| near(&c));
        // Over the top of the announced climb, the next one, however close,
        // is a climb of its own.
        if state.announced && state.on_climb && !current && coming {
            state.announced = false;
        }
        if coming {
            // Once per climb: a rider who pages away is left there. Only the
            // ride screen can show it; until then the climb stays unannounced.
            if !state.announced && self.navigation.page == Page::Ride {
                state.announced = true;
                state.on_climb = current;
                self.navigation.follow_climb(ClimbEvent::Confirmed);
            } else if state.announced {
                state.on_climb |= current;
            }
        } else if core::mem::take(&mut state.announced) && self.ride.climbs.climbing().is_none() {
            let summary = self.ride.climbs.last_climb().unwrap_or_default();
            self.navigation.follow_climb(ClimbEvent::Finished(summary));
        }
    }

    /// The ride's own (barometric) detector saw a climb start or end. Its
    /// start does not bring the climb page up again for a climb the map has
    /// already announced (the rider may have paged away). Its end does not
    /// take the page away while the map still shows a climb under way or
    /// close: the map knows the road goes on up.
    pub(super) fn ride_climb_event(&mut self, event: ClimbEvent) {
        let announced = self.lookahead.as_deref().is_some_and(|s| s.announced);
        let forecast = self.forecast();
        let map_climb = forecast.current().is_some()
            || forecast
                .upcoming()
                .is_some_and(|c| c.summary.start_mm <= ANNOUNCE_MM);
        match event {
            ClimbEvent::Confirmed if announced && map_climb => return,
            ClimbEvent::Finished(_) if map_climb => return,
            _ => {}
        }
        self.navigation.follow_climb(event);
    }

    /// What the look-ahead did last: no pack, off the map, working or idle.
    pub fn terrain_status(&self) -> Option<terrain::Status> {
        self.lookahead.as_deref().and_then(|state| state.status)
    }

    /// The forecast the climb page draws.
    pub fn forecast(&self) -> &Forecast {
        forecast_of(&self.lookahead)
    }

    /// The forecast, for tests that stand in for the engine.
    #[cfg(test)]
    pub(super) fn forecast_mut(&mut self) -> &mut Forecast {
        &mut self
            .lookahead
            .as_deref_mut()
            .expect("tests attach a Terrain first")
            .forecast
    }
}
