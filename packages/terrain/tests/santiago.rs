//! The committed central-Santiago pack: the 3 x 3 tiles around Plaza de
//! Armas, the densest roads a map draws. Data (c) OpenStreetMap
//! contributors (ODbL) and Copernicus WorldDEM-30; see `tests/data/README.md`.
use std::collections::BTreeMap;
use std::hint::black_box;
use std::time::Instant;
use terrain::pack::HEADER_BYTES;
use terrain::{Header, Lookahead, PAGE_BYTES, RoadClass, Status, TileKey};

const PACK: &[u8] = include_bytes!("data/santiago-centro.terrain");
/// Plaza de Armas, (lat_e7, lon_e7).
const PLAZA: (i32, i32) = (-334_378_000, -706_505_000);

fn header() -> Header {
    Header::parse(PACK[..HEADER_BYTES].try_into().unwrap()).unwrap()
}

/// The whole grid as a view's corners.
fn everything(header: &Header) -> [(i32, i32); 4] {
    let (south, west) = (header.south * 10, header.west * 10);
    let north = south + i32::from(header.rows) * header.tile as i32 * 10;
    let east = west + i32::from(header.cols) * header.tile as i32 * 10;
    [(south, west), (north, west), (north, east), (south, east)]
}

/// A look-ahead with every tile of the pack cached, as a map would have it.
fn cached(arena: &mut [u8]) -> (Lookahead<'_>, Vec<TileKey>) {
    let header = header();
    let keys: Vec<TileKey> = header.tiles_under(&everything(&header)).collect();
    let mut engine = Lookahead::new(arena, 10_000.0);
    let mut source: &[u8] = PACK;
    engine.want(keys.iter().copied());
    let mut steps = 0;
    while engine.step(&mut source) == Status::Working {
        steps += 1;
        assert!(steps < 1_000);
    }
    (engine, keys)
}

fn metres(a: (i32, i32), b: (i32, i32)) -> f64 {
    let north = f64::from(b.0 - a.0) * 0.011_119_5;
    let east = f64::from(b.1 - a.1) * 0.011_119_5 * (f64::from(a.0) * 1e-7).to_radians().cos();
    east.hypot(north)
}

#[test]
fn every_tile_of_central_santiago_draws_its_roads_inside_it() {
    let header = header();
    let mut arena = vec![0u8; 64 * PAGE_BYTES];
    let (engine, keys) = cached(&mut arena);
    assert_eq!(keys.len(), 9);
    let side = header.tile as i32 * 10;
    let (mut edges, mut segments) = (0, 0);
    let mut by_class: BTreeMap<RoadClass, usize> = BTreeMap::new();
    let mut nearest_to_plaza = f64::MAX;
    for key in keys {
        let (south, west) = header.origin(key);
        let (south, west) = (south * 10, west * 10);
        for road in engine.roads(key).expect("every tile is cached") {
            let points: Vec<_> = road.points().collect();
            for &(lat, lon) in &points {
                assert!((south..=south + side).contains(&lat), "{key:?} {lat}");
                assert!((west..=west + side).contains(&lon), "{key:?} {lon}");
                nearest_to_plaza = nearest_to_plaza.min(metres(PLAZA, (lat, lon)));
            }
            edges += 1;
            segments += points.len() - 1;
            *by_class.entry(road.class()).or_default() += points.len() - 1;
        }
    }
    println!("segments by class: {by_class:?}");
    // `cycling terrain validate` counts 6,482 edges and 1,385 shape vertices.
    assert_eq!((edges, segments), (6_482, 6_482 + 1_385));
    assert!(nearest_to_plaza < 80.0, "{nearest_to_plaza:.0} m");
}

/// Host time to read every road of a tile and turn its points into metres
/// from the tile's corner, as a map's extraction does. A proxy for the
/// device only: run it with
/// `cargo test -p terrain --release --test santiago -- --ignored --nocapture`.
#[test]
#[ignore]
fn extraction_cost_per_tile() {
    const RUNS: usize = 200;
    let header = header();
    let mut arena = vec![0u8; 64 * PAGE_BYTES];
    let (engine, keys) = cached(&mut arena);
    println!("tile        edges  points  segments  median µs  min µs  max µs");
    for key in keys {
        let (south, west) = header.origin(key);
        let origin = (south * 10, west * 10);
        let lon_m = 0.011_119_5 * (f64::from(origin.0) * 1e-7).to_radians().cos() as f32;
        let mut times = Vec::with_capacity(RUNS);
        let (mut edges, mut points) = (0, 0);
        for _ in 0..RUNS {
            let started = Instant::now();
            let (mut sum, mut count, mut roads) = (0f32, 0usize, 0usize);
            for road in engine.roads(black_box(key)).unwrap() {
                roads += 1;
                for (lat, lon) in road.points() {
                    let north = (lat - origin.0) as f32 * 0.011_119_5;
                    let east = (lon - origin.1) as f32 * lon_m;
                    sum += north + east;
                    count += 1;
                }
            }
            black_box(sum);
            times.push(started.elapsed().as_secs_f64() * 1e6);
            (edges, points) = (roads, count);
        }
        times.sort_by(f64::total_cmp);
        println!(
            "{:>4},{:<4}  {edges:>6}  {points:>6}  {:>8}  {:>9.1}  {:>6.1}  {:>6.1}",
            key.row,
            key.col,
            points - edges,
            times[RUNS / 2],
            times[0],
            times[RUNS - 1],
        );
    }
}
