//! Synthetic road layouts, encoded the way `tools/terrain` encodes packs,
//! for the junction rules the sample pack cannot isolate.
use crate::pack::{HEADER_BYTES, MAGIC};
use crate::tile::{class, crc32, flag};
use crate::{End, Fix, Lookahead, PAGE_BYTES, Status};
use std::vec;
use std::vec::Vec;

const TILE: i32 = 15_625;
/// The grid's south-west corner: one tile near Santiago.
const SOUTH: i32 = -33_406_250;
const WEST: i32 = -70_484_375;
const LON_UM: f64 = 92_826.0;
/// Synthetic roads are drawn in metres from the middle of the tile.
const MIDDLE: (f64, f64) = (TILE as f64 / 2.0, TILE as f64 / 2.0);

#[derive(Clone)]
struct Road {
    points: Vec<(f64, f64, f64)>,
    class: u8,
    flags: u8,
    group: u16,
}

fn road(points: &[(f64, f64, f64)], class: u8, group: u16) -> Road {
    Road {
        points: points.to_vec(),
        class,
        flags: 0,
        group,
    }
}

/// Metres east/north of the tile middle to tile-local micro-degrees.
fn local(east: f64, north: f64) -> (u16, u16) {
    let x = MIDDLE.0 + east / (LON_UM * 1e-6);
    let y = MIDDLE.1 + north / 0.111_195;
    (x.round() as u16, y.round() as u16)
}

fn z(metres: f64) -> u16 {
    (metres * 10.0).round() as u16 + 2000
}

/// A one-tile pack (1 x 1 grid) holding `roads`; junctions are shared ends.
fn pack(roads: &[Road]) -> Vec<u8> {
    let mut nodes: Vec<((u16, u16), u16)> = Vec::new();
    for road in roads {
        for &(e, n, h) in [road.points[0], *road.points.last().unwrap()].iter() {
            let key = local(e, n);
            if !nodes.iter().any(|(k, _)| *k == key) {
                nodes.push((key, z(h)));
            }
        }
    }
    nodes.sort_by_key(|((x, y), _)| (*y, *x));
    let index = |key: (u16, u16)| nodes.iter().position(|(k, _)| *k == key).unwrap() as u16;
    let mut incident: Vec<Vec<u16>> = vec![Vec::new(); nodes.len()];
    let mut edges = Vec::new();
    let mut vertices = Vec::new();
    for (number, road) in roads.iter().enumerate() {
        let first = road.points[0];
        let last = *road.points.last().unwrap();
        let from = index(local(first.0, first.1));
        let to = index(local(last.0, last.1));
        incident[usize::from(from)].push((number as u16) << 1);
        incident[usize::from(to)].push((number as u16) << 1 | 1);
        let inner = &road.points[1..road.points.len() - 1];
        edges.extend_from_slice(&from.to_le_bytes());
        edges.extend_from_slice(&to.to_le_bytes());
        edges.extend_from_slice(&((vertices.len() / 6) as u32).to_le_bytes());
        edges.extend_from_slice(&(inner.len() as u16).to_le_bytes());
        edges.extend_from_slice(&[road.class, road.flags]);
        edges.extend_from_slice(&road.group.to_le_bytes());
        edges.extend_from_slice(&[0, 0]);
        for &(e, n, h) in inner {
            let (x, y) = local(e, n);
            vertices.extend_from_slice(&x.to_le_bytes());
            vertices.extend_from_slice(&y.to_le_bytes());
            vertices.extend_from_slice(&z(h).to_le_bytes());
        }
    }
    let mut node_bytes = Vec::new();
    let mut adjacency = Vec::new();
    for (i, ((x, y), height)) in nodes.iter().enumerate() {
        node_bytes.extend_from_slice(&x.to_le_bytes());
        node_bytes.extend_from_slice(&y.to_le_bytes());
        node_bytes.extend_from_slice(&height.to_le_bytes());
        node_bytes.extend_from_slice(&((adjacency.len() / 2) as u16).to_le_bytes());
        for entry in &incident[i] {
            adjacency.extend_from_slice(&entry.to_le_bytes());
        }
    }
    let mut tile = vec![0u8; 32];
    tile[..4].copy_from_slice(&[b'V', b'T', 1, 0]);
    tile[8..10].copy_from_slice(&(nodes.len() as u16).to_le_bytes());
    tile[10..12].copy_from_slice(&(roads.len() as u16).to_le_bytes());
    tile[12..16].copy_from_slice(&((vertices.len() / 6) as u32).to_le_bytes());
    tile[16..20].copy_from_slice(&(LON_UM as u32).to_le_bytes());
    tile.extend(node_bytes);
    tile.extend(adjacency);
    tile.extend(edges);
    tile.extend(vertices);
    let crc = crc32(&tile);
    tile[24..28].copy_from_slice(&crc.to_le_bytes());

    let directory = HEADER_BYTES as u32;
    let data = directory + 8;
    let mut out = vec![0u8; HEADER_BYTES];
    out[..16].copy_from_slice(MAGIC);
    out[16..18].copy_from_slice(&1u16.to_le_bytes());
    out[18..20].copy_from_slice(&1u16.to_le_bytes());
    out[20..24].copy_from_slice(&(TILE as u32).to_le_bytes());
    out[24..28].copy_from_slice(&SOUTH.to_le_bytes());
    out[28..32].copy_from_slice(&WEST.to_le_bytes());
    out[32..34].copy_from_slice(&1u16.to_le_bytes());
    out[34..36].copy_from_slice(&1u16.to_le_bytes());
    out[36..40].copy_from_slice(&directory.to_le_bytes());
    out[40..44].copy_from_slice(&data.to_le_bytes());
    out[44..48].copy_from_slice(&(data + tile.len() as u32).to_le_bytes());
    out[48..52].copy_from_slice(&1u32.to_le_bytes());
    let crc = crc32(&out[..HEADER_BYTES - 4]);
    out[HEADER_BYTES - 4..].copy_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&data.to_le_bytes());
    out.extend_from_slice(&(tile.len() as u32).to_le_bytes());
    out.extend(tile);
    out
}

