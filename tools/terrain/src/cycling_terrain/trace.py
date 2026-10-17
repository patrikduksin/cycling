"""A named road through a pack, as a track: test fixtures and scenarios.

The longest chain of edges sharing a road group (its `ref`, else its name)
is written from its lowest end, one line per vertex: latitude, longitude
(degrees) and elevation (metres).
"""

from __future__ import annotations

import itertools
import math
import zlib

from . import pack


def group_of(label: str) -> int:
    return (zlib.crc32(label.strip().lower().encode()) & 0xFFFF) or 1


def _metres(a, b) -> float:
    dy = (b[0] - a[0]) * 0.111195
    dx = (b[1] - a[1]) * 0.111195 * math.cos(math.radians(a[0] * 1e-6))
    return math.hypot(dx, dy)


def _length(points) -> float:
    return sum(_metres(a, b) for a, b in itertools.pairwise(points))


def road(blob: bytes, label: str) -> list[tuple[int, int, int]]:
    """(lat µdeg, lon µdeg, z dm) of the longest chain of `label`'s edges."""
    header = pack.read_header(blob)
    group = group_of(label)
    pieces = []
    for row, col, tile in pack.tiles(blob):
        south = header.south + row * header.tile
        west = header.west + col * header.tile
        for number, edge in enumerate(tile.edges):
            if edge[6] != group:
                continue
            pieces.append([(south + y, west + x, z - pack.Z_OFFSET_DM) for x, y, z in tile.geometry(number)])
    ends: dict[tuple[int, int], list[int]] = {}
    for index, piece in enumerate(pieces):
        for point in (piece[0], piece[-1]):
            ends.setdefault(point[:2], []).append(index)
    lengths = [_length(piece) for piece in pieces]

    def longest(end, used: set[int]) -> tuple[float, list]:
        """The longest continuation from `end` through unused pieces: every
        branch is tried (a road's own pieces are few)."""
        best: tuple[float, list] = (0.0, [])
        for index in ends.get(end, []):
            if index in used:
                continue
            piece = pieces[index]
            if piece[0][:2] != end:
                piece = list(reversed(piece))
            used.add(index)
            length, rest = longest(piece[-1][:2], used)
            used.discard(index)
            if lengths[index] + length > best[0]:
                best = (lengths[index] + length, piece[1:] + rest)
        return best

    best: list = []
    best_length = 0.0
    for start in range(len(pieces)):
        for reverse in (False, True):
            chain = list(reversed(pieces[start])) if reverse else list(pieces[start])
            length, rest = longest(chain[-1][:2], {start})
            if lengths[start] + length > best_length:
                best_length = lengths[start] + length
                best = chain + rest
    if best and best[0][2] > best[-1][2]:
        best.reverse()
    return best


def csv(points) -> str:
    lines = ['# lat,lon,elevation_m; (c) OpenStreetMap contributors (ODbL), Copernicus WorldDEM-30']
    lines += [f'{lat / 1e6:.6f},{lon / 1e6:.6f},{z / 10:.1f}' for lat, lon, z in points]
    return '\n'.join(lines) + '\n'
