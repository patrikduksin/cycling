"""Cycling-relevant ways from an OpenStreetMap extract.

Kept: trunk down to residential and living streets, plain service roads, and
tracks, cycleways, paths and bridleways (flagged by class). Dropped: motorways
and expressways unless bicycles are explicitly allowed, footways and
pedestrian areas unless bicycles are, steps, construction, areas, private
roads and anything that forbids bicycles.
"""

from __future__ import annotations

import zlib
from dataclasses import dataclass

import numpy as np
import osmium

# Road classes: lower is more major. The device ranks continuations with them.
TRUNK, PRIMARY, SECONDARY, TERTIARY, UNCLASSIFIED, RESIDENTIAL, SERVICE, TRACK, CYCLEWAY, PATH = range(1, 11)
CLASSES = {
    'trunk': TRUNK,
    'trunk_link': TRUNK,
    'primary': PRIMARY,
    'primary_link': PRIMARY,
    'secondary': SECONDARY,
    'secondary_link': SECONDARY,
    'tertiary': TERTIARY,
    'tertiary_link': TERTIARY,
    'unclassified': UNCLASSIFIED,
    'road': UNCLASSIFIED,
    'residential': RESIDENTIAL,
    'living_street': RESIDENTIAL,
    'service': SERVICE,
    'track': TRACK,
    'cycleway': CYCLEWAY,
    'path': PATH,
    'bridleway': PATH,
    # Only with bicycles allowed (below).
    'motorway': TRUNK,
    'motorway_link': TRUNK,
    'footway': PATH,
    'pedestrian': PATH,
}
NEEDS_BICYCLE = {'motorway', 'motorway_link', 'footway', 'pedestrian'}
BICYCLE_OK = {'yes', 'designated', 'permissive', 'official'}
BICYCLE_NO = {'no', 'use_sidepath', 'dismount'}
ACCESS_NO = {'no', 'private'}

# Edge flags, as the device reads them.
ONEWAY, LINK, UNPAVED, ROUNDABOUT, BRIDGE, TUNNEL = (1 << bit for bit in range(6))
UNPAVED_SURFACES = {
    'unpaved',
    'gravel',
    'fine_gravel',
    'dirt',
    'earth',
    'ground',
    'grass',
    'sand',
    'mud',
    'compacted',
    'pebblestone',
    'rock',
    'woodchips',
    'grass_paver',
}
PAVED_SURFACES = {'asphalt', 'paved', 'concrete', 'concrete:plates', 'paving_stones', 'sett', 'cobblestone', 'chipseal'}


@dataclass
class Way:
    refs: np.ndarray  # int64 node ids
    lat: np.ndarray  # float64 degrees
    lon: np.ndarray
    road_class: int
    flags: int
    group: int  # 16-bit hash of ref or name; 0 when unnamed


def road_group(tags) -> int:
    """A 16-bit id shared by ways of one named road (ref first, then name)."""
    label = tags.get('ref') or tags.get('name')
    if not label:
        return 0
    return (zlib.crc32(label.strip().lower().encode()) & 0xFFFF) or 1


def classify(tags) -> tuple[int, int, bool] | None:
    """(class, flags, reversed) for a way to keep, or None."""
    highway = tags.get('highway')
    road_class = CLASSES.get(highway)
    if road_class is None or tags.get('area') == 'yes':
        return None
    bicycle = tags.get('bicycle')
    if bicycle in BICYCLE_NO:
        return None
    if highway in NEEDS_BICYCLE and bicycle not in BICYCLE_OK:
        return None
    if tags.get('access') in ACCESS_NO and bicycle not in BICYCLE_OK:
        return None
    if highway == 'service' and tags.get('service'):
        return None
    if tags.get('motorroad') == 'yes' and bicycle not in BICYCLE_OK:
        return None
    flags = 0
    if highway.endswith('_link'):
        flags |= LINK
    surface = tags.get('surface')
    if surface in UNPAVED_SURFACES or (highway == 'track' and surface not in PAVED_SURFACES):
        flags |= UNPAVED
    if tags.get('junction') in ('roundabout', 'circular'):
        flags |= ROUNDABOUT | ONEWAY
    if tags.get('bridge', 'no') != 'no':
        flags |= BRIDGE
    if tags.get('tunnel', 'no') not in ('no', 'building_passage'):
        flags |= TUNNEL
    oneway = tags.get('oneway')
    reverse = oneway == '-1'
    if oneway in ('yes', '1', 'true', '-1'):
        flags |= ONEWAY
    if oneway == 'no':
        flags &= ~ONEWAY
    contraflow = tags.get('oneway:bicycle') == 'no' or tags.get('cycleway') in (
        'opposite',
        'opposite_lane',
        'opposite_track',
    )
    if contraflow:
        flags &= ~ONEWAY
        reverse = False
    return road_class, flags, reverse


class _Collector(osmium.SimpleHandler):
    def __init__(self, bbox: tuple[float, float, float, float] | None):
        super().__init__()
        self.bbox = bbox
        self.ways: list[Way] = []

    def way(self, w):
        kept = classify(w.tags)
        if kept is None:
            return
        road_class, flags, reverse = kept
        refs, lat, lon = [], [], []
        for node in w.nodes:
            if not node.location.valid():
                return
            refs.append(node.ref)
            lat.append(node.location.lat)
            lon.append(node.location.lon)
        if len(refs) < 2:
            return
        lat_a = np.array(lat)
        lon_a = np.array(lon)
        if self.bbox is not None and not overlaps(self.bbox, lat_a, lon_a):
            return
        refs_a = np.array(refs, dtype=np.int64)
        if reverse:
            refs_a, lat_a, lon_a = refs_a[::-1].copy(), lat_a[::-1].copy(), lon_a[::-1].copy()
        self.ways.append(Way(refs_a, lat_a, lon_a, road_class, flags, road_group(w.tags)))


def overlaps(bbox: tuple[float, float, float, float], lat: np.ndarray, lon: np.ndarray) -> bool:
    """Whether a way's bounding box meets `bbox`: conservative, so a way that
    crosses it without a node inside is kept. The tiler clips the rest."""
    south, west, north, east = bbox
    return bool(lat.min() <= north and lat.max() >= south and lon.min() <= east and lon.max() >= west)


def read_ways(pbf: str, bbox: tuple[float, float, float, float] | None = None) -> list[Way]:
    """Every kept way with node locations; with `bbox` (south, west, north,
    east), only ways whose bounding box meets it. Geometry outside is clipped
    later."""
    collector = _Collector(bbox)
    collector.apply_file(pbf, locations=True, idx='flex_mem')
    return collector.ways
