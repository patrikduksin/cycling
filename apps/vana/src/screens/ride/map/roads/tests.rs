use super::*;
use std::boxed::Box;
use std::println;
use std::vec::Vec;
use terrain::pack::HEADER_BYTES;
use terrain::{Header, Lookahead, PAGE_BYTES, Status};

/// The 3 x 3 tiles around Plaza de Armas. Data (c) OpenStreetMap
/// contributors (ODbL); see `packages/terrain/tests/data/README.md`.
pub(crate) const SANTIAGO: &[u8] =
    include_bytes!("../../../../../../../packages/terrain/tests/data/santiago-centro.terrain");
/// Plaza de Armas, (lat_e7, lon_e7).
pub(crate) const PLAZA: (i32, i32) = (-334_378_000, -706_505_000);

pub(crate) fn header() -> Header {
    Header::parse(SANTIAGO[..HEADER_BYTES].try_into().unwrap()).unwrap()
}

pub(crate) fn every_tile(header: &Header) -> Vec<TileKey> {
    (0..header.rows)
        .flat_map(|row| (0..header.cols).map(move |col| TileKey { row, col }))
        .collect()
}

/// A look-ahead with every tile of the fixture cached.
pub(crate) fn cached(arena: &mut [u8]) -> Lookahead<'_> {
    let mut engine = Lookahead::new(arena, 10_000.0);
    let mut source: &[u8] = SANTIAGO;
    engine.want(every_tile(&header()));
    let mut steps = 0;
    while engine.step(&mut source) == Status::Working {
        steps += 1;
        assert!(steps < 1_000);
    }
    engine
}

fn corner(header: &Header, key: TileKey) -> (i32, i32) {
    let (south, west) = header.origin(key);
    (south * 10, west * 10)
}

/// Hold every tile of the fixture; whether each fit whole.
fn extract_all(roads: &mut Roads, engine: &Lookahead, frame: RideFrame, detail: Detail) -> bool {
    let header = header();
    let keys = every_tile(&header);
    keys.iter().fold(true, |whole, &key| {
        let tile = engine.roads(key).expect("cached");
        roads.extract(frame, &keys, key, detail, corner(&header, key), tile) && whole
    })
}

fn plaza_frame() -> RideFrame {
    RideFrame::at(PLAZA)
}

#[test]
fn every_point_of_a_tile_is_within_half_a_pixel_of_a_held_road_of_its_class() {
    let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
    let engine = cached(&mut arena);
    let header = header();
    let frame = plaza_frame();
    let key = header.tiles_under(&[PLAZA; 4]).next().unwrap();
    let mut roads = Box::new(Roads::default());
    roads.extract(
        frame,
        &[key],
        key,
        Detail::Streets,
        corner(&header, key),
        engine.roads(key).unwrap(),
    );
    let held: Vec<(RoadClass, Vec<(f32, f32)>)> = roads
        .each(Detail::Streets, (0.0, 0.0))
        .map(|road| (road.class, road.points().collect()))
        .collect();
    let (edges, points) = engine
        .roads(key)
        .unwrap()
        .fold((0, 0), |(e, p), road| (e + 1, p + road.points().count()));
    let kept: usize = held.iter().map(|(_, road)| road.len()).sum();
    assert!(
        held.len() < edges * 3 / 4,
        "{} roads for {edges} edges",
        held.len()
    );
    assert!(kept < points * 3 / 4, "{kept} points for {points}");
    assert!(
        held.first().is_some_and(|(class, _)| class.major()),
        "majors are held first"
    );
    let tolerance = Detail::Streets.tolerance_m() + 0.06;
    for road in engine.roads(key).unwrap() {
        for (lat, lon) in road.points() {
            let p = frame.place(lat, lon);
            let near = held.iter().any(|(class, kept)| {
                *class == road.class()
                    && kept
                        .windows(2)
                        .any(|w| !strays_from(p, w[0], w[1], tolerance))
            });
            assert!(
                near,
                "{p:?} of a {:?} strays from every held road",
                road.class()
            );
        }
    }
}

fn room(roads: &mut Roads) -> Room<'_> {
    Room {
        roads,
        runs: 0,
        points: 0,
        full: false,
        corner: (-334_000_000, -707_000_000),
        scale: (0.093, 0.111),
        tolerance: 1.0,
        open: None,
        stretch: heapless::Vec::new(),
    }
}

fn at(x: i32, y: i32) -> (i32, i32) {
    (-334_000_000 + y * 10, -707_000_000 + x * 10)
}