/// Ride `path` (metres) at 5 m/s with a fix a second; the last result.
fn ride(pack: &[u8], path: &[(f64, f64)]) -> crate::Ahead {
    ride_to(pack, path, 10_000.0)
}

/// `ride`, looking `horizon_m` ahead.
fn ride_to(pack: &[u8], path: &[(f64, f64)], horizon_m: f32) -> crate::Ahead {
    let mut arena = vec![0u8; 4 * PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, horizon_m);
    let mut source: &[u8] = pack;
    let mut second = 0;
    for pair in path.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        let length = ((b.0 - a.0).powi(2) + (b.1 - a.1).powi(2)).sqrt();
        let mut at = 0.0;
        while at < length {
            let t = at / length;
            let (x, y) = local(a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            ahead.fix(Fix {
                lat: SOUTH + i32::from(y),
                lon: WEST + i32::from(x),
                at_ms: second * 1000,
            });
            let mut steps = 0;
            while ahead.step(&mut source) == Status::Working {
                steps += 1;
                assert!(steps < 500);
            }
            second += 1;
            at += 5.0;
        }
    }
    ahead.ahead().clone()
}

/// The `n`th sample, in metres.
fn metres_at(ahead: &crate::Ahead, metres: usize) -> f64 {
    f64::from(ahead.samples[metres / 20]) / 10.0
}

