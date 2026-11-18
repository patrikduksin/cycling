# Course navigation design

These are the approved mockups for following a course on VANA. The SVGs are the
source of truth for geometry, sizes and colours, drawn in device pixels on the
240×320 screen at scale 1; [UI](../../ui.md#how-screens-scale) says how other
panels scale. `png/` holds them rendered at 2x, plus `sheet.png` (all
screens) and `glance-sheet.png` (blurred, roughly a half-second look at arm's
length). [UX flows](../../ux-flows.md#course) says what the rider sees and
when, and [UI](../../ui.md#course-sign) how the sign and the
[map](../../ui.md#map-page) are drawn.

| Screen | Scenario |
| --- | --- |
| `ride-straight` | Main data page on a long straight, next turn 6.4 km away |
| `ride-turns` | Main data page, left turn in 120 m |
| `ride-offcourse` | Main data page, missed a right turn, 140 m past it |
| `map-approach` | Map page, right turn in 250 m |
| `map-close` | Map page, right turn in 40 m |
| `map-offcourse` | Map page, the same missed turn |
| `arrived` | End of the course, ride totals |

## Tokens

The mockups use these values on top of the palette in
`apps/vana/src/screens/chrome.rs`.

| Token | Value | Use |
| --- | --- | --- |
| Turn fill | `#0a6fe0` | Turn banner, map badge and at-rest chip square |
| Off-course fill | `#d67400` | Off-course banner and badge |
| Arrived fill | `#1b9a45` | Arrived banner |
| Sign ink | `#ffffff` | Every glyph, digit and unit on a coloured sign |
| Map ground | `#101012` | Map page background |
| Minor road | `#2a2a2d`, 5 px | Every road class below secondary |
| Major road | `#444448`, 9 px | Trunk, primary and secondary |
| Route ahead | `ACCENT` `#0a84ff`, 12 px on an 18 px `#04264c` casing | Course still to ride |
| Ridden | `#1f4a78`, 5 px | The ride's own track, on a ride without a course |
| Off-course marks | `AMBER` `#ff9f0a` | Rider puck, chevrons back, missed-junction ring |

The typeface is Nunito at weights 800 and 900, as everywhere else in VANA.

## Checking a screen against its mockup

The three map mockups put the rider's puck where the map does, at x 120,
row 223, and draw only the course ahead, with no ridden line. On
map-offcourse the junction lies below the map, so its three chevrons run
from the puck towards the bottom edge, and the route past the junction runs
up to the status bar. Each map mockup is drawn at one scale, and its
scenario rides at the speed that gives that scale.

`cycling mockup-compare` compares each mockup with the simulator checkpoint a
scenario names after it (`.local/sim/latest/<scenario>@c606/NN-ride-turns.png`, run
`cycling sim` first). Each element's interior must be its token exactly in
RGB565, a colour's ink may reach at most one pixel past where the mockup draws
it, and a sign's fill must fill exactly the mockup's box. `<screen>.rules.toml`
names what a screen draws differently on purpose, each with its reason: the
clock and page dots, readings whose digits change, text drawn in an existing
face, and the illustrative roads. It writes `expected.png`, `actual.png` and
`diff.png` per screen to `.local/mockup/`. `--checkpoint` compares one screen
with any PNG or SVG.
