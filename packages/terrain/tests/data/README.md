# Terrain test data

- `farellones.terrain`: a sample pack of the roads from Santiago's eastern
  edge up to Farellones (bbox -33.40,-70.49 to -33.33,-70.27), built with
  `cycling terrain build --sample`.
- `farellones-g21.csv`: route G-21 through that pack from its lowest point,
  one line per vertex (latitude, longitude, elevation in metres), written by
  `cycling terrain trace`.
- `santiago-centro.terrain`: the 3 × 3 tiles around Plaza de Armas, the
  densest roads a map draws (6,482 edges, 1,385 shape vertices), built from
  the OSM extract and the cached DEM tiles with

  ```sh
  cycling terrain build --offline --bbox=-33.46875,-70.671875,-33.421875,-70.625 \
    --out packages/terrain/tests/data/santiago-centro.terrain
  ```

  The centre tile is byte for byte the national pack's. A border tile can
  differ by a vertex, because the build sees only the ways that touch it.
- `santiago-centro-course.gpx`: an 8 km course along that pack's streets
  from Plaza de Armas, with about 20 turns. Nobody rode it. It was written by

  ```sh
  cycling terrain course packages/terrain/tests/data/santiago-centro.terrain \
    --start=-33.4378,-70.6505 --km 8 --name "Santiago centro (synthetic)" \
    --out packages/terrain/tests/data/santiago-centro-course.gpx
  ```

All of them are derived from:

- Road data © OpenStreetMap contributors, available under the
  [Open Database License (ODbL) 1.0](https://www.openstreetmap.org/copyright).
  They are derived databases under the ODbL; `tools/terrain` rebuilds them
  from the OSM extract named in the pack header.
- Elevation produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and
  © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by the
  European Union and ESA; all rights reserved.

Each pack carries the same text in its attribution sectors.
