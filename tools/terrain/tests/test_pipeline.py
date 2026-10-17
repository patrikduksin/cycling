"""The builder's rules on small inputs, and the committed sample pack."""

from pathlib import Path

import numpy as np
import pytest

from cycling_terrain import osm, pack, trace
from cycling_terrain.graph import Edge, Grid, _cut_segment, cut_into_tiles, distinct_positions, simplify, smooth

SAMPLE = Path(__file__).resolve().parents[3] / 'packages/terrain/tests/data/farellones.terrain'
TILE = 15_625


def test_classify_keeps_rideable_roads_with_their_flags():
    assert osm.classify({'highway': 'secondary'}) == (osm.CLASSES['secondary'], 0, False)
    assert osm.classify({'highway': 'motorway'}) is None
    assert osm.classify({'highway': 'footway'}) is None
    assert osm.classify({'highway': 'footway', 'bicycle': 'designated'}) is not None
    assert osm.classify({'highway': 'residential', 'access': 'private'}) is None
    assert osm.classify({'highway': 'service', 'service': 'parking_aisle'}) is None
    assert osm.classify({'highway': 'primary', 'motorroad': 'yes'}) is None
    _, flags, reverse = osm.classify({'highway': 'tertiary', 'oneway': '-1'})
    assert flags & osm.ONEWAY and reverse
    _, flags, reverse = osm.classify({'highway': 'tertiary', 'oneway': 'yes', 'oneway:bicycle': 'no'})
    assert not flags & osm.ONEWAY and not reverse
    _, flags, _ = osm.classify({'highway': 'track'})
    assert flags & osm.UNPAVED
    _, flags, _ = osm.classify({'highway': 'track', 'surface': 'asphalt'})
    assert not flags & osm.UNPAVED
    _, flags, _ = osm.classify({'highway': 'primary', 'junction': 'roundabout'})
    assert flags & osm.ROUNDABOUT and flags & osm.ONEWAY
    _, flags, _ = osm.classify({'highway': 'primary_link', 'bridge': 'yes', 'tunnel': 'no'})
    assert flags & osm.LINK and flags & osm.BRIDGE and not flags & osm.TUNNEL


def test_a_road_keeps_one_group_by_ref_then_name():
    assert osm.road_group({'ref': 'G-21', 'name': 'Camino a Farellones'}) == osm.road_group({'ref': ' g-21 '})
    assert osm.road_group({'ref': 'G-21'}) == trace.group_of('G-21')
    assert osm.road_group({}) == 0


def test_a_segment_is_cut_exactly_on_grid_lines():
    # From inside tile (0, 0) diagonally across the corner into (1, 1).
    a, b = (10_000, 10_000, 100.0), (20_000, 21_250, 200.0)
    points = _cut_segment(a, b, TILE, 0, 0)
    assert points[-1] == b
    inner = points[:-1]
    assert len(inner) == 2
    assert all(y == TILE or x == TILE for y, x, _ in inner)
    # Elevation is interpolated along the segment.
    assert all(100.0 < z < 200.0 for _, _, z in inner)
    # An end on a grid line needs no cut.
    assert _cut_segment((0, 5_000, 0.0), (TILE, 5_000, 0.0), TILE, 0, 0) == [(TILE, 5_000, 0.0)]


def edge(points, road_class=3, flags=0, group=7):
    lat = np.array([p[0] for p in points]) / 1e6
    lon = np.array([p[1] for p in points]) / 1e6
    return Edge(lat, lon, np.array([p[2] for p in points], dtype=float), road_class, flags, group)


def test_roads_across_tiles_meet_at_matching_portals():
    grid = Grid.around(-33.4, -70.5, -33.35, -70.44, TILE)
    south, west = grid.south, grid.west
    roads = [
        # West to east across two boundaries, and a diagonal through a corner.
        edge([(south + 7_000, west + 3_000, 900.0), (south + 7_100, west + 40_000, 950.0)]),
        edge([(south + 1_000, west + 1_000, 880.0), (south + 30_000, west + 30_000, 990.0)], group=9),
    ]
    tiles = cut_into_tiles(roads, grid)
    blob = pack.encode_pack(tiles, 'test attribution', 'test', 0)
    stats = pack.validate(blob)
    assert stats['tiles'] >= 4
    assert stats['portals'] >= 4
    assert stats['unmatched_portals'] == 0
    header = pack.read_header(blob)
    assert header.attribution.startswith('test attribution')


