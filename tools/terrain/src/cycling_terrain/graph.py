"""Road graph with elevations: OSM ways to junction-to-junction edges, each a
polyline in integer micro-degrees with an elevation per vertex, cut at tile
boundaries.

Elevation is decided here once, so the device never reads the DEM:

1. Each edge is densified to at most `STEP_M` between points and sampled
   from the DEM.
2. The samples are smoothed along the road (Gaussian, `SIGMA_M`), which
   removes most of the DEM's pixel noise without flattening real climbs. The
   ends are then pinned to their junction's elevation, so every edge that
   meets at a junction agrees on it.
3. Bridges and tunnels take a straight line between their ends: the DEM
   holds the valley or the mountain, not the deck.
4. The polyline is simplified in 3D: a point is dropped only while the
   simplified line stays within `HORIZONTAL_M` of it horizontally and
   `VERTICAL_M` vertically.
"""

from __future__ import annotations

import itertools
import math
from collections import defaultdict
from dataclasses import dataclass, field

import numpy as np

from . import osm
from .dem import Dem

STEP_M = 20.0
SIGMA_M = 30.0
HORIZONTAL_M = 3.0
VERTICAL_M = 1.0
# The spherical Earth the device also uses: metres per degree of latitude.
METRES_PER_DEGREE = 6_371_008.8 * math.pi / 180


@dataclass
class Edge:
    """A road between two junctions (or a junction and a dead end)."""

    lat: np.ndarray
    lon: np.ndarray
    z: np.ndarray  # metres
    road_class: int
    flags: int
    group: int


def split_at_junctions(ways: list[osm.Way]) -> list[tuple[osm.Way, int, int]]:
    """(way, first, last) index ranges between junctions: nodes shared by
    ways, used twice by one way, or ending a way."""
    refs = np.concatenate([way.refs for way in ways]) if ways else np.zeros(0, np.int64)
    unique, counts = np.unique(refs, return_counts=True)
    shared = set(unique[counts > 1].tolist())
    pieces = []
    for way in ways:
        cuts = [0]
        for index in range(1, len(way.refs) - 1):
            if int(way.refs[index]) in shared:
                cuts.append(index)
        cuts.append(len(way.refs) - 1)
        for first, last in itertools.pairwise(cuts):
            if last > first:
                pieces.append((way, first, last))
    return pieces


def local_metres(lat: np.ndarray, lon: np.ndarray) -> tuple[np.ndarray, np.ndarray]:
    scale = math.cos(math.radians(float(np.mean(lat))))
    return (lon - lon[0]) * METRES_PER_DEGREE * scale, (lat - lat[0]) * METRES_PER_DEGREE


def densify(lat: np.ndarray, lon: np.ndarray) -> tuple[np.ndarray, np.ndarray, np.ndarray, np.ndarray]:
    """Points at most STEP_M apart, a mask of the original vertices, and the
    distance along the line of every point."""
    x, y = local_metres(lat, lon)
    lengths = np.hypot(np.diff(x), np.diff(y))
    parts = np.maximum(1, np.ceil(lengths / STEP_M)).astype(np.int64)
    out_lat, out_lon, original = [lat[:1]], [lon[:1]], [np.array([True])]
    for index, count in enumerate(parts):
        t = np.arange(1, count + 1) / count
        out_lat.append(lat[index] + (lat[index + 1] - lat[index]) * t)
        out_lon.append(lon[index] + (lon[index + 1] - lon[index]) * t)
        mask = np.zeros(count, dtype=bool)
        mask[-1] = True
        original.append(mask)
    dense_lat = np.concatenate(out_lat)
    dense_lon = np.concatenate(out_lon)
    dx, dy = local_metres(dense_lat, dense_lon)
    distance = np.concatenate([[0.0], np.cumsum(np.hypot(np.diff(dx), np.diff(dy)))])
    return dense_lat, dense_lon, np.concatenate(original), distance


def smooth(z: np.ndarray, distance: np.ndarray, start: float, end: float) -> np.ndarray:
    """Gaussian smoothing along the road (on a uniform 5 m resampling, so it
    costs O(n)), then a linear correction that puts the ends exactly on the
    junction elevations."""
    total = distance[-1]
    if len(z) > 2 and total > 0:
        step = 5.0
        uniform = np.arange(0.0, total + step, step)
        values = np.interp(uniform, distance, z)
        reach = int(3 * SIGMA_M / step)
        kernel = np.exp(-0.5 * (np.arange(-reach, reach + 1) * step / SIGMA_M) ** 2)
        padded = np.pad(values, reach, mode='edge')
        weights = np.convolve(np.ones_like(padded), kernel, mode='valid')
        smoothed = np.convolve(padded, kernel, mode='valid') / weights
        z = np.interp(distance, uniform, smoothed)
    t = distance / (total if total > 0 else 1.0)
    return z + (start - z[0]) * (1 - t) + (end - z[-1]) * t


