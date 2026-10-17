"""cycling-terrain: download sources, build and validate terrain packs.

Big inputs and outputs belong in ignored `.local/terrain/`. Only the small
sample pack under `packages/terrain/tests/data/` is committed.
"""

from __future__ import annotations

import argparse
import datetime
import json
import sys
import time
import urllib.request
from pathlib import Path

import osmium

from . import dem as dem_module
from . import graph, osm, pack, trace

GEOFABRIK = 'https://download.geofabrik.de/south-america/chile-latest.osm.pbf'
OSM_ATTRIBUTION = (
    'Road data (c) OpenStreetMap contributors, available under the Open Database License (ODbL) 1.0: '
    'https://www.openstreetmap.org/copyright. This pack is a derived database under the ODbL; '
    'tools/terrain in https://github.com/patrikduksin/cycling rebuilds it from the OSM extract named below.'
)
# Mainland Chile with its Andean border roads (Rapa Nui and Juan Fernandez need their own packs).
CHILE = (-56.0, -76.0, -17.4, -66.0)
# The committed sample: the Farellones road (G-21) from Corral Quemado up to Farellones, Santiago.
SAMPLE = (-33.40, -70.49, -33.33, -70.27)
DEFAULT_TILE = 15_625  # 1/64 degree


def bbox(text: str) -> tuple[float, float, float, float]:
    values = tuple(float(v) for v in text.split(','))
    if len(values) != 4 or values[0] >= values[2] or values[1] >= values[3]:
        raise argparse.ArgumentTypeError('bbox is south,west,north,east')
    return values  # type: ignore[return-value]


def osm_timestamp(pbf: Path) -> tuple[int, str]:
    reader = osmium.io.Reader(str(pbf), osmium.osm.osm_entity_bits.NOTHING)
    stamp = reader.header().get('osmosis_replication_timestamp', '')
    reader.close()
    if stamp:
        when = datetime.datetime.fromisoformat(stamp.replace('Z', '+00:00'))
        return int(when.timestamp()), f'osm {when:%Y-%m-%d}'
    return int(pbf.stat().st_mtime), f'osm {pbf.name[:24]}'


def download(args) -> int:
    args.cache.mkdir(parents=True, exist_ok=True)
    target = args.cache / 'chile-latest.osm.pbf'
    if not target.exists():
        print(f'downloading {GEOFABRIK}', file=sys.stderr)
        partial = target.with_suffix('.partial')
        urllib.request.urlretrieve(GEOFABRIK, partial)
        partial.rename(target)
    dem = dem_module.Dem(args.cache / 'dem')
    keys = dem.keys(*args.bbox)
    for number, key in enumerate(keys, 1):
        dem.tile(key)
        dem.tiles.clear()
        print(f'dem {number}/{len(keys)} {dem_module.tile_name(*key)}', file=sys.stderr)
    return 0


def build(args) -> int:
    started = time.monotonic()
    grid = graph.Grid.around(*args.bbox, args.tile)
    # Every way that touches a tile of the grid, so border tiles are whole.
    ways = osm.read_ways(str(args.pbf), grid.bounds())
    print(f'{len(ways)} ways in {time.monotonic() - started:.0f}s', file=sys.stderr)
    dem = dem_module.Dem(args.dem, offline=args.offline)
    edges = graph.build_edges(ways, dem)
    print(f'{len(edges)} edges in {time.monotonic() - started:.0f}s', file=sys.stderr)
    tiles = graph.cut_into_tiles(edges, grid)
    created, source = osm_timestamp(args.pbf)
    attribution = f'{OSM_ATTRIBUTION} Source: {source}.\n{dem_module.ATTRIBUTION}\n'
    blob = pack.encode_pack(tiles, attribution, source, created)
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_bytes(blob)
    stats = pack.validate(blob)
    stats['seconds'] = round(time.monotonic() - started)
    print(json.dumps(stats))
    return 0


def trace_road(args) -> int:
    points = trace.road(args.pack.read_bytes(), args.road)
    if not points:
        print(f'no road {args.road!r} in the pack', file=sys.stderr)
        return 1
    args.out.write_text(trace.csv(points))
    return 0


def validate(args) -> int:
    blob = args.pack.read_bytes()
    try:
        stats = pack.validate(blob)
    except ValueError as error:
        print(f'invalid: {error}', file=sys.stderr)
        return 1
    header = pack.read_header(blob)
    stats.update(
        source=header.source,
        rows=header.rows,
        cols=header.cols,
        tile_udeg=header.tile,
        south=header.south,
        west=header.west,
    )
    print(json.dumps(stats))
    if args.attribution:
        print(header.attribution)
    return 0 if stats['unmatched_portals'] == 0 or args.allow_dead_ends else 1


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(prog='cycling-terrain', description=__doc__)
    sub = parser.add_subparsers(dest='command', required=True)
    local = Path('.local/terrain')

    p = sub.add_parser('download', help='fetch the Chile OSM extract and the DEM tiles for a bbox')
    p.add_argument('--cache', type=Path, default=local)
    p.add_argument('--bbox', type=bbox, default=CHILE)
    p.set_defaults(run=download)

    p = sub.add_parser('build', help='build a pack from an OSM extract and the DEM cache')
    p.add_argument('--pbf', type=Path, default=local / 'chile-latest.osm.pbf')
    p.add_argument('--dem', type=Path, default=local / 'dem')
    p.add_argument('--bbox', type=bbox, default=CHILE)
    p.add_argument('--tile', type=int, default=DEFAULT_TILE, help='tile size in micro-degrees')
    p.add_argument('--offline', action='store_true', help='never download DEM tiles; missing ones are sea level')
    p.add_argument('--sample', action='store_const', dest='bbox', const=SAMPLE, help='the committed sample bbox')
    p.add_argument('--out', type=Path, default=local / 'chile.terrain')
    p.set_defaults(run=build)

    p = sub.add_parser('validate', help='check a pack and print its statistics')
    p.add_argument('pack', type=Path)
    p.add_argument('--attribution', action='store_true', help='print the attribution it carries')
    p.add_argument('--allow-dead-ends', action='store_true', help='accept roads that end on a tile boundary')
    p.set_defaults(run=validate)

    p = sub.add_parser('trace', help="write a named road's longest chain as a lat,lon,elevation track")
    p.add_argument('pack', type=Path)
    p.add_argument('road', help='its ref or name, for example G-21')
    p.add_argument('--out', type=Path, required=True)
    p.set_defaults(run=trace_road)

    args = parser.parse_args(argv)
    return args.run(args)


if __name__ == '__main__':
    raise SystemExit(main())
