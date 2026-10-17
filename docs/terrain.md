# Terrain look-ahead

VANA predicts the climbs on the road ahead without a planned route. An offline
pack of Chile's rideable roads, each carrying its elevation, lives in the MMC
`TERRAIN` volume. On the bike, GPS fixes are matched to a road and the road is
followed ahead until it forks ambiguously. The elevation profile of that
stretch feeds the same climb detector the ride already uses. The climb page can
then announce a climb before it starts, and while climbing it shows what is
left.

This document covers the design. The code owns the exact constants:

- `tools/terrain` builds packs;
- `packages/terrain` reads them, matches and predicts;
- `apps/vana` turns predictions into climbs and screens.

## Data sources and licences

| Data | Source | Licence | Why |
| --- | --- | --- | --- |
| Roads | OpenStreetMap, Geofabrik `chile-latest.osm.pbf` (350 MB) | ODbL 1.0 | The only open road network with Chile's mountain roads, tracks and cycleways, and their `ref`, `oneway`, `surface`, `bridge` and `tunnel` tags. |
| Elevation | Copernicus GLO-30 DEM, 1 arc-second (about 30 m), AWS open data bucket `copernicus-dem-30m` | Copernicus DEM licence: free use and redistribution of derived products with attribution | See below. |

**The pack is a derived database under the ODbL.** It carries this
attribution in its header sectors, and `cycling-terrain validate
--attribution` prints it:

> Road data (c) OpenStreetMap contributors, available under the Open Database
> License (ODbL) 1.0: https://www.openstreetmap.org/copyright. This pack is a
> derived database under the ODbL; tools/terrain in
> https://github.com/patrikduksin/cycling rebuilds it from the OSM extract
> named below.
>
> Elevation produced using Copernicus WorldDEM-30 (c) DLR e.V. 2010-2014 and
> (c) Airbus Defence and Space GmbH 2014-2018 provided under COPERNICUS by the
> European Union and ESA; all rights reserved. The organisations in charge of
> the Copernicus programme by law or by delegation do not incur any liability
> for any use of the Copernicus WorldDEM-30.

The pack names the extract it came from, for example `osm 2026-10-16`. Anyone
can rebuild it with the public tools, which satisfies the ODbL offer of the
derived database. A device that shows map-derived data should credit
OpenStreetMap on an about or settings screen.

**Why GLO-30.** The alternatives each fail on one point:

- SRTM 1" (2000) has voids and C-band artefacts in the steep Andes, which is
  exactly where Chile's climbs are, and it is 25 years older.
- ALOS AW3D30 is good but has more voids at high relief and a weaker vertical
  accuracy statement.
- FABDEM removes trees and buildings, but its licence is non-commercial.
- Local 12.5 m ALOS PALSAR RTC data is not a consistent national DEM.

GLO-30 comes from TanDEM-X (2011-2015). It has the best published accuracy of
the free global DEMs (about 2 m absolute vertical, under 4 m in steep
terrain), no voids, and a licence that allows redistributing derived products.

GLO-30 is a surface model, so trees, buildings and bridge decks show up in it.
The builder compensates in three ways:

- It smooths along each road.
- It takes junction elevations from several samples.
- It draws tagged bridges and tunnels as straight lines between their ends.

## Size for Chile

These are the measured figures from the full build, with OSM data from
2026-10-16, the 15,625 µdeg grid and every rideable class:

| | |
| --- | --- |
| Tiles with roads | 115,048 of a 2,471 × 640 grid |
| Junction nodes / edges / shape vertices | 1.63 M / 1.62 M / 3.67 M |
| Pack | **83.8 MB** (12.7 MB of it the dense directory) |
| Tile size | median 278 B, p99 7.4 KB, max 45.3 KB (Santiago centre); 34 tiles over 32 KiB |
| Portals (nodes on tile boundaries) | 548,264; 46 unmatched (roads that really end on a boundary line) |
| Build time | 40 min on the laptop: 40 s reading OSM, the rest DEM sampling and smoothing |
| DEM download | 142 tiles, 4.1 GB, cached in `.local/terrain/dem` |

The pack fits the 1.25 GiB `TERRAIN` volume about fifteen times over, so
nothing needs cutting. Kept classes:

- trunk, primary, secondary and tertiary;
- unclassified, residential and living streets;
- untyped service roads (any `service=*`, such as parking aisles or
  driveways, is dropped);
- tracks;
- cycleways, paths and bridleways;
- footways, pedestrian ways and motorways that allow bicycles.

Ways that are private, `motorroad=yes` or closed to bicycles are dropped.

If neighbouring countries are ever added, Argentina and Bolivia would fit in
the same volume. The dense directory would then be the first thing to make
sparse.

## Building a pack

`tools/terrain` is a uv project. It runs as `mise run terrain -- <command>`
and keeps downloads and national packs in the ignored `.local/terrain`:

1. **Download.** Get the Geofabrik extract and the GLO-30 tiles that cover
   the bounding box. Ocean tiles (403/404) are remembered as missing.
2. **Read ways.** `pyosmium` keeps rideable ways inside the grid's bounds and
   records for each:
   - its class;
   - its flags: oneway, link, unpaved, roundabout, bridge, tunnel;
   - its road group, a 16-bit hash of `ref`, else of `name`.

   `oneway=-1` reverses the way. Contraflow tags such as `oneway:bicycle=no`
   clear the oneway flag.
3. **Split at junctions.** A node shared by two ways, or appearing twice in
   one way, becomes a graph node.
4. **Elevation per edge.**
   - Densify to 20 m and sample the DEM bilinearly.
   - Smooth with a Gaussian (σ = 30 m) along the road, on a 5 m resampling.
   - Correct linearly so that both ends land exactly on their junction's
     elevation. A junction's elevation is the mean of five DEM samples within
     15 m, so every edge at a junction agrees.
   - Bridges and tunnels are straight lines between their ends.
5. **Simplify.** Run a 3D Douglas-Peucker: a point is kept if dropping it
   would move the road more than 3 m horizontally or 1 m vertically.
6. **Cut into tiles.** Each segment is cut exactly on grid lines, and each
   piece goes to the tile that holds its midpoint. Cut points become *portal*
   nodes that appear identically in both neighbouring tiles. This is how the
   device follows a road across a boundary without a global node id.
7. **Encode and validate.**
   - `validate` re-reads the whole pack.
   - It checks every CRC and every index range.
   - It checks that each adjacency entry belongs to its own node.
   - It checks that every portal has a twin at the same elevation.

The sample pack (`--sample`, Farellones and route G-21, 92 KB) and a G-21
track are committed under `packages/terrain/tests/data` for tests.

## Pack format (v2)

The pack is a single opaque image. The host writes it into the `TERRAIN`
volume through the volume pack tooling and the device reads it by byte
offset:

```sh
mise run terrain-write -- .local/terrain/chile.terrain --tag CHILE-TERRAIN-V1 --verify
```

All integers are little-endian.

| Offset | Size | Content |
| --- | --- | --- |
| 0 | 512 | Header (below), CRC32 in its last 4 bytes |
| 512 | n × 512 | Attribution text, UTF-8 |
| `directory` | rows × cols × 8 | Per tile, row-major from the south-west: offset u32, length u32 (0 = no roads) |
| `data` | … | Tiles back to back |

Header fields, in order:

- magic `VANA-TERRAIN\0\0\0\0`;
- version u16 = 2, header sectors u16 (the header and its attribution);
- tile size µdeg u32, south i32 µdeg, west i32 µdeg;
- rows u16, cols u16;
- directory offset u32, data offset u32, total bytes u32, tile count u32;
- OSM timestamp u64;
- attribution offset u32, attribution length u32;
- source label (32 bytes).

The directory CRC32 sits at byte 120.

A tile is 1/64° square, about 1.7 × 1.4 km at Santiago. It has a 32-byte
header:

- `VT`, version, flags;
- row and col (so a misplaced read is caught);
- node, edge and vertex counts;
- longitude micro-metres per µdeg at the tile's latitude;
- CRC32 at byte 24, computed with that field zeroed.

Four arrays follow, all in tile-local µdeg (0..=tile size), with elevation as
a u16 of 0.2 m steps from −500 m (up to 12,607 m; the builder refuses anything outside):

| Record | Bytes | Fields |
| --- | --- | --- |
| Node | 8 | x, y, z, first adjacency index; sorted by (y, x) for binary search |
| Adjacency | 2 | edge << 1 \| (1 if the edge ends here) |
| Edge | 16 | from, to, first vertex u32, vertex count u16, class, flags, group u16, reserved |
| Vertex | 6 | x, y, z of interior shape points |

Version rules:

- The pack header version changes on any layout change; the device refuses
  unknown versions and treats the volume as having no pack.
- The volume pack tag is the host's label (`CHILE-TERRAIN-V1`). The volume
  pack's CRC covers the image. The device trusts the terrain header's own
  magic and version, not the tag.

## On the device

`packages/terrain` is `no_std` and allocation-free. It sees the pack only
through `trait Source { fn read(&mut self, offset, buf) }`.

### Tile cache

The cache lives in a caller-owned arena: 512 KiB of PSRAM on the C606. It is
cut into 8 KiB pages. A tile takes as many consecutive pages as it needs, up
to 48 KiB (the builder enforces the same limit). Nearly every tile fits in one
page, so the cache usually holds 64 tiles, about 150 km² of roads.

A new tile replaces the run of pages whose newest use is oldest. Loading is
incremental: one directory read, then 2 KiB chunks. The CRC and structure are
checked once, when the last chunk arrives. Tiles that are empty, corrupt or
too large go to a small "missing" ring, so they cost one read.

### Matching