#[test]
fn a_straight_road_through_a_crossing_is_followed_to_its_end() {
    // A road north for 1 km at 5%, crossed by a residential street at 300 m.
    let roads = [
        road(
            &[(0.0, -400.0, 100.0), (0.0, -100.0, 115.0)],
            class::SECONDARY,
            7,
        ),
        road(
            &[(0.0, -100.0, 115.0), (0.0, 600.0, 150.0)],
            class::SECONDARY,
            7,
        ),
        road(
            &[(-300.0, -100.0, 115.0), (0.0, -100.0, 115.0)],
            class::RESIDENTIAL,
            0,
        ),
        road(
            &[(0.0, -100.0, 115.0), (300.0, -100.0, 115.0)],
            class::RESIDENTIAL,
            0,
        ),
    ];
    let pack = pack(&roads);
    let ahead = ride(&pack, &[(0.0, -400.0), (0.0, -300.0)]);
    let matched = ahead.matched.unwrap();
    assert_eq!(matched.forward, Some(true));
    assert_eq!(ahead.end, End::DeadEnd);
    // 900 m from about 100 m in to the road's end.
    assert!((880.0..920.0).contains(&ahead.end_m), "{}", ahead.end_m);
    // 5% all the way: +10 m at 200 m, +45 m at 900 m.
    assert!((metres_at(&ahead, 200) - 10.0).abs() < 0.6);
    assert!((metres_at(&ahead, 880) - 44.0).abs() < 0.6);
    assert!(ahead.confidence > 80, "{}", ahead.confidence);
}

#[test]
fn an_even_split_is_a_fork_not_a_guess() {
    // Two tertiary roads leave at 20 degrees either side.
    let fork = (0.0, 0.0, 120.0);
    let roads = [
        road(&[(0.0, -500.0, 100.0), fork], class::TERTIARY, 0),
        road(&[fork, (171.0, 470.0, 200.0)], class::TERTIARY, 0),
        road(&[fork, (-171.0, 470.0, 120.0)], class::TERTIARY, 0),
    ];
    let pack = pack(&roads);
    let ahead = ride(&pack, &[(0.0, -500.0), (0.0, -400.0)]);
    assert_eq!(ahead.end, End::Fork);
    assert!((380.0..420.0).contains(&ahead.end_m), "{}", ahead.end_m);
}

#[test]
fn the_named_road_wins_over_a_straighter_side_road() {
    // G-21-like: the numbered road bends left 80 degrees at a junction where
    // an unclassified road carries straight on.
    let bend = (0.0, 0.0, 100.0);
    let roads = [
        road(&[(0.0, -500.0, 80.0), bend], class::SECONDARY, 21),
        road(&[bend, (-490.0, 86.0, 160.0)], class::SECONDARY, 21),
        road(&[bend, (0.0, 500.0, 90.0)], class::UNCLASSIFIED, 0),
    ];
    let pack = pack(&roads);
    let ahead = ride(&pack, &[(0.0, -500.0), (0.0, -400.0)]);
    // Climbing 60 m over 500 m after the bend, not dropping 10 m.
    assert_eq!(ahead.end, End::DeadEnd);
    assert!(
        metres_at(&ahead, 880) > 40.0,
        "{:?}",
        &ahead.samples[..ahead.len]
    );
}

#[test]
fn a_oneway_against_the_rider_is_never_the_way_ahead() {
    let junction = (0.0, 0.0, 100.0);
    let mut against = road(&[(0.0, 500.0, 100.0), junction], class::SECONDARY, 5);
    against.flags = flag::ONEWAY;
    let roads = [
        road(&[(0.0, -500.0, 100.0), junction], class::SECONDARY, 5),
        against,
        road(&[junction, (500.0, 0.0, 130.0)], class::RESIDENTIAL, 0),
    ];
    let pack = pack(&roads);
    let ahead = ride(&pack, &[(0.0, -500.0), (0.0, -400.0)]);
    // The only legal way is the right turn, up 30 m.
    assert!(
        metres_at(&ahead, 880) > 25.0,
        "{:?}",
        &ahead.samples[..ahead.len]
    );
}

