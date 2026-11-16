# Terrain packs

Host pipeline for the offline road and elevation pack behind VANA's climb
look-ahead. The design, data sources, licences and pack format are in
[docs/terrain.md](../../docs/terrain.md).

```sh
cycling terrain download            # Geofabrik Chile + Copernicus GLO-30 into .local/terrain
cycling terrain build               # .local/terrain/chile.terrain (about 40 min, 84 MB)
cycling terrain validate .local/terrain/chile.terrain --allow-dead-ends
cycling terrain build --sample --out packages/terrain/tests/data/farellones.terrain
cycling terrain trace packages/terrain/tests/data/farellones.terrain G-21 --out track.csv
cycling terrain course packages/terrain/tests/data/santiago-centro.terrain --start=-33.4378,-70.6505 --out course.gpx
```

Downloads and national packs stay in the ignored `.local/terrain`. The small
Farellones and central-Santiago packs are committed for tests, with their
attribution in
[packages/terrain/tests/data](../../packages/terrain/tests/data/README.md).
