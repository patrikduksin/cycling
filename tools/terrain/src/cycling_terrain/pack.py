"""The terrain pack: the on-device format (docs/terrain.md owns the layout).

All integers are little-endian. A pack is a header sector, attribution text,
a dense tile directory and the tiles. The device reads it in 512-byte
sectors and never needs more than a directory entry and one tile at a time.
"""

from __future__ import annotations

import math
import struct
import zlib
from dataclasses import dataclass

from .graph import METRES_PER_DEGREE, Grid, Tiles

MAGIC = b'VANA-TERRAIN\0\0\0\0'
VERSION = 1
SECTOR = 512
TILE_MAGIC = b'VT'
TILE_VERSION = 1
TILE_HEADER = 32
NODE = 8
ADJ = 2
EDGE = 16
VERTEX = 6
ENTRY = 8
# Elevation is stored as decimetres above -500 m.
Z_OFFSET_DM = 5000
# The largest tile the device can cache; the builder refuses bigger ones.
MAX_TILE_BYTES = 48 * 1024  # the firmware cache limit (packages/terrain cache.rs)
MAX_EDGES = 0x7FFF

HEADER = struct.Struct('<16sHHIiiHHIIIIQII32s')
assert HEADER.size <= SECTOR - 4


def encode_z(metres: float) -> int:
    return max(0, min(0xFFFF, round(metres * 10) + Z_OFFSET_DM))


def decode_z(raw: int) -> float:
    return (raw - Z_OFFSET_DM) / 10


def lon_scale_um(grid: Grid, row: int) -> int:
    """Micrometres per micro-degree of longitude at the tile's middle."""
    lat = (grid.south + (row + 0.5) * grid.tile) / 1e6
    return round(METRES_PER_DEGREE * math.cos(math.radians(lat)))


LAT_SCALE_UM = round(METRES_PER_DEGREE)