Every new fix is matched against the edges within 40 m in the fix's tile and
its neighbours. A candidate's cost is:

- (distance / 15 m)²;
- plus (heading error / 35°)²;
- plus 4 when riding a oneway the wrong way;
- minus 1 for the current edge, or minus 0.5 for an edge joined to it.

The heading comes from the fix history: the bearing to the newest fix at
least 15 m back.

Hysteresis keeps the match steady:

- Another road must beat the current one by 1.0 on two consecutive fixes.
- The direction of travel on the current edge flips only for a heading error
  over 135°.
- Three fixes with nothing in range lose the match.

The match's confidence combines nearness, the margin over the runner-up and
steadiness.

### Path prediction

From the matched point the predictor walks the graph and samples elevation
every 20 m, up to 10 km. At each junction it scores every way on:

- the turn angle (1 per 30°, capped at 4, so a hairpin is just a turn);
- staying on the same road group (−3);
- class changes (+0.8 per step down, −0.2 per step up);
- links (+1.5), tracks and paths (+2), cycleways (+1);
- wrong-way oneways and the road just ridden (never candidates).

On a roundabout it judges exits against the heading it entered with.

A clear winner (by at least 1.0) continues, with confidence scaled down by
the margin. A tie ends the prediction with **Fork**: VANA never guesses at
ambiguous junctions.

Other endings are:

- **Horizon** — the full 10 km was reached;
- **DeadEnd**;
- **MapEdge** — the pack has no tile there;
- **Uncertain** — confidence fell below 0.3;
- **Memory** — the road ahead needs more tiles than the cache can hold.

Portals carry the walk across tile boundaries.

### Scheduling

`Lookahead::step` is a state machine. Each call does one bounded unit of
work:

- read the header;
- load one chunk;
- match one fix;
- run one prediction.

A prediction that meets an absent tile returns its key. The engine loads it
over the next calls and restarts. Predictions run every 2 s, or sooner when
the match changes.

VANA calls `step` once per main-loop pass, after the ride tick, so the
look-ahead never blocks a frame. On the sample ride a fix needs at most about
25 steps.

The device finds the ready pack in the `TERRAIN` volume, through
`volume::open` and `pack::open`. It looks at the volume again every 10 s. A
new, rewritten or removed pack resets the engine, as does a pack the engine
gave up on after read errors.

### Climbs ahead

`apps/vana/src/ride/climb_ahead.rs` reruns the ride's own detector (#154
thresholds) over the predicted samples. A climb starts at 3% over 200 m and is
confirmed by 20 m of gain. It ends 10 m below its top, or when the grade over
200 m falls under 1%. For up to three climbs it records:

- the distance to the foot;
- length, gain and average grade;
- whether the climb runs past the horizon;
- its category.

The category uses the Garmin/Strava score, length (m) × grade (%), which is
the same as gain (m) × 100:

| Category | Cat 4 | Cat 3 | Cat 2 | Cat 1 | HC |
| --- | --- | --- | --- | --- | --- |
| Score from | 8,000 | 16,000 | 32,000 | 48,000 | 64,000 |

The forecast is usable only when the match is on a road and the prediction's
confidence is at least 50%. Otherwise the climb page keeps its reactive,
barometer-only view.

The map brings up the climb page 500 m before a climb starts, once per climb.

## Memory and CPU budget

| | |
| --- | --- |
| PSRAM | 512 KiB tile arena |
| Internal RAM | engine state about 3 KB (match, fix history, 501-sample profile); VANA forecast about 1.1 KB, copied into the view |
| MMC reads | at most one 2 KiB read per step; a ride across new country loads a tile every minute or two |
| CPU | one step per main-loop pass: a match is tens of edges, a prediction at most 2,000 edges and 501 samples |

The device validation in the integration PR covers frame time against the
25 ms budget, press-to-frame latency and step timing.

## Tests

- `tools/terrain/tests` checks the builder's rules:
  - classification;
  - exact cuts and matching portals;
  - smoothing and simplification;
  - corrupt tiles;
  - the committed sample.
- `packages/terrain/src/tests.rs` builds synthetic one-tile packs for:
  - a crossing;
  - a fork;
  - a named road winning;
  - oneways;
  - roundabouts;
  - parallel-road hysteresis;
  - no pack or off the map;
  - a corrupt tile.

  Cache paging has its own unit test.
- `packages/terrain/tests/sample.rs` rides G-21 up to Farellones at 5 m/s with
  wandering GPS noise. It checks:
  - more than 98% of fixes match G-21;
  - more than 85% are confident;
  - fewer than 0.5% of confident predictions leave the real profile by more
    than 15 m;
  - the predicted reach has a median of at least 9 km and a p10 of at least
    3 km.

  An ignored test runs the same ride through the national pack and reads
  every tile in it. Run it with `CYCLING_TERRAIN_PACK=…/chile.terrain cargo
  test -p terrain -- --ignored`.