#[test]
fn a_hairpin_keeps_its_tip_and_a_straight_road_only_its_ends() {
    let mut roads = Box::new(Roads::default());
    let mut room = room(&mut roads);
    room.road(RoadClass::Primary, (0..20).map(|i| at(i * 50, i * 50)));
    room.close();
    let hairpin = [
        (0, 0),
        (100, 400),
        (200, 800),
        (210, 820),
        (220, 800),
        (320, 400),
        (420, 0),
    ];
    room.road(
        RoadClass::Residential,
        hairpin.into_iter().map(|(x, y)| at(x, y)),
    );
    room.close();
    room.road(
        RoadClass::Residential,
        (0..100).map(|i| at(i * 10, if i % 2 == 0 { 0 } else { 100 })),
    );
    room.close();
    let ends: Vec<usize> = roads.runs[..3].iter().map(|r| usize::from(r.end)).collect();
    assert_eq!(ends[0], 2, "a straight road keeps its two ends");
    assert!(
        roads.points[ends[0]..ends[1]].contains(&(210, 820)),
        "the hairpin's tip is kept"
    );
    assert_eq!(
        ends[2] - ends[1],
        100,
        "a zigzag longer than a stretch keeps every corner"
    );
}

#[test]
fn a_road_that_carries_on_where_one_of_its_class_ended_joins_it() {
    let mut roads = Box::new(Roads::default());
    let mut room = room(&mut roads);
    for block in 0..5 {
        room.road(
            RoadClass::Residential,
            [at(block * 100, 0), at(block * 100 + 100, 0)].into_iter(),
        );
    }
    room.road(RoadClass::Tertiary, [at(500, 0), at(600, 0)].into_iter());
    room.road(RoadClass::Tertiary, [at(0, 300), at(100, 300)].into_iter());
    room.close();
    let runs = room.runs;
    let ends: Vec<usize> = roads.runs[..runs]
        .iter()
        .map(|r| usize::from(r.end))
        .collect();
    assert_eq!(
        ends,
        [2, 4, 6],
        "five straight blocks are one segment; another class or a gap is a new road"
    );
    assert_eq!(&roads.points[..2], &[(0, 0), (500, 0)]);
}

#[test]
fn central_santiago_fits_at_every_detail() {
    let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
    let engine = cached(&mut arena);
    let frame = plaza_frame();
    for detail in [Detail::Streets, Detail::Town, Detail::Arteries] {
        let mut roads = Box::new(Roads::default());
        let whole = extract_all(&mut roads, &engine, frame, detail);
        let (runs, points) = roads.ends();
        println!("{detail:?}: 9 tiles, {runs} roads, {points} points");
        assert_eq!(roads.tiles, 9);
        assert!(
            whole || detail == Detail::Streets,
            "{detail:?} holds the 3 x 3 tiles whole"
        );
    }
}

#[test]
fn tiles_out_of_the_view_make_room_and_a_new_detail_replaces_the_old() {
    let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
    let engine = cached(&mut arena);
    let header = header();
    let frame = plaza_frame();
    let keys = every_tile(&header);
    let mut roads = Box::new(Roads::default());
    extract_all(&mut roads, &engine, frame, Detail::Arteries);
    let count = |roads: &Roads| roads.each(Detail::Streets, (0.0, 0.0)).count();
    let all = count(&roads);
    let one = keys[4];
    let generation = roads.generation();
    roads.extract(
        frame,
        &[one],
        one,
        Detail::Town,
        corner(&header, one),
        engine.roads(one).unwrap(),
    );
    assert_eq!(roads.tiles, 1, "the tiles out of the view are let go");
    assert!(roads.generation() != generation);
    assert!(count(&roads) < all);
    assert_eq!(roads.lacking(&keys, Detail::Town, frame).count(), 8);
    assert_eq!(roads.lacking(&keys, Detail::Arteries, frame).count(), 9);
    let moved = RideFrame::at((PLAZA.0 + 1_000, PLAZA.1));
    assert_eq!(
        roads.lacking(&[one], Detail::Town, moved).count(),
        1,
        "a new ride's frame places them again"
    );
}

#[test]
fn letting_a_tile_go_keeps_the_others_roads_intact() {
    let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
    let engine = cached(&mut arena);
    let header = header();
    let frame = plaza_frame();
    let keys = every_tile(&header);
    let mut roads = Box::new(Roads::default());
    extract_all(&mut roads, &engine, frame, Detail::Town);
    let points = |roads: &Roads, key: TileKey| -> Vec<(f32, f32)> {
        let t = roads.held[..roads.tiles]
            .iter()
            .position(|h| h.key == Some(key))
            .unwrap();
        let corner = roads.held[t].corner;
        roads
            .each(Detail::Town, corner)
            .take_while(|road| core::ptr::eq(road.held, &roads.held[t]))
            .flat_map(|road| road.points().collect::<Vec<_>>())
            .collect()
    };
    let before = points(&roads, keys[8]);
    let kept: Vec<TileKey> = keys.iter().copied().filter(|&k| k != keys[2]).collect();
    roads.keep(|held| held.key.is_some_and(|k| kept.contains(&k)));
    assert_eq!(roads.tiles, 8);
    assert_eq!(points(&roads, keys[8]), before);
}