#[test]
fn a_roundabout_is_crossed_to_the_exit_straight_ahead() {
    // A square roundabout of 30 m with four arms; the north arm climbs.
    let c = |e: f64, n: f64| (e, n, 100.0);
    let ring = |a: (f64, f64, f64), b: (f64, f64, f64)| {
        let mut r = road(&[a, b], class::TERTIARY, 0);
        r.flags = flag::ROUNDABOUT | flag::ONEWAY;
        r
    };
    let (s, e, n, w) = (c(0.0, -15.0), c(15.0, 0.0), c(0.0, 15.0), c(-15.0, 0.0));
    let roads = [
        road(&[(0.0, -500.0, 100.0), s], class::TERTIARY, 9),
        // Counter-clockwise, as in Chile.
        ring(s, e),
        ring(e, n),
        ring(n, w),
        ring(w, s),
        road(&[e, (500.0, 0.0, 100.0)], class::TERTIARY, 0),
        road(&[n, (0.0, 500.0, 150.0)], class::TERTIARY, 0),
        road(&[w, (-500.0, 0.0, 100.0)], class::TERTIARY, 0),
    ];
    let pack = pack(&roads);
    let ahead = ride(&pack, &[(0.0, -500.0), (0.0, -400.0)]);
    assert_eq!(ahead.end, End::DeadEnd, "{:?}", ahead);
    assert!(
        metres_at(&ahead, 800) > 30.0,
        "{:?}",
        &ahead.samples[..ahead.len]
    );
}

#[test]
fn a_parallel_road_does_not_steal_the_match() {
    // Two parallel roads 25 m apart; the rider rides the west one, with
    // fixes drifting 15 m east for a few seconds.
    let roads = [
        road(
            &[(-12.0, -600.0, 100.0), (-12.0, 600.0, 100.0)],
            class::SECONDARY,
            1,
        ),
        road(
            &[(13.0, -600.0, 100.0), (13.0, 600.0, 160.0)],
            class::SECONDARY,
            2,
        ),
    ];
    let pack = pack(&roads);
    let mut arena = vec![0u8; 2 * PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, 10_000.0);
    let mut source: &[u8] = &pack;
    for second in 0..60u64 {
        let north = -500.0 + second as f64 * 5.0;
        let east = if (30..33).contains(&second) {
            8.0
        } else {
            -12.0
        };
        let (x, y) = local(east, north);
        ahead.fix(Fix {
            lat: SOUTH + i32::from(y),
            lon: WEST + i32::from(x),
            at_ms: second * 1000,
        });
        while ahead.step(&mut source) == Status::Working {}
        if second > 3 {
            let flat = ahead.ahead().samples[..ahead.ahead().len]
                .iter()
                .all(|s| *s == 0);
            assert!(flat, "switched roads at {second}");
        }
    }
}

#[test]
fn no_pack_and_off_the_map_say_so() {
    let mut arena = vec![0u8; PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, 10_000.0);
    let mut garbage: &[u8] = &[0u8; 1024];
    assert_eq!(ahead.step(&mut garbage), Status::NoPack);

    let pack = pack(&[road(
        &[(0.0, 0.0, 0.0), (0.0, 100.0, 0.0)],
        class::PRIMARY,
        0,
    )]);
    let mut arena = vec![0u8; PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, 10_000.0);
    let mut source: &[u8] = &pack;
    ahead.fix(Fix::from_e7(-500_000_000, -700_000_000, 0));
    let mut status = Status::Working;
    while status == Status::Working {
        status = ahead.step(&mut source);
    }
    assert_eq!(status, Status::OffMap);
    assert!(!ahead.ahead().on_road());
}

#[test]
fn a_corrupt_tile_is_off_the_map_not_a_crash() {
    let mut pack = pack(&[road(
        &[(0.0, 0.0, 0.0), (0.0, 300.0, 0.0)],
        class::PRIMARY,
        0,
    )]);
    let last = pack.len() - 1;
    pack[last] ^= 0xff;
    let ahead = ride(&pack, &[(0.0, 0.0), (0.0, 100.0)]);
    assert!(ahead.matched.is_none());
}

#[test]
fn a_long_last_segment_still_gives_the_heading_into_a_junction() {
    // East along one 300 m segment; on east 700 m, or a branch north.
    let roads = [
        road(
            &[(-400.0, 0.0, 100.0), (-100.0, 0.0, 100.0)],
            class::TERTIARY,
            0,
        ),
        road(
            &[(-100.0, 0.0, 100.0), (600.0, 0.0, 135.0)],
            class::TERTIARY,
            0,
        ),
        road(
            &[(-100.0, 0.0, 100.0), (-100.0, 300.0, 100.0)],
            class::TERTIARY,
            0,
        ),
    ];
    let ahead = ride(&pack(&roads), &[(-400.0, 0.0), (-300.0, 0.0)]);
    assert_eq!(ahead.end, End::DeadEnd);
    // To the eastern end, about 900 m on: not 500 m up the branch north.
    assert!((880.0..920.0).contains(&ahead.end_m), "{}", ahead.end_m);
    assert!((metres_at(&ahead, 800) - 30.0).abs() < 1.0);
}

