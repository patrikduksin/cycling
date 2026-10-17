//! The committed sample pack: the Farellones road (G-21) east of Santiago,
//! from Corral Quemado (850 m) past Farellones (2,700 m). Data (c)
//! OpenStreetMap contributors (ODbL) and Copernicus WorldDEM-30; see
//! `tests/data/README.md`.
use terrain::pack::{ENTRY_BYTES, HEADER_BYTES, Header, Source};
use terrain::tile::Tile;
use terrain::{End, Fix, Lookahead, SAMPLE_M, Status, TileKey};

const PACK: &[u8] = include_bytes!("data/farellones.terrain");
const TRACK: &str = include_str!("data/farellones-g21.csv");
/// `zlib.crc32(b"g-21") & 0xffff`, the builder's road group for G-21.
const G21: u16 = 37214;

fn header() -> Header {
    Header::parse(PACK[..HEADER_BYTES].try_into().unwrap()).unwrap()
}

fn tiles(header: &Header) -> Vec<Tile<'static>> {
    let mut out = Vec::new();
    for row in 0..header.rows {
        for col in 0..header.cols {
            let key = TileKey { row, col };
            let mut entry = [0u8; ENTRY_BYTES];
            (&PACK[..]).read(header.entry(key), &mut entry).unwrap();
            if let Some((offset, len)) = header.locate(&entry).unwrap() {
                let bytes = &PACK[offset as usize..(offset + len) as usize];
                out.push(Tile::parse(bytes, key, header.tile).unwrap());
            }
        }
    }
    out
}

/// G-21 from its lowest point, as (lat µdeg, lon µdeg, z dm) every vertex:
/// `cycling-terrain trace farellones.terrain G-21` wrote it from the pack.
fn g21() -> Vec<(i32, i32, i32)> {
    TRACK
        .lines()
        .filter(|line| !line.starts_with('#'))
        .map(|line| {
            let mut fields = line.split(',').map(|f| f.parse::<f64>().unwrap());
            let mut next = || fields.next().unwrap();
            (
                (next() * 1e6).round() as i32,
                (next() * 1e6).round() as i32,
                (next() * 10.0).round() as i32,
            )
        })
        .collect()
}

fn metres(a: (i32, i32, i32), b: (i32, i32, i32)) -> f64 {
    let lat = f64::from(a.0) * 1e-6;
    let dy = f64::from(b.0 - a.0) * 0.111_195;
    let dx = f64::from(b.1 - a.1) * 0.111_195 * lat.to_radians().cos();
    dx.hypot(dy)
}

/// A route with its cumulative distances, for lookups along it.
struct Road {
    points: Vec<(i32, i32, i32)>,
    cumulative: Vec<f64>,
}

impl Road {
    fn new(points: Vec<(i32, i32, i32)>) -> Self {
        let mut cumulative = vec![0.0];
        for pair in points.windows(2) {
            cumulative.push(cumulative.last().unwrap() + metres(pair[0], pair[1]));
        }
        Self { points, cumulative }
    }

    /// (lat, lon, z) at `distance` metres along the road.
    fn at(&self, distance: f64) -> Option<(f64, f64, f64)> {
        let index = self.cumulative.partition_point(|&d| d < distance);
        if index >= self.points.len() {
            return None;
        }
        if index == 0 {
            let p = self.points[0];
            return Some((f64::from(p.0), f64::from(p.1), f64::from(p.2)));
        }
        let (a, b) = (self.points[index - 1], self.points[index]);
        let span = self.cumulative[index] - self.cumulative[index - 1];
        let t = if span > 0.0 {
            (distance - self.cumulative[index - 1]) / span
        } else {
            0.0
        };
        let lerp = |a: i32, b: i32| f64::from(a) + f64::from(b - a) * t;
        Some((lerp(a.0, b.0), lerp(a.1, b.1), lerp(a.2, b.2)))
    }
}

fn length(route: &[(i32, i32, i32)]) -> f64 {
    route.windows(2).map(|p| metres(p[0], p[1])).sum()
}

