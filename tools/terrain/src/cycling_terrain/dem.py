"""Copernicus GLO-30 elevation, sampled bilinearly from 1-degree COG tiles.

Tiles come from the public AWS open-data bucket and are cached in a local
directory (ignored `.local/`). Each tile is a pixel-is-point grid whose first
row lies on its north edge and first column on its west edge; tiles north of
50 degrees have fewer columns. Ocean has no tile: it samples as sea level.
"""

from __future__ import annotations

import math
import urllib.error
import urllib.request
from pathlib import Path

import numpy as np
import tifffile

BUCKET = 'https://copernicus-dem-30m.s3.amazonaws.com'
ATTRIBUTION = (
    'Elevation produced using Copernicus WorldDEM-30 (c) DLR e.V. 2010-2014 and (c) Airbus Defence and Space '
    'GmbH 2014-2018 provided under COPERNICUS by the European Union and ESA; all rights reserved. The '
    'organisations in charge of the Copernicus programme by law or by delegation do not incur any liability '
    'for any use of the Copernicus WorldDEM-30.'
)


def tile_name(lat_floor: int, lon_floor: int) -> str:
    ns = f'N{lat_floor:02d}' if lat_floor >= 0 else f'S{-lat_floor:02d}'
    ew = f'E{lon_floor:03d}' if lon_floor >= 0 else f'W{-lon_floor:03d}'
    return f'Copernicus_DSM_COG_10_{ns}_00_{ew}_00_DEM'


class Dem:
    """Samples elevation in metres; `offline` never downloads."""

    def __init__(self, cache: Path, offline: bool = False):
        self.cache = cache
        self.offline = offline
        self.tiles: dict[tuple[int, int], np.ndarray | None] = {}
        self.keep = 6

    def path(self, key: tuple[int, int]) -> Path:
        return self.cache / f'{tile_name(*key)}.tif'

    def tile(self, key: tuple[int, int]) -> np.ndarray | None:
        if key in self.tiles:
            return self.tiles[key]
        # A tile is 52 MB decoded: keep only the few a batch is sampling.
        while len(self.tiles) >= self.keep:
            self.tiles.pop(next(iter(self.tiles)))
        path = self.path(key)
        missing = path.with_suffix('.missing')
        grid = None
        if not path.exists() and not missing.exists() and not self.offline:
            self.download(key, path, missing)
        if path.exists():
            grid = tifffile.imread(path).astype(np.float32)
        self.tiles[key] = grid
        return grid

    def download(self, key: tuple[int, int], path: Path, missing: Path) -> None:
        name = tile_name(*key)
        self.cache.mkdir(parents=True, exist_ok=True)
        partial = path.with_suffix('.partial')
        try:
            with urllib.request.urlopen(f'{BUCKET}/{name}/{name}.tif', timeout=120) as response:
                partial.write_bytes(response.read())
        except urllib.error.HTTPError as error:
            # The bucket answers 403/404 for tiles that do not exist (open ocean).
            if error.code in (403, 404):
                missing.write_text('no tile\n')
                return
            raise
        partial.rename(path)

    def sample(self, lat: np.ndarray, lon: np.ndarray) -> np.ndarray:
        """Bilinear elevation at each point; edges clamp inside their tile."""
        lat = np.asarray(lat, dtype=np.float64)
        lon = np.asarray(lon, dtype=np.float64)
        out = np.zeros(lat.shape, dtype=np.float64)
        keys = np.stack([np.floor(lat), np.floor(lon)], axis=-1).astype(np.int64)
        for key in {tuple(k) for k in keys.reshape(-1, 2).tolist()}:
            grid = self.tile((int(key[0]), int(key[1])))
            if grid is None:
                continue
            mask = (keys[..., 0] == key[0]) & (keys[..., 1] == key[1])
            rows, cols = grid.shape
            y = (key[0] + 1 - lat[mask]) * rows
            x = (lon[mask] - key[1]) * cols
            y = np.clip(y, 0, rows - 1)
            x = np.clip(x, 0, cols - 1)
            y0 = np.minimum(np.floor(y).astype(np.int64), rows - 2)
            x0 = np.minimum(np.floor(x).astype(np.int64), cols - 2)
            fy = y - y0
            fx = x - x0
            top = grid[y0, x0] * (1 - fx) + grid[y0, x0 + 1] * fx
            bottom = grid[y0 + 1, x0] * (1 - fx) + grid[y0 + 1, x0 + 1] * fx
            out[mask] = top * (1 - fy) + bottom * fy
        return out

    def keys(self, south: float, west: float, north: float, east: float) -> list[tuple[int, int]]:
        return [
            (lat, lon)
            for lat in range(math.floor(south), math.ceil(north))
            for lon in range(math.floor(west), math.ceil(east))
        ]
