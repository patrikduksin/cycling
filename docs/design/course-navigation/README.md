# Course navigation design

These are the approved mockups for following a course on VANA. The SVGs are the
source of truth for geometry, sizes and colours, drawn in device pixels on the
240×320 screen. `png/` holds them rendered at 2x, plus `sheet.png` (all
screens) and `glance-sheet.png` (blurred, roughly a half-second look at arm's
length). `spec.md` is the interaction spec. `png/before-*.png` are the screens
this design replaces.

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
| Minor road | `#2a2a2d`, 5 px | Residential and smaller |
| Major road | `#444448`, 9 px | Primary and secondary classes |
| Route ahead | `ACCENT` `#0a84ff`, 12 px on an 18 px `#04264c` casing | Course still to ride |
| Ridden | `#1f4a78`, 5 px | Course already ridden |
| Off-course marks | `AMBER` `#ff9f0a` | Rider puck, chevrons back, missed-junction ring |

The typeface is Nunito at weights 800 and 900, as everywhere else in VANA.

## Checking a screen against its mockup

`cycling mockup-compare` compares each mockup with the simulator checkpoint a
scenario names after it (`.local/sim/latest/<scenario>/NN-ride-turns.png`, run
`cycling sim` first). Each element's interior must be its token exactly in
RGB565, a colour's ink may reach at most one pixel past where the mockup draws
it, and a sign's fill must fill exactly the mockup's box. `<screen>.rules.toml`
names what a screen draws differently on purpose, each with its reason: the
clock and page dots, readings whose digits change, text drawn in an existing
face, and the illustrative roads. It writes `expected.png`, `actual.png` and
`diff.png` per screen to `.local/mockup/`. `--checkpoint` compares one screen
with any PNG or SVG.