def test_a_corrupt_tile_is_rejected():
    grid = Grid.around(-33.4, -70.5, -33.39, -70.49, TILE)
    tiles = cut_into_tiles(
        [edge([(grid.south + 100, grid.west + 100, 1.0), (grid.south + 900, grid.west + 900, 2.0)])], grid
    )
    blob = bytearray(pack.encode_pack(tiles, 'a', 'b', 0))
    blob[-3] ^= 0xFF
    with pytest.raises(ValueError):
        pack.validate(bytes(blob))


def test_smoothing_pins_the_ends_and_simplifying_drops_straight_points():
    distance = np.arange(0.0, 1000.0, 20.0)
    noisy = distance * 0.05 + np.where(np.arange(len(distance)) % 2, 3.0, -3.0)
    z = smooth(noisy, distance, 10.0, 60.0)
    assert z[0] == pytest.approx(10.0) and z[-1] == pytest.approx(60.0)
    assert np.abs(np.diff(z)).max() < 2.0
    x = distance
    y = np.zeros_like(distance)
    assert simplify(x, y, distance * 0.05).tolist() == [0, len(distance) - 1]
    # A 5 m bump is kept.
    bump = distance * 0.05
    bump[25] += 5.0
    assert 25 in simplify(x, y, bump).tolist()


def test_the_sample_pack_validates_and_holds_g21():
    blob = SAMPLE.read_bytes()
    stats = pack.validate(blob)
    assert stats['unmatched_portals'] == 0
    assert stats['max_tile_bytes'] <= pack.MAX_TILE_BYTES
    header = pack.read_header(blob)
    assert 'OpenStreetMap contributors' in header.attribution
    assert 'Copernicus' in header.attribution
    road = trace.road(blob, 'G-21')
    gain = (road[-1][2] - road[0][2]) / 10
    assert gain > 1500


def test_unconnected_nodes_at_one_rounded_point_stay_apart():
    # Two OSM nodes 1 cm apart round to the same micro-degree.
    ends = {1: (-33.4000001, -70.5000001), 2: (-33.4000000, -70.5000000), 3: (-33.41, -70.51)}
    positions = distinct_positions(ends)
    assert len({(round(a * 1e6), round(b * 1e6)) for a, b in positions.values()}) == 3
    assert positions[3] == (-33.41, -70.51)


def test_a_way_crossing_the_bbox_without_a_node_inside_is_kept():
    bbox = (0.0, 0.0, 0.015625, 0.015625)
    assert osm.overlaps(bbox, np.array([0.005, 0.005]), np.array([-0.01, 0.03]))
    assert not osm.overlaps(bbox, np.array([0.02, 0.03]), np.array([-0.01, 0.03]))


def test_elevations_a_pack_cannot_hold_are_refused():
    assert pack.decode_z(pack.encode_z(6_108.0)) == pytest.approx(6_108.0)
    assert pack.decode_z(pack.encode_z(-150.0)) == pytest.approx(-150.0)
    for metres in (6_400.0, -250.0):
        with pytest.raises(ValueError):
            pack.encode_z(metres)


def test_trace_joins_the_two_longest_arms_of_a_branching_road():
    # A three-arm junction at the tile middle; the short arm comes first.
    grid = Grid.around(-33.4, -70.5, -33.39, -70.49, TILE)
    middle = (grid.south + 7_800, grid.west + 7_800)
    arms = [(middle[0], middle[1] + 500), (middle[0] - 6_000, middle[1]), (middle[0] + 6_000, middle[1])]
    roads = [edge([(*arm, 900.0 + n), (*middle, 950.0)], group=trace.group_of('G-1')) for n, arm in enumerate(arms)]
    blob = pack.encode_pack(cut_into_tiles(roads, grid), 'a', 'b', 0)
    road = trace.road(blob, 'G-1')
    lats = [lat for lat, _, _ in road]
    assert max(lats) - min(lats) == 12_000


def test_trace_rides_oneways_forwards_and_goes_uphill():
    grid = Grid.around(-33.4, -70.5, -33.39, -70.49, TILE)
    south, west = grid.south + 1_000, grid.west + 1_000
    group = trace.group_of('G-2')
    roads = [
        # Up 100 m north, then a oneway that only runs back down south.
        edge([(south, west, 900.0), (south + 3_000, west, 1_000.0)], group=group),
        edge([(south + 9_000, west, 1_200.0), (south + 3_000, west, 1_000.0)], flags=osm.ONEWAY, group=group),
    ]
    road = trace.road(pack.encode_pack(cut_into_tiles(roads, grid), 'a', 'b', 0), 'G-2')
    assert road[0][2] < road[-1][2]
    assert max(lat for lat, _, _ in road) == south + 3_000