def simplify(x: np.ndarray, y: np.ndarray, z: np.ndarray) -> np.ndarray:
    """Indices kept by 3D Douglas-Peucker with separate horizontal and
    vertical tolerances."""
    keep = np.zeros(len(x), dtype=bool)
    keep[0] = keep[-1] = True
    stack = [(0, len(x) - 1)]
    while stack:
        first, last = stack.pop()
        if last - first < 2:
            continue
        ax, ay, az = x[first], y[first], z[first]
        bx, by, bz = x[last], y[last], z[last]
        px, py, pz = x[first + 1 : last], y[first + 1 : last], z[first + 1 : last]
        dx, dy = bx - ax, by - ay
        length2 = dx * dx + dy * dy
        if length2 == 0:
            t = np.zeros_like(px)
            horizontal = np.hypot(px - ax, py - ay)
        else:
            t = np.clip(((px - ax) * dx + (py - ay) * dy) / length2, 0, 1)
            horizontal = np.hypot(px - (ax + t * dx), py - (ay + t * dy))
        vertical = np.abs(pz - (az + t * (bz - az)))
        error = np.maximum(horizontal / HORIZONTAL_M, vertical / VERTICAL_M)
        worst = int(np.argmax(error))
        if error[worst] > 1:
            middle = first + 1 + worst
            keep[middle] = True
            stack.append((first, middle))
            stack.append((middle, last))
    return np.flatnonzero(keep)


def node_elevations(dem: Dem, lat: np.ndarray, lon: np.ndarray) -> np.ndarray:
    """A junction's elevation: the mean of five DEM samples 15 m around it,
    which averages away a single noisy pixel."""
    d_lat = 15 / METRES_PER_DEGREE
    d_lon = d_lat / np.cos(np.radians(lat))
    samples = [
        dem.sample(lat, lon),
        dem.sample(lat + d_lat, lon),
        dem.sample(lat - d_lat, lon),
        dem.sample(lat, lon + d_lon),
        dem.sample(lat, lon - d_lon),
    ]
    return np.mean(samples, axis=0)


def distinct_positions(ends: dict[int, tuple[float, float]]) -> dict[int, tuple[float, float]]:
    """Junction positions on the micro-degree grid the pack stores, one per
    OSM node. The pack joins edges by position, so two unconnected nodes that
    round to the same point would become a false junction; the later one moves
    a micro-degree (about 0.1 m) east until its point is free."""
    taken: dict[tuple[int, int], int] = {}
    out = {}
    for node, (lat, lon) in ends.items():
        y, x = round(lat * 1e6), round(lon * 1e6)
        while taken.get((y, x), node) != node:
            x += 1
        taken[(y, x)] = node
        out[node] = (y / 1e6, x / 1e6)
    return out


def build_edges(ways: list[osm.Way], dem: Dem) -> list[Edge]:
    pieces = split_at_junctions(ways)
    # Junction elevations, keyed by OSM node id, sampled in one batch.
    ends: dict[int, tuple[float, float]] = {}
    for way, first, last in pieces:
        for index in (first, last):
            ends[int(way.refs[index])] = (float(way.lat[index]), float(way.lon[index]))
    ends = distinct_positions(ends)
    ids = list(ends)
    coords = np.array([ends[i] for i in ids]) if ids else np.zeros((0, 2))
    heights = node_elevations(dem, coords[:, 0], coords[:, 1]) if ids else np.zeros(0)
    junction = dict(zip(ids, heights.tolist(), strict=True))

    dense = [densify(way.lat[first : last + 1], way.lon[first : last + 1]) for way, first, last in pieces]
    all_lat = np.concatenate([d[0] for d in dense]) if dense else np.zeros(0)
    all_lon = np.concatenate([d[1] for d in dense]) if dense else np.zeros(0)
    all_z = dem.sample(all_lat, all_lon)
    edges = []
    offset = 0
    for (way, first, last), (lat, lon, _original, distance) in zip(pieces, dense, strict=True):
        z = all_z[offset : offset + len(lat)]
        offset += len(lat)
        start = junction[int(way.refs[first])]
        end = junction[int(way.refs[last])]
        if way.flags & (osm.BRIDGE | osm.TUNNEL):
            total = distance[-1] if distance[-1] > 0 else 1.0
            z = start + (end - start) * distance / total
        else:
            z = smooth(z, distance, start, end)
        # Ends sit exactly on their junction's (distinct) position.
        lat[0], lon[0] = ends[int(way.refs[first])]
        lat[-1], lon[-1] = ends[int(way.refs[last])]
        x, y = local_metres(lat, lon)
        kept = simplify(x, y, z)
        edges.append(Edge(lat[kept], lon[kept], z[kept], way.road_class, way.flags, way.group))
    return edges


