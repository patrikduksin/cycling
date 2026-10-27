# Terrain test data

- `farellones.terrain`: a sample pack of the roads from Santiago's eastern
  edge up to Farellones (bbox -33.40,-70.49 to -33.33,-70.27), built with
  `cycling terrain build --sample`.
- `farellones-g21.csv`: route G-21 through that pack from its lowest point,
  one line per vertex (latitude, longitude, elevation in metres), written by
  `cycling terrain trace`.

Both are derived from:

- Road data © OpenStreetMap contributors, available under the
  [Open Database License (ODbL) 1.0](https://www.openstreetmap.org/copyright).
  They are derived databases under the ODbL; `tools/terrain` rebuilds them
  from the OSM extract named in the pack header.
- Elevation produced using Copernicus WorldDEM-30 © DLR e.V. 2010-2014 and
  © Airbus Defence and Space GmbH 2014-2018, provided under COPERNICUS by the
  European Union and ESA; all rights reserved.

The pack carries the same text in its attribution sectors.
