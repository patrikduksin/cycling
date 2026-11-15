"""A course along a pack's streets, as GPX: map scenarios and benches.

From the junction nearest a start point the walk keeps straight on and,
every leg of a few hundred metres, turns onto a cross street. It never rides
an edge twice, a oneway backwards or a road outside `classes`, and it avoids
roads that lead nowhere new. The seed picks the legs and turns, so the same
arguments give the same course. The course is synthetic; nobody rode it.
"""

from __future__ import annotations

import itertools
import math
import random
from dataclasses import dataclass

from . import pack
from .osm import ONEWAY, RESIDENTIAL

# How far along a road its direction is read, so a kink at a junction does
# not count as a turn.
BEARING_M = 20.0
STRAIGHT_DEG = 40.0
TURN_DEG = (50.0, 130.0)


@dataclass
class Road:
    points: list[tuple[int, int, int]]  # lat µdeg, lon µdeg, z dm
    oneway: bool


def _east_north(a, b) -> tuple[float, float]:
    north = (b[0] - a[0]) * 0.111195
    east = (b[1] - a[1]) * 0.111195 * math.cos(math.radians(a[0] * 1e-6))
    return east, north


def _metres(a, b) -> float:
    return math.hypot(*_east_north(a, b))


def _bearing(points) -> float:
    """Degrees from north of the road leaving `points[0]`."""
    a = points[0]
    b = next((p for p in points[1:] if _metres(a, p) >= BEARING_M), points[-1])
    return math.degrees(math.atan2(*_east_north(a, b))) % 360


def _turn(heading: float, bearing: float) -> float:
    return abs((bearing - heading + 180) % 360 - 180)


def roads(blob: bytes, classes: int = RESIDENTIAL) -> list[Road]:
    """Every edge of class `classes` or more major, in global coordinates."""
    header = pack.read_header(blob)
    out = []
    for row, col, tile in pack.tiles(blob):
        south = header.south + row * header.tile
        west = header.west + col * header.tile
        for number, edge in enumerate(tile.edges):
            if edge[4] <= classes:
                points = [
                    (south + y, west + x, z * pack.Z_STEP_DM - pack.Z_OFFSET_DM) for x, y, z in tile.geometry(number)
                ]
                out.append(Road(points, bool(edge[5] & ONEWAY)))
    return out


def walk(
    blob: bytes,
    start: tuple[float, float],
    metres: float,
    seed: int = 1,
    leg: tuple[float, float] = (300.0, 900.0),
    classes: int = RESIDENTIAL,
) -> list[tuple[int, int, int]]:
    """(lat µdeg, lon µdeg, z dm) of a course of about `metres` from the
    junction nearest `start` (degrees)."""
    rng = random.Random(seed)
    network = roads(blob, classes)
    exits: dict[tuple[int, int], list[tuple[int, list]]] = {}
    for number, road in enumerate(network):
        exits.setdefault(road.points[0][:2], []).append((number, road.points))
        if not road.oneway:
            exits.setdefault(road.points[-1][:2], []).append((number, road.points[::-1]))
    here = (round(start[0] * 1e6), round(start[1] * 1e6))
    node = min(exits, key=lambda point: _metres(here, point))
    used: set[int] = set()

    def onward(choice) -> bool:
        number, points = choice
        return any(other != number and other not in used for other, _ in exits.get(points[-1][:2], []))

    course = [exits[node][0][1][0]]
    heading = None
    ridden = since_turn = 0.0
    next_turn = rng.uniform(*leg)
    while ridden < metres:
        options = [(n, p) for n, p in exits.get(node, []) if n not in used]
        options = [o for o in options if onward(o)] or options
        if not options:
            break
        if heading is None:
            choice = rng.choice(options)
        else:
            turns = sorted(options, key=lambda o: _turn(heading, _bearing(o[1])))
            straight = _turn(heading, _bearing(turns[0][1])) <= STRAIGHT_DEG
            cross = [o for o in turns if TURN_DEG[0] <= _turn(heading, _bearing(o[1])) <= TURN_DEG[1]]
            if straight and (since_turn < next_turn or not cross):
                choice = turns[0]
            else:
                choice = rng.choice(cross or turns)
                since_turn = 0.0
                next_turn = rng.uniform(*leg)
        number, points = choice
        used.add(number)
        length = sum(_metres(a, b) for a, b in itertools.pairwise(points))
        ridden += length
        since_turn += length
        heading = (_bearing(points[::-1]) + 180) % 360
        course.extend(points[1:])
        node = points[-1][:2]
    return course


def gpx(name: str, points) -> str:
    lines = [
        '<?xml version="1.0" encoding="UTF-8"?>',
        '<gpx version="1.1" creator="cycling-terrain course" xmlns="http://www.topografix.com/GPX/1/1">',
        '<!-- Road data (c) OpenStreetMap contributors (ODbL), elevation Copernicus WorldDEM-30 -->',
        f'<trk><name>{name}</name><trkseg>',
    ]
    lines += [
        f'<trkpt lat="{lat / 1e6:.6f}" lon="{lon / 1e6:.6f}"><ele>{z / 10:.1f}</ele></trkpt>' for lat, lon, z in points
    ]
    lines.append('</trkseg></trk></gpx>')
    return '\n'.join(lines) + '\n'
