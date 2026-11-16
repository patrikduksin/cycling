use super::*;
use crate::screens::ride::map::Zoom;
use std::boxed::Box;
use std::println;
use std::vec::Vec;
use terrain::pack::HEADER_BYTES;
use terrain::{Header, Lookahead, PAGE_BYTES, PackId, Status};

/// The 3 x 3 tiles around Plaza de Armas. Data (c) OpenStreetMap
/// contributors (ODbL); see `packages/terrain/tests/data/README.md`.
pub(crate) const SANTIAGO: &[u8] =
    include_bytes!("../../../../../../../packages/terrain/tests/data/santiago-centro.terrain");
pub(crate) const PLAZA: (i32, i32) = (-334_378_000, -706_505_000);

pub(crate) fn header() -> Header {
    Header::parse(SANTIAGO[..HEADER_BYTES].try_into().unwrap()).unwrap()
}

pub(crate) fn every_tile(header: &Header) -> Vec<TileKey> {
    (0..header.rows)
        .flat_map(|row| (0..header.cols).map(move |col| TileKey { row, col }))
        .collect()
}

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

fn extract_all(roads: &mut Roads, engine: &Lookahead, detail: Detail) -> bool {
    let header = header();
    let keys = every_tile(&header);
    keys.iter().fold(true, |whole, &key| {
        let tile = engine.roads(key).expect("cached");
        roads.extract(&keys, key, detail, corner(&header, key), tile) && whole
    })
}

fn plaza_frame() -> RideFrame {
    RideFrame::at(PLAZA)
}

fn roads_in(frame: RideFrame) -> Box<Roads> {
    let mut roads = Box::new(Roads::default());
    roads.hold(Some(Source {
        pack: PackId::default(),
        frame,
    }));
    roads
}

#[test]
fn every_point_of_a_tile_is_within_half_a_pixel_of_a_held_road_of_its_class() {
    let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
    let engine = cached(&mut arena);
    let header = header();
    let frame = plaza_frame();
    let key = header.tiles_under(&[PLAZA; 4]).next().unwrap();
    let mut roads = roads_in(frame);
    roads.extract(
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
    let tolerance = Detail::Streets.half_pixel_m() + 0.06;
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
        corner_e7: (-334_000_000, -707_000_000),
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
    for detail in [Detail::Streets, Detail::Town] {
        let mut roads = roads_in(plaza_frame());
        let whole = extract_all(&mut roads, &engine, detail);
        let (runs, points) = roads.ends();
        println!("{detail:?}: 9 tiles, {runs} roads, {points} points");
        assert_eq!(roads.tiles, 9);
        assert_eq!(
            whole,
            detail != Detail::Streets,
            "{detail:?} holds the 3 x 3 tiles whole, but for some of their streets"
        );
    }
}

#[test]
fn tiles_out_of_the_view_make_room_and_a_new_detail_replaces_the_old() {
    let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
    let engine = cached(&mut arena);
    let header = header();
    let keys = every_tile(&header);
    let mut roads = roads_in(plaza_frame());
    extract_all(&mut roads, &engine, Detail::Streets);
    let count = |roads: &Roads| roads.each(Detail::Streets, (0.0, 0.0)).count();
    let all = count(&roads);
    let one = keys[4];
    let generation = roads.generation();
    roads.extract(
        &[one],
        one,
        Detail::Town,
        corner(&header, one),
        engine.roads(one).unwrap(),
    );
    assert_eq!(roads.tiles, 1, "the tiles out of the view are let go");
    assert!(roads.generation() != generation);
    assert!(count(&roads) < all);
    assert_eq!(roads.lacking(&keys, Detail::Town).count(), 8);
    assert_eq!(roads.lacking(&keys, Detail::Streets).count(), 9);
    let generation = roads.generation();
    roads.hold(Some(Source {
        pack: PackId::default(),
        frame: RideFrame::at((PLAZA.0 + 1_000, PLAZA.1)),
    }));
    assert_eq!(
        roads.lacking(&[one], Detail::Town).count(),
        1,
        "a new ride's frame places them again"
    );
    assert!(roads.generation() != generation, "and the map fills again");
}

#[test]
fn letting_a_tile_go_keeps_the_others_roads_intact() {
    let mut arena = std::vec![0u8; 64 * PAGE_BYTES];
    let engine = cached(&mut arena);
    let header = header();
    let keys = every_tile(&header);
    let mut roads = roads_in(plaza_frame());
    extract_all(&mut roads, &engine, Detail::Town);
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

fn details_riding_at(zoom: &mut Zoom, speeds_mm_s: &[u32]) -> Vec<Detail> {
    speeds_mm_s
        .iter()
        .map(|&speed| {
            zoom.observe_riding_at(speed);
            zoom.reach().detail()
        })
        .collect()
}

fn switches(details: &[Detail]) -> usize {
    details.windows(2).filter(|pair| pair[0] != pair[1]).count()
}

fn noisy(mean_mm_s: u32, spread_mm_s: u32, fixes: usize) -> Vec<u32> {
    let mut seed = 7u32;
    (0..fixes)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            mean_mm_s - spread_mm_s + (seed >> 8) % (2 * spread_mm_s + 1)
        })
        .collect()
}

#[test]
fn speed_noise_near_the_switch_holds_the_detail() {
    let alternating: Vec<u32> = (0..600).map(|i| [12_500, 14_000][i % 2]).collect();
    for riding in [noisy(13_200, 300, 600), alternating] {
        for coming_from in [10_000, 20_000] {
            let mut zoom = Zoom::default();
            details_riding_at(&mut zoom, &[coming_from; 60]);
            details_riding_at(&mut zoom, &riding[..60]);
            let settled = details_riding_at(&mut zoom, &riding[60..]);
            assert_eq!(
                switches(&settled),
                0,
                "from {coming_from} mm/s: {:?}",
                &settled[..20]
            );
        }
    }
}

#[test]
fn a_change_of_speed_across_the_switch_changes_the_detail_once() {
    let mut zoom = Zoom::default();
    details_riding_at(&mut zoom, &[10_000; 60]);
    let faster = details_riding_at(&mut zoom, &[16_000; 60]);
    assert_eq!((switches(&faster), faster[59]), (1, Detail::Town));
    let slower = details_riding_at(&mut zoom, &[10_000; 60]);
    assert_eq!((switches(&slower), slower[59]), (1, Detail::Streets));
}