# --- Tiling -----------------------------------------------------------------


@dataclass
class Grid:
    """Tiles of `tile` micro-degrees from a south-west corner aligned to it."""

    tile: int
    south: int
    west: int
    rows: int
    cols: int

    @classmethod
    def around(cls, south: float, west: float, north: float, east: float, tile: int) -> Grid:
        s = math.floor(south * 1e6 / tile) * tile
        w = math.floor(west * 1e6 / tile) * tile
        n = math.ceil(north * 1e6 / tile) * tile
        e = math.ceil(east * 1e6 / tile) * tile
        return cls(tile, s, w, (n - s) // tile, (e - w) // tile)

    def bounds(self) -> tuple[float, float, float, float]:
        return (
            self.south / 1e6,
            self.west / 1e6,
            (self.south + self.rows * self.tile) / 1e6,
            (self.west + self.cols * self.tile) / 1e6,
        )

    def contains(self, row: int, col: int) -> bool:
        return 0 <= row < self.rows and 0 <= col < self.cols


@dataclass
class TileEdge:
    points: list[tuple[int, int, float]]  # global micro-degrees and metres
    road_class: int
    flags: int
    group: int


@dataclass
class Tiles:
    grid: Grid
    edges: dict[tuple[int, int], list[TileEdge]] = field(default_factory=lambda: defaultdict(list))


def _cut_segment(a: tuple[int, int, float], b: tuple[int, int, float], tile: int, south: int, west: int):
    """Points where segment a-b crosses tile boundaries, in order (excluding
    a, including b). Crossing points lie exactly on the grid line."""
    (ay, ax, az), (by, bx, bz) = a, b
    # Parameter along the segment -> the grid lines crossed there.
    crossings: dict[float, dict[int, int]] = defaultdict(dict)
    for axis, (p, q, origin) in enumerate(((ay, by, south), (ax, bx, west))):
        if p == q:
            continue
        low, high = sorted((p, q))
        # Lines strictly between the ends; an end on a line needs no cut.
        for k in range((low - origin) // tile + 1, -((origin - high) // tile)):
            line = origin + k * tile
            if low < line < high:
                crossings[(line - p) / (q - p)][axis] = line
    points = []
    for t in sorted(crossings):
        lines = crossings[t]
        y = lines.get(0, round(ay + (by - ay) * t))
        x = lines.get(1, round(ax + (bx - ax) * t))
        if (y, x) != (ay, ax) and (y, x) != (by, bx):
            points.append((y, x, az + (bz - az) * t))
    points.append(b)
    return points


def tile_of(point: tuple[int, int, float], grid: Grid) -> tuple[int, int]:
    return ((point[0] - grid.south) // grid.tile, (point[1] - grid.west) // grid.tile)


def cut_into_tiles(edges: list[Edge], grid: Grid) -> Tiles:
    tiles = Tiles(grid)
    for edge in edges:
        ys = np.round(edge.lat * 1e6).astype(np.int64).tolist()
        xs = np.round(edge.lon * 1e6).astype(np.int64).tolist()
        points = [(ys[0], xs[0], float(edge.z[0]))]
        for index in range(1, len(ys)):
            b = (ys[index], xs[index], float(edge.z[index]))
            if b[:2] == points[-1][:2]:
                continue
            points.extend(_cut_segment(points[-1], b, grid.tile, grid.south, grid.west))
        if len(points) < 2:
            continue
        piece = [points[0]]
        current = None
        for a, b in itertools.pairwise(points):
            # A sub-segment belongs to the tile holding its midpoint.
            mid = ((a[0] + b[0]) / 2, (a[1] + b[1]) / 2)
            key = (int((mid[0] - grid.south) // grid.tile), int((mid[1] - grid.west) // grid.tile))
            if current is not None and key != current:
                _emit(tiles, current, piece, edge)
                piece = [a]
            current = key
            piece.append(b)
        if current is not None:
            _emit(tiles, current, piece, edge)
    return tiles


def _emit(tiles: Tiles, key: tuple[int, int], piece: list, edge: Edge) -> None:
    if len({point[:2] for point in piece}) < 2:
        return
    if not tiles.grid.contains(*key):
        return
    tiles.edges[key].append(TileEdge(list(piece), edge.road_class, edge.flags, edge.group))