def encode_tile(grid: Grid, row: int, col: int, edges) -> bytes:
    south = grid.south + row * grid.tile
    west = grid.west + col * grid.tile

    def local(point):
        y = min(max(point[0] - south, 0), grid.tile)
        x = min(max(point[1] - west, 0), grid.tile)
        return x, y

    nodes: dict[tuple[int, int], int] = {}
    node_z: dict[tuple[int, int], int] = {}
    for edge in edges:
        for point in (edge.points[0], edge.points[-1]):
            key = local(point)
            nodes.setdefault(key, 0)
            node_z.setdefault(key, encode_z(point[2]))
    order = sorted(nodes, key=lambda k: (k[1], k[0]))
    index = {key: i for i, key in enumerate(order)}
    incident: list[list[int]] = [[] for _ in order]
    edge_bytes = bytearray()
    vertex_bytes = bytearray()
    vertex_count = 0
    for number, edge in enumerate(edges):
        start = index[local(edge.points[0])]
        end = index[local(edge.points[-1])]
        incident[start].append(number << 1)
        incident[end].append(number << 1 | 1)
        interior = edge.points[1:-1]
        edge_bytes += struct.pack(
            '<HHIHBBHH', start, end, vertex_count, len(interior), edge.road_class, edge.flags, edge.group, 0
        )
        for point in interior:
            x, y = local(point)
            vertex_bytes += struct.pack('<HHH', x, y, encode_z(point[2]))
        vertex_count += len(interior)
    if len(edges) > MAX_EDGES or len(order) > 0xFFFF:
        raise ValueError(f'tile {row},{col} has {len(edges)} edges and {len(order)} nodes')
    node_bytes = bytearray()
    adj_bytes = bytearray()
    for i, key in enumerate(order):
        node_bytes += struct.pack('<HHHH', key[0], key[1], node_z[key], len(adj_bytes) // ADJ)
        for entry in incident[i]:
            adj_bytes += struct.pack('<H', entry)
    header = bytearray(
        struct.pack(
            '<2sBBHHHHIIII',
            TILE_MAGIC,
            TILE_VERSION,
            0,
            row,
            col,
            len(order),
            len(edges),
            vertex_count,
            lon_scale_um(grid, row),
            0,
            0,
        )
    )
    header += bytes(TILE_HEADER - len(header))
    body = bytes(header) + node_bytes + adj_bytes + edge_bytes + vertex_bytes
    crc = zlib.crc32(body)
    body = body[:24] + struct.pack('<I', crc) + body[28:]
    if len(body) > MAX_TILE_BYTES:
        raise ValueError(f'tile {row},{col} is {len(body)} bytes, over {MAX_TILE_BYTES}')
    return body


def encode_pack(tiles: Tiles, attribution: str, source: str, created: int) -> bytes:
    grid = tiles.grid
    attribution_bytes = attribution.encode('utf-8')
    attribution_sectors = -(-len(attribution_bytes) // SECTOR)
    directory_offset = SECTOR * (1 + attribution_sectors)
    entries = grid.rows * grid.cols
    data_offset = directory_offset + entries * ENTRY
    directory = bytearray(entries * ENTRY)
    data = bytearray()
    for row, col in sorted(tiles.edges):
        edges = tiles.edges[(row, col)]
        if not edges:
            continue
        blob = encode_tile(grid, row, col, edges)
        struct.pack_into('<II', directory, (row * grid.cols + col) * ENTRY, data_offset + len(data), len(blob))
        data += blob
    total = data_offset + len(data)
    if total >= 1 << 32:
        raise ValueError('pack exceeds 4 GiB')
    header = bytearray(SECTOR)
    HEADER.pack_into(
        header,
        0,
        MAGIC,
        VERSION,
        1 + attribution_sectors,
        grid.tile,
        grid.south,
        grid.west,
        grid.rows,
        grid.cols,
        directory_offset,
        data_offset,
        total,
        sum(1 for edges in tiles.edges.values() if edges),
        created,
        SECTOR,
        len(attribution_bytes),
        source.encode()[:32].ljust(32, b'\0'),
    )
    struct.pack_into('<I', header, 120, zlib.crc32(bytes(directory)))
    struct.pack_into('<I', header, SECTOR - 4, zlib.crc32(bytes(header[: SECTOR - 4])))
    attribution_block = attribution_bytes.ljust(attribution_sectors * SECTOR, b'\0')
    return bytes(header) + attribution_block + bytes(directory) + bytes(data)


# --- Reading and validation ---------------------------------------------------


@dataclass
class Header:
    tile: int
    south: int
    west: int
    rows: int
    cols: int
    directory_offset: int
    data_offset: int
    total: int
    tiles: int
    created: int
    attribution: str
    source: str
    directory_crc: int


@dataclass
class Tile:
    row: int
    col: int
    lon_scale_um: int
    nodes: list[tuple[int, int, int, int]]  # x, y, z, adjacency start
    adjacency: list[int]
    edges: list[tuple[int, int, int, int, int, int, int]]  # from, to, vstart, vcount, class, flags, group
    vertices: list[tuple[int, int, int]]

    def geometry(self, number: int) -> list[tuple[int, int, int]]:
        start, end, vstart, vcount = self.edges[number][:4]
        a, b = self.nodes[start], self.nodes[end]
        return [a[:3], *self.vertices[vstart : vstart + vcount], b[:3]]


def read_header(blob: bytes) -> Header:
    if len(blob) < SECTOR:
        raise ValueError('pack shorter than its header')
    sector = blob[:SECTOR]
    if struct.unpack_from('<I', sector, SECTOR - 4)[0] != zlib.crc32(sector[: SECTOR - 4]):
        raise ValueError('header CRC mismatch')
    fields = HEADER.unpack_from(sector)
    if fields[0] != MAGIC or fields[1] != VERSION:
        raise ValueError('not a version 1 terrain pack')
    (
        _,
        _,
        _,
        tile,
        south,
        west,
        rows,
        cols,
        directory_offset,
        data_offset,
        total,
        tiles,
        created,
        a_off,
        a_len,
        src,
    ) = fields
    return Header(
        tile,
        south,
        west,
        rows,
        cols,
        directory_offset,
        data_offset,
        total,
        tiles,
        created,
        blob[a_off : a_off + a_len].decode('utf-8'),
        src.rstrip(b'\0').decode(),
        struct.unpack_from('<I', sector, 120)[0],
    )


def read_tile(blob: bytes, offset: int, length: int) -> Tile:
    data = blob[offset : offset + length]
    if len(data) != length or length < TILE_HEADER:
        raise ValueError('tile outside the pack')
    magic, version, _flags, row, col, node_count, edge_count, vertex_count, lon_scale, _r0, crc = struct.unpack_from(
        '<2sBBHHHHIIII', data
    )
    if magic != TILE_MAGIC or version != TILE_VERSION:
        raise ValueError('bad tile magic')
    if zlib.crc32(data[:24] + b'\0\0\0\0' + data[28:]) != crc:
        raise ValueError(f'tile {row},{col} CRC mismatch')
    at = TILE_HEADER
    nodes = [struct.unpack_from('<HHHH', data, at + i * NODE) for i in range(node_count)]
    at += node_count * NODE
    adjacency = [struct.unpack_from('<H', data, at + i * ADJ)[0] for i in range(2 * edge_count)]
    at += 2 * edge_count * ADJ
    edges = [struct.unpack_from('<HHIHBBHH', data, at + i * EDGE)[:7] for i in range(edge_count)]
    at += edge_count * EDGE
    vertices = [struct.unpack_from('<HHH', data, at + i * VERTEX) for i in range(vertex_count)]
    at += vertex_count * VERTEX
    if at != length:
        raise ValueError(f'tile {row},{col} length {length} != {at}')
    return Tile(row, col, lon_scale, nodes, adjacency, edges, vertices)


def tiles(blob: bytes):
    """Every (row, col, tile) in the pack."""
    header = read_header(blob)
    for row in range(header.rows):
        for col in range(header.cols):
            offset, length = struct.unpack_from(
                '<II', blob, header.directory_offset + (row * header.cols + col) * ENTRY
            )
            if length:
                yield row, col, read_tile(blob, offset, length)


def validate(blob: bytes) -> dict:
    """Check every structural invariant the device relies on; returns stats."""
    header = read_header(blob)
    if header.total != len(blob):
        raise ValueError(f'pack is {len(blob)} bytes, header says {header.total}')
    directory = blob[header.directory_offset : header.data_offset]
    if len(directory) != header.rows * header.cols * ENTRY or zlib.crc32(directory) != header.directory_crc:
        raise ValueError('directory size or CRC mismatch')
    found = {}
    stats = {'tiles': 0, 'nodes': 0, 'edges': 0, 'vertices': 0, 'max_tile_bytes': 0, 'bytes': len(blob)}
    for row, col, tile in tiles(blob):
        if (tile.row, tile.col) != (row, col):
            raise ValueError(f'tile at {row},{col} claims {tile.row},{tile.col}')
        _offset, length = struct.unpack_from('<II', directory, (row * header.cols + col) * ENTRY)
        stats['max_tile_bytes'] = max(stats['max_tile_bytes'], length)
        if length > MAX_TILE_BYTES:
            raise ValueError(f'tile {row},{col} too large')
        for x, y, _z in tile.vertices:
            if x > header.tile or y > header.tile:
                raise ValueError(f'tile {row},{col} vertex outside')
        for x, y, _z, _a in tile.nodes:
            if x > header.tile or y > header.tile:
                raise ValueError(f'tile {row},{col} node outside')
        # Adjacency lists every edge end exactly once, at its own node.
        seen = set()
        for number, node in enumerate(tile.nodes):
            end = tile.nodes[number + 1][3] if number + 1 < len(tile.nodes) else len(tile.adjacency)
            for entry in tile.adjacency[node[3] : end]:
                edge = tile.edges[entry >> 1]
                if edge[entry & 1] != number:
                    raise ValueError(f'tile {row},{col} adjacency mismatch')
                seen.add(entry)
        if len(seen) != len(tile.adjacency):
            raise ValueError(f'tile {row},{col} adjacency incomplete')
        for edge in tile.edges:
            if edge[2] + edge[3] > len(tile.vertices):
                raise ValueError(f'tile {row},{col} vertex range')
        found[(row, col)] = tile
        stats['tiles'] += 1
        stats['nodes'] += len(tile.nodes)
        stats['edges'] += len(tile.edges)
        stats['vertices'] += len(tile.vertices)
    if stats['tiles'] != header.tiles:
        raise ValueError('tile count mismatch')
    stats['portals'], stats['unmatched_portals'] = _check_portals(header, found)
    return stats


def neighbours(x: int, y: int, size: int):
    """(row step, col step, x, y) of the same point in each tile that shares
    a boundary point: one for an edge, three for a corner."""
    cols = [(0, x)] + ([(-1, size)] if x == 0 else []) + ([(1, 0)] if x == size else [])
    rows = [(0, y)] + ([(-1, size)] if y == 0 else []) + ([(1, 0)] if y == size else [])
    for d_row, ny in rows:
        for d_col, nx in cols:
            if d_row or d_col:
                yield d_row, d_col, nx, ny


def _check_portals(header: Header, found: dict) -> tuple[int, int]:
    """A node on a tile boundary continues, at the same elevation, in a tile
    that shares it, or the device could not follow the road across. A road
    that really ends on a boundary line is legal but rare; count both."""
    portals = unmatched = 0
    lookup = {key: {(n[0], n[1]): n[2] for n in tile.nodes} for key, tile in found.items()}
    for (row, col), tile in found.items():
        for x, y, z, _a in tile.nodes:
            shared = [
                n
                for n in neighbours(x, y, header.tile)
                if 0 <= row + n[0] < header.rows and 0 <= col + n[1] < header.cols
            ]
            if not shared:
                continue
            portals += 1
            if not any(
                lookup.get((row + d_row, col + d_col), {}).get((nx, ny)) == z for d_row, d_col, nx, ny in shared
            ):
                unmatched += 1
    return portals, unmatched
