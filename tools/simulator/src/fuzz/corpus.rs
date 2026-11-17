use crate::route::{Point, Route, distance_m, from_e7};
use crate::trace::world::{self, World};
use device_api::gps::FixState;
use firmware_services::trace::{Event, decode};
use std::path::{Path, PathBuf};

const SPACING_M: f64 = 5.0;
const SHORTEST_M: f64 = 500.0;

#[derive(Default)]
pub struct Corpus {
    pub path: Option<PathBuf>,
    /// Sorted by path, so a pick names the same course on every run.
    routes: Vec<(PathBuf, Route)>,
    /// Traces that gave no course, and why.
    pub skipped: Vec<(PathBuf, String)>,
}

impl Corpus {
    pub fn none() -> Self {
        Self::default()
    }

    /// The trace at `path`, or every `.trace` under it; a missing path is an
    /// empty corpus.
    pub fn load(path: Option<&Path>) -> Self {
        let mut corpus = Self {
            path: path.map(Path::to_path_buf),
            ..Self::none()
        };
        let mut paths = Vec::new();
        match path {
            Some(file) if file.is_file() => paths.push(file.to_path_buf()),
            Some(dir) => traces(dir, &mut paths),
            None => {}
        }
        paths.sort();
        for path in paths {
            match std::fs::read(&path)
                .map_err(|e| e.to_string())
                .and_then(|bytes| track(&bytes))
            {
                Ok(route) => corpus.routes.push((path, route)),
                Err(why) => corpus.skipped.push((path, why)),
            }
        }
        corpus
    }

    pub fn len(&self) -> usize {
        self.routes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.routes.is_empty()
    }

    pub fn route(&self, pick: u64) -> Option<&(PathBuf, Route)> {
        self.routes
            .get(usize::try_from(pick % self.len().max(1) as u64).ok()?)
    }
}

fn traces(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() {
            traces(&path, out);
        } else if path.extension().is_some_and(|e| e == "trace") {
            out.push(path);
        }
    }
}

fn track(bytes: &[u8]) -> Result<Route, String> {
    let (_, events) = decode(bytes).map_err(|e| e.to_string())?;
    let mut points: Vec<Point> = Vec::new();
    for event in events {
        let Event::Call(call) = event else {
            continue;
        };
        let Some(World::Position(Some(snapshot))) = world::read(&call) else {
            continue;
        };
        let gps = snapshot.gps;
        let (FixState::Fresh, Some(lat), Some(lon)) =
            (gps.state, gps.latitude_e7, gps.longitude_e7)
        else {
            continue;
        };
        let point = from_e7(lat, lon);
        if points
            .last()
            .is_none_or(|last| distance_m(*last, point) >= SPACING_M)
        {
            points.push(point);
        }
    }
    let route = Route::new(points, false);
    if route.length_m() < SHORTEST_M {
        return Err(format!("under {SHORTEST_M} m of GPS track"));
    }
    Ok(route)
}