#[test]
fn the_sample_pack_is_whole_and_attributed() {
    let header = header();
    let start = header.attribution_offset as usize;
    let text = core::str::from_utf8(&PACK[start..start + header.attribution_len as usize]).unwrap();
    assert!(text.contains("OpenStreetMap contributors"), "{text}");
    assert!(text.contains("Open Database License"), "{text}");
    assert!(text.contains("Copernicus WorldDEM-30"), "{text}");
    assert_eq!(header.total as usize, PACK.len());
    let tiles = tiles(&header);
    assert_eq!(tiles.len(), header.tiles as usize);
    let route = g21();
    let km = length(&route) / 1000.0;
    // The climb: about 1,860 m up over 25-35 km of hairpins.
    assert!((20.0..40.0).contains(&km), "{km} km");
    let gain = route.last().unwrap().2 - route[0].2;
    assert!((15_000..21_000).contains(&gain), "{gain} dm");
}

/// Ride the road at 18 km/h with a fix a second, each a few metres off the
/// road, and compare the predicted profile with the real road ahead.
/// What riding a road through a pack showed.
#[derive(Debug)]
struct Ride {
    checked: u32,
    on_road: u32,
    confident: u32,
    wrong: u32,
    forks: u32,
    /// Metres the prediction agreed with the road, per fix, sorted.
    reach: Vec<usize>,
    /// The most look-ahead steps one fix took.
    steps: u32,
}

impl Ride {
    fn reach(&self, percent: usize) -> usize {
        self.reach[self.reach.len() * percent / 100]
    }
}

/// Ride `route` up through `pack` at 5 m/s, one noisy fix a second.
fn ride(pack: &[u8], route: Vec<(i32, i32, i32)>, group: u16) -> Ride {
    let total = length(&route);
    let road = Road::new(route);
    let mut arena = vec![0u8; 64 * terrain::PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, 10_000.0);
    let mut source: &[u8] = pack;
    let header = Header::parse(pack[..HEADER_BYTES].try_into().unwrap()).unwrap();
    let speed = 5.0;
    let (mut checked, mut on_road, mut confident, mut wrong) = (0u32, 0u32, 0u32, 0u32);
    let (mut forks, mut most) = (0u32, 0u32);
    let mut reach = Vec::new();
    let mut distance = 0.0;
    let mut second = 0u64;
    // GPS error wanders: a random walk of up to 1 m a second, within 5 m.
    let mut seed = 0x2545_f491_4f6c_dd1du64;
    let mut noise = (0.0f64, 0.0f64);
    while distance + 10_000.0 < total {
        let (lat, lon, _) = road.at(distance).unwrap();
        for value in [&mut noise.0, &mut noise.1] {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let step = (seed % 2001) as f64 / 1000.0 - 1.0;
            *value = (*value + step).clamp(-5.0, 5.0);
        }
        let fix = Fix {
            lat: (lat + noise.1 / 0.111_195) as i32,
            lon: (lon + noise.0 / 0.093) as i32,
            at_ms: second * 1000,
        };
        ahead.fix(fix);
        let mut steps = 0;
        while ahead.step(&mut source) == Status::Working {
            steps += 1;
            assert!(steps < 2000, "the look-ahead never settles");
        }
        most = most.max(steps);
        let result = ahead.ahead();
        if second > 5 {
            let matched = result.matched.expect("matched on the road");
            let tile = tile_at(pack, &header, matched.tile);
            on_road += u32::from(tile.edge(matched.edge).group == group);
            forks += u32::from(result.end == End::Fork);
            // How far the prediction follows the road's real profile.
            let start = road.at(distance).unwrap().2;
            let mut agree = 0;
            let mut diverged = false;
            for (index, sample) in result.samples[..result.len].iter().enumerate() {
                let Some(truth) = road.at(distance + index as f64 * f64::from(SAMPLE_M)) else {
                    break;
                };
                if (f64::from(*sample) - (truth.2 - start)).abs() > 150.0 {
                    diverged = true;
                    break;
                }
                agree = index * 20;
            }
            if result.confidence >= 50 {
                confident += 1;
                wrong += u32::from(diverged);
            }
            reach.push(agree);
            checked += 1;
        }
        distance += speed;
        second += 1;
    }
    reach.sort();
    Ride {
        checked,
        on_road,
        confident,
        wrong,
        forks,
        reach,
        steps: most,
    }
}

