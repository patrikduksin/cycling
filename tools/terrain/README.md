# Terrain packs

Host pipeline for the offline road and elevation pack behind VANA's climb
look-ahead. The design, data sources, licences and pack format are in
[docs/terrain.md](../../docs/terrain.md).

```sh
mise run terrain -- download            # Geofabrik Chile + Copernicus GLO-30 into .local/terrain
mise run terrain -- build               # .local/terrain/chile.terrain (about 40 min, 84 MB)
mise run terrain -- validate .local/terrain/chile.terrain --allow-dead-ends
mise run terrain -- build --sample --out packages/terrain/tests/data/farellones.terrain
mise run terrain -- trace packages/terrain/tests/data/farellones.terrain G-21 --out track.csv
```

Downloads and national packs stay in the ignored `.local/terrain`. The small
Farellones sample pack is committed for tests, with its attribution in
[packages/terrain/tests/data](../../packages/terrain/tests/data/README.md).