#[test]
fn the_profile_stops_at_the_horizon_inside_a_segment() {
    let roads = [road(
        &[(0.0, -400.0, 100.0), (0.0, 600.0, 200.0)],
        class::SECONDARY,
        7,
    )];
    let ahead = ride_to(&pack(&roads), &[(0.0, -400.0), (0.0, -300.0)], 100.0);
    assert_eq!(ahead.end, End::Horizon);
    assert!((ahead.end_m - 100.0).abs() < 0.01, "{}", ahead.end_m);
    // Samples at 0, 20 .. 100 m, the last +10 m on a 10% road.
    assert_eq!(ahead.len, 6);
    assert!((metres_at(&ahead, 100) - 10.0).abs() < 0.2);
}

#[test]
fn a_reset_matches_the_standing_fix_again() {
    let roads = [road(
        &[(0.0, -400.0, 100.0), (0.0, 600.0, 200.0)],
        class::SECONDARY,
        7,
    )];
    let pack = pack(&roads);
    let mut arena = vec![0u8; 4 * PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, 10_000.0);
    let mut source: &[u8] = &pack;
    for second in 0..10u64 {
        let (x, y) = local(0.0, -400.0 + second as f64 * 5.0);
        ahead.fix(Fix {
            lat: SOUTH + i32::from(y),
            lon: WEST + i32::from(x),
            at_ms: second * 1000,
        });
        while ahead.step(&mut source) == Status::Working {}
    }
    assert!(ahead.ahead().matched.is_some());
    ahead.reset();
    assert!(ahead.ahead().matched.is_none());
    // The rider stands still: no new fix, yet the road comes back.
    let mut steps = 0;
    while ahead.step(&mut source) == Status::Working {
        steps += 1;
        assert!(steps < 100);
    }
    assert!(ahead.ahead().matched.is_some());
}

/// Reads that fail every `every`th time.
struct Flaky<'p> {
    pack: &'p [u8],
    reads: u32,
    every: u32,
}

impl crate::Source for Flaky<'_> {
    fn read(&mut self, offset: u64, buf: &mut [u8]) -> Result<(), crate::Error> {
        self.reads += 1;
        if self.reads.is_multiple_of(self.every) {
            return Err(crate::Error::Media);
        }
        let mut pack = self.pack;
        pack.read(offset, buf)
    }
}

#[test]
fn isolated_read_errors_never_add_up_to_no_pack() {
    let roads = [road(
        &[(0.0, -400.0, 100.0), (0.0, 600.0, 200.0)],
        class::SECONDARY,
        7,
    )];
    let pack = pack(&roads);
    let mut arena = vec![0u8; 4 * PAGE_BYTES];
    let mut ahead = Lookahead::new(&mut arena, 10_000.0);
    let mut source = Flaky {
        pack: &pack,
        reads: 0,
        every: 3,
    };
    for second in 0..40u64 {
        // Forget the tile now and then, so it loads again through errors.
        if second % 4 == 0 {
            ahead.reset();
        }
        let (x, y) = local(0.0, -400.0 + second as f64 * 5.0);
        ahead.fix(Fix {
            lat: SOUTH + i32::from(y),
            lon: WEST + i32::from(x),
            at_ms: second * 1000,
        });
        let mut status = Status::Working;
        let mut steps = 0;
        while status == Status::Working {
            status = ahead.step(&mut source);
            steps += 1;
            assert!(steps < 500);
        }
        assert_ne!(status, Status::NoPack, "second {second}");
    }
    assert!(source.reads > 30);
    assert!(ahead.ahead().matched.is_some());
}