/// `key`'s tile in `pack`, checked.
fn tile_at<'p>(pack: &'p [u8], header: &Header, key: TileKey) -> Tile<'p> {
    let entry = header.entry(key) as usize;
    let (offset, len) = header
        .locate(pack[entry..entry + ENTRY_BYTES].try_into().unwrap())
        .unwrap()
        .unwrap();
    Tile::parse(&pack[offset as usize..][..len as usize], key, header.tile).unwrap()
}

/// The ride's shape: on the road, confident, rarely wrong, far-sighted.
fn assert_rides_well(ride: &Ride) {
    eprintln!(
        "{} fixes: on road {}, confident {}, wrong {}, forks {}, reach p10 {} p50 {} m, at most {} steps a fix",
        ride.checked,
        ride.on_road,
        ride.confident,
        ride.wrong,
        ride.forks,
        ride.reach(10),
        ride.reach(50),
        ride.steps,
    );
    assert!(ride.checked > 4000, "{ride:?}");
    assert!(
        ride.on_road * 100 > ride.checked * 98,
        "matched the road on {} of {} fixes",
        ride.on_road,
        ride.checked
    );
    // A confident prediction is the road's profile (within 15 m), but for
    // a few seconds at some hairpins, where noise puts the rider on the leg
    // above or below.
    assert!(
        ride.confident * 100 > ride.checked * 85,
        "{} confident of {}",
        ride.confident,
        ride.checked
    );
    assert!(
        ride.wrong * 100 < ride.confident,
        "{} wrong of {} confident",
        ride.wrong,
        ride.confident
    );
    // Usually the whole 10 km horizon; rarely under 3 km (forks, Farellones).
    assert!(
        ride.reach(50) >= 9_000 && ride.reach(10) >= 3_000,
        "{:?}",
        ride.reach
    );
}

#[test]
fn riding_up_g21_matches_the_road_and_predicts_its_profile() {
    assert_rides_well(&ride(PACK, g21(), G21));
}

/// The national pack, when `CYCLING_TERRAIN_PACK` names one: every tile
/// reads, and G-21 rides as well through it as through the sample.
#[test]
#[ignore = "needs a national pack built by tools/terrain"]
fn the_national_pack_reads_whole_and_rides_g21() {
    let path = std::env::var("CYCLING_TERRAIN_PACK").expect("CYCLING_TERRAIN_PACK");
    let pack = std::fs::read(path).unwrap();
    let header = Header::parse(pack[..HEADER_BYTES].try_into().unwrap()).unwrap();
    let directory = &pack[header.directory as usize..header.data as usize];
    assert_eq!(
        terrain::tile::crc32(directory).to_le_bytes(),
        pack[120..124]
    );
    let (mut tiles, mut largest) = (0, 0);
    for row in 0..header.rows {
        for col in 0..header.cols {
            let key = TileKey { row, col };
            let entry = header.entry(key) as usize;
            if let Some((_, len)) = header
                .locate(pack[entry..entry + ENTRY_BYTES].try_into().unwrap())
                .unwrap()
            {
                tile_at(&pack, &header, key);
                tiles += 1;
                largest = largest.max(len as usize);
            }
        }
    }
    eprintln!("{tiles} tiles, the largest {largest} bytes");
    assert_eq!(tiles, header.tiles);
    assert!(largest <= terrain::MAX_TILE_BYTES);
    assert_rides_well(&ride(&pack, g21(), G21));
}

/// A cache too small for the tiles around a fix gives up instead of loading
/// them in turn for ever.
#[test]
fn tiles_that_cannot_share_the_cache_end_the_attempt() {
    let header = header();
    // A corner of four tiles with roads, near Santiago's edge.
    let lat = header.south + header.tile as i32;
    let lon = header.west + header.tile as i32;
    let mut arena = vec![0u8; terrain::PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, 10_000.0);
    let mut source: &[u8] = PACK;
    for second in 0..5u64 {
        ahead.fix(Fix {
            lat: lat + second as i32 * 40,
            lon,
            at_ms: second * 1000,
        });
        let mut steps = 0;
        while ahead.step(&mut source) == Status::Working {
            steps += 1;
            assert!(steps < 2_000, "never settles");
        }
    }
    assert!(ahead.loads() > 4, "{} loads", ahead.loads());
}
